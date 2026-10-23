use warpui::integration::AssertionCallback;

use crate::integration_testing::cloud_object::assert_metadata_revision;
use crate::workflows::{CloudWorkflowModel, WorkflowId};

/// Asserts metadata exists for the workflow with the given key and that the revision in that
/// metadata matches the given expected revision.
pub fn assert_workflow_metadata_revision(
    id: impl AsRef<str>,
    expected_revision: i64,
) -> AssertionCallback {
    assert_metadata_revision::<WorkflowId, CloudWorkflowModel>(id.as_ref(), expected_revision)
}
