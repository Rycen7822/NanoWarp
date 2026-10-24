use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use chrono::{DateTime, Utc};
use futures::lock::Mutex as AsyncMutex;
use instant::Duration;
use parking_lot::Mutex;
use warp_graphql::client::RequestOptions;
use warp_isolation_platform::{IsolationPlatformError, WorkloadToken, issue_workload_token};
use warp_server_auth::auth_state::AuthState;
use warp_server_auth::credentials::AuthToken;
#[cfg(feature = "agent_mode_evals")]
use warp_server_auth::credentials::Credentials;

use crate::auth::{AuthEvent, AuthSession, UserUid};

/// Header key for the ambient agent workload token attached to authenticated requests.
pub const AMBIENT_WORKLOAD_TOKEN_HEADER: &str = "X-Warp-Ambient-Workload-Token";

/// Header used to communicate the source of an agent run.
pub const AGENT_SOURCE_HEADER: &str = "X-Oz-Api-Source";

/// Header carrying the request-local team scope for an operation whose team is inferred
/// from the current window rather than named explicitly in the request body. See
/// `specs/multi-team-api-context/TECH.md`. The server authenticates membership and rejects
/// a header that disagrees with the request body or an existing resource; adding this header
/// is not itself proof of membership.
pub const TEAM_UID_HEADER: &str = "X-Warp-Team-Uid";

/// IDs in the staging database that were created specifically for evals.
///
/// Keep this list in sync with `script/populate_agent_mode_eval_user.sql` in warp-server.
#[cfg(feature = "agent_mode_evals")]
const EVAL_USER_IDS: [i32; 11] = [
    2162, 2164, 2165, 2166, 2167, 2168, 2169, 2172, 2173, 2174, 2175,
];

/// Duration for which an ambient agent workload token is valid.
const AMBIENT_WORKLOAD_TOKEN_DURATION: Duration = Duration::from_secs(3 * 60 * 60);

/// Margin by which a token must outlast the moment it is needed, so one that is
/// nominally still valid cannot expire while a request using it is in flight.
const AMBIENT_WORKLOAD_TOKEN_EXPIRY_MARGIN: Duration = Duration::from_secs(5 * 60);

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
    external_client: Arc<http_client::Client>,
    auth_state: Arc<AuthState>,
    event_sender: async_channel::Sender<AuthEvent>,
    auth_session: Arc<AuthSession>,
    ambient_workload_token: Arc<Mutex<Option<WorkloadToken>>>,
    ambient_workload_token_refresh: AsyncMutex<()>,
    agent_source: Option<String>,
    graphql_routing: GraphqlRoutingConfig,
    authenticated_graphql: AuthenticatedGraphqlConfig,
}

impl BaseClient {
    pub fn new(
        client: Arc<http_client::Client>,
        external_client: Arc<http_client::Client>,
        auth_state: Arc<AuthState>,
        event_sender: async_channel::Sender<AuthEvent>,
        agent_source: Option<String>,
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
        // We generate one random user ID per client so evals can run in parallel.
        #[cfg(feature = "agent_mode_evals")]
        let eval_user_id = {
            use rand::Rng as _;

            Some(EVAL_USER_IDS[rand::thread_rng().gen_range(0..EVAL_USER_IDS.len())])
        };
        #[cfg(feature = "agent_mode_evals")]
        if let Some(eval_user_id) = eval_user_id {
            // Set a deterministic per-user API key so all requests — including
            // REST endpoints like the SSE event stream — carry a real
            // Authorization header. The key format mirrors what SeedEvalAPIKeys()
            // inserts in warp-server at eval startup:
            // wk-1.<user_id as 64-char zero-padded lowercase hex>.
            let eval_key = format!("wk-1.{eval_user_id:0>64x}");
            auth_state.set_credentials(Some(Credentials::ApiKey {
                key: eval_key,
                owner_type: None,
            }));
        }
        let auth_session = Arc::new(AuthSession::new(
            client.clone(),
            auth_state.clone(),
            event_sender.clone(),
        ));
        Self {
            client,
            external_client,
            auth_state,
            event_sender,
            auth_session,
            ambient_workload_token: Arc::new(Mutex::new(None)),
            ambient_workload_token_refresh: AsyncMutex::new(()),
            agent_source,
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
            AMBIENT_WORKLOAD_TOKEN_HEADER,
            AGENT_SOURCE_HEADER,
            TEAM_UID_HEADER,
        ]
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
    }

    /// Returns the shared HTTP client for request construction.
    pub fn http_client(&self) -> &http_client::Client {
        self.client.as_ref()
    }

    /// Returns the pooled external HTTP client.
    pub fn external_http_client(&self) -> &http_client::Client {
        self.external_client.as_ref()
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

    #[cfg(any(test, feature = "test-util"))]
    pub fn set_ambient_workload_token_for_test(
        &self,
        token: String,
        expires_at: Option<DateTime<Utc>>,
    ) {
        *self.ambient_workload_token.lock() = Some(WorkloadToken { token, expires_at });
    }

    /// Returns an ambient agent workload token when the current runtime can issue one.
    ///
    /// The token is guaranteed to be valid for the request it is attached to.
    pub async fn get_or_create_ambient_workload_token(&self) -> Result<Option<String>> {
        let valid_until = Utc::now() + AMBIENT_WORKLOAD_TOKEN_EXPIRY_MARGIN;
        Ok(self
            .workload_token_valid_until(valid_until, AMBIENT_WORKLOAD_TOKEN_DURATION)
            .await?
            .map(|workload_token| workload_token.token))
    }

    /// Returns a cached or freshly issued workload token that is still valid at `valid_until`,
    /// asking the isolation platform for `requested_duration` when a new one is needed.
    ///
    /// The platform may issue a shorter-lived token than requested, so the result is not
    /// guaranteed to reach `valid_until`.
    async fn workload_token_valid_until(
        &self,
        valid_until: DateTime<Utc>,
        requested_duration: Duration,
    ) -> Result<Option<WorkloadToken>> {
        self.workload_token_valid_until_with(valid_until, requested_duration, issue_workload_token)
            .await
    }

    async fn workload_token_valid_until_with<F>(
        &self,
        valid_until: DateTime<Utc>,
        requested_duration: Duration,
        issue_token: impl FnOnce(Option<Duration>) -> F,
    ) -> Result<Option<WorkloadToken>>
    where
        F: Future<Output = Result<WorkloadToken, IsolationPlatformError>>,
    {
        if cfg!(target_family = "wasm") {
            return Ok(None);
        }
        if let Some(token) = self.cached_workload_token_valid_until(valid_until) {
            return Ok(Some(token));
        }

        // Concurrent NSC processes can read its first-run config before initialization finishes.
        let _refresh_guard = self.ambient_workload_token_refresh.lock().await;
        if let Some(token) = self.cached_workload_token_valid_until(valid_until) {
            return Ok(Some(token));
        }
        let workload_token = match issue_token(Some(requested_duration)).await {
            Ok(token) => token,
            Err(IsolationPlatformError::NoIsolationPlatformDetected) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        *self.ambient_workload_token.lock() = Some(workload_token.clone());
        Ok(Some(workload_token))
    }
    fn cached_workload_token_valid_until(
        &self,
        valid_until: DateTime<Utc>,
    ) -> Option<WorkloadToken> {
        self.ambient_workload_token
            .lock()
            .as_ref()
            .filter(|token| {
                token
                    .expires_at
                    .is_none_or(|expires_at| valid_until < expires_at)
            })
            .cloned()
    }

    /// Returns the workload proof and agent source for authenticated requests.
    pub async fn ambient_headers(&self) -> Result<Vec<(String, String)>> {
        let workload_token = self
            .get_or_create_ambient_workload_token()
            .await
            .context("Failed to get ambient agent workload token")?;
        Ok(workload_token
            .map(|token| (AMBIENT_WORKLOAD_TOKEN_HEADER.to_string(), token))
            .into_iter()
            .chain(
                self.agent_source
                    .clone()
                    .map(|source| (AGENT_SOURCE_HEADER.to_string(), source)),
            )
            .collect())
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
        options.headers.extend(self.ambient_headers().await?);
        Ok(options)
    }
}

#[cfg(test)]
#[path = "base_client_tests.rs"]
mod tests;
