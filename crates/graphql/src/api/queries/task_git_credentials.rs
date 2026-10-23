use crate::schema;

#[derive(cynic::QueryFragment, Debug)]
pub struct TaskGitCredential {
    pub token: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub host: String,
}
