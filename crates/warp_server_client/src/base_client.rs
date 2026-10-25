use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use instant::Duration;
use warp_graphql::client::RequestOptions;
use warp_server_auth::auth_state::AuthState;
use warp_server_auth::credentials::AuthToken;

use crate::auth::{AuthEvent, AuthSession, UserUid};

/// Header carrying the request-local team scope for an operation whose team is inferred
/// from the current window rather than named explicitly in the request body. See
/// `specs/multi-team-api-context/TECH.md`. The server authenticates membership and rejects
/// a header that disagrees with the request body or an existing resource; adding this header
/// is not itself proof of membership.
pub const TEAM_UID_HEADER: &str = "X-Warp-Team-Uid";

/// Provides GraphQL path routing that applies independently of authentication.
#[derive(Clone, Debug, Default)]
pub struct GraphqlRoutingConfig {
    pub path_prefix: Option<String>,
}

/// Provides headers added only to session-authenticated GraphQL operations.
#[derive(Clone, Debug, Default)]
pub struct AuthenticatedGraphqlConfig {
    pub headers: HashMap<String, String>,
}

/// Owns shared transport, authentication, and authenticated request decoration.
pub struct BaseClient {
    client: Arc<http_client::Client>,
    auth_state: Arc<AuthState>,
    event_sender: async_channel::Sender<AuthEvent>,
    auth_session: Arc<AuthSession>,
    graphql_routing: GraphqlRoutingConfig,
    authenticated_graphql: AuthenticatedGraphqlConfig,
}

impl BaseClient {
    pub fn new(
        client: Arc<http_client::Client>,
        auth_state: Arc<AuthState>,
        event_sender: async_channel::Sender<AuthEvent>,
        graphql_routing: GraphqlRoutingConfig,
        mut authenticated_graphql: AuthenticatedGraphqlConfig,
    ) -> Self {
        authenticated_graphql.headers.retain(|name, _| {
            if Self::is_reserved_authenticated_graphql_header(name) {
                log::warn!("Ignoring reserved authenticated GraphQL header configuration: {name}");
                false
            } else {
                true
            }
        });
        let auth_session = Arc::new(AuthSession::new(
            client.clone(),
            auth_state.clone(),
            event_sender.clone(),
        ));
        Self {
            client,
            auth_state,
            event_sender,
            auth_session,
            graphql_routing,
            authenticated_graphql,
        }
    }

    /// Returns whether authenticated GraphQL decoration would override BaseClient-owned headers.
    fn is_reserved_authenticated_graphql_header(name: &str) -> bool {
        [
            http::header::AUTHORIZATION.as_str(),
            http::header::CONTENT_TYPE.as_str(),
            http::header::CONTENT_LENGTH.as_str(),
            http::header::PROXY_AUTHORIZATION.as_str(),
            TEAM_UID_HEADER,
        ]
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
    }

    /// Returns the shared HTTP client for request construction.
    pub fn http_client(&self) -> &http_client::Client {
        self.client.as_ref()
    }

    /// Returns an owned handle to the shared HTTP client for GraphQL operations.
    pub fn owned_http_client(&self) -> Arc<http_client::Client> {
        self.client.clone()
    }

    pub fn auth_session(&self) -> Arc<AuthSession> {
        self.auth_session.clone()
    }

    pub fn anonymous_id(&self) -> String {
        self.auth_state.anonymous_id()
    }

    pub fn user_id(&self) -> Option<UserUid> {
        self.auth_state.user_id()
    }

    /// Returns whether the authenticated principal is a service account.
    pub fn is_service_account(&self) -> bool {
        self.auth_state.is_service_account()
    }

    pub fn access_token_ignoring_validity(&self) -> Option<String> {
        self.auth_state.get_access_token_ignoring_validity()
    }

    pub fn allowed_to_refresh_token(&self) -> bool {
        self.auth_session.allowed_to_refresh_token()
    }

    pub async fn get_or_refresh_access_token(&self) -> Result<AuthToken> {
        self.auth_session.get_or_refresh_access_token().await
    }

    /// Returns a sender for asynchronous work that emits auth events without borrowing this client.
    pub fn event_sender(&self) -> async_channel::Sender<AuthEvent> {
        self.event_sender.clone()
    }
    /// Sends an auth event from synchronous client-owned response handling.
    pub fn send_auth_event(
        &self,
        event: AuthEvent,
    ) -> Result<(), async_channel::TrySendError<AuthEvent>> {
        self.event_sender.try_send(event)
    }

    pub fn is_auth_refresh_allowed(&self) -> bool {
        self.allowed_to_refresh_token()
    }

    /// Returns GraphQL options for bootstrap or explicit-token operations.
    pub fn graphql_request_options_with_token(&self, auth_token: Option<String>) -> RequestOptions {
        RequestOptions {
            auth_token,
            path_prefix: self.graphql_routing.path_prefix.clone(),
            ..RequestOptions::default()
        }
    }

    /// Returns GraphQL options for a session-authenticated operation.
    pub async fn graphql_request_options(
        &self,
        timeout: Option<Duration>,
    ) -> Result<RequestOptions> {
        let auth_token = self
            .get_or_refresh_access_token()
            .await
            .context("Failed to get access token for GraphQL request")?;
        let mut options = self.graphql_request_options_with_token(auth_token.bearer_token());
        options.timeout = timeout;
        options.headers = self.authenticated_graphql.headers.clone();
        Ok(options)
    }
}

#[cfg(test)]
#[path = "base_client_tests.rs"]
mod tests;
