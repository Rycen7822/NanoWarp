use crate::schema;

#[derive(cynic::InputObject, Debug)]
pub struct TaskInput {
    pub task_id: cynic::Id,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct TaskAttachment {
    pub file_id: cynic::Id,
    pub filename: String,
    pub download_url: String,
    pub mime_type: String,
}
