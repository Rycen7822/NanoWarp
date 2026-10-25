use crate::mutations::create_anonymous_user::AnonymousUserType;
use crate::object_permissions::OwnerType;
use crate::request_context::RequestContext;
use crate::scalars::Time;
use crate::schema;

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RootQuery", variables = "GetUserVariables")]
pub struct GetUser {
    #[arguments(requestContext: $request_context)]
    pub user: UserResult,
}
crate::client::define_operation! {
    get_user(GetUserVariables) -> GetUser;
}

#[derive(cynic::QueryVariables, Debug)]
pub struct GetUserVariables {
    pub request_context: RequestContext,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct UserOutput {
    pub api_key_owner_type: Option<OwnerType>,
    pub principal_type: Option<PrincipalType>,
    pub user: User,
}

#[derive(cynic::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalType {
    User,
    ServiceAccount,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct User {
    pub anonymous_user_info: Option<AnonymousUserInfo>,
    pub is_onboarded: bool,
    pub is_on_work_domain: bool,
    pub profile: FirebaseProfile,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct FirebaseProfile {
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub needs_sso_link: bool,
    pub photo_url: Option<String>,
    pub uid: String,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct AnonymousUserInfo {
    pub anonymous_user_type: AnonymousUserType,
    pub linked_at: Option<Time>,
}

#[derive(cynic::InlineFragments, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum UserResult {
    UserOutput(UserOutput),
    #[cynic(fallback)]
    Unknown,
}
