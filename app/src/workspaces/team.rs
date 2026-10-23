use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::workspace::{BillingMetadata, EmailInvite, InviteLinkDomainRestriction, TeamSettings};
use crate::ai::llms::ModelsByFeature;
use crate::auth::UserUid;
use crate::server::ids::ServerId;

#[derive(Clone, Copy, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub enum MembershipRole {
    Owner,
    Admin,
    User,
}

/// Governs which workspace members can discover and join a team. Orthogonal to
/// workspace-level discoverability. Only `Open` teams support an invite link;
/// `Private` and `Hidden` teams rely on admin-sent email invites instead.
#[derive(Clone, Copy, Eq, PartialEq, Debug, Default)]
pub enum TeamVisibility {
    #[default]
    Open,
    Private,
    Hidden,
}

impl TeamVisibility {
    pub fn supports_invite_link(&self) -> bool {
        matches!(self, TeamVisibility::Open)
    }
}

impl MembershipRole {
    pub fn is_admin_or_owner(&self) -> bool {
        matches!(self, MembershipRole::Admin | MembershipRole::Owner)
    }

    pub fn is_owner(&self) -> bool {
        matches!(self, MembershipRole::Owner)
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct TeamMember {
    pub uid: UserUid,
    pub email: String,
    pub role: MembershipRole,
    pub is_disabled: bool,
}

impl PartialOrd for TeamMember {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TeamMember {
    fn cmp(&self, other: &Self) -> Ordering {
        self.email.cmp(&other.email)
    }
}

#[derive(Clone, Debug)]
pub struct Team {
    pub uid: ServerId,
    pub name: String,
    /// The team's brand color as a hex string (e.g. "#7c3aed"), if set by the team admin.
    pub color: Option<String>,
    pub invite_link: Option<String>,
    pub members: Vec<TeamMember>,
    pub pending_email_invites: Vec<EmailInvite>,
    pub invite_link_domain_restrictions: Vec<InviteLinkDomainRestriction>,
    pub billing_metadata: BillingMetadata,
    pub stripe_customer_id: Option<String>,
    /// The team's effective settings, sourced from the server's `Team.settings`.
    pub settings: TeamSettings,
    pub feature_model_choice: ModelsByFeature,
    /// If the team is eligible for discovery, then show toggle for setting discoverability to the team's admin
    pub is_eligible_for_discovery: bool,
    pub visibility: TeamVisibility,
}

impl Team {
    pub fn from_local_cache(
        uid: ServerId,
        name: String,
        settings: Option<TeamSettings>,
        billing_metadata: Option<BillingMetadata>,
        members: Option<Vec<TeamMember>>,
        feature_model_choice: Option<ModelsByFeature>,
    ) -> Self {
        Self {
            uid,
            name,
            color: None,
            invite_link: Default::default(),
            members: members.unwrap_or_default(),
            pending_email_invites: Default::default(),
            invite_link_domain_restrictions: Default::default(),
            billing_metadata: billing_metadata.unwrap_or_default(),
            stripe_customer_id: Default::default(),
            settings: settings.unwrap_or_default(),
            feature_model_choice: feature_model_choice.unwrap_or_default(),
            is_eligible_for_discovery: false,
            visibility: TeamVisibility::default(),
        }
    }

    fn get_member_by_email(&self, email: &str) -> Option<&TeamMember> {
        self.members.iter().find(|member| member.email == email)
    }

    pub fn is_multi_admin_enabled(&self) -> bool {
        self.billing_metadata
            .tier
            .multi_admin_policy
            .is_some_and(|policy| policy.enabled)
    }

    pub fn has_admin_permissions(&self, user_email: &str) -> bool {
        self.get_member_by_email(user_email).is_some_and(|member| {
            member.role.is_owner()
                || (member.role == MembershipRole::Admin && self.is_multi_admin_enabled())
        })
    }

    pub fn is_custom_llm_enabled(&self) -> bool {
        self.settings.llm_settings.enabled
    }
}
