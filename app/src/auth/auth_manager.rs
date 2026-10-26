use std::result::Result as StdResult;
use std::sync::Arc;

use settings::Setting as _;
use warp_errors::{report_error, report_if_error};
use warp_server_auth::API_KEY_PREFIX;
use warp_server_auth::user::persistence::PersistedUser;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::AuthStateProvider;
use super::auth_state::{AuthState, PersistAction};
use super::credentials::{Credentials, LoginToken};
use super::user::User;
use crate::persistence::ModelEvent;
use crate::server::server_api::ServerApi;
use crate::server::server_api::auth::{AuthClient, FetchUserResult, UserAuthenticationError};

use crate::GlobalResourceHandlesProvider;
use crate::terminal::general_settings::GeneralSettings;

/// AuthManager is a singleton model which manages the currently logged-in user's state.
/// If you need to access the state, use `AuthStateProvider`.
pub struct AuthManager {
    auth_state: Arc<AuthState>,
    server_api: Arc<ServerApi>,
    auth_client: Arc<dyn AuthClient>,
}

impl AuthManager {
    /// Creates a new instance of the AuthManager. The auth state must already be initialized through
    /// [`AuthStateProvider`].
    pub fn new(
        server_api: Arc<ServerApi>,
        auth_client: Arc<dyn AuthClient>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();

        Self {
            auth_state,
            server_api,
            auth_client,
        }
    }

    #[cfg(test)]
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        use crate::server::server_api::ServerApiProvider;

        let server_api_provider = ServerApiProvider::as_ref(ctx);
        let server_api = server_api_provider.get();
        let auth_client = server_api_provider.get_auth_client();
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();

        Self {
            auth_state,
            server_api,
            auth_client,
        }
    }

    /// Refreshes the user's auth state using their existing credentials.
    pub fn refresh_user(&self, ctx: &mut ModelContext<Self>) {
        let Some(credentials) = self.auth_state.credentials() else {
            log::warn!("Attempted to refresh user without credentials");
            return;
        };

        let Some(token) = credentials.login_token() else {
            log::info!("Attempted to refresh a user with no login token, skipping");
            return;
        };

        let auth_client = self.auth_client.clone();
        let _ = ctx.spawn(
            async move { auth_client.fetch_user(token).await },
            Self::on_user_fetched,
        );
    }
    /// Validates a startup API key without exposing it through shared auth state.
    ///
    /// [`Self::on_user_fetched`] promotes the returned user and credentials only
    /// after the server accepts the key. A failed request leaves the client
    /// fully logged out.
    pub fn authenticate_api_key(&self, api_key: String, ctx: &mut ModelContext<Self>) {
        log::info!("Authenticating via pending API key");
        let api_key = if api_key.starts_with(API_KEY_PREFIX) {
            api_key
        } else {
            format!("{API_KEY_PREFIX}{api_key}")
        };
        let auth_client = self.auth_client.clone();
        let _ = ctx.spawn(
            async move { auth_client.fetch_user(LoginToken::ApiKey(api_key)).await },
            Self::on_user_fetched,
        );
    }

    /// Callback for handling a successful fetch of a user from warp-server and Firebase.
    /// This does the heavy-lifting of setting up all components of the application that depend
    /// on a user's authenticated state, and emits events to subscribers that let them know
    /// an auth event has occurred.
    fn on_user_fetched(
        &mut self,
        fetch_user_result: StdResult<FetchUserResult, UserAuthenticationError>,
        ctx: &mut ModelContext<Self>,
    ) {
        match fetch_user_result {
            Ok(fetch_user_result) => {
                let FetchUserResult {
                    user_output,
                    credentials,
                } = fetch_user_result;
                let user: User = user_output.into();

                self.complete_authentication(user.clone(), credentials, ctx);

                self.set_needs_reauth(false);

                if !user.is_user_anonymous() {
                    GeneralSettings::handle(ctx).update(ctx, |settings, ctx| {
                        report_if_error!(
                            settings.did_non_anonymous_user_log_in.set_value(true, ctx)
                        );
                    });
                }

                let global_resource_handles =
                    GlobalResourceHandlesProvider::as_ref(ctx).get().clone();

                if let Some(model_event_sender) = &global_resource_handles.model_event_sender
                    && let Err(e) =
                        model_event_sender.send(ModelEvent::UpsertCurrentUserInformation {
                            user_information: PersistedCurrentUserInformation {
                                email: self.auth_state.user_email().unwrap_or_default(),
                            },
                        })
                {
                    report_error!(
                        anyhow::Error::new(e)
                            .context("Error persisting user information to database")
                    );
                };

                let server_api = self.server_api.clone();
                let _ = ctx.spawn(async move { server_api.notify_login().await }, |_, _, _| {});
            }
            Err(error) => match error {
                UserAuthenticationError::DeniedAccessToken(_) => {
                    self.set_needs_reauth(true);
                }
                UserAuthenticationError::UserAccountDisabled(_) => {}
                UserAuthenticationError::Unexpected(_) => {}
                UserAuthenticationError::InvalidStateParameter => {}
                UserAuthenticationError::MissingStateParameter => {}
            },
        }
    }

    /// Sets the user and credentials in auth state and persists to secure storage.
    /// Persistence depends on the credential type - currently, we only persist
    /// state if authenticated via a Firebase token.
    fn complete_authentication(
        &self,
        user: User,
        credentials: Credentials,
        ctx: &mut ModelContext<Self>,
    ) {
        self.set_and_persist(Some(user), Some(credentials), ctx);
    }
    fn set_and_persist(
        &self,
        user: Option<User>,
        credentials: Option<Credentials>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.auth_state.set_user(user);
        self.auth_state.set_credentials(credentials);
        self.persist(ctx);
    }

    /// Persists (or removes) the current user and credentials to/from secure storage,
    /// based on the current auth state.
    fn persist(&self, ctx: &mut ModelContext<Self>) {
        match self.auth_state.persist_action() {
            PersistAction::Persist(persisted_user) => {
                if persisted_user.auth_tokens.refresh_token.is_empty() {
                    log::warn!("Skipping user persistence due to empty refresh token");
                    return;
                }
                let _ = persisted_user.write_to_secure_storage(ctx).map_err(|err| {
                    log::warn!("Unable to persist user to secure storage: {err:?}");
                });
            }
            PersistAction::Remove => {
                let _ = PersistedUser::remove_from_secure_storage(ctx).map_err(|err| {
                    log::warn!("Unable to clear user from secure storage: {err:?}");
                });
            }
            PersistAction::DoNothing => {}
        }
    }

    /// Clears the cloud account without changing local terminal sessions or persistence.
    pub(crate) fn log_out(&mut self, ctx: &mut ModelContext<Self>) {
        self.set_and_persist(None, None, ctx);
    }

    /// Sets whether or not this user's Firebase credentials are invalid and thus needs to reauth.
    pub fn set_needs_reauth(&self, needs_reauth: bool) {
        self.auth_state.set_needs_reauth(needs_reauth);
    }
}

#[derive(Clone, Debug)]
pub struct PersistedCurrentUserInformation {
    pub email: String,
}

impl Entity for AuthManager {
    type Event = ();
}

impl SingletonEntity for AuthManager {}

#[cfg(test)]
#[path = "auth_manager_tests.rs"]
mod auth_manager_test;
