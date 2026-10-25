use super::team_workspace_settings::TeamScope;
use regex::Regex;
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use warp_graphql::billing::{
    BillingMetadata as GqlBillingMetadata, CustomerType as GqlCustomerType,
    DelinquencyStatus as GqlDelinquencyStatus, Tier as GqlTier,
};
use warp_graphql::queries::get_workspaces_metadata_for_user::{
    User as GqlUser, UserProfile as GqlUserProfile,
};
use warp_graphql::workspace::{
    BooleanSettingInfo as GqlBooleanSettingInfo, LinkSharingSettings as GqlLinkSharingSettings,
    LinkSharingSettingsInfo as GqlLinkSharingSettingsInfo, MembershipRole as GqlMembershipRole,
    SecretRedactionRegexListInfo as GqlSecretRedactionRegexListInfo,
    SecretRedactionSettings as GqlSecretRedactionSettings,
    SecretRedactionSettingsInfo as GqlSecretRedactionSettingsInfo, Team as GqlTeam,
    TeamMember as GqlTeamMember, TeamSettings as GqlTeamSettings,
    TeamVisibility as GqlTeamVisibility, TelemetrySettings as GqlTelemetrySettings,
    Workspace as GqlWorkspace, WorkspaceSettings as GqlWorkspaceSettings,
};
use warpui::elements::Empty;
use warpui::platform::WindowStyle;
use warpui::{AddSingletonModel, App, Element, TypedActionView, View, ViewHandle, WindowId};

use super::*;
use crate::workspaces::gql_convert::workspaces_metadata_response_from_gql;
use crate::workspaces::team::{Team, TeamVisibility};
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::Workspace;

impl UserWorkspaces {
    pub fn set_team_for_window(
        &mut self,
        window_id: WindowId,
        team_uid: ServerId,
        ctx: &mut ModelContext<Self>,
    ) {
        let window_team_uid = self.window_team_uids.entry(window_id).or_default();
        if window_team_uid.is_none() {
            *window_team_uid = Some(team_uid);
            ctx.emit(UserWorkspacesEvent::WindowTeam);
            ctx.notify();
        }
    }

    pub fn has_teams(&self) -> bool {
        if let Some(workspace) = self.current_workspace() {
            !workspace.teams.is_empty()
        } else {
            false
        }
    }
}

fn initialize_window_team_test_app(app: &mut App, workspaces: Vec<Workspace>) {
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(|ctx| UserWorkspaces::mock(workspaces, ctx));
}

fn team_for_test() -> Team {
    Team {
        uid: 123.into(),
        name: "test".to_string(),
        color: None,
        invite_link: None,
        members: vec![],
        pending_email_invites: vec![],
        invite_link_domain_restrictions: vec![],
        billing_metadata: Default::default(),
        stripe_customer_id: None,
        settings: Default::default(),
        is_eligible_for_discovery: false,
        visibility: TeamVisibility::Open,
    }
}

fn workspace_for_test(team: &Team) -> Workspace {
    Workspace {
        uid: "workspace_uid123456789".to_string().into(),
        name: "test".to_string(),
        stripe_customer_id: None,
        teams: vec![team.clone()],
        billing_metadata: team.billing_metadata.clone(),
        settings: Default::default(),
        invite_link_domain_restrictions: vec![],
        pending_email_invites: vec![],
        is_eligible_for_discovery: false,
        members: vec![],
    }
}

#[test]
fn test_current_workspace_billing_metadata_uses_selected_teamless_workspace() {
    let first_team = team_for_test();
    let first_workspace = workspace_for_test(&first_team);
    let mut second_workspace = workspace_for_test(&first_team);
    second_workspace.uid = "workspace_uid987654321".to_string().into();
    second_workspace.teams.clear();
    second_workspace.billing_metadata.customer_type = CustomerType::Enterprise;
    let second_workspace_uid = second_workspace.uid;

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![first_workspace, second_workspace]);

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_current_workspace_uid(second_workspace_uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .current_workspace()
                    .map(|workspace| workspace.billing_metadata.customer_type),
                Some(CustomerType::Enterprise)
            );
        });
    })
}
#[test]
fn test_window_team_assignment_is_immutable() {
    let first_team = team_for_test();
    let mut second_team = team_for_test();
    second_team.uid = 456.into();
    second_team.name = "second".to_string();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(second_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, second_team.uid, ctx);
            user_workspaces.set_team_for_window(window_id, first_team.uid, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.team_uid_for_window(window_id),
                Some(second_team.uid)
            );
            assert_eq!(
                user_workspaces
                    .team_for_window(window_id)
                    .map(|team| team.uid),
                Some(second_team.uid)
            );
        });
    })
}

#[test]
fn test_window_team_assignment_inherits_from_source_or_default_team() {
    let first_team = team_for_test();
    let mut second_team = team_for_test();
    second_team.uid = 456.into();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(second_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let source_window_id = WindowId::new();
        let inherited_window_id = WindowId::new();
        let fallback_window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(source_window_id, second_team.uid, ctx);
            let inherited_team_uid =
                user_workspaces.inherited_or_default_team_uid(Some(source_window_id));
            let fallback_team_uid = user_workspaces.inherited_or_default_team_uid(None);
            user_workspaces.register_window(inherited_window_id, inherited_team_uid, ctx);
            user_workspaces.register_window(fallback_window_id, fallback_team_uid, ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.team_uid_for_window(inherited_window_id),
                Some(second_team.uid)
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(fallback_window_id),
                Some(first_team.uid)
            );
        });
    })
}

#[test]
fn test_every_departed_team_window_falls_back_when_team_is_removed() {
    let first_team = team_for_test();
    let mut removed_team = team_for_test();
    removed_team.uid = 456.into();
    let mut workspace = workspace_for_test(&first_team);
    workspace.teams.push(removed_team.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace.clone()]);
        let first_removed_team_window = WindowId::new();
        let second_removed_team_window = WindowId::new();
        let existing_fallback_team_window = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(first_removed_team_window, removed_team.uid, ctx);
            user_workspaces.set_team_for_window(second_removed_team_window, removed_team.uid, ctx);
            user_workspaces.set_team_for_window(existing_fallback_team_window, first_team.uid, ctx);
            workspace.teams.retain(|team| team.uid != removed_team.uid);
            user_workspaces.update_workspaces(vec![workspace], ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces.team_uid_for_window(first_removed_team_window),
                Some(first_team.uid)
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(second_removed_team_window),
                Some(first_team.uid)
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(existing_fallback_team_window),
                Some(first_team.uid)
            );
        });
    })
}

#[test]
fn test_every_departed_team_window_becomes_teamless_without_remaining_memberships() {
    let removed_team = team_for_test();
    let mut workspace = workspace_for_test(&removed_team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace.clone()]);

        let first_window = WindowId::new();
        let second_window = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(first_window, removed_team.uid, ctx);
            user_workspaces.set_team_for_window(second_window, removed_team.uid, ctx);
            workspace.teams.clear();
            user_workspaces.update_workspaces(vec![workspace], ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(user_workspaces.team_uid_for_window(first_window), None);
            assert_eq!(user_workspaces.team_uid_for_window(second_window), None);
        });
    })
}

#[test]
fn test_window_team_assignment_reconciles_when_current_workspace_changes() {
    let first_team = team_for_test();
    let first_workspace = workspace_for_test(&first_team);
    let mut second_team = team_for_test();
    second_team.uid = 456.into();
    let mut second_workspace = workspace_for_test(&second_team);
    second_workspace.uid = "workspace_uid987654321".to_string().into();
    let second_workspace_uid = second_workspace.uid;

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![first_workspace, second_workspace]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
            user_workspaces.set_current_workspace_uid(second_workspace_uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                Some(second_team.uid)
            );
        });
    })
}

#[derive(Default)]
struct TeamContextTestView;

impl Entity for TeamContextTestView {
    type Event = ();
}

impl View for TeamContextTestView {
    fn ui_name() -> &'static str {
        "TeamContextTestView"
    }

    fn render(&self, _: &AppContext) -> Box<dyn Element> {
        Empty::new().finish()
    }
}

impl TypedActionView for TeamContextTestView {
    type Action = ();
}

fn create_test_window(app: &mut App) -> (WindowId, ViewHandle<TeamContextTestView>) {
    app.add_window(WindowStyle::NotStealFocus, |_| TeamContextTestView)
}

fn two_teams() -> (Team, Team) {
    let team_a = team_for_test();
    let mut team_b = team_for_test();
    team_b.uid = 456.into();
    team_b.name = "team-b".to_string();
    (team_a, team_b)
}

/// Reconciliation updates the team used by a view's live render context.
#[test]
fn test_window_team_reconciliation_moves_rendering_to_the_new_team() {
    let (team_a, team_b) = two_teams();
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams.push(team_b.clone());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        let weak_view = view.downgrade();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_id, team_a.uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .team_context(&weak_view, ctx)
                    .team_uid(),
                Some(team_a.uid),
            );
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace_for_test(&team_b)], ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx)
                    .team_context(&weak_view, ctx)
                    .team_uid(),
                Some(team_b.uid),
                "a freshly resolved render context should follow the window to team B"
            );
        });
    })
}

fn set_team_remote_session_policy(team: &mut Team, patterns: &[&str]) {
    team.settings.remote_session_regex_list = patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("test pattern should compile"))
        .collect();
}

fn set_workspace_remote_session_policy(workspace: &mut Workspace, patterns: &[&str]) {
    workspace.settings.remote_session_regex_list = patterns
        .iter()
        .map(|pattern| Regex::new(pattern).expect("test pattern should compile"))
        .collect();
}

fn remote_session_patterns_for_surface(
    view: &ViewHandle<TeamContextTestView>,
    app: &AppContext,
) -> HashSet<String> {
    let user_workspaces = UserWorkspaces::as_ref(app);
    let scope = user_workspaces.team_context(&view.downgrade(), app);
    user_workspaces
        .get_remote_session_regex_list(&scope)
        .iter()
        .map(|regex| regex.as_str().to_string())
        .collect()
}

#[test]
fn remote_session_policy_follows_each_surfaces_own_team() {
    let (mut team_a, mut team_b) = two_teams();
    set_team_remote_session_policy(&mut team_a, &["^kubectl"]);
    set_team_remote_session_policy(&mut team_b, &["^ssh"]);
    let mut workspace = workspace_for_test(&team_a);
    workspace.teams = vec![team_a.clone(), team_b.clone()];

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_a, view_a) = create_test_window(&mut app);
        let (window_b, view_b) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.set_team_for_window(window_a, team_a.uid, ctx);
            user_workspaces.set_team_for_window(window_b, team_b.uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                remote_session_patterns_for_surface(&view_a, ctx),
                HashSet::from(["^kubectl".to_string()])
            );
            assert_eq!(
                remote_session_patterns_for_surface(&view_b, ctx),
                HashSet::from(["^ssh".to_string()])
            );
        });
    })
}

#[test]
fn remote_session_policy_falls_back_to_the_workspace_for_a_user_with_no_teams() {
    let team = team_for_test();
    let mut workspace = workspace_for_test(&team);
    workspace.teams.clear();
    set_workspace_remote_session_policy(&mut workspace, &["^workspace-only"]);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                remote_session_patterns_for_surface(&view, ctx),
                HashSet::from(["^workspace-only".to_string()])
            );
        });
    })
}

fn team_named(uid: i64, name: &str) -> Team {
    let mut team = team_for_test();
    team.uid = uid.into();
    team.name = name.to_owned();
    team
}

/// Two teams in workspace order, so a test can tell the default apart from a chosen team.
fn platform_and_security() -> (Team, Team, Workspace) {
    let platform = team_named(123, "Platform");
    let security = team_named(456, "Security");
    let mut workspace = workspace_for_test(&platform);
    workspace.teams.push(security.clone());
    (platform, security, workspace)
}

#[test]
fn switching_a_window_to_a_team_overwrites_and_announces_it() {
    let (platform, security, workspace) = platform_and_security();

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let window_id = WindowId::new();
        let changes = Rc::new(Cell::new(0));
        let changes_for_subscription = changes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, event, _| {
                if matches!(event, UserWorkspacesEvent::WindowTeam) {
                    changes_for_subscription.set(changes_for_subscription.get() + 1);
                }
            });
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, Some(platform.uid), ctx);
        });
        let changes_after_register = changes.get();

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.switch_window_to_team(window_id, security.uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                Some(security.uid),
                "switching must overwrite, unlike the insert-only registration paths"
            );
        });
        assert_eq!(
            changes.get(),
            changes_after_register + 1,
            "the switch must announce itself so scoped consumers resync"
        );
    })
}

#[test]
fn switching_a_window_to_its_current_team_announces_nothing() {
    let team = team_for_test();
    let workspace = workspace_for_test(&team);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, Some(team.uid), ctx);
        });

        let changes = Rc::new(Cell::new(0));
        let changes_for_subscription = changes.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, event, _| {
                if matches!(event, UserWorkspacesEvent::WindowTeam) {
                    changes_for_subscription.set(changes_for_subscription.get() + 1);
                }
            });
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.switch_window_to_team(window_id, team.uid, ctx);
        });

        assert_eq!(changes.get(), 0);
    })
}

#[test]
fn metadata_update_retains_memberships_and_preserves_the_current_window() {
    let (platform, security, joined_workspace) = platform_and_security();
    let initial_workspace = workspace_for_test(&platform);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![initial_workspace]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, Some(platform.uid), ctx);
            user_workspaces.update_workspaces(vec![joined_workspace], ctx);
        });

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                user_workspaces
                    .current_workspace()
                    .expect("workspace should remain selected")
                    .teams
                    .iter()
                    .map(|team| team.uid)
                    .collect::<Vec<_>>(),
                vec![platform.uid, security.uid]
            );
            assert_eq!(
                user_workspaces.team_uid_for_window(window_id),
                Some(platform.uid)
            );
        });
    })
}

/// The switcher's own visibility rule, which the TUI indicator reuses rather than
/// reimplementing so the two front-ends cannot disagree about who counts as multi-team.
#[test]
fn can_switch_teams_only_with_more_than_one_team() {
    let (_, _, two_team_workspace) = platform_and_security();
    let one_team_workspace = workspace_for_test(&team_for_test());

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![one_team_workspace]);
        app.read(|ctx| assert!(!UserWorkspaces::as_ref(ctx).can_switch_teams()));

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![two_team_workspace], ctx);
        });
        app.read(|ctx| assert!(UserWorkspaces::as_ref(ctx).can_switch_teams()));
    })
}

#[test]
fn test_team_contexts_represent_a_registered_teamless_window() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let (window_id, view) = create_test_window(&mut app);
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });

        let weak_view = view.downgrade();
        app.read(|ctx| {
            let context = UserWorkspaces::as_ref(ctx).team_context(&weak_view, ctx);
            assert_eq!(context.team_uid(), None);
        });
    })
}

#[test]
fn test_unassigned_window_is_initialized_after_workspace_metadata_loads() {
    let team = team_for_test();
    let workspace = workspace_for_test(&team);
    let workspace_uid = workspace.uid;

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let window_id = WindowId::new();
        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.register_window(window_id, None, ctx);
        });
        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                None
            );
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.update_workspaces(vec![workspace], ctx);
            user_workspaces.set_current_workspace_uid(workspace_uid, ctx);
        });

        app.read(|ctx| {
            assert_eq!(
                UserWorkspaces::as_ref(ctx).team_uid_for_window(window_id),
                Some(team.uid)
            );
        });
    })
}

#[test]
fn test_team_switcher_hidden_with_zero_teams() {
    // When the user is in no workspace / no teams, `can_switch_teams` must return
    // false so the pill does not render.
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);
        app.read(|ctx| {
            assert!(
                !UserWorkspaces::as_ref(ctx).can_switch_teams(),
                "0 teams: switcher should be hidden"
            );
        });
    })
}

#[test]
fn test_team_switcher_hidden_with_single_team() {
    // With exactly 1 team, `can_switch_teams` must return false.
    let team = team_for_test();
    let workspace = workspace_for_test(&team);
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        app.read(|ctx| {
            assert!(
                !UserWorkspaces::as_ref(ctx).can_switch_teams(),
                "1 team: switcher should be hidden"
            );
        });
    })
}

#[test]
fn test_team_switcher_visible_with_multiple_teams() {
    // With 2+ teams, `can_switch_teams` must return true so the pill is shown.
    let team1 = team_for_test();
    let mut team2 = team_for_test();
    team2.uid = 456.into();
    team2.name = "Second Team".to_string();
    let mut workspace = workspace_for_test(&team1);
    workspace.teams.push(team2);

    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![workspace]);
        app.read(|ctx| {
            assert!(
                UserWorkspaces::as_ref(ctx).can_switch_teams(),
                "2 teams: switcher should be visible"
            );
        });
    })
}

fn gql_tier() -> GqlTier {
    GqlTier {
        name: "Free".to_string(),
        description: "Free tier".to_string(),
        team_size_policy: None,
        session_sharing_policy: None,
        telemetry_data_collection_policy: None,
        multi_admin_policy: None,
        native_workspaces_policy: None,
    }
}

fn gql_workspace(uid: &str) -> GqlWorkspace {
    GqlWorkspace {
        uid: uid.into(),
        name: "workspace".to_string(),
        stripe_customer_id: None,
        members: vec![],
        teams: vec![],
        billing_metadata: GqlBillingMetadata {
            customer_type: GqlCustomerType::Free,
            delinquency_status: GqlDelinquencyStatus::NoDelinquency,
            tier: gql_tier(),
            service_agreements: vec![],
        },
        settings: GqlWorkspaceSettings {
            ai_permissions_settings: warp_graphql::workspace::AiPermissionsSettings {
                remote_session_regex_list: vec![],
            },
            is_discoverable: false,
            is_invite_link_enabled: false,
            telemetry_settings: GqlTelemetrySettings {
                force_enabled: false,
            },
            link_sharing_settings: GqlLinkSharingSettings {
                anyone_with_link_sharing_enabled: true,
                direct_link_sharing_enabled: true,
            },
            secret_redaction_settings: GqlSecretRedactionSettings {
                enabled: false,
                regexes: vec![],
            },
        },
        pending_email_invites: vec![],
        invite_link_domain_restrictions: vec![],
        is_eligible_for_discovery: false,
    }
}

/// Team settings with every group at its neutral value, so a fixture only has to
/// override the field the test cares about.
fn gql_team_settings() -> GqlTeamSettings {
    fn bool_info(value: bool) -> GqlBooleanSettingInfo {
        GqlBooleanSettingInfo {
            value,
            is_enforced_by_workspace: false,
        }
    }

    GqlTeamSettings {
        ai_permissions: warp_graphql::workspace::AiPermissionsSettingsInfo {
            remote_session_regex_list: warp_graphql::workspace::StringListSettingInfo {
                values: vec![],
            },
        },
        secret_redaction: GqlSecretRedactionSettingsInfo {
            enabled: bool_info(false),
            regexes: GqlSecretRedactionRegexListInfo {
                values: vec![],
                workspace_entries: vec![],
                team_entries: vec![],
            },
        },
        link_sharing: GqlLinkSharingSettingsInfo {
            anyone_with_link_sharing_enabled: bool_info(true),
            direct_link_sharing_enabled: bool_info(true),
        },
        telemetry_settings: GqlTelemetrySettings {
            force_enabled: false,
        },
    }
}

fn gql_team(uid: &str, name: &str, member_uids: &[&str]) -> GqlTeam {
    GqlTeam {
        // `ServerId` rejects anything but a 22-character id.
        uid: format!("{uid:0>22}").into(),
        name: name.to_string(),
        color: None,
        members: member_uids
            .iter()
            .map(|member_uid| GqlTeamMember {
                uid: (*member_uid).into(),
                email: format!("{member_uid}@example.com"),
                role: GqlMembershipRole::User,
                is_disabled: false,
            })
            .collect(),
        settings: gql_team_settings(),
        invite_link: None,
        visibility: GqlTeamVisibility::Open,
    }
}

fn apply_workspaces_metadata(app: &mut App, metadata: Vec<Workspace>) {
    UserWorkspaces::handle(app).update(app, |user_workspaces, ctx| {
        let first_workspace_uid = metadata.first().map(|workspace| workspace.uid);
        user_workspaces.update_workspaces(metadata, ctx);
        if user_workspaces.current_workspace().is_none()
            && let Some(workspace_uid) = first_workspace_uid
        {
            user_workspaces.set_current_workspace_uid(workspace_uid, ctx);
        }
    });
}

fn current_team_names(user_workspaces: &UserWorkspaces) -> Vec<String> {
    user_workspaces
        .current_workspace()
        .map(|workspace| {
            workspace
                .teams
                .iter()
                .map(|team| team.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn test_team_switcher_drops_teams_the_admin_is_not_a_member_of() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        // The server hands a workspace admin every team in the workspace, but only
        // the team they actually joined is one they can operate as in the client.
        let mut workspace = gql_workspace("workspace_uid123456789");
        workspace.teams = vec![
            gql_team("member-team", "Member Team", &["test-user"]),
            gql_team("other-team", "Other Team", &["someone-else"]),
        ];

        apply_workspaces_metadata(
            &mut app,
            workspaces_metadata_response_from_gql(gql_user(vec![workspace]), false),
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(current_team_names(user_workspaces), ["Member Team"]);
            assert!(
                !user_workspaces.can_switch_teams(),
                "a single membership should hide the switcher"
            );
        });
    })
}

#[test]
fn test_team_switcher_keeps_every_team_the_user_is_a_member_of() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        let mut workspace = gql_workspace("workspace_uid123456789");
        workspace.teams = vec![
            gql_team("first-team", "First Team", &["test-user"]),
            gql_team("other-team", "Other Team", &["someone-else"]),
            gql_team("second-team", "Second Team", &["test-user"]),
        ];

        apply_workspaces_metadata(
            &mut app,
            workspaces_metadata_response_from_gql(gql_user(vec![workspace]), false),
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert_eq!(
                current_team_names(user_workspaces),
                ["First Team", "Second Team"]
            );
            assert!(
                user_workspaces.can_switch_teams(),
                "multiple memberships should keep the switcher visible"
            );
        });
    })
}

#[test]
fn test_nonmember_admin_remains_teamless() {
    App::test((), |mut app| async move {
        initialize_window_team_test_app(&mut app, vec![]);

        // A workspace admin who joined none of the workspace's teams is teamless
        // in the client, so the workspace's own settings supply their defaults.
        let mut workspace = gql_workspace("workspace_uid123456789");
        workspace.teams = vec![gql_team("other-team", "Other Team", &["someone-else"])];

        apply_workspaces_metadata(
            &mut app,
            workspaces_metadata_response_from_gql(gql_user(vec![workspace]), false),
        );

        app.read(|ctx| {
            let user_workspaces = UserWorkspaces::as_ref(ctx);
            assert!(
                !user_workspaces.has_teams(),
                "an admin with no membership should end up teamless"
            );
        });
    })
}

fn gql_user(workspaces: Vec<GqlWorkspace>) -> GqlUser {
    GqlUser {
        profile: GqlUserProfile {
            uid: "test-user".to_string(),
        },
        workspaces,
    }
}
