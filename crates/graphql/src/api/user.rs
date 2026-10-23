use crate::schema;

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct PublicUserProfile {
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub photo_url: Option<String>,
    pub uid: String,
}
