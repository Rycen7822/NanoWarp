use crate::experiment::Experiment;
use crate::request_context::RequestContext;
use crate::schema;
use crate::workspace::Workspace;

#[derive(cynic::QueryVariables, Debug)]
pub struct GetWorkspacesMetadataForUserVariables {
    pub request_context: RequestContext,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct UserOutput {
    pub user: User,
}

#[derive(cynic::InlineFragments, Debug)]
pub enum UserResult {
    UserOutput(UserOutput),
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct User {
    pub profile: UserProfile,
    pub workspaces: Vec<Workspace>,
    pub experiments: Option<Vec<Experiment>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FirebaseProfile")]
pub struct UserProfile {
    pub uid: String,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(
    graphql_type = "RootQuery",
    variables = "GetWorkspacesMetadataForUserVariables"
)]
pub struct GetWorkspacesMetadataForUser {
    #[arguments(requestContext: $request_context)]
    pub user: UserResult,
}
crate::client::define_operation! {
    get_workspaces_metadata_for_user(GetWorkspacesMetadataForUserVariables) -> GetWorkspacesMetadataForUser;
}
