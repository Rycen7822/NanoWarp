use super::object::{CloudObjectEventEntrypoint, ObjectMetadata};
use super::object_permissions::ObjectPermissions;
use crate::schema;

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct GenericStringObject {
    pub format: GenericStringObjectFormat,
    pub metadata: ObjectMetadata,
    pub permissions: ObjectPermissions,
    pub serialized_model: String,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(non_exhaustive)]
pub enum GenericStringObjectFormat {
    #[cynic(rename = "JsonPreference")]
    JsonPreference,
    /// Fallback for GSO formats this client build does not recognize (for example
    /// server-only formats such as `JsonRunner`). Without this, decoding a Drive
    /// sync response that contains an unknown format fails for the entire response.
    /// This variant only arises when deserializing; we never serialize it.
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::InputObject, Debug)]
pub struct GenericStringObjectUniqueKey {
    pub key: String,
    pub unique_per: UniquePer,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
pub enum UniquePer {
    #[cynic(rename = "User")]
    User,
}

impl std::fmt::Display for GenericStringObjectFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            GenericStringObjectFormat::JsonPreference => "JsonPreference",
            GenericStringObjectFormat::Unknown => "Unknown",
        };
        write!(f, "{s}")
    }
}

#[derive(cynic::InputObject, Debug)]
pub struct GenericStringObjectInput {
    pub client_id: cynic::Id,
    pub entrypoint: CloudObjectEventEntrypoint,
    pub format: GenericStringObjectFormat,
    pub initial_folder_id: Option<cynic::Id>,
    pub serialized_model: String,
    pub uniqueness_key: Option<GenericStringObjectUniqueKey>,
}
