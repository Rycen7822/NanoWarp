//! This module contains model, controller, and view logic for Blocklist AI.
mod action_model;
pub mod agent_view;
pub mod block;
pub mod code_block;
mod context_model;
mod controller;
pub(crate) mod conversation_selection;
pub(crate) mod diff_storage;
pub(crate) mod diff_types;

pub(crate) mod local_agent_task_sync_model;
pub(crate) mod queued_query;
pub(super) use controller::RequestInput;
pub mod history_model;
pub mod inline_action;
mod input_mode_policy;
mod input_model;
mod permissions;
mod persistence;
pub mod prompt;
pub mod summarization_cancel_dialog;
pub mod usage;

pub(crate) mod telemetry_banner;
pub(crate) mod view_util;

pub use action_model::{
    BlocklistAIActionEvent, BlocklistAIActionModel, ShellCommandExecutor,
    ShellCommandExecutorEvent, ShellRecoveryResult,
};
#[cfg_attr(target_family = "wasm", allow(unused_imports))]
pub(crate) use action_model::{ReadFileContextResult, read_local_file_context};
pub use block::keyboard_navigable_buttons;
#[cfg(any(test, feature = "integration_tests"))]
pub(crate) use block::model::testing::FakeAIBlockModel;
pub(crate) use block::{AIBlock, AIBlockEvent, RequestedEditResolution, init, model};
pub(crate) use context_model::block_context_from_terminal_model;
pub use context_model::{
    AttachmentType, BlocklistAIContextEvent, BlocklistAIContextModel, PendingAttachment,
    PendingFile,
};
pub use controller::BlocklistAIController;
pub use controller::input_context::{
    BLOCK_CONTEXT_ATTACHMENT_REGEX, DIFF_HUNK_ATTACHMENT_REGEX, DRIVE_OBJECT_ATTACHMENT_REGEX,
};
#[cfg(test)]
pub(crate) use controller::response_stream::ResponseStream;
pub(crate) use controller::response_stream::ResponseStreamId;
pub(crate) use controller::{
    BlocklistAIControllerEvent, ClientIdentifiers, SessionContext, SlashCommandRequest,
};
pub(crate) use conversation_selection::{
    ConversationSelection, ConversationSelectionEvent, ConversationSelectionHandle,
    PendingQueryState,
};
pub(crate) use history_model::{
    BeginConversationRenameError, BlocklistAIHistoryEvent, BlocklistAIHistoryModel,
    ConversationStatusUpdate, FORK_PREFIX, PRE_REWIND_PREFIX,
};
pub(crate) use input_model::BlocklistAIInputEvent;
pub use input_model::{
    BlocklistAIInputModel, InputConfig, InputType, InputTypeAutoDetectionSource,
};
#[cfg(test)]
pub(crate) use permissions::is_agent_mode_autonomy_allowed;
pub use permissions::{BlocklistAIPermissions, CommandExecutionPermissionAllowedReason};
#[cfg_attr(target_family = "wasm", allow(unused))]
pub(crate) use persistence::PersistedAIInputType;
#[cfg_attr(target_family = "wasm", allow(unused))]
pub use persistence::maybe_build_ai_query_upsert_event;
pub(crate) use persistence::{PersistedAIInput, SerializedBlockListItem};
pub(crate) use queued_query::{
    AutofireAction, QueuedQuery, QueuedQueryId, QueuedQueryOrigin, is_lrc_auto_queue_active,
};
pub use queued_query::{QueuedQueryEvent, QueuedQueryModel};
pub use view_util::error_color;
pub(crate) use view_util::{
    ATTACH_AS_AGENT_MODE_CONTEXT_TEXT, CLAUDE_ORANGE, NEW_AGENT_PANE_LABEL, ai_brand_color,
    ai_indicator_height, format_credits, get_ai_block_overflow_menu_element_position_id,
    get_attached_blocks_chip_element_position_id, render_ai_agent_mode_icon,
    render_ai_follow_up_icon,
};

pub use crate::ai::blocklist::block::{AIBlockResponseRating, TextLocation, secret_redaction};
