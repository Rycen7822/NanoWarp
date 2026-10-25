use crate::schema;
use crate::workspace::Workspace;

#[derive(cynic::QueryFragment, Debug)]
pub struct User {
    pub profile: UserProfile,
    pub workspaces: Vec<Workspace>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FirebaseProfile")]
pub struct UserProfile {
    pub uid: String,
}
