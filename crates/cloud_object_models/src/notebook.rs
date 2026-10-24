#[cfg(not(target_family = "wasm"))]
pub mod persistence;

use cloud_objects::cloud_object::{
    GenericCloudObject, GenericServerObject, ObjectType, ServerObjectModel,
};
use cloud_objects::ids::{ServerId, SyncId};
use serde::{Deserialize, Serialize};

/// Serialized representation of a notebook for sync queue
/// Retired document markers are accepted only to reject old create inputs.
#[derive(Serialize, Deserialize)]
pub struct SerializedNotebook {
    pub data: String,
    #[serde(default, skip_serializing)]
    pub ai_document_id: Option<String>,
    #[serde(default, skip_serializing)]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloudNotebookModel {
    pub title: String,
    pub data: String,
}

impl ServerObjectModel for CloudNotebookModel {
    fn object_type(&self) -> ObjectType {
        ObjectType::Notebook
    }
}

/// This is the notebook_id in the database associated with this notebook.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct NotebookId(ServerId);
cloud_objects::server_id_traits! { NotebookId, "Notebook" }

impl From<NotebookId> for SyncId {
    fn from(id: NotebookId) -> Self {
        Self::ServerId(id.into())
    }
}

/// `CloudNotebook` is a notebook retrieved from the server.
pub type CloudNotebook = GenericCloudObject<NotebookId, CloudNotebookModel>;
pub type ServerNotebook = GenericServerObject<NotebookId, CloudNotebookModel>;
