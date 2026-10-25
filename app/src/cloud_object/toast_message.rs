use warpui::AppContext;

use super::CloudObject;
use crate::server::cloud_objects::update_manager::{
    InitiatedBy, ObjectOperation, OperationSuccessType,
};

pub struct CloudObjectToastMessage;

impl CloudObjectToastMessage {
    pub fn toast_message(
        object: &dyn CloudObject,
        operation: &ObjectOperation,
        success_type: &OperationSuccessType,
        app: &AppContext,
    ) -> Option<String> {
        let object_name = object.model_type_name().to_owned();
        let object_name_lowercase = object_name.to_ascii_lowercase();

        match (operation, success_type) {
            // We should only show toasts for creates initiated by the user, not by the system
            (
                ObjectOperation::Create {
                    initiated_by: InitiatedBy::User,
                },
                OperationSuccessType::Success,
            ) => {
                let containing_object_name = object.containing_object_name(app);
                Some(format!("{object_name} saved to {containing_object_name}"))
            }
            (ObjectOperation::Update, OperationSuccessType::Success) => {
                Some(format!("{object_name} updated"))
            }
            (ObjectOperation::MoveToFolder, OperationSuccessType::Success) => {
                let containing_object_name = object.containing_object_name(app);
                Some(format!("{object_name} moved to {containing_object_name}"))
            }
            (ObjectOperation::Trash, OperationSuccessType::Success) => {
                Some(format!("{object_name} trashed"))
            }
            (ObjectOperation::Untrash, OperationSuccessType::Success) => {
                Some(format!("{object_name} restored"))
            }
            (ObjectOperation::Leave, OperationSuccessType::Success) => {
                Some(format!("Left {object_name}"))
            }
            (
                ObjectOperation::Create {
                    initiated_by: InitiatedBy::User,
                },
                OperationSuccessType::Failure,
            ) => Some(format!("Failed to create {object_name_lowercase}")),
            (
                ObjectOperation::Create {
                    initiated_by: InitiatedBy::User,
                },
                OperationSuccessType::Denied(message),
            ) => Some(message.to_string()),
            (ObjectOperation::Update, OperationSuccessType::Failure) => {
                Some(format!("Failed to update {object_name_lowercase}"))
            }
            (ObjectOperation::MoveToFolder, OperationSuccessType::Failure) => {
                Some(format!("Failed to move {object_name_lowercase}"))
            }
            (ObjectOperation::Trash, OperationSuccessType::Failure) => {
                Some(format!("Failed to trash {object_name_lowercase}"))
            }
            (ObjectOperation::Untrash, OperationSuccessType::Failure) => {
                Some(format!("Failed to restore {object_name_lowercase}"))
            }
            // We should only show deletion failure toasts for user-initiated deletions.
            (
                ObjectOperation::Delete {
                    initiated_by: InitiatedBy::User,
                },
                OperationSuccessType::Failure,
            ) => Some(format!("Failed to delete {object_name_lowercase}")),
            (ObjectOperation::Leave, OperationSuccessType::Failure) => {
                Some(format!("Failed to leave {object_name}"))
            }
            (ObjectOperation::UpdatePermissions, OperationSuccessType::Success) => Some(format!(
                "Successfully updated permissions for {object_name_lowercase}"
            )),
            (ObjectOperation::UpdatePermissions, OperationSuccessType::Failure) => Some(format!(
                "Failed to update permissions for {object_name_lowercase}"
            )),
            _ => None,
        }
    }

    pub fn toast_deletion_confirm_message(
        num_objects: i32,
        operation: &ObjectOperation,
        success_type: &OperationSuccessType,
    ) -> Option<String> {
        let count_objects_message = match num_objects {
            1 => "1 object".to_string(),
            n => {
                format!("{n} objects")
            }
        };
        match (operation, success_type) {
            // We should only show deletion failure toasts for user-initiated deletions.
            (
                ObjectOperation::Delete {
                    initiated_by: InitiatedBy::User,
                },
                OperationSuccessType::Success,
            ) => Some(format!("{count_objects_message} deleted forever")),
            (ObjectOperation::EmptyTrash, OperationSuccessType::Success) => Some(format!(
                "Trash emptied: {count_objects_message} deleted forever"
            )),
            (ObjectOperation::EmptyTrash, OperationSuccessType::Failure) => {
                Some("Failed to empty trash".to_string())
            }
            (ObjectOperation::EmptyTrash, OperationSuccessType::Rejection) => {
                Some("No objects in trash to empty".to_string())
            }
            _ => None,
        }
    }
}
