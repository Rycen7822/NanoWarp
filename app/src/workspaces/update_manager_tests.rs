use super::*;
use crate::auth::AuthManager;
use crate::server::ids::ServerId;
use crate::server::server_api::ServerApiProvider;
use crate::server::server_api::team::MockTeamClient;
use crate::settings::PrivacySettings;
use crate::system::SystemStats;
use crate::workspaces::team::Team;
use crate::workspaces::user_workspaces::UserWorkspacesEvent;
use crate::workspaces::workspace::{Workspace, WorkspaceUid};
use warpui::{AddSingletonModel, App};

fn initialize_app(workspaces: Vec<Workspace>, app: &mut App) {
    app.add_singleton_model(|_| NetworkStatus::new());
    app.add_singleton_model(|_| SystemStats::new());
    app.add_singleton_model(TeamTesterStatus::new);
    app.add_singleton_model(|ctx| UserWorkspaces::mock(workspaces, ctx));
    app.add_singleton_model(PrivacySettings::mock);
    app.add_singleton_model(|_| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());

    app.add_singleton_model(AuthManager::new_for_test);
}

#[test]
fn out_of_band_metadata_refresh_applies_metadata_without_waiting_for_the_receiver() {
    App::test((), |mut app| async move {
        let mut attempts = 0;
        let mut team_client = MockTeamClient::new();
        team_client
            .expect_workspaces_metadata()
            .times(2)
            .returning(move || {
                attempts += 1;
                if attempts == 1 {
                    return Err(anyhow::anyhow!("workspace metadata unavailable"));
                }
                Ok(vec![])
            });
        let team_client = Arc::new(team_client);
        initialize_app(vec![], &mut app);
        let manager = app.add_singleton_model(|ctx| TeamUpdateManager::new(team_client, None, ctx));
        let (sender, applied) = oneshot::channel();
        app.update(|ctx| {
            let mut sender = Some(sender);
            ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), move |_, event, _| {
                if matches!(event, UserWorkspacesEvent::Teams)
                    && let Some(sender) = sender.take()
                {
                    let _ = sender.send(());
                }
            });
        });

        manager.update(&mut app, |manager, ctx| {
            drop(manager.refresh_workspace_metadata(ctx));
        });
        applied.await.unwrap();
    });
}

#[test]
fn on_workspaces_updated_keeps_teams_distinct_and_prunes_a_team_the_response_omits() {
    App::test((), |mut app| async move {
        let team_client = Arc::new(MockTeamClient::new());
        initialize_app(vec![], &mut app);
        let team_update_manager =
            app.add_singleton_model(|ctx| TeamUpdateManager::new(team_client, None, ctx));

        let workspace_uid = WorkspaceUid::from(ServerId::from(999));
        let team_a = ServerId::from(1);
        let team_b = ServerId::from(2);

        let workspace_with_teams = |teams: Vec<Team>| {
            Workspace::from_local_cache(workspace_uid, "Test Workspace".to_owned(), Some(teams))
        };
        let team_with_uid =
            |uid: ServerId| Team::from_local_cache(uid, format!("Team {uid}"), None, None, None);

        team_update_manager.update(&mut app, |manager, ctx| {
            manager.on_workspaces_updated(
                Ok(vec![workspace_with_teams(vec![
                    team_with_uid(team_a),
                    team_with_uid(team_b),
                ])]),
                ctx,
            );
        });

        team_update_manager.update(&mut app, |manager, ctx| {
            manager.on_workspaces_updated(
                Ok(vec![workspace_with_teams(vec![team_with_uid(team_b)])]),
                ctx,
            );
        });

        app.read(|ctx| {
            let workspaces = UserWorkspaces::as_ref(ctx);
            assert!(
                workspaces.team_from_uid(team_a).is_none(),
                "team A should have been removed once the response stopped \
                 naming it, not left stale"
            );
        });

        team_update_manager.update(&mut app, |manager, ctx| {
            manager.on_workspaces_updated(Ok(vec![workspace_with_teams(vec![])]), ctx);
        });

        app.read(|ctx| {
            let workspaces = UserWorkspaces::as_ref(ctx);
            assert!(
                workspaces.team_from_uid(team_b).is_none(),
                "an authoritative empty response must remove every remaining team, not be \
                 skipped as a no-op"
            );
        });
    });
}
