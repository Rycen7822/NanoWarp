use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use warp_core::features::FeatureFlag;
use warp_core::settings::ChangeEventReason;
use warp_core::user_preferences::GetUserPreferences;
use warpui::{
    AppContext, Entity, ModelContext, SingletonEntity, Tracked, ViewContext, WeakViewHandle,
    WindowId,
};

#[cfg(test)]
use super::team::MembershipRole;
use super::team::Team;
#[cfg(test)]
use super::team::TeamVisibility;
#[cfg(test)]
use super::workspace::WorkspaceMemberUsageInfo;
use super::workspace::{
    AdminEnablementSetting, EnterpriseSecretRegex, UgcCollectionEnablementSetting, Workspace,
    WorkspaceUid,
};
use crate::ai::credit_availability::AICreditAvailability;
use crate::ai::llms::{AvailableLLMs, MODELS_BY_FEATURE_CACHE_KEY, ModelsByFeature};
use crate::auth::{AuthStateProvider, UserUid};
use crate::channel::ChannelState;
use crate::cloud_object::model::persistence::CloudModel;
use crate::cloud_object::{ObjectType, Owner, Space};
use crate::server::experiments::{ServerExperiment, ServerExperiments, ServerExperimentsEvent};
use crate::server::ids::ServerId;
#[cfg(test)]
use crate::server::server_api::workspace::MockWorkspaceClient;
use crate::server::server_api::workspace::WorkspaceClient;
use crate::settings::PrivacySettings;
#[cfg(test)]
use crate::workspaces::workspace::{
    AIAutonomyPolicy, AiAutonomySettings, BillingMetadata, CustomerType, SplitListSetting,
    WorkspaceMember, WorkspaceSettings,
};
use crate::workspaces::workspace::{AiOverages, UserTier};
pub(crate) mod billing_workspace_settings;
pub(crate) mod team_workspace_settings;
#[cfg(not(target_family = "wasm"))]
pub(crate) use team_workspace_settings::GeminiEnterpriseBackgroundHost;
#[cfg(test)]
pub(crate) use team_workspace_settings::TeamlessScopeForTest;
pub use team_workspace_settings::{
    ResolvedTeamScope, TeamContext, TeamContextForOperation, TeamContextResolver, TeamScope,
};

const STRIPE_SUBSCRIPTION_INTERVAL_PAGE_PREFIX: &str = "/upgrade";

#[derive(Debug)]
pub enum UserWorkspacesEvent {
    AiOveragesUpdated,
    /// Fired whenever the set of teams the user is on changes.
    TeamsChanged,
    /// Fired when the selected workspace actually changes to a different one.
    CurrentWorkspaceChanged,
    /// Fired when a single window's team assignment changes. Windows are independent, so
    /// subscribers that hold per-window state must only react to their own window.
    WindowTeamChanged {
        window_id: WindowId,
    },
}

/// UserWorkspaces is a singleton model that holds workspace metadata (name, members, etc).
/// It should be used for getting information about the workspaces, teams, current teams,
/// and all other things related to operating on workspace and team data.
/// TODO: move other server_api calls to update_manager to correctly update sqlite.
pub struct UserWorkspaces {
    current_workspace_uid: Tracked<Option<WorkspaceUid>>,
    workspaces: Tracked<Vec<Workspace>>,
    window_team_uids: HashMap<WindowId, Option<ServerId>>,
    /// The user-level plan terms from the latest workspaces-metadata response; the fallback for
    /// users whose only workspace is the server's placeholder filtered out of `workspaces`.
    user_tier: UserTier,
    /// The model catalog to fall back to when no current workspace exists: before login, or
    /// for a logged-in user whose only workspace is the server's placeholder, which is
    /// filtered out of `workspaces`.
    workspaceless_models_by_feature: Option<ModelsByFeature>,
    workspace_client: Arc<dyn WorkspaceClient>,
}

/// Represents the workspaces a user potentially has access to.
#[derive(Clone)]
pub struct WorkspacesMetadataResponse {
    /// The list of workspaces the user is currently on.
    pub workspaces: Vec<Workspace>,
    /// The list of experiments applicable to the user.
    pub experiments: Option<Vec<ServerExperiment>>,
    /// The server-authoritative AI credit availability decision, piggybacked
    /// on the metadata query so every refresh keeps the shared state fresh.
    pub ai_credit_availability: Option<AICreditAvailability>,
    /// The user-level plan terms; the teamless fallback (see
    /// [`UserWorkspaces::purchase_policy`] and [`UserWorkspaces::charge_unit`]).
    pub user_tier: UserTier,
}

// A representation of all data we fetch at a single time via our 10 minute poll.
// Prefer adding to this struct if you need relatively fresh data vs making
// independent queries.
pub struct WorkspacesMetadataWithPricing {
    pub metadata: WorkspacesMetadataResponse,
    pub pricing_info: Option<warp_graphql::billing::PricingInfo>,
}

#[derive(Debug, thiserror::Error)]
pub enum SoleTeamError {
    #[error("you are not on a team")]
    NoTeam,
    #[error("you are on {} teams, so no single team applies", .team_uids.len())]
    MoreThanOneTeam { team_uids: Vec<ServerId> },
}

impl UserWorkspaces {
    #[cfg(test)]
    pub fn mock(
        workspace_client: Arc<dyn WorkspaceClient>,
        cached_workspaces: Vec<Workspace>,
        _ctx: &mut ModelContext<Self>,
    ) -> Self {
        // In tests, avoid subscribing to [`ServerExperiments`] because it
        // requires us to register that singleton along with _its_ dependencies
        // for all tests that use [`UserWorkspaces`] (a lot of them do).
        Self {
            current_workspace_uid: cached_workspaces.first().map(|w| w.uid).into(),
            workspaces: cached_workspaces.into(),
            window_team_uids: Default::default(),
            user_tier: Default::default(),
            workspaceless_models_by_feature: None,
            workspace_client,
        }
    }

    #[cfg(test)]
    pub fn default_mock(ctx: &mut ModelContext<Self>) -> Self {
        Self::mock(Arc::new(MockWorkspaceClient::new()), vec![], ctx)
    }

    pub fn new(
        workspace_client: Arc<dyn WorkspaceClient>,
        cached_workspaces: Vec<Workspace>,
        current_workspace_uid: Option<WorkspaceUid>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&ServerExperiments::handle(ctx), |me, _, event, ctx| {
            let ServerExperimentsEvent::ExperimentsUpdated = event;
            me.update_session_sharing_enablement(ctx);
        });

        let mut me = Self {
            current_workspace_uid: current_workspace_uid.into(),
            workspaces: cached_workspaces.into(),
            window_team_uids: Default::default(),
            user_tier: Default::default(),
            workspaceless_models_by_feature: None,
            workspace_client,
        };

        // One-release migration: moving feature_model_choices off of `LLMPreferences` to `Workspace`.
        // This means that on the first time the user opens a version of warp without a
        // Workspace.feature_model_choice saved in their sqlite db, we can fall back to reading feature
        // model choices from the old LLMPreferences cache.
        // TODO: delete once it's safe to assume every client has fetched at least once since
        // this migration shipped.
        if me
            .current_workspace()
            .is_some_and(|workspace| workspace.feature_model_choice == ModelsByFeature::default())
            && let Some(legacy_catalog) = migrate_legacy_feature_model_choices_cache(ctx)
            && let Some(workspace) = me.current_workspace_mut()
        {
            workspace.feature_model_choice = legacy_catalog;
        }

        me
    }

    pub(crate) fn set_workspaceless_models_by_feature(&mut self, models: ModelsByFeature) {
        self.workspaceless_models_by_feature = Some(models);
    }

    pub fn upgrade_link(user_id: UserUid) -> String {
        format!(
            "{}{}/{}/{}",
            ChannelState::server_root_url(),
            STRIPE_SUBSCRIPTION_INTERVAL_PAGE_PREFIX,
            "user",
            user_id.as_str()
        )
    }

    pub fn upgrade_link_for_team(team_uid: ServerId) -> String {
        format!(
            "{}{}/{}",
            ChannelState::server_root_url(),
            STRIPE_SUBSCRIPTION_INTERVAL_PAGE_PREFIX,
            team_uid
        )
    }

    pub fn warp_agent_cli_upgrade_link(user_id: Option<UserUid>) -> String {
        let upgrade_link = user_id.map_or_else(
            || {
                format!(
                    "{}{}",
                    ChannelState::server_root_url().trim_end_matches('/'),
                    STRIPE_SUBSCRIPTION_INTERVAL_PAGE_PREFIX
                )
            },
            Self::upgrade_link,
        );
        format!("{upgrade_link}?source=warp-agent-cli")
    }
    pub fn team_from_uid(&self, team_uid: ServerId) -> Option<&Team> {
        self.current_workspace()
            .and_then(|w| w.teams.iter().find(|t| t.uid == team_uid))
    }

    pub fn register_window(
        &mut self,
        window_id: WindowId,
        team_uid: Option<ServerId>,
        ctx: &mut ModelContext<Self>,
    ) {
        let previous_team_uid = self.team_uid_for_window(window_id);
        self.window_team_uids.entry(window_id).or_insert(team_uid);
        if self.team_uid_for_window(window_id) != previous_team_uid {
            ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
        }
        ctx.notify();
    }

    pub fn inherited_or_default_team_uid(
        &self,
        source_window_id: Option<WindowId>,
    ) -> Option<ServerId> {
        source_window_id
            .and_then(|source_window_id| self.team_uid_for_window(source_window_id))
            .or_else(|| {
                self.current_workspace()
                    .and_then(|workspace| workspace.teams.first())
                    .map(|team| team.uid)
            })
    }

    pub fn set_team_for_window(
        &mut self,
        window_id: WindowId,
        team_uid: ServerId,
        ctx: &mut ModelContext<Self>,
    ) {
        let window_team_uid = self.window_team_uids.entry(window_id).or_default();
        if window_team_uid.is_none() {
            *window_team_uid = Some(team_uid);
            ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
            ctx.notify();
        }
    }

    pub fn team_uid_for_window(&self, window_id: WindowId) -> Option<ServerId> {
        self.window_team_uids.get(&window_id).copied().flatten()
    }

    pub fn switch_window_to_team(
        &mut self,
        window_id: WindowId,
        team_uid: ServerId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.set_window_team_uid(window_id, Some(team_uid), ctx);
    }

    pub(crate) fn set_window_team_uid(
        &mut self,
        window_id: WindowId,
        team_uid: Option<ServerId>,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.team_uid_for_window(window_id) == team_uid {
            return;
        }
        self.window_team_uids.insert(window_id, team_uid);
        ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
        ctx.notify();
    }

    /// Returns `true` when the user belongs to more than one team in the current
    /// workspace, meaning the team-switcher pill and dropdown should be shown.
    /// Single-team and no-workspace users return `false` so their UI is unchanged.
    pub fn can_switch_teams(&self) -> bool {
        self.current_workspace()
            .map(|ws| ws.teams.len() > 1)
            .unwrap_or(false)
    }
    pub fn team_for_window(&self, window_id: WindowId) -> Option<&Team> {
        self.team_uid_for_window(window_id)
            .and_then(|team_uid| self.team_from_uid(team_uid))
    }
    pub fn team_for_view<T: Entity>(&self, ctx: &ViewContext<T>) -> Option<&Team> {
        self.team_for_window(ctx.window_id())
    }

    pub fn team_for_view_handle<T: Entity>(
        &self,
        view_handle: &WeakViewHandle<T>,
        ctx: &AppContext,
    ) -> Option<&Team> {
        view_handle
            .window_id(ctx)
            .and_then(|window_id| self.team_for_window(window_id))
    }

    /// Returns the windows whose team assignment changed.
    #[must_use]
    fn reconcile_window_team_assignments(&mut self) -> Vec<WindowId> {
        let team_uids = self
            .current_workspace()
            .map(|workspace| {
                workspace
                    .teams
                    .iter()
                    .map(|team| team.uid)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let fallback_team_uid = team_uids.first().copied();

        let mut reassigned_windows = Vec::new();
        for (window_id, window_team_uid) in self.window_team_uids.iter_mut() {
            if window_team_uid.is_none_or(|team_uid| !team_uids.contains(&team_uid))
                && *window_team_uid != fallback_team_uid
            {
                *window_team_uid = fallback_team_uid;
                reassigned_windows.push(*window_id);
            }
        }
        reassigned_windows
    }

    fn emit_window_team_changed(windows: Vec<WindowId>, ctx: &mut ModelContext<Self>) {
        for window_id in windows {
            ctx.emit(UserWorkspacesEvent::WindowTeamChanged { window_id });
        }
    }

    pub fn team_from_uid_across_all_workspaces(&self, team_uid: ServerId) -> Option<&Team> {
        self.workspaces
            .iter()
            .flat_map(|w| w.teams.iter())
            .find(|t| t.uid == team_uid)
    }

    /// The teams [`Self::owner_to_space`] recognizes. An owner naming a team outside this set
    /// resolves to the shared space instead of that team's space, so a change here remaps
    /// objects between spaces without any of them changing.
    pub fn team_uids_across_all_workspaces(&self) -> HashSet<ServerId> {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.teams.iter())
            .map(|team| team.uid)
            .collect()
    }

    pub fn workspace_from_uid(&self, workspace_uid: WorkspaceUid) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.uid == workspace_uid)
    }

    pub fn workspace_from_uid_mut(
        &mut self,
        workspace_uid: WorkspaceUid,
    ) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.uid == workspace_uid)
    }

    pub fn is_at_tier_limit_for_object_type(
        team_uid: ServerId,
        object_type: ObjectType,
        ctx: &AppContext,
    ) -> bool {
        match object_type {
            ObjectType::Notebook => {
                !UserWorkspaces::has_capacity_for_shared_notebooks(team_uid, ctx, 1)
            }
            ObjectType::Workflow => {
                !UserWorkspaces::has_capacity_for_shared_workflows(team_uid, ctx, 1)
            }
            ObjectType::Folder => false,
            ObjectType::GenericStringObject(_) => false,
        }
    }

    pub fn is_at_tier_limit_for_some_warp_drive_objects(
        team_uid: ServerId,
        ctx: &AppContext,
    ) -> bool {
        UserWorkspaces::is_at_tier_limit_for_object_type(team_uid, ObjectType::Notebook, ctx)
            || UserWorkspaces::is_at_tier_limit_for_object_type(team_uid, ObjectType::Workflow, ctx)
    }

    // Checks if the team has capacity for another shared notebook for their current
    // billing tier, given their current notebook count and delinquency status.
    pub fn has_capacity_for_shared_notebooks(
        team_uid: ServerId,
        ctx: &AppContext,
        new_shared_notebooks: usize,
    ) -> bool {
        let current_shared_notebooks = CloudModel::as_ref(ctx)
            .active_notebooks_in_space(Space::Team { team_uid }, ctx)
            .count();

        let team = UserWorkspaces::as_ref(ctx).team_from_uid(team_uid);
        if let Some(team) = team {
            // If the team is past due or unpaid, then don't allow new notebooks.
            if team.billing_metadata.is_delinquent_due_to_payment_issue() {
                return false;
            }

            if let Some(policy) = team.billing_metadata.tier.shared_notebooks_policy {
                // Allow new notebooks if policy is unlimited or if the number of notebooks
                // is less than the limit.
                policy.is_unlimited
                    || current_shared_notebooks + new_shared_notebooks
                        <= policy
                            .limit
                            .try_into()
                            .expect("shared notebooks limit should be within max i64 range")
            } else {
                // If no policy is set, then allow it to go through by default (should still be enforced server-side)
                true
            }
        } else {
            // If the team is not found, then allow it to go through by default (should still be enforced server-side)
            true
        }
    }

    // Checks if the team has capacity for another shared workflow for their current
    // billing tier, given their current workflow count and delinquency status.
    pub fn has_capacity_for_shared_workflows(
        team_uid: ServerId,
        ctx: &AppContext,
        new_shared_workflows: usize,
    ) -> bool {
        let current_shared_workflows = CloudModel::as_ref(ctx)
            .active_workflows_in_space(Space::Team { team_uid }, ctx)
            .count();

        let team = UserWorkspaces::as_ref(ctx).team_from_uid(team_uid);
        if let Some(team) = team {
            // If the team is past due or unpaid, then don't allow new workflows.
            if team.billing_metadata.is_delinquent_due_to_payment_issue() {
                return false;
            }

            if let Some(policy) = team.billing_metadata.tier.shared_workflows_policy {
                // Allow new workflows if policy is unlimited or if the number of workflows
                // is less than the limit.
                policy.is_unlimited
                    || current_shared_workflows + new_shared_workflows
                        <= policy
                            .limit
                            .try_into()
                            .expect("shared workflows limit should be within max i64 range")
            } else {
                // If no policy is set, then allow it to go through by default (should still be enforced server-side)
                true
            }
        } else {
            // If the team is not found, then allow it to go through by default (should still be enforced server-side)
            true
        }
    }

    /// The user's single team in the current workspace.
    ///
    /// Having no workspace is reported as [`SoleTeamError::NoTeam`]: a user with no workspace
    /// is on no team, and no caller can act differently on the distinction.
    pub fn sole_team(&self) -> Result<&Team, SoleTeamError> {
        let teams = self
            .current_workspace()
            .map(|workspace| workspace.teams.as_slice())
            .unwrap_or_default();
        match teams {
            [] => Err(SoleTeamError::NoTeam),
            [team] => Ok(team),
            _ => Err(SoleTeamError::MoreThanOneTeam {
                team_uids: teams.iter().map(|team| team.uid).collect(),
            }),
        }
    }

    pub fn sole_team_uid(&self) -> Result<ServerId, SoleTeamError> {
        self.sole_team().map(|team| team.uid)
    }

    /// Note that the workspace is populated with dummy data until the initial fetch
    /// completes (only workspace name/ID and workspace team's name/ID are cached in
    /// sqlite locally).
    /// Consider whether you need to wait for the results of the fetch before checking the
    /// values of other fields.
    pub fn current_workspace(&self) -> Option<&Workspace> {
        self.current_workspace_uid
            .and_then(|workspace_uid| self.workspace_from_uid(workspace_uid))
    }

    /// Updates the user-level plan terms captured from a workspaces-metadata response. Must be
    /// called on every path that applies such a response so the teamless fallback can't go
    /// stale.
    pub fn set_user_tier(&mut self, user_tier: UserTier) {
        self.user_tier = user_tier;
    }

    pub fn current_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.current_workspace_uid
            .and_then(|workspace_uid| self.workspace_from_uid_mut(workspace_uid))
    }

    pub fn workspaces(&self) -> &Vec<Workspace> {
        &self.workspaces
    }

    pub fn set_current_workspace_uid(
        &mut self,
        workspace_uid: WorkspaceUid,
        ctx: &mut ModelContext<Self>,
    ) {
        let changed = *self.current_workspace_uid != Some(workspace_uid);
        *self.current_workspace_uid = Some(workspace_uid);
        let reassigned_windows = self.reconcile_window_team_assignments();
        self.notify_and_emit_teams_changed(ctx);
        Self::emit_window_team_changed(reassigned_windows, ctx);
        if changed {
            ctx.emit(UserWorkspacesEvent::CurrentWorkspaceChanged);
        }
    }

    // Returns a Vec of the user's active spaces, based on their
    // team membership.
    pub fn team_spaces(&self) -> Vec<Space> {
        if let Some(workspace) = self.current_workspace() {
            workspace
                .teams
                .iter()
                .map(|team| Space::Team { team_uid: team.uid })
                .collect()
        } else {
            // If the user has no workspace, they have no team spaces.
            vec![]
        }
    }

    pub fn spaces_for_window(&self, window_id: WindowId, ctx: &AppContext) -> Vec<Space> {
        if AuthStateProvider::as_ref(ctx)
            .get()
            .is_user_web_anonymous_user()
            .unwrap_or_default()
        {
            return vec![Space::Shared];
        }
        let mut spaces = vec![];
        if let Some(team) = self.team_for_window(window_id) {
            spaces.push(Space::Team { team_uid: team.uid });
        }

        if FeatureFlag::SharedWithMe.is_enabled()
            && CloudModel::as_ref(ctx).has_directly_shared_objects(self, ctx)
        {
            spaces.push(Space::Shared);
        }
        spaces.push(Space::Personal);

        spaces
    }

    // Returns the [`Owner`] for the user's personal drive. If the user is not authenticated, this
    // returns `None`.
    pub fn personal_drive(&self, ctx: &AppContext) -> Option<Owner> {
        AuthStateProvider::as_ref(ctx)
            .get()
            .user_id()
            .map(|user_uid| Owner::User { user_uid })
    }

    // Maps a [`Space`] into an [`Owner`], based on the user's team memberships. If the space
    // does not directly identify an owner (it's the space for shared objects), returns `None`.
    pub fn space_to_owner(&self, space: Space, ctx: &AppContext) -> Option<Owner> {
        match space {
            Space::Team { team_uid } => Some(Owner::Team { team_uid }),
            Space::Personal => self.personal_drive(ctx),
            Space::Shared => None,
        }
    }

    // Maps an [`Owner`] into a [`Space`], based on the user's team memberships.
    // This is always possible, as unknown owners imply the shared space.
    pub fn owner_to_space(&self, owner: Owner, ctx: &AppContext) -> Space {
        match owner {
            Owner::User { user_uid } => {
                if !FeatureFlag::SharedWithMe.is_enabled() {
                    return Space::Personal;
                }

                let current_user = AuthStateProvider::as_ref(ctx).get().user_id();
                if Some(user_uid) == current_user {
                    Space::Personal
                } else {
                    Space::Shared
                }
            }
            Owner::Team { team_uid } => {
                if !FeatureFlag::SharedWithMe.is_enabled()
                    || self.team_from_uid_across_all_workspaces(team_uid).is_some()
                {
                    Space::Team { team_uid }
                } else {
                    Space::Shared
                }
            }
        }
    }

    pub fn has_teams(&self) -> bool {
        if let Some(workspace) = self.current_workspace() {
            !workspace.teams.is_empty()
        } else {
            false
        }
    }

    pub fn has_workspaces(&self) -> bool {
        !self.workspaces.is_empty()
    }

    /// Cloud agents require membership when a native workspace has no teams.
    pub fn cloud_agents_require_team(&self) -> bool {
        self.current_workspace().is_some_and(|workspace| {
            workspace.is_native_workspaces_enabled() && workspace.teams.is_empty()
        })
    }

    pub fn update_workspaces(&mut self, workspaces: Vec<Workspace>, ctx: &mut ModelContext<Self>) {
        *self.workspaces = workspaces;
        let reassigned_windows = self.reconcile_window_team_assignments();
        self.notify_and_emit_teams_changed(ctx);
        Self::emit_window_team_changed(reassigned_windows, ctx);
    }

    fn notify_and_emit_teams_changed(&self, ctx: &mut ModelContext<Self>) {
        // Update session-sharing enablement since it depends on what teams the user
        // is part of.
        self.update_session_sharing_enablement(ctx);

        // PrivacySettings can't observe UserWorkspaces for updates, as it's initialized too early in
        // the app initialization flow. So, we update it manually whenever teams data changes.
        PrivacySettings::handle(ctx).update(ctx, |settings, ctx| {
            settings.set_is_telemetry_force_enabled(self.is_telemetry_force_enabled());
            settings.set_enterprise_secret_redaction_settings(
                self.is_enterprise_secret_redaction_enabled(),
                self.get_enterprise_secret_redaction_regex_list(),
                ChangeEventReason::CloudSync,
                ctx,
            );
        });

        ctx.emit(UserWorkspacesEvent::TeamsChanged);
        ctx.notify();
    }

    pub fn refresh_ai_overages(&mut self, ctx: &mut ModelContext<Self>) {
        let workspace_client = self.workspace_client.clone();
        let _ = ctx.spawn(
            async move { workspace_client.refresh_ai_overages().await },
            Self::on_refresh_ai_overages,
        );
    }

    fn on_refresh_ai_overages(&mut self, result: Result<AiOverages>, ctx: &mut ModelContext<Self>) {
        match result {
            Ok(fresh_ai_overages) => {
                // TODO: We really need to stop having duplicate billing metadata...
                if let Some(workspace) = self.current_workspace_mut() {
                    workspace.billing_metadata.ai_overages = Some(fresh_ai_overages.clone());
                    for team in &mut workspace.teams {
                        team.billing_metadata.ai_overages = Some(fresh_ai_overages.clone());
                    }
                }

                ctx.emit(UserWorkspacesEvent::AiOveragesUpdated);
                ctx.notify();
            }
            Err(e) => {
                log::warn!("Failed to refresh AI overages for workspace: {e:?}");
            }
        }
    }

    pub fn is_telemetry_force_enabled(&self) -> bool {
        self.current_workspace()
            .map(|workspace| workspace.settings.telemetry_settings.force_enabled)
            .unwrap_or(false)
    }

    pub fn is_enterprise_secret_redaction_enabled(&self) -> bool {
        self.current_workspace()
            .map(|workspace| workspace.settings.secret_redaction_settings.enabled)
            .unwrap_or(false)
    }

    pub fn get_enterprise_secret_redaction_regex_list(&self) -> Vec<EnterpriseSecretRegex> {
        self.current_workspace()
            .map(|workspace| workspace.settings.secret_redaction_settings.regexes.clone())
            .unwrap_or_default()
    }

    pub fn get_ugc_collection_enablement_setting(&self) -> UgcCollectionEnablementSetting {
        self.current_workspace()
            .map(|workspace| workspace.settings.ugc_collection_settings.setting.clone())
            .unwrap_or_default()
    }

    pub fn get_cloud_conversation_storage_enablement_setting(&self) -> AdminEnablementSetting {
        self.current_workspace()
            .map(|workspace| {
                workspace
                    .settings
                    .cloud_conversation_storage_settings
                    .setting
                    .clone()
            })
            .unwrap_or_default()
    }

    /// Updates whether or not session sharing is enabled based on the current team's tier policy.
    fn update_session_sharing_enablement(&self, ctx: &AppContext) {
        if cfg!(any(test, feature = "integration_tests")) {
            return;
        }

        // If we have experiment state to unconditionally enable / disable the feature,
        // then we defer to that.
        let server_experiments = ServerExperiments::as_ref(ctx);
        if server_experiments.is_experiment_enabled(&ServerExperiment::SessionSharingControl)
            || server_experiments.is_experiment_enabled(&ServerExperiment::SessionSharingExperiment)
        {
            return;
        }

        let is_session_sharing_enabled_via_tier_policy = self
            .current_workspace()
            .and_then(|workspace| workspace.billing_metadata.tier.session_sharing_policy)
            .map(|policy| policy.is_enabled)
            .unwrap_or(true);
        FeatureFlag::CreatingSharedSessions.set_enabled(is_session_sharing_enabled_via_tier_policy);
    }
}

#[cfg(test)]
fn split_test_list(values: Option<Vec<String>>) -> SplitListSetting<String> {
    match values {
        Some(values) => SplitListSetting {
            team_entries: values.clone(),
            values,
            ..Default::default()
        },
        None => Default::default(),
    }
}

#[cfg(test)]
impl UserWorkspaces {
    /// Creates a test workspace with a team and sets it as the current workspace.
    /// Returns the workspace UID and admin UID for use in tests.
    pub fn setup_test_workspace(&mut self, ctx: &mut ModelContext<Self>) {
        let workspace_uid = WorkspaceUid::from(ServerId::from(1));
        let owner_uid = UserUid::new("test_owner");

        let workspace_settings = WorkspaceSettings::default();

        let workspace = Workspace {
            uid: workspace_uid,
            name: "Test Workspace".to_string(),
            stripe_customer_id: None,
            teams: vec![Team {
                uid: ServerId::from(2),
                name: "Test Team".to_string(),
                settings: Default::default(),
                color: None,
                billing_metadata: BillingMetadata::default(),
                members: vec![],
                invite_link: None,
                pending_email_invites: vec![],
                invite_link_domain_restrictions: vec![],
                stripe_customer_id: None,
                feature_model_choice: Default::default(),
                is_eligible_for_discovery: false,
                visibility: TeamVisibility::Open,
            }],
            members: vec![WorkspaceMember {
                uid: owner_uid,
                email: "test@example.com".to_string(),
                role: MembershipRole::Owner,
                is_disabled: false,
                usage_info: WorkspaceMemberUsageInfo {
                    requests_used_since_last_refresh: 0,
                    request_limit: 1000,
                    is_unlimited: false,
                    included_usage_cents: None,
                    usage_cents_used_since_last_refresh: None,
                    is_request_limit_prorated: false,
                },
            }],
            billing_metadata: BillingMetadata::default(),
            bonus_grants_purchased_this_month: Default::default(),
            settings: workspace_settings,
            feature_model_choice: Default::default(),
            invite_link_domain_restrictions: vec![],
            pending_email_invites: vec![],
            is_eligible_for_discovery: false,
            total_requests_used_since_last_refresh: 0,
        };

        self.update_workspaces(vec![workspace], ctx);
        self.set_current_workspace_uid(workspace_uid, ctx);
    }

    /// Updates the current workspace by applying a mutation function.
    pub fn update_current_workspace<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut Workspace),
    {
        if let Some(workspace) = self.current_workspace() {
            if workspace.teams.is_empty() {
                panic!("No team found in current workspace. Did you call setup_test_workspace()?");
            }

            let mut new_workspace = workspace.clone();
            f(&mut new_workspace);

            self.update_workspaces(vec![new_workspace], ctx);
        } else {
            panic!("No workspace found. Did you call setup_test_workspace()?");
        }
    }

    /// Sets the sandboxed-agent command denylist on [`Self::setup_test_workspace`]'s team, the
    /// team [`Self::sandboxed_agent_execute_commands_denylist_for_scope`] reads for a scope on
    /// it.
    pub fn update_team_sandboxed_agent_denylist<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut SplitListSetting<String>),
    {
        self.update_current_workspace(
            |workspace| {
                f(&mut workspace
                    .teams
                    .first_mut()
                    .expect("test workspace should have a team")
                    .settings
                    .sandboxed_agent
                    .execute_commands_denylist);
            },
            ctx,
        );
    }

    pub fn update_ai_autonomy_settings<F>(&mut self, f: F, ctx: &mut ModelContext<Self>)
    where
        F: FnOnce(&mut AiAutonomySettings),
    {
        self.update_current_workspace(
            |workspace| {
                f(&mut workspace.settings.ai_autonomy_settings);
                let settings = &workspace.settings.ai_autonomy_settings;
                let team_settings = &mut workspace
                    .teams
                    .first_mut()
                    .expect("test workspace should have a team")
                    .settings
                    .ai_autonomy;
                team_settings.apply_code_diffs.value = settings.apply_code_diffs_setting;
                team_settings.read_files.value = settings.read_files_setting;
                team_settings.execute_commands.value = settings.execute_commands_setting;
                team_settings.write_to_pty.value = settings.write_to_pty_setting;
                team_settings.read_files_allowlist =
                    split_test_list(settings.read_files_allowlist.as_ref().map(|items| {
                        items
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect()
                    }));
                team_settings.execute_commands_allowlist = split_test_list(
                    settings
                        .execute_commands_allowlist
                        .as_ref()
                        .map(|items| items.iter().map(ToString::to_string).collect()),
                );
                team_settings.execute_commands_denylist = split_test_list(
                    settings
                        .execute_commands_denylist
                        .as_ref()
                        .map(|items| items.iter().map(ToString::to_string).collect()),
                );
            },
            ctx,
        );
    }
}

/// Reads the legacy, pre-team-keyed model catalog cache (`MODELS_BY_FEATURE_CACHE_KEY`), for
/// the one-release migration in [`UserWorkspaces::new`]. Understands both real shapes an older
/// client could have written: the (more recent) single `ModelsByFeature`, and (older still) a
/// bare `AvailableLLMs`, which becomes the `agent_mode` field.
fn migrate_legacy_feature_model_choices_cache(app: &mut AppContext) -> Option<ModelsByFeature> {
    let value = app
        .private_user_preferences()
        .read_value(MODELS_BY_FEATURE_CACHE_KEY)
        .ok()
        .flatten()?;

    match serde_json::from_str::<ModelsByFeature>(&value) {
        Ok(models) => Some(models),
        Err(e1) => match serde_json::from_str::<AvailableLLMs>(&value) {
            Ok(agent_mode) => Some(ModelsByFeature {
                agent_mode,
                ..Default::default()
            }),
            Err(e2) => {
                log::warn!("Failed to deserialize legacy cached LLMs: {e1}\n{e2}");
                None
            }
        },
    }
}

impl Entity for UserWorkspaces {
    type Event = UserWorkspacesEvent;
}

/// Mark UserWorkspaces as global application state.
impl SingletonEntity for UserWorkspaces {}

#[cfg(test)]
#[path = "user_workspaces_tests.rs"]
mod user_workspaces_tests;
