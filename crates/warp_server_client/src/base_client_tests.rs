use std::collections::HashMap;
use std::sync::Arc;

use futures::executor::block_on;
use warp_server_auth::auth_state::AuthState;

use super::{AuthenticatedGraphqlConfig, BaseClient, GraphqlRoutingConfig};

fn client() -> BaseClient {
    let (event_sender, _) = async_channel::unbounded();
    let mut authenticated_headers = HashMap::new();
    authenticated_headers.insert("X-Test-Authenticated".to_string(), "true".to_string());
    BaseClient::new(
        Arc::new(http_client::Client::new()),
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
