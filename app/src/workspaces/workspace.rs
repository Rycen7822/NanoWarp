use std::cmp::Ordering;

use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use warp_graphql::billing::{ServiceAgreement, ServiceAgreementType};

use super::team::{MembershipRole, Team};
use crate::auth::UserUid;
use crate::server::ids::ServerId;

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
pub struct WorkspaceUid(ServerId);
impl From<String> for WorkspaceUid {
    fn from(uid: String) -> Self {
        WorkspaceUid(ServerId::from_string_lossy(uid))
    }
}
impl From<WorkspaceUid> for String {
    fn from(workspace_uid: WorkspaceUid) -> String {
        workspace_uid.0.to_string()
    }
}
impl From<ServerId> for WorkspaceUid {
    fn from(uid: ServerId) -> Self {
        WorkspaceUid(uid)
    }
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub uid: WorkspaceUid,
    pub name: String,
    pub stripe_customer_id: Option<String>,
    pub teams: Vec<Team>,
    pub billing_metadata: BillingMetadata,
    pub settings: WorkspaceSettings,
    pub invite_link_domain_restrictions: Vec<InviteLinkDomainRestriction>,
    pub pending_email_invites: Vec<EmailInvite>,
    // If the team is eligible for discovery, then show toggle for setting discoverability to the team's admin
    pub is_eligible_for_discovery: bool,
    pub members: Vec<WorkspaceMember>,
}

impl Workspace {
    pub fn from_local_cache(uid: WorkspaceUid, name: String, teams: Option<Vec<Team>>) -> Self {
        // Derive the workspace billing metadata from the first team's cached billing
        // metadata, if available. This ensures the workspace-level billing info is
        // consistent with team-level data loaded from the cache.
        let billing_metadata = teams
            .as_ref()
            .and_then(|t| t.first())
            .map(|team| team.billing_metadata.clone())
            .unwrap_or_default();
        Self {
            uid,
            name,
            stripe_customer_id: Default::default(),
            teams: teams.unwrap_or_default(),
            billing_metadata,
            settings: Default::default(), // TODO: persistence wrapper instead of default
            invite_link_domain_restrictions: Default::default(),
            pending_email_invites: Default::default(),
            is_eligible_for_discovery: false,
            members: Default::default(),
        }
    }

    fn get_member_by_email(&self, email: &str) -> Option<&WorkspaceMember> {
        self.members.iter().find(|member| member.email == email)
    }

    pub fn is_workspace_admin(&self, user_email: &str) -> bool {
        self.get_member_by_email(user_email)
            .is_some_and(|member| member.role.is_admin_or_owner())
    }

    pub fn is_native_workspaces_enabled(&self) -> bool {
        self.billing_metadata
            .tier
            .native_workspaces_policy
            .is_some_and(|policy| policy.enabled)
    }

    pub fn is_native_workspaces_admin(&self, user_email: &str) -> bool {
        self.is_workspace_admin(user_email) && self.is_native_workspaces_enabled()
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct WorkspaceMember {
    pub uid: UserUid,
    pub email: String,
    pub role: MembershipRole,
    pub is_disabled: bool,
}

impl PartialOrd for WorkspaceMember {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkspaceMember {
    fn cmp(&self, other: &Self) -> Ordering {
        self.email.cmp(&other.email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct EmailInvite {
    pub invitee_email: String,
    pub expired: bool,
    pub team_uid: Option<ServerId>,
}

impl PartialOrd for EmailInvite {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EmailInvite {
    fn cmp(&self, other: &Self) -> Ordering {
        self.invitee_email.cmp(&other.invitee_email)
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct InviteLinkDomainRestriction {
    pub uid: ServerId,
    pub domain: String,
}

impl PartialOrd for InviteLinkDomainRestriction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InviteLinkDomainRestriction {
    fn cmp(&self, other: &Self) -> Ordering {
        self.domain.cmp(&other.domain)
    }
}

/// This enum is the rust representation of `CustomerType` from the GraphQL Schema.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum CustomerType {
    #[default]
    Free,
    Turbo,
    SelfServe,
    Prosumer,
    Legacy,
    Enterprise,
    Business,
    Lightspeed,
    Build,
    BuildMax,
    Unknown,
}

impl CustomerType {
    pub fn to_display_string(self) -> String {
        match self {
            CustomerType::Free => "Free".to_string(),
            CustomerType::Turbo => "Turbo".to_string(),
            CustomerType::SelfServe => "Team".to_string(),
            CustomerType::Prosumer => "Pro".to_string(),
            CustomerType::Legacy => "Early adopter".to_string(),
            CustomerType::Enterprise => "Enterprise".to_string(),
            CustomerType::Business => "Business".to_string(),
            CustomerType::Lightspeed => "Lightspeed".to_string(),
            CustomerType::Build => "Build".to_string(),
            CustomerType::BuildMax => "Max".to_string(),
            CustomerType::Unknown => "".to_string(),
        }
    }
}

/// This enum is the rust representation of `DelinquencyStatus` from the GraphQL Schema.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum DelinquencyStatus {
    #[default]
    NoDelinquency,
    PastDue,
    Unpaid,
    TeamLimitExceeded,
    Unknown,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct WorkspaceSizePolicy {
    pub is_unlimited: bool,
    pub limit: i64,
}
#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct SharedNotebooksPolicy {
    pub is_unlimited: bool,
    pub limit: i64,
}
#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct SessionSharingPolicy {
    pub is_enabled: bool,
    pub max_session_size: u64,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct TelemetryDataCollectionPolicy {
    pub default: bool,
    pub toggleable: bool,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct MultiAdminPolicy {
    pub enabled: bool,
}

#[derive(Clone, Debug, Copy, Serialize, Deserialize)]
pub struct NativeWorkspacesPolicy {
    pub enabled: bool,
}

/// This struct is the rust representation of `Tier` from the GraphQL Schema.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tier {
    pub name: String,
    pub description: String,
    pub workspace_size_policy: Option<WorkspaceSizePolicy>,
    pub shared_notebooks_policy: Option<SharedNotebooksPolicy>,
    pub session_sharing_policy: Option<SessionSharingPolicy>,
    pub telemetry_data_collection_policy: Option<TelemetryDataCollectionPolicy>,
    pub multi_admin_policy: Option<MultiAdminPolicy>,
    pub native_workspaces_policy: Option<NativeWorkspacesPolicy>,
}

/// This struct is the rust representation of `BillingMetadata` from the GraphQL Schema.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BillingMetadata {
    pub tier: Tier,
    pub customer_type: CustomerType,
    pub delinquency_status: DelinquencyStatus,
    #[serde(skip)]
    pub service_agreements: Vec<ServiceAgreement>,
}

impl BillingMetadata {
    /**
     * Returns whether customer can upgrade to the Build plan based on their current tier.
     */
    pub fn can_upgrade_to_build_plan(&self) -> bool {
        match self.customer_type {
            CustomerType::Unknown
            | CustomerType::Business
            | CustomerType::Enterprise
            | CustomerType::Build
            | CustomerType::BuildMax => false,
            CustomerType::Free
            | CustomerType::Legacy
            | CustomerType::Prosumer
            | CustomerType::Turbo
            | CustomerType::SelfServe
            | CustomerType::Lightspeed => true,
        }
    }

    /**
     * Returns whether customer can upgrade to the Build Max plan based on their current tier.
     * Users on Build can upgrade to Build Max.
     */
    pub fn can_upgrade_to_build_max_plan(&self) -> bool {
        self.can_upgrade_to_build_plan() || self.customer_type == CustomerType::Build
    }

    /**
     * Returns whether customer can upgrade to a higher tier based on their current tier.
     */
    pub fn can_upgrade_to_higher_tier_plan(&self) -> bool {
        self.can_upgrade_to_build_plan()
    }

    pub fn is_stripe_paid_plan(customer_type: CustomerType) -> bool {
        match customer_type {
            CustomerType::Turbo
            | CustomerType::SelfServe
            | CustomerType::Prosumer
            | CustomerType::Business
            | CustomerType::Lightspeed
            | CustomerType::Build
            | CustomerType::BuildMax => true,
            CustomerType::Free
            | CustomerType::Enterprise
            | CustomerType::Legacy
            | CustomerType::Unknown => false,
        }
    }

    pub fn is_user_on_paid_plan(&self) -> bool {
        match self.customer_type {
            CustomerType::Turbo
            | CustomerType::SelfServe
            | CustomerType::Prosumer
            | CustomerType::Business
            | CustomerType::Lightspeed
            | CustomerType::Enterprise
            | CustomerType::Legacy
            | CustomerType::Build
            | CustomerType::BuildMax => true,
            CustomerType::Free | CustomerType::Unknown => false,
        }
    }

    pub fn is_on_stripe_paid_plan(&self) -> bool {
        BillingMetadata::is_stripe_paid_plan(self.customer_type)
    }

    pub fn is_on_build_plan(&self) -> bool {
        self.customer_type == CustomerType::Build
    }

    pub fn is_on_build_max_plan(&self) -> bool {
        self.customer_type == CustomerType::BuildMax
    }

    pub fn is_on_build_business_plan(&self) -> bool {
        self.customer_type == CustomerType::Business
            && matches!(
                self.service_agreements.first().map(|sa| &sa.type_),
                Some(ServiceAgreementType::SelfServe)
            )
    }

    pub fn is_on_legacy_business_plan(&self) -> bool {
        self.customer_type == CustomerType::Business && !self.is_on_build_business_plan()
    }

    pub fn is_enterprise_plan(&self) -> bool {
        self.customer_type == CustomerType::Enterprise
    }

    pub fn is_free_plan(&self) -> bool {
        self.customer_type == CustomerType::Free
    }

    pub fn is_on_legacy_paid_plan(&self) -> bool {
        match self.customer_type {
            CustomerType::Prosumer
            | CustomerType::Turbo
            | CustomerType::Lightspeed
            | CustomerType::SelfServe => true,
            CustomerType::Business => self.is_on_legacy_business_plan(),
            CustomerType::Free
            | CustomerType::Legacy
            | CustomerType::Enterprise
            | CustomerType::Build
            | CustomerType::BuildMax
            | CustomerType::Unknown => false,
        }
    }

    pub fn is_delinquent_due_to_payment_issue(&self) -> bool {
        self.delinquency_status == DelinquencyStatus::PastDue
            || self.delinquency_status == DelinquencyStatus::Unpaid
    }

    pub fn has_active_subscription(&self) -> bool {
        if let Some(newest_service_agreement) = self.service_agreements.first() {
            let not_expired = Utc::now() < newest_service_agreement.current_period_end.utc();
            let not_delinquent = !self.is_delinquent_due_to_payment_issue();
            not_expired && not_delinquent
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TelemetrySettings {
    pub force_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LinkSharingSettings {
    pub anyone_with_link_sharing_enabled: bool,
    pub direct_link_sharing_enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnterpriseSecretRegex {
    pub pattern: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SecretRedactionSettings {
    pub enabled: bool,
    pub regexes: Vec<EnterpriseSecretRegex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    #[serde(default, with = "serde_regex")]
    pub remote_session_regex_list: Vec<Regex>,
    pub telemetry_settings: TelemetrySettings,
    pub link_sharing_settings: LinkSharingSettings,
    pub secret_redaction_settings: SecretRedactionSettings,
    pub is_invite_link_enabled: bool,
    pub is_discoverable: bool,
}

/// A workspace-governable setting carried on [`TeamSettings`]: the effective
/// `value` plus whether the workspace layer enforces it (mirrors the server's
/// `*SettingInfo` wrappers). The enforcement bit is preserved so future admin UI
/// can distinguish workspace-enforced values from team-owned ones.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnforceableSetting<T> {
    pub value: T,
    #[serde(default)]
    pub is_enforced_by_workspace: bool,
}

/// A list setting split by the layer that contributed each entry (mirrors the
/// server's `StringListSettingInfo` / `SecretRedactionRegexListInfo`). `values`
/// is the authoritative merged result; `workspace_entries` / `team_entries` are
/// preserved so future admin UI can present the layers separately.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SplitListSetting<T> {
    pub values: Vec<T>,
    #[serde(default)]
    pub workspace_entries: Vec<T>,
    #[serde(default)]
    pub team_entries: Vec<T>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamSecretRedactionSettings {
    pub enabled: EnforceableSetting<bool>,
    pub regexes: SplitListSetting<EnterpriseSecretRegex>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamLinkSharingSettings {
    pub anyone_with_link_sharing_enabled: EnforceableSetting<bool>,
    pub direct_link_sharing_enabled: EnforceableSetting<bool>,
}

/// The effective settings that apply to a team, combining the workspace layer
/// with the team's own configuration.
///
/// This is intentionally a distinct type from [`WorkspaceSettings`] rather than
/// an alias: it is sourced from the server's effective `Team.settings`. Each
/// workspace-governable group keeps both its effective value **and** the
/// `is_enforced_by_workspace` / workspace-vs-team split metadata (via
/// [`EnforceableSetting`] / [`SplitListSetting`]) so future admin UI can recover
/// those details. Unlike `WorkspaceSettings`, it does not carry the
/// workspace-scoped `is_invite_link_enabled` / `is_discoverable` flags (those
/// remain on [`WorkspaceSettings`] and are read from the current workspace).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TeamSettings {
    #[serde(default, with = "serde_regex")]
    pub remote_session_regex_list: Vec<Regex>,
    pub secret_redaction: TeamSecretRedactionSettings,
    pub link_sharing: TeamLinkSharingSettings,
    pub telemetry_settings: TelemetrySettings,
}
