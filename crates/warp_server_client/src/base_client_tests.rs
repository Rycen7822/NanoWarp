use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cloud_objects::ids::ServerId;
use futures::executor::block_on;
use warp_core::channel::ChannelState;
use warp_server_auth::auth_state::AuthState;

use super::{AuthenticatedGraphqlConfig, BaseClient, GraphqlRoutingConfig, TEAM_UID_HEADER};
use crate::auth::{AuthClient, AuthClientImpl};

fn client() -> BaseClient {
    let (event_sender, _) = async_channel::unbounded();
    let mut authenticated_headers = HashMap::new();
    authenticated_headers.insert("X-Test-Authenticated".to_string(), "true".to_string());
    BaseClient::new(
        Arc::new(http_client::Client::new()),
        Arc::new(http_client::Client::new_external()),
        Arc::new(AuthState::new_for_test()),
        event_sender,
        GraphqlRoutingConfig {
            path_prefix: Some("/routing-only".to_string()),
        },
        AuthenticatedGraphqlConfig {
            headers: authenticated_headers,
        },
    )
}

#[test]
fn explicit_token_graphql_options_route_without_authenticated_headers() {
    let client = client();

    let options = client.graphql_request_options_with_token(Some("token".to_string()));

    assert_eq!(options.path_prefix.as_deref(), Some("/routing-only"));
    assert_eq!(options.auth_token.as_deref(), Some("token"));
    assert!(options.headers.is_empty());
}

#[test]
fn authenticated_graphql_options_include_configured_headers() {
    let client = client();

    let options = block_on(client.graphql_request_options(None)).unwrap();

    assert_eq!(options.path_prefix.as_deref(), Some("/routing-only"));
    assert_eq!(
        options
            .headers
            .get("X-Test-Authenticated")
            .map(String::as_str),
        Some("true")
    );
}

#[test]
fn authenticated_graphql_configuration_cannot_override_base_client_owned_headers() {
    let (event_sender, _) = async_channel::unbounded();
    let mut headers = HashMap::new();
    headers.insert("authorization".to_string(), "malicious".to_string());
    headers.insert("content-type".to_string(), "text/plain".to_string());
    headers.insert("CONTENT-LENGTH".to_string(), "9999".to_string());
    headers.insert(
        http::header::PROXY_AUTHORIZATION.as_str().to_string(),
        "malicious".to_string(),
    );
    headers.insert("x-eval-user-id".to_string(), "1234".to_string());
    let client = BaseClient::new(
        Arc::new(http_client::Client::new()),
        Arc::new(http_client::Client::new_external()),
        Arc::new(AuthState::new_for_test()),
        event_sender,
        GraphqlRoutingConfig::default(),
        AuthenticatedGraphqlConfig { headers },
    );

    let options = block_on(client.graphql_request_options(None)).unwrap();

    assert!(!options.headers.contains_key("authorization"));
    assert!(!options.headers.contains_key("content-type"));
    assert!(!options.headers.contains_key("CONTENT-LENGTH"));
    assert!(
        !options
            .headers
            .contains_key(http::header::PROXY_AUTHORIZATION.as_str())
    );
    assert_eq!(
        options.headers.get("x-eval-user-id").map(String::as_str),
        Some("1234")
    );
}

fn api_key_client(path_prefix: &str) -> (AuthClientImpl, Arc<Mutex<Option<String>>>) {
    let observed_team_uid = Arc::new(Mutex::new(None));
    let observed_team_uid_for_request = observed_team_uid.clone();
    let mut http_client = http_client::Client::new();
    http_client.set_before_request_fn(Box::new(move |request, _| {
        *observed_team_uid_for_request.lock().unwrap() = request
            .headers()
            .get(TEAM_UID_HEADER)
            .map(|value| value.to_str().unwrap().to_string());
    }));
    let auth_state = AuthState::new_logged_out_for_test();
    auth_state.set_remote_server_bearer_token("test-token".to_string());
    let (event_sender, _) = async_channel::unbounded();
    let base_client = BaseClient::new(
        Arc::new(http_client),
        Arc::new(http_client::Client::new_external()),
        Arc::new(auth_state),
        event_sender,
        GraphqlRoutingConfig {
            path_prefix: Some(path_prefix.to_string()),
        },
        AuthenticatedGraphqlConfig::default(),
    );

    (
        AuthClientImpl::new(Arc::new(base_client)),
        observed_team_uid,
    )
}

fn mock_api_key_list(path_prefix: &str) -> mockito::Mock {
    let mut server = ChannelState::mock_server();
    server
        .mock(
            "POST",
            mockito::Matcher::Regex(format!("^{path_prefix}/graphql/v2")),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"data":{"apiKeys":{"__typename":"APIKeyPropertiesOutput","apiKeys":[],"responseContext":{"serverVersion":null}}}}"#,
        )
        .create()
}

#[test]
fn list_api_keys_sends_selected_team_header() {
    let team_uid = "abcdefghijklmnopqrstuv";
    let request = mock_api_key_list("/api-key-selected");
    let (auth_client, observed_team_uid) = api_key_client("/api-key-selected");

    let keys =
        block_on(auth_client.list_api_keys(Some(ServerId::try_from(team_uid).unwrap()))).unwrap();

    assert!(keys.is_empty());
    assert_eq!(observed_team_uid.lock().unwrap().as_deref(), Some(team_uid));
    request.assert();
}

#[test]
fn list_api_keys_omits_team_header_when_unscoped() {
    let request = mock_api_key_list("/api-key-unscoped");
    let (auth_client, observed_team_uid) = api_key_client("/api-key-unscoped");

    let keys = block_on(auth_client.list_api_keys(None)).unwrap();

    assert!(keys.is_empty());
    assert_eq!(observed_team_uid.lock().unwrap().as_deref(), None);
    request.assert();
}
