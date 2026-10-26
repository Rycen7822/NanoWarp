use std::cell::Cell;
use std::rc::Rc;
use warpui::{AddSingletonModel, App, WindowId};

use super::*;
use crate::settings::PrivacySettings;
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
            ctx.emit(());
            ctx.notify();
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
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, _, _| {
                changes_for_subscription.set(changes_for_subscription.get() + 1);
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
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, _, _| {
                changes_for_subscription.set(changes_for_subscription.get() + 1);
            });
        });

        UserWorkspaces::handle(&app).update(&mut app, |user_workspaces, ctx| {
            user_workspaces.switch_window_to_team(window_id, team.uid, ctx);
        });

        assert_eq!(changes.get(), 0);
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
