use std::sync::Arc;

pub use cloud_object_models::{CloudWorkflow, CloudWorkflowModel, WorkflowId};
use serde::{Deserialize, Serialize};

use anyhow::Result;
use workflow::Workflow;

pub mod command_parser;
mod env_var_selector;
pub mod info_box;
pub mod workflow;
pub mod workflow_enum;

use async_trait::async_trait;

use crate::appearance::Appearance;
use crate::cloud_object::{
    CloudModelType, CloudObjectEventEntrypoint, CloudObjectUpsertParams, CreateCloudObjectResult,
    CreateObjectRequest, GenericServerObject, ObjectType, Revision, UpdateCloudObjectResult,
};
use crate::drive::CloudObjectTypeAndId;
use crate::drive::items::WarpDriveItem;
use crate::drive::items::workflow::WarpDriveWorkflow;
use crate::notebooks::{NotebookId, NotebookLocation};
use crate::persistence::ModelEvent;
use crate::server::cloud_objects::update_manager::InitiatedBy;
use crate::server::ids::{ServerId, SyncId};
use crate::server::server_api::object::ObjectClient;
use crate::server::sync_queue::{QueueItem, SerializedModel};

#[derive(Copy, Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash)]
pub enum WorkflowSource {
    Team {
        team_uid: ServerId,
    },
    PersonalCloud,
    WarpAI,
    Notebook {
        notebook_id: Option<NotebookId>,
        team_uid: Option<ServerId>,
        location: NotebookLocation,
    },
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash, PartialOrd)]
pub enum WorkflowSelectionSource {
    WarpDrive,
    CommandPalette,
    WarpAI,
    Notebook,
    SlashMenu,
    Undefined,
}

/// Workflows used by cloud and notebooks.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowType {
    /// Saved workflows from personal or team collections, saved using cloud-sync.
    Cloud(Box<CloudWorkflow>),
    /// A workflow that's part of a cloud notebook.
    Notebook(Box<Workflow>),
}

impl WorkflowType {
    pub fn as_workflow(&self) -> &Workflow {
        match self {
            WorkflowType::Cloud(workflow) => &workflow.model().data,
            WorkflowType::Notebook(workflow) => workflow,
        }
    }

    /// The object type and ID for the cloud object containing this workflow, if there is
    /// one. This is currently only supported for cloud workflows, not workflows within notebooks.
    pub fn object_id(&self) -> Option<CloudObjectTypeAndId> {
        match self {
            WorkflowType::Cloud(workflow) => Some(CloudObjectTypeAndId::Workflow(workflow.id)),
            _ => None,
        }
    }

    pub fn sync_id(&self) -> Option<SyncId> {
        match self {
            WorkflowType::Cloud(workflow) => Some(workflow.id),
            _ => None,
        }
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl CloudModelType for CloudWorkflowModel {
    type CloudObjectType = CloudWorkflow;
    type IdType = WorkflowId;

    fn model_type_name(&self) -> &'static str {
        "Workflow"
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Workflow
    }

    fn cloud_object_type_and_id(&self, id: SyncId) -> CloudObjectTypeAndId {
        CloudObjectTypeAndId::Workflow(id)
    }

    fn display_name(&self) -> String {
        self.data.name().to_string()
    }

    fn set_display_name(&mut self, name: &str) {
        self.data.set_name(name);
    }

    fn upsert_event(params: CloudObjectUpsertParams<Self>) -> ModelEvent {
        ModelEvent::UpsertWorkflow {
            workflow: CloudWorkflow::from(params),
        }
    }

    fn bulk_upsert_event(objects: Vec<CloudObjectUpsertParams<Self>>) -> ModelEvent {
        ModelEvent::UpsertWorkflows(objects.into_iter().map(CloudWorkflow::from).collect())
    }

    fn create_object_queue_item(
        &self,
        workflow: &CloudWorkflow,
        entrypoint: CloudObjectEventEntrypoint,
        initiated_by: InitiatedBy,
    ) -> Option<QueueItem> {
        if let SyncId::ClientId(client_id) = workflow.id {
            return Some(QueueItem::CreateWorkflow {
                object_type: self.object_type(),
                owner: workflow.permissions.owner,
                model: Arc::new(workflow.model().clone()),
                initial_folder_id: workflow.metadata.folder_id,
                entrypoint,
                id: client_id,
                initiated_by,
            });
        }
        None
    }

    fn update_object_queue_item(
        &self,
        revision_ts: Option<Revision>,
        workflow: &CloudWorkflow,
    ) -> QueueItem {
        QueueItem::UpdateWorkflow {
            // Note that this is intentionally a deep clone of the model because we are grabbing
            // a snapshot to update at a moment in time.
            model: workflow.model().clone().into(),
            id: workflow.id,
            revision: revision_ts.or(workflow.metadata.revision),
        }
    }

    fn should_update_after_server_conflict(&self) -> bool {
        true
    }

    fn serialized(&self) -> SerializedModel {
        SerializedModel::new(
            serde_json::to_string(&self.data).expect("failed to serialize workflow"),
        )
    }

    async fn send_create_request(
        object_client: Arc<dyn ObjectClient>,
        request: CreateObjectRequest,
    ) -> Result<CreateCloudObjectResult> {
        object_client.create_workflow(request).await
    }

    async fn send_update_request(
        &self,
        object_client: Arc<dyn ObjectClient>,
        server_id: ServerId,
        revision: Option<Revision>,
    ) -> Result<UpdateCloudObjectResult<GenericServerObject<WorkflowId, Self>>> {
        object_client
            .update_workflow(
                server_id.into(),
                serde_json::to_string(&self.data)?.into(),
                revision,
            )
            .await
    }

    fn renders_in_warp_drive(&self) -> bool {
        !self.data.is_agent_mode_workflow()
    }

    fn to_warp_drive_item(
        &self,
        id: SyncId,
        _appearance: &Appearance,
        workflow: &CloudWorkflow,
    ) -> Option<Box<dyn WarpDriveItem>> {
        if self.data.is_agent_mode_workflow() {
            return None;
        }
        Some(Box::new(WarpDriveWorkflow::new(
            self.cloud_object_type_and_id(id),
            workflow.clone(),
        )))
    }
}
