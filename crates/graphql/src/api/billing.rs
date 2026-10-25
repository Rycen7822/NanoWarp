use crate::scalars::Time;
use crate::schema;

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
