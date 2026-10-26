use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::anyhow;
use warpui::{App, SingletonEntity};

use super::AuthManager;
use crate::ServerApiProvider;
use crate::auth::credentials::{Credentials, LoginToken};
use crate::auth::user::{FirebaseAuthTokens, TEST_USER_UID, User};
use crate::auth::{AuthStateProvider, UserUid};
use crate::server::server_api::auth::{MockAuthClient, UserAuthenticationError};

fn initialize_app(app: &mut App) {
    app.add_singleton_model(|_ctx| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
}

#[test]
fn pending_api_key_failure_leaves_auth_state_logged_out() {
    App::test((), |mut app| async move {
        let rejection_returned = Arc::new(AtomicBool::new(false));
        let rejection_for_mock = rejection_returned.clone();
        let mut auth_client = MockAuthClient::new();
        auth_client
            .expect_fetch_user()
            .withf(|token| matches!(token, LoginToken::ApiKey(key) if key == "wk-inherited-key"))
            .times(1)
            .return_once(move |_| {
                rejection_for_mock.store(true, Ordering::Relaxed);
                Err(UserAuthenticationError::Unexpected(anyhow!("Unauthorized")))
            });

        initialize_app(&mut app);
        let auth_state = app.read(|ctx| AuthStateProvider::as_ref(ctx).get().clone());
        auth_state.set_user(None);
        auth_state.set_credentials(None);
        AuthManager::handle(&app).update(&mut app, |auth_manager, _| {
            auth_manager.auth_client = Arc::new(auth_client);
        });

        AuthManager::handle(&app).update(&mut app, |auth_manager, ctx| {
            auth_manager.authenticate_api_key("inherited-key".to_owned(), ctx);
        });

        assert!(auth_state.credentials().is_none());
        assert!(auth_state.user_id().is_none());
        assert!(!auth_state.is_logged_in());

        warpui::r#async::Timer::after(Duration::from_millis(100)).await;

        assert!(rejection_returned.load(Ordering::Relaxed));
        assert!(auth_state.credentials().is_none());
        assert!(auth_state.user_id().is_none());
        assert!(!auth_state.is_logged_in());
    });
}

#[test]
fn validated_api_key_is_promoted_with_its_user() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let auth_state = app.read(|ctx| AuthStateProvider::as_ref(ctx).get().clone());
        auth_state.set_user(None);
        auth_state.set_credentials(None);

        AuthManager::handle(&app).update(&mut app, |auth_manager, ctx| {
            auth_manager.complete_authentication(
                User::test(),
                Credentials::ApiKey {
                    key: "wk-validated-key".to_owned(),
                    owner_type: None,
                },
                ctx,
            );
        });

        assert_eq!(auth_state.api_key().as_deref(), Some("wk-validated-key"));
        assert_eq!(auth_state.user_id(), Some(UserUid::new(TEST_USER_UID)));
        assert!(auth_state.is_logged_in());
    });
}

// These two tests verify that `persist` skips writing to secure storage under certain conditions.
// They rely on the fact that no secure storage singleton is registered in the test app: if
// `write_to_secure_storage` were ever called, it would panic trying to look up the unregistered
// singleton, causing the test to fail.

#[test]
fn test_persist_skips_when_refresh_token_is_empty() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        // Override default test credentials with Firebase tokens that have an empty refresh token.
        app.update(|ctx| {
            let tokens = FirebaseAuthTokens {
                id_token: String::new(),
                refresh_token: String::new(),
                expiration_time: chrono::Utc::now().fixed_offset() + chrono::Duration::days(365),
            };
            AuthStateProvider::as_ref(ctx)
                .get()
                .set_credentials(Some(Credentials::Firebase(tokens)));
        });

        AuthManager::handle(&app).update(&mut app, |auth_manager, ctx| {
            auth_manager.persist(ctx);
        });
    });
}

#[test]
fn test_persist_skips_when_api_key_authenticated() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        app.update(|ctx| {
            AuthStateProvider::as_ref(ctx)
                .get()
                .set_credentials(Some(Credentials::ApiKey {
                    key: "wk-test-key".to_owned(),
                    owner_type: None,
                }));
        });

        AuthManager::handle(&app).update(&mut app, |auth_manager, ctx| {
            auth_manager.persist(ctx);
        });
    });
}
