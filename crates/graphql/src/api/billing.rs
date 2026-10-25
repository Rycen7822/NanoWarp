use crate::scalars::Time;
use crate::schema;

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct BillingMetadata {
    pub customer_type: CustomerType,
    pub delinquency_status: DelinquencyStatus,
    pub tier: Tier,
    pub service_agreements: Vec<ServiceAgreement>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct ServiceAgreement {
    pub current_period_end: Time,
    pub status: ServiceAgreementStatus,
    pub stripe_subscription_id: Option<String>,
    #[cynic(rename = "type")]
    pub type_: ServiceAgreementType,
}

#[derive(cynic::Enum, Clone, Debug)]
pub enum ServiceAgreementStatus {
    Active,
    Canceled,
    PastDue,
    Unpaid,
    #[cynic(fallback)]
    Other(String),
}

#[derive(cynic::Enum, Clone, Debug, PartialEq)]
pub enum ServiceAgreementType {
    Enterprise,
    Legacy,
    ProTrial,
    Prosumer,
    SelfServe,
    TeamTrial,
    Turbo,
    Business,
    Lightspeed,
    #[cynic(fallback)]
    Other(String),
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct Tier {
    pub name: String,
    pub description: String,
    pub team_size_policy: Option<TeamSizePolicy>,
    pub shared_notebooks_policy: Option<SharedNotebooksPolicy>,
    pub session_sharing_policy: Option<SessionSharingPolicy>,
    pub telemetry_data_collection_policy: Option<TelemetryDataCollectionPolicy>,
    pub multi_admin_policy: Option<MultiAdminPolicy>,
    pub native_workspaces_policy: Option<NativeWorkspacesPolicy>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct SessionSharingPolicy {
    pub enabled: bool,
    pub max_session_bytes_size: i32,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct SharedNotebooksPolicy {
    pub is_unlimited: bool,
    pub limit: i32,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct TeamSizePolicy {
    pub is_unlimited: bool,
    pub limit: i32,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct TelemetryDataCollectionPolicy {
    pub default: bool,
    pub toggleable: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct MultiAdminPolicy {
    pub enabled: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct NativeWorkspacesPolicy {
    pub enabled: bool,
}

#[derive(cynic::Enum, Clone, Debug)]
pub enum CustomerType {
    Enterprise,
    Free,
    Legacy,
    ProTrial,
    Prosumer,
    SelfServe,
    TeamTrial,
    Turbo,
    Business,
    Lightspeed,
    Build,
    BuildMax,
    #[cynic(fallback)]
    Other(String),
}

#[derive(cynic::Enum, Clone, Debug)]
pub enum DelinquencyStatus {
    NoDelinquency,
    PastDue,
    TeamLimitExceeded,
    Unpaid,
    #[cynic(fallback)]
    Other(String),
}

#[derive(cynic::Enum, Clone, Debug, PartialEq)]
pub enum StripeSubscriptionPlan {
    Business,
    Lightspeed,
    Pro,
    Team,
    Turbo,
    Build,
    BuildBusiness,
    BuildMax,
    #[cynic(fallback)]
    Other(String),
}
