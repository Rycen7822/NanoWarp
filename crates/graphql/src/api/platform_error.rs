use std::collections::BTreeMap;
use std::str::FromStr;

use crate::schema;

/// Machine-readable error code from the platform error catalog.
/// Identifies the class of a structured server error.
/// See platformerrors package for the canonical definitions.
#[derive(cynic::Enum, Clone, Copy, Debug, PartialEq)]
pub enum PlatformErrorCode {
    #[cynic(rename = "AGENT_STREAM_FAILURE")]
    AgentStreamFailure,
    #[cynic(rename = "AGENT_STREAM_NETWORK_ERROR")]
    AgentStreamNetworkError,
    #[cynic(rename = "AUTHENTICATION_REQUIRED")]
    AuthenticationRequired,
    #[cynic(rename = "BUDGET_EXCEEDED")]
    BudgetExceeded,
    #[cynic(rename = "CONTENT_POLICY_VIOLATION")]
    ContentPolicyViolation,
    #[cynic(rename = "ENVIRONMENT_SETUP_FAILED")]
    EnvironmentSetupFailed,
    #[cynic(rename = "EXTERNAL_AUTHENTICATION_REQUIRED")]
    ExternalAuthenticationRequired,
    #[cynic(rename = "FEATURE_NOT_AVAILABLE")]
    FeatureNotAvailable,
    #[cynic(rename = "INSUFFICIENT_CREDITS")]
    InsufficientCredits,
    #[cynic(rename = "INTEGRATION_DISABLED")]
    IntegrationDisabled,
    #[cynic(rename = "INTEGRATION_NOT_CONFIGURED")]
    IntegrationNotConfigured,
    #[cynic(rename = "INTERNAL_ERROR")]
    InternalError,
    #[cynic(rename = "INVALID_REQUEST")]
    InvalidRequest,
    #[cynic(rename = "NOT_AUTHORIZED")]
    NotAuthorized,
    #[cynic(rename = "RESOURCE_UNAVAILABLE")]
    ResourceUnavailable,
    #[cynic(rename = "RESOURCE_NOT_FOUND")]
    ResourceNotFound,
}

impl FromStr for PlatformErrorCode {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "authentication_required" => Ok(Self::AuthenticationRequired),
            "budget_exceeded" => Ok(Self::BudgetExceeded),
            "content_policy_violation" => Ok(Self::ContentPolicyViolation),
            "environment_setup_failed" => Ok(Self::EnvironmentSetupFailed),
            "external_authentication_required" => Ok(Self::ExternalAuthenticationRequired),
            "feature_not_available" => Ok(Self::FeatureNotAvailable),
            "insufficient_credits" => Ok(Self::InsufficientCredits),
            "integration_disabled" => Ok(Self::IntegrationDisabled),
            "integration_not_configured" => Ok(Self::IntegrationNotConfigured),
            "internal_error" => Ok(Self::InternalError),
            "invalid_request" => Ok(Self::InvalidRequest),
            "not_authorized" => Ok(Self::NotAuthorized),
            "resource_unavailable" => Ok(Self::ResourceUnavailable),
            "resource_not_found" => Ok(Self::ResourceNotFound),
            _ => Err(()),
        }
    }
}

/// Canonical client representation of structured platform-error information.
///
/// GraphQL exposes separate output and input object types for this data. This
/// type provides the stable representation used between those wire boundaries.
#[derive(Clone, Debug, PartialEq)]
pub struct PlatformErrorInfo {
    pub error_message: Option<String>,
    pub code: PlatformErrorCode,
    pub http_status: Option<i32>,
    pub user_facing_messages: BTreeMap<PlatformErrorMessageFormat, String>,
    pub detail: Option<String>,
    pub retryable: bool,
    pub is_user_error: Option<bool>,
    pub metadata: BTreeMap<String, String>,
    pub debug: Option<String>,
    pub metrics_category: Option<String>,
    pub trace_id: Option<String>,
}

#[derive(cynic::Enum, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PlatformErrorMessageFormat {
    #[cynic(rename = "PLAIN_TEXT")]
    PlainText,
    #[cynic(rename = "SLACK_MRKDWN")]
    SlackMrkdwn,
    #[cynic(rename = "MARKDOWN")]
    Markdown,
}

#[derive(cynic::QueryFragment, Clone, Debug)]
#[cynic(graphql_type = "PlatformErrorMessage")]
pub struct PlatformErrorMessage {
    pub format: PlatformErrorMessageFormat,
    pub message: String,
}

/// GraphQL output-side representation of [`PlatformErrorInfo`].
#[derive(cynic::QueryFragment, Clone, Debug)]
#[cynic(graphql_type = "PlatformErrorInfo")]
pub struct PlatformErrorInfoResponse {
    pub error_message: String,
    pub code: PlatformErrorCode,
    pub http_status: i32,
    pub user_facing_messages: Vec<PlatformErrorMessage>,
    pub detail: Option<String>,
    pub retryable: bool,
    pub is_user_error: bool,
    pub metadata: Vec<PlatformErrorMetadataResponse>,
    pub debug: Option<String>,
    pub metrics_category: String,
    pub trace_id: Option<String>,
}

#[derive(cynic::QueryFragment, Clone, Debug)]
#[cynic(graphql_type = "PlatformErrorMetadata")]
pub struct PlatformErrorMetadataResponse {
    pub key: String,
    pub value: String,
}

impl From<PlatformErrorInfoResponse> for PlatformErrorInfo {
    fn from(response: PlatformErrorInfoResponse) -> Self {
        Self {
            error_message: Some(response.error_message),
            code: response.code,
            http_status: Some(response.http_status),
            user_facing_messages: response
                .user_facing_messages
                .into_iter()
                .map(|entry| (entry.format, entry.message))
                .collect(),
            detail: response.detail,
            retryable: response.retryable,
            is_user_error: Some(response.is_user_error),
            metadata: response
                .metadata
                .into_iter()
                .map(|entry| (entry.key, entry.value))
                .collect(),
            debug: response.debug,
            metrics_category: Some(response.metrics_category),
            trace_id: response.trace_id,
        }
    }
}

#[cfg(test)]
#[path = "platform_error_tests.rs"]
mod tests;
