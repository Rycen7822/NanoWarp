use crate::folder::{Folder, FolderWithDescendants};
use crate::generic_string_object::GenericStringObject;
use crate::notebook::Notebook;
use crate::scalars::Time;
use crate::schema;

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct ObjectMetadata {
    pub creator_uid: Option<cynic::Id>,
    pub current_editor_uid: Option<cynic::Id>,
    pub is_welcome_object: bool,
    pub last_editor_uid: Option<cynic::Id>,
    pub metadata_last_updated_ts: Time,
    pub parent: Container,
    pub revision_ts: Time,
    pub trashed_ts: Option<Time>,
    pub uid: cynic::Id,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
#[cynic(non_exhaustive)]
pub enum ObjectType {
    #[cynic(rename = "Folder")]
    Folder,
    #[cynic(rename = "GenericStringObject")]
    GenericStringObject,
    #[cynic(rename = "Notebook")]
    Notebook,
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct ObjectUpdateSuccess {
    pub last_editor_uid: cynic::Id,
    pub revision_ts: Time,
}

#[allow(clippy::large_enum_variant)]
#[derive(cynic::InlineFragments, Debug, Clone)]
pub enum CloudObject {
    Folder(Folder),
    GenericStringObject(GenericStringObject),
    Notebook(Notebook),
    #[cynic(fallback)]
    Unknown,
}

#[allow(clippy::large_enum_variant)]
#[derive(cynic::InlineFragments, Debug, Clone)]
pub enum CloudObjectWithDescendants {
    FolderWithDescendants(FolderWithDescendants),
    GenericStringObject(GenericStringObject),
    Notebook(Notebook),
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
pub enum CloudObjectEventEntrypoint {
    #[cynic(rename = "Blocklist")]
    Blocklist,
    #[cynic(rename = "DriveIndex")]
    DriveIndex,
    #[cynic(rename = "ImportModal")]
    ImportModal,
    #[cynic(rename = "Onboarding")]
    Onboarding,
    #[cynic(rename = "ResourceCenter")]
    ResourceCenter,
    #[cynic(rename = "TeamSettings")]
    TeamSettings,
    #[cynic(rename = "UniversalSearch")]
    UniversalSearch,
    #[cynic(rename = "Unknown")]
    Unknown,
    #[cynic(rename = "UpgradePage")]
    UpgradePage,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct Space {
    pub uid: cynic::Id,
    #[cynic(rename = "type")]
    pub type_: SpaceType,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
pub enum SpaceType {
    #[cynic(rename = "Team")]
    Team,
    #[cynic(rename = "User")]
    User,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct FolderContainer {
    pub folder_uid: cynic::Id,
}

#[derive(cynic::InlineFragments, Debug, Clone)]
pub enum Container {
    FolderContainer(FolderContainer),
    Space(Space),
    #[cynic(fallback)]
    Unknown,
}
