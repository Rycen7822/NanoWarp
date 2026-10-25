use cloud_objects::cloud_object::{
    GenericCloudObject, GenericServerObject, GenericStringModel, JsonObjectType,
};
use cloud_objects::ids::GenericStringObjectId;
use serde::{Deserialize, Serialize};

use crate::{JsonModel, JsonSerializer};

/// A finite set of user-specified workflow argument values.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash, PartialOrd)]
pub struct WorkflowEnum {
    /// Enum name
    pub name: String,
    /// Whether or not the variable should be visible to other workflows
    pub is_shared: bool,
    /// The variants for this enum
    pub variants: EnumVariants,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq, Hash, PartialOrd)]
pub enum EnumVariants {
    Static(Vec<String>), // contains the explicit variants for a static enum
}

impl JsonModel for WorkflowEnum {
    fn json_object_type() -> JsonObjectType {
        JsonObjectType::WorkflowEnum
    }
}

pub type CloudWorkflowEnum = GenericCloudObject<GenericStringObjectId, CloudWorkflowEnumModel>;
pub type CloudWorkflowEnumModel = GenericStringModel<WorkflowEnum, JsonSerializer>;
pub type ServerWorkflowEnum = GenericServerObject<GenericStringObjectId, CloudWorkflowEnumModel>;
