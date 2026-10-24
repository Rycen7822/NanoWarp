//! Shared-session attachment context.

use session_sharing_protocol::common::AgentAttachment;

use crate::terminal::model::BlockId;

/// Splits shared-session attachments into block references and selected text for the context model.
/// File references remain in the original user-query metadata.
pub(crate) fn resolve_agent_attachments(
    attachments: Vec<AgentAttachment>,
) -> (Vec<BlockId>, Vec<String>) {
    let mut block_ids = Vec::new();
    let mut selected_text_parts = Vec::new();
    for attachment in attachments {
        match attachment {
            AgentAttachment::BlockReference { block_id } => {
                block_ids.push(BlockId::from(block_id.to_string()));
            }
            AgentAttachment::PlainText { content } => {
                selected_text_parts.push(content);
            }
            AgentAttachment::FileReference { .. } => {}
        }
    }
    (block_ids, selected_text_parts)
}
