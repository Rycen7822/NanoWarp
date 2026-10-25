use super::platform_error::PlatformErrorInfoResponse;
use crate::response_context::ResponseContext;
use crate::schema;

#[derive(cynic::QueryFragment, Debug)]
pub struct UserFacingError {
    pub error: UserFacingErrorInterface,
    pub response_context: ResponseContext,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct PlatformError {
    pub message: String,
    pub detail: Option<String>,
    pub info: PlatformErrorInfoResponse,
}

#[derive(cynic::InlineFragments, Debug)]
pub enum UserFacingErrorInterface {
    PlatformError(Box<PlatformError>),
    AccountDelinquencyError(AccountDelinquencyError),
    BudgetExceededError(BudgetExceededError),
    PaymentMethodDeclinedError(PaymentMethodDeclinedError),
    InvalidAttachmentError(InvalidAttachmentError),
    #[cynic(fallback)]
    Unknown(UserFacingErrorFallback),
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "UserFacingErrorInterface")]
pub struct UserFacingErrorFallback {
    pub message: String,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct AccountDelinquencyError {
    pub message: String,
}

#[derive(cynic::QueryFragment, Debug, thiserror::Error)]
#[error("{message}")]
pub struct BudgetExceededError {
    pub message: String,
}

#[derive(cynic::QueryFragment, Debug, thiserror::Error)]
#[error("{message}")]
pub struct PaymentMethodDeclinedError {
    pub message: String,
}

#[derive(cynic::QueryFragment, Debug, thiserror::Error)]
#[error("{message}")]
pub struct InvalidAttachmentError {
    pub message: String,
}
