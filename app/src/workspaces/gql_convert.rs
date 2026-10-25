use super::team::{MembershipRole, Team, TeamMember, TeamVisibility};
use super::user_workspaces::WorkspacesMetadataResponse;
use super::workspace::{
    BillingMetadata, CustomerType, DelinquencyStatus, EmailInvite, EnforceableSetting,
    EnterpriseSecretRegex, InviteLinkDomainRestriction, LinkSharingSettings, MultiAdminPolicy,
    NativeWorkspacesPolicy, SecretRedactionSettings, SessionSharingPolicy, SharedNotebooksPolicy,
    SharedWorkflowsPolicy, SplitListSetting, TeamLinkSharingSettings, TeamSecretRedactionSettings,
    TeamSettings, TelemetryDataCollectionPolicy, TelemetrySettings, Tier, Workspace,
    WorkspaceMember, WorkspaceSettings, WorkspaceSizePolicy,
};
use crate::auth::UserUid;
use crate::convert_to_server_experiment;
use crate::server::cloud_objects::listener::ObjectUpdateMessage;
use crate::server::experiments::ServerExperiment;
use crate::server::graphql::schema::object_action_history_from_gql;
use crate::server::ids::ServerId;
use anyhow::{Result, anyhow, bail};
use regex::Regex;
use warp_errors::report_error;
use warp_graphql::billing::{
    BillingMetadata as GqlBillingMetadata, CustomerType as GqlCustomerType,
    DelinquencyStatus as GqlDelinquencyStatus, MultiAdminPolicy as GqlMultiAdminPolicy,
    NativeWorkspacesPolicy as GqlNativeWorkspacesPolicy, ServiceAgreementType,
    SessionSharingPolicy as GqlSessionSharingPolicy,
    SharedNotebooksPolicy as GqlSharedNotebooksPolicy,
    SharedWorkflowsPolicy as GqlSharedWorkflowsPolicy, StripeSubscriptionPlan,
    TeamSizePolicy as GqlTeamSizePolicy,
    TelemetryDataCollectionPolicy as GqlTelemetryDataCollectionPolicy, Tier as GqlTier,
};
use warp_graphql::queries::get_workspaces_metadata_for_user::User as GqlUser;
use warp_graphql::subscriptions::get_warp_drive_updates::WarpDriveUpdate;
use warp_graphql::workspace::{
    EmailInvite as GqlEmailInvite, InviteLinkDomainRestriction as GqlInviteLinkDomainRestriction,
    MembershipRole as GqlMembershipRole, Team as GqlTeam, TeamMember as GqlTeamMember,
    TeamSettings as GqlTeamSettings, TeamVisibility as GqlTeamVisibility,
    Workspace as GqlWorkspace, WorkspaceMember as GqlWorkspaceMember,
    WorkspaceSettings as GqlWorkspaceSettings,
};

pub const PLACEHOLDER_WORKSPACE_UID: &str = "NOT_A_REAL_WORKSPACE_UID";

impl From<GqlTeamMember> for TeamMember {
    fn from(gql_team_member: GqlTeamMember) -> TeamMember {
        Self {
            uid: UserUid::new(&gql_team_member.uid.into_inner()),
            email: gql_team_member.email,
            role: gql_team_member.role.into(),
            is_disabled: gql_team_member.is_disabled,
        }
    }
}

/// Narrows a workspace to the teams the authenticated user actually belongs to.
///
/// The server hands workspace admins every team in the workspace so admin
/// surfaces can manage them, but a team the user is not a member of is not one
/// they can operate as in the client. Filtering here keeps every consumer of
/// `Workspace::teams` (team switcher, team spaces, warp drive teams, ...)
/// scoped to real memberships.
///
/// Service accounts are never recorded as a human `TeamMember` (that list only tracks
/// user-role memberships), so this membership check cannot recognize them and would strip
/// every team out from under any service account. Skip it in that case and trust the server
/// to have already scoped `teams` to the service account's own team.
fn retain_authenticated_teams(
    workspace: &mut Workspace,
    user_uid: UserUid,
    is_service_account: bool,
) {
    if is_service_account {
        return;
    }
    workspace
        .teams
        .retain(|team| team.members.iter().any(|member| member.uid == user_uid));
}

impl From<GqlMembershipRole> for MembershipRole {
    fn from(role: GqlMembershipRole) -> Self {
        match role {
            GqlMembershipRole::Owner => MembershipRole::Owner,
            GqlMembershipRole::Admin => MembershipRole::Admin,
            GqlMembershipRole::User => MembershipRole::User,
            GqlMembershipRole::Unknown => {
                report_error!(anyhow!(
                    "Invalid MembershipRole from server; treating as User"
                ));
                MembershipRole::User
            }
        }
    }
}

impl From<MembershipRole> for GqlMembershipRole {
    fn from(role: MembershipRole) -> Self {
        match role {
            MembershipRole::Owner => GqlMembershipRole::Owner,
            MembershipRole::Admin => GqlMembershipRole::Admin,
            MembershipRole::User => GqlMembershipRole::User,
        }
    }
}

impl From<GqlTeamVisibility> for TeamVisibility {
    fn from(visibility: GqlTeamVisibility) -> Self {
        match visibility {
            GqlTeamVisibility::Open => TeamVisibility::Open,
            GqlTeamVisibility::Private => TeamVisibility::Private,
            GqlTeamVisibility::Hidden => TeamVisibility::Hidden,
            GqlTeamVisibility::Other(value) => {
                report_error!(
                    "Invalid TeamVisibility from server; treating as Private",
                    extra: { "value" => %value },
                    warp_errors::ReportErrorLogMode::OncePerRun
                );
                // Fail closed: an unrecognized value must not be treated as Open,
                // since that would surface the invite-by-link control.
                TeamVisibility::Private
            }
        }
    }
}

impl From<GqlWorkspaceMember> for WorkspaceMember {
    fn from(gql_workspace_member: GqlWorkspaceMember) -> WorkspaceMember {
        Self {
            uid: UserUid::new(&gql_workspace_member.uid.into_inner()),
            email: gql_workspace_member.email,
            role: gql_workspace_member.role.into(),
            is_disabled: gql_workspace_member.is_disabled,
        }
    }
}

impl From<GqlEmailInvite> for EmailInvite {
    fn from(gql_email_invite: GqlEmailInvite) -> EmailInvite {
        Self {
            invitee_email: gql_email_invite.email,
            expired: gql_email_invite.expired,
            team_uid: gql_email_invite
                .team_uid
                .map(|uid| ServerId::from_string_lossy(uid.into_inner())),
        }
    }
}

impl From<GqlInviteLinkDomainRestriction> for InviteLinkDomainRestriction {
    fn from(
        gql_invite_link_domain_restriction: GqlInviteLinkDomainRestriction,
    ) -> InviteLinkDomainRestriction {
        InviteLinkDomainRestriction {
            uid: ServerId::from_string_lossy(gql_invite_link_domain_restriction.uid.inner()),
            domain: gql_invite_link_domain_restriction.domain,
        }
    }
}

impl From<GqlTeamSizePolicy> for WorkspaceSizePolicy {
    fn from(gql_workspace_size_policy: GqlTeamSizePolicy) -> WorkspaceSizePolicy {
        Self {
            is_unlimited: gql_workspace_size_policy.is_unlimited,
            limit: i64::from(gql_workspace_size_policy.limit),
        }
    }
}

impl From<GqlSharedNotebooksPolicy> for SharedNotebooksPolicy {
    fn from(gql_shared_notebooks_policy: GqlSharedNotebooksPolicy) -> SharedNotebooksPolicy {
        Self {
            is_unlimited: gql_shared_notebooks_policy.is_unlimited,
            limit: i64::from(gql_shared_notebooks_policy.limit),
        }
    }
}

impl From<GqlSharedWorkflowsPolicy> for SharedWorkflowsPolicy {
    fn from(gql_shared_workflows_policy: GqlSharedWorkflowsPolicy) -> SharedWorkflowsPolicy {
        Self {
            is_unlimited: gql_shared_workflows_policy.is_unlimited,
            limit: i64::from(gql_shared_workflows_policy.limit),
        }
    }
}

impl From<GqlSessionSharingPolicy> for SessionSharingPolicy {
    fn from(gql_session_sharing_policy: GqlSessionSharingPolicy) -> SessionSharingPolicy {
        Self {
            is_enabled: gql_session_sharing_policy.enabled,
            max_session_size: u64::try_from(gql_session_sharing_policy.max_session_bytes_size)
                .unwrap_or_default(),
        }
    }
}

impl From<GqlTelemetryDataCollectionPolicy> for TelemetryDataCollectionPolicy {
    fn from(
        gql_telemetry_data_collection_policy: GqlTelemetryDataCollectionPolicy,
    ) -> TelemetryDataCollectionPolicy {
        Self {
            default: gql_telemetry_data_collection_policy.default,
            toggleable: gql_telemetry_data_collection_policy.toggleable,
        }
    }
}

impl From<GqlMultiAdminPolicy> for MultiAdminPolicy {
    fn from(gql_policy: GqlMultiAdminPolicy) -> MultiAdminPolicy {
        Self {
            enabled: gql_policy.enabled,
        }
    }
}

impl From<GqlNativeWorkspacesPolicy> for NativeWorkspacesPolicy {
    fn from(gql_policy: GqlNativeWorkspacesPolicy) -> NativeWorkspacesPolicy {
        Self {
            enabled: gql_policy.enabled,
        }
    }
}

impl From<GqlTier> for Tier {
    fn from(gql_tier: GqlTier) -> Tier {
        Self {
            name: gql_tier.name,
            description: gql_tier.description,
            workspace_size_policy: gql_tier.team_size_policy.map(From::from),
            shared_notebooks_policy: gql_tier.shared_notebooks_policy.map(From::from),
            shared_workflows_policy: gql_tier.shared_workflows_policy.map(From::from),
            session_sharing_policy: gql_tier.session_sharing_policy.map(From::from),
            telemetry_data_collection_policy: gql_tier
                .telemetry_data_collection_policy
                .map(From::from),
            multi_admin_policy: gql_tier.multi_admin_policy.map(From::from),
            native_workspaces_policy: gql_tier.native_workspaces_policy.map(From::from),
        }
    }
}

impl From<GqlCustomerType> for CustomerType {
    fn from(gql_customer_type: GqlCustomerType) -> CustomerType {
        match gql_customer_type {
            GqlCustomerType::Free => CustomerType::Free,
            GqlCustomerType::Turbo => CustomerType::Turbo,
            GqlCustomerType::SelfServe => CustomerType::SelfServe,
            GqlCustomerType::Prosumer => CustomerType::Prosumer,
            GqlCustomerType::Legacy => CustomerType::Legacy,
            GqlCustomerType::Enterprise => CustomerType::Enterprise,
            GqlCustomerType::Business => CustomerType::Business,
            GqlCustomerType::Lightspeed => CustomerType::Lightspeed,
            GqlCustomerType::Build => CustomerType::Build,
            GqlCustomerType::BuildMax => CustomerType::BuildMax,
            GqlCustomerType::ProTrial | GqlCustomerType::TeamTrial | GqlCustomerType::Other(_) => {
                CustomerType::Unknown
            }
        }
    }
}

impl From<GqlDelinquencyStatus> for DelinquencyStatus {
    fn from(gql_delinquency_status: GqlDelinquencyStatus) -> DelinquencyStatus {
        match gql_delinquency_status {
            GqlDelinquencyStatus::NoDelinquency => DelinquencyStatus::NoDelinquency,
            GqlDelinquencyStatus::PastDue => DelinquencyStatus::PastDue,
            GqlDelinquencyStatus::Unpaid => DelinquencyStatus::Unpaid,
            GqlDelinquencyStatus::TeamLimitExceeded => DelinquencyStatus::TeamLimitExceeded,
            GqlDelinquencyStatus::Other(_) => DelinquencyStatus::Unknown,
        }
    }
}

impl From<GqlBillingMetadata> for BillingMetadata {
    fn from(gql_billing_metadata: GqlBillingMetadata) -> BillingMetadata {
        Self {
            tier: gql_billing_metadata.tier.into(),
            customer_type: gql_billing_metadata.customer_type.into(),
            delinquency_status: gql_billing_metadata.delinquency_status.into(),
            service_agreements: gql_billing_metadata.service_agreements,
        }
    }
}

impl TryFrom<&BillingMetadata> for StripeSubscriptionPlan {
    type Error = ();

    fn try_from(billing_metadata: &BillingMetadata) -> Result<Self, Self::Error> {
        match billing_metadata.customer_type {
            CustomerType::Turbo => Ok(StripeSubscriptionPlan::Turbo),
            CustomerType::SelfServe => Ok(StripeSubscriptionPlan::Team),
            CustomerType::Prosumer => Ok(StripeSubscriptionPlan::Pro),
            CustomerType::Business => {
                // Check if this is a legacy Business Plan, or a new Build Business plan based on service agreement type
                // See: https://github.com/warpdotdev/warp-server/pull/6828#discussion_r2496242091
                match billing_metadata
                    .service_agreements
                    .first()
                    .map(|sa| sa.type_.clone())
                {
                    Some(ServiceAgreementType::SelfServe) => {
                        Ok(StripeSubscriptionPlan::BuildBusiness)
                    }
                    _ => Ok(StripeSubscriptionPlan::Business),
                }
            }
            CustomerType::Lightspeed => Ok(StripeSubscriptionPlan::Lightspeed),
            CustomerType::Build => Ok(StripeSubscriptionPlan::Build),
            CustomerType::BuildMax => Ok(StripeSubscriptionPlan::BuildMax),
            // legacy customer types we don't support anymore, or customer types that don't get billed via stripe
            CustomerType::Free
            | CustomerType::Legacy
            | CustomerType::Enterprise
            | CustomerType::Unknown => Err(()),
        }
    }
}

/// Compiles each remote-session command pattern into a [`Regex`], dropping (and reporting) any
/// pattern that fails to compile so one bad entry in an org's configuration cannot suppress the
/// rest of the list.
///
/// Throttled to once per run: an uncompilable pattern is a static configuration problem that
/// does not resolve itself between polls of the workspaces-metadata query, so reporting it every
/// time would page the same broken pattern at the poll rate for every affected user.
fn compile_remote_session_regex_list(patterns: impl IntoIterator<Item = String>) -> Vec<Regex> {
    patterns
        .into_iter()
        .filter_map(|pattern| match Regex::new(&pattern) {
            Ok(regex) => Some(regex),
            Err(_) => {
                report_error!(
                    "Invalid regex pattern for remote session detection",
                    extra: { "pattern" => %pattern },
                    warp_errors::ReportErrorLogMode::OncePerRun
                );
                None
            }
        })
        .collect()
}

impl From<GqlWorkspaceSettings> for WorkspaceSettings {
    fn from(gql_workspace_settings: GqlWorkspaceSettings) -> WorkspaceSettings {
        Self {
            remote_session_regex_list: compile_remote_session_regex_list(
                gql_workspace_settings
                    .ai_permissions_settings
                    .remote_session_regex_list,
            ),
            telemetry_settings: TelemetrySettings {
                force_enabled: gql_workspace_settings.telemetry_settings.force_enabled,
            },
            link_sharing_settings: LinkSharingSettings {
                anyone_with_link_sharing_enabled: gql_workspace_settings
                    .link_sharing_settings
                    .anyone_with_link_sharing_enabled,
                direct_link_sharing_enabled: gql_workspace_settings
                    .link_sharing_settings
                    .direct_link_sharing_enabled,
            },
            secret_redaction_settings: SecretRedactionSettings {
                enabled: gql_workspace_settings.secret_redaction_settings.enabled,
                regexes: gql_workspace_settings
                    .secret_redaction_settings
                    .regexes
                    .into_iter()
                    .map(|gql_regex| EnterpriseSecretRegex {
                        pattern: gql_regex.pattern,
                        name: gql_regex.name,
                    })
                    .collect(),
            },
            is_invite_link_enabled: gql_workspace_settings.is_invite_link_enabled,
            is_discoverable: gql_workspace_settings.is_discoverable,
        }
    }
}

impl From<GqlTeamSettings> for TeamSettings {
    fn from(gql_team_settings: GqlTeamSettings) -> TeamSettings {
        let map_regexes =
            |regexes: Vec<warp_graphql::workspace::SecretRedactionRegex>| -> Vec<EnterpriseSecretRegex> {
                regexes
                    .into_iter()
                    .map(|gql_regex| EnterpriseSecretRegex {
                        pattern: gql_regex.pattern,
                        name: gql_regex.name,
                    })
                    .collect()
            };
        Self {
            remote_session_regex_list: compile_remote_session_regex_list(
                gql_team_settings
                    .ai_permissions
                    .remote_session_regex_list
                    .values,
            ),
            secret_redaction: TeamSecretRedactionSettings {
                enabled: EnforceableSetting {
                    value: gql_team_settings.secret_redaction.enabled.value,
                    is_enforced_by_workspace: gql_team_settings
                        .secret_redaction
                        .enabled
                        .is_enforced_by_workspace,
                },
                regexes: SplitListSetting {
                    values: map_regexes(gql_team_settings.secret_redaction.regexes.values),
                    workspace_entries: map_regexes(
                        gql_team_settings.secret_redaction.regexes.workspace_entries,
                    ),
                    team_entries: map_regexes(
                        gql_team_settings.secret_redaction.regexes.team_entries,
                    ),
                },
            },
            link_sharing: TeamLinkSharingSettings {
                anyone_with_link_sharing_enabled: EnforceableSetting {
                    value: gql_team_settings
                        .link_sharing
                        .anyone_with_link_sharing_enabled
                        .value,
                    is_enforced_by_workspace: gql_team_settings
                        .link_sharing
                        .anyone_with_link_sharing_enabled
                        .is_enforced_by_workspace,
                },
                direct_link_sharing_enabled: EnforceableSetting {
                    value: gql_team_settings
                        .link_sharing
                        .direct_link_sharing_enabled
                        .value,
                    is_enforced_by_workspace: gql_team_settings
                        .link_sharing
                        .direct_link_sharing_enabled
                        .is_enforced_by_workspace,
                },
            },
            telemetry_settings: TelemetrySettings {
                force_enabled: gql_team_settings.telemetry_settings.force_enabled,
            },
        }
    }
}

/// Derives a team's effective settings from the GraphQL payload. The settings
/// always come from the **team** payload (`gql_team.settings`), never from a
/// clone of the workspace settings. Workspace-scoped flags such as
/// discoverability are intentionally not part of `TeamSettings` and are read
/// from the workspace settings at their call sites.
///
/// Extracted from [`Team::from_gql`] so the team-payload sourcing is
/// unit-testable without constructing a full `GqlWorkspace`.
pub(crate) fn team_settings_from_gql(team_settings: GqlTeamSettings) -> TeamSettings {
    team_settings.into()
}

pub(crate) fn team_pending_email_invites_from_gql(
    workspace_pending_email_invites: &[GqlEmailInvite],
    team_uid: &cynic::Id,
) -> Vec<EmailInvite> {
    workspace_pending_email_invites
        .iter()
        .filter(|invite| invite.team_uid.as_ref() == Some(team_uid))
        .cloned()
        .map(Into::into)
        .collect()
}

impl Team {
    pub fn from_gql(gql_workspace: GqlWorkspace, gql_team: GqlTeam) -> Team {
        Self {
            uid: ServerId::from_string_lossy(gql_team.uid.inner()),
            name: gql_team.name.clone(),
            color: gql_team.color.clone(),
            members: gql_team
                .members
                .clone()
                .into_iter()
                .map(|gql_member| gql_member.into())
                .collect(),
            invite_link: gql_team.invite_link.clone(),
            pending_email_invites: team_pending_email_invites_from_gql(
                &gql_workspace.pending_email_invites,
                &gql_team.uid,
            ),
            invite_link_domain_restrictions: gql_workspace
                .invite_link_domain_restrictions
                .clone()
                .into_iter()
                .map(|gql_domain_restriction| gql_domain_restriction.into())
                .collect(),
            billing_metadata: gql_workspace.billing_metadata.clone().into(),
            stripe_customer_id: gql_workspace
                .stripe_customer_id
                .as_ref()
                .map(|id| id.clone().into_inner()),
            // Team-effective settings come from the team payload, not from a
            // clone of the workspace settings.
            settings: team_settings_from_gql(gql_team.settings),
            is_eligible_for_discovery: gql_workspace.is_eligible_for_discovery,
            visibility: gql_team.visibility.into(),
        }
    }
}

impl From<GqlWorkspace> for Workspace {
    fn from(gql_workspace: GqlWorkspace) -> Workspace {
        Self {
            uid: ServerId::from_string_lossy(gql_workspace.uid.inner()).into(),
            name: gql_workspace.name.clone(),
            stripe_customer_id: gql_workspace
                .stripe_customer_id
                .as_ref()
                .map(|id| id.clone().into_inner()),
            teams: gql_workspace
                .teams
                .clone()
                .into_iter()
                .map(|gql_team| Team::from_gql(gql_workspace.clone(), gql_team))
                .collect(),
            billing_metadata: gql_workspace.billing_metadata.clone().into(),
            settings: gql_workspace.settings.clone().into(),
            invite_link_domain_restrictions: gql_workspace
                .invite_link_domain_restrictions
                .clone()
                .into_iter()
                .map(|gql_domain_restriction| gql_domain_restriction.into())
                .collect(),
            pending_email_invites: gql_workspace
                .pending_email_invites
                .clone()
                .into_iter()
                .map(|gql_email_invite| gql_email_invite.into())
                .collect(),
            is_eligible_for_discovery: gql_workspace.is_eligible_for_discovery,
            members: gql_workspace
                .members
                .clone()
                .into_iter()
                .map(|gql_member| gql_member.into())
                .collect(),
        }
    }
}

/// Converts the `GetWorkspacesMetadataForUser` response into [`WorkspacesMetadataResponse`].
///
/// `is_service_account` controls whether [`retain_authenticated_teams`] filters each
/// workspace's teams down to the caller's own human memberships; see that function's doc
/// comment for why service accounts must skip it.
pub fn workspaces_metadata_response_from_gql(
    gql_user: GqlUser,
    is_service_account: bool,
) -> WorkspacesMetadataResponse {
    let user_uid = UserUid::new(&gql_user.profile.uid);

    let workspaces: Vec<Workspace> = gql_user
        .workspaces
        .clone()
        .into_iter()
        .filter(|gql_workspace| {
            // TODO(skambashi): REV-717: Clean up this code once every user always has
            // a workspace, and the server no longer returns a placeholder workspace.
            gql_workspace.uid != PLACEHOLDER_WORKSPACE_UID.into()
        })
        .map(|gql_workspace| {
            let mut workspace = gql_workspace.into();
            retain_authenticated_teams(&mut workspace, user_uid, is_service_account);
            workspace
        })
        .collect();

    let experiments = gql_user
        .experiments
        .and_then(|experiments| convert_to_server_experiment!(experiments));

    // TODO(skambashi) refactor to return back workspaces, and not teams
    WorkspacesMetadataResponse {
        workspaces,
        experiments,
    }
}

#[cfg(test)]
#[path = "gql_convert_tests.rs"]
mod tests;

pub fn object_update_message_from_gql(value: WarpDriveUpdate) -> Result<ObjectUpdateMessage> {
    match value {
        WarpDriveUpdate::ObjectActionOccurred(message) => {
            Ok(ObjectUpdateMessage::ObjectActionOccurred {
                history: object_action_history_from_gql(message.history)?,
            })
        }
        WarpDriveUpdate::ObjectContentUpdated(message) => {
            let server_object = message.object.try_into()?;
            let last_editor = message.last_editor.map(|e| e.into());
            Ok(ObjectUpdateMessage::ObjectContentChanged {
                server_object: Box::new(server_object),
                last_editor,
            })
        }
        WarpDriveUpdate::ObjectDeleted(message) => Ok(ObjectUpdateMessage::ObjectDeleted {
            object_uid: ServerId::from_string_lossy(message.object_uid.inner()),
        }),
        WarpDriveUpdate::ObjectMetadataUpdated(message) => {
            Ok(ObjectUpdateMessage::ObjectMetadataChanged {
                metadata: message.metadata.try_into()?,
            })
        }
        WarpDriveUpdate::ObjectPermissionsUpdated(message) => {
            Ok(ObjectUpdateMessage::ObjectPermissionsChangedV2 {
                object_uid: ServerId::from_string_lossy(message.object_uid.inner()),
                user_profiles: message
                    .user_profiles
                    .into_iter()
                    .flatten()
                    .map(Into::into)
                    .collect(),
                permissions: message.permissions.try_into()?,
            })
        }
        WarpDriveUpdate::TeamMembershipsChanged(_) => {
            Ok(ObjectUpdateMessage::TeamMembershipsChanged)
        }
        WarpDriveUpdate::Unknown => bail!("Unexpected WarpDriveUpdate variant"),
    }
}
