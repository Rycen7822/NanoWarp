use super::billing::BillingMetadata;
use crate::schema;

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct Workspace {
    pub uid: cynic::Id,
    pub name: String,
    pub stripe_customer_id: Option<cynic::Id>,
    pub members: Vec<WorkspaceMember>,
    pub teams: Vec<Team>,
    pub billing_metadata: BillingMetadata,
    pub settings: WorkspaceSettings,
    pub pending_email_invites: Vec<EmailInvite>,
    pub invite_link_domain_restrictions: Vec<InviteLinkDomainRestriction>,
    pub is_eligible_for_discovery: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct WorkspaceMember {
    pub uid: cynic::Id,
    pub email: String,
    pub role: MembershipRole,
    pub is_disabled: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct WorkspaceSettings {
    pub ai_permissions_settings: AiPermissionsSettings,
    pub is_discoverable: bool,
    pub is_invite_link_enabled: bool,
    pub telemetry_settings: TelemetrySettings,
    pub link_sharing_settings: LinkSharingSettings,
    pub secret_redaction_settings: SecretRedactionSettings,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct TelemetrySettings {
    pub force_enabled: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct LinkSharingSettings {
    pub anyone_with_link_sharing_enabled: bool,
    pub direct_link_sharing_enabled: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct SecretRedactionRegex {
    pub name: Option<String>,
    pub pattern: String,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct SecretRedactionSettings {
    pub enabled: bool,
    pub regexes: Vec<SecretRedactionRegex>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct Team {
    pub uid: cynic::Id,
    pub name: String,
    pub members: Vec<TeamMember>,
    pub settings: TeamSettings,
    pub color: Option<String>,
    pub invite_link: Option<String>,
    pub visibility: TeamVisibility,
}

/// Governs which workspace members can discover and join a team. Orthogonal to
/// workspace-level discoverability (`is_discoverable`).
#[derive(cynic::Enum, Clone, Debug, PartialEq, Eq)]
pub enum TeamVisibility {
    Open,
    Private,
    Hidden,
    #[cynic(fallback)]
    Other(String),
}

/// The effective settings that apply to a team, combining the workspace layer
/// with the team's own configuration. Workspace-governable groups are wrapped in
/// `*SettingInfo` types carrying the effective `value` plus whether the value is
/// enforced by the workspace; list settings carry the merged `values`.
#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct TeamSettings {
    pub ai_permissions: AiPermissionsSettingsInfo,
    pub secret_redaction: SecretRedactionSettingsInfo,
    pub link_sharing: LinkSharingSettingsInfo,
    pub telemetry_settings: TelemetrySettings,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct BooleanSettingInfo {
    pub value: bool,
    pub is_enforced_by_workspace: bool,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct SecretRedactionRegexListInfo {
    pub values: Vec<SecretRedactionRegex>,
    pub workspace_entries: Vec<SecretRedactionRegex>,
    pub team_entries: Vec<SecretRedactionRegex>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct SecretRedactionSettingsInfo {
    pub enabled: BooleanSettingInfo,
    pub regexes: SecretRedactionRegexListInfo,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct LinkSharingSettingsInfo {
    pub anyone_with_link_sharing_enabled: BooleanSettingInfo,
    pub direct_link_sharing_enabled: BooleanSettingInfo,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct TeamMember {
    pub uid: cynic::Id,
    pub email: String,
    pub role: MembershipRole,
    pub is_disabled: bool,
}

#[derive(cynic::Enum, Clone, Debug, PartialEq, Eq, Copy)]
pub enum MembershipRole {
    Admin,
    Owner,
    User,
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct InviteLinkDomainRestriction {
    pub uid: cynic::Id,
    pub domain: String,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct EmailInvite {
    pub email: String,
    pub expired: bool,
    pub team_uid: Option<cynic::Id>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct AiPermissionsSettings {
    pub remote_session_regex_list: Vec<String>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct AiPermissionsSettingsInfo {
    pub remote_session_regex_list: StringListSettingInfo,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct StringListSettingInfo {
    pub values: Vec<String>,
}
