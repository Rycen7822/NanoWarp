//! AI Assistant has since been renamed to "Warp AI" in the product.
use std::collections::HashSet;
use std::sync::Arc;

use warp_core::command::ExitCode;
use warp_graphql::ai::{
    RequestLimitInfo as RequestLimitInfoGraphql,
    RequestLimitRefreshDuration as RequestLimitRefreshDurationGraphql,
};

use crate::ai::{RequestLimitInfo, RequestLimitRefreshDuration};
use crate::server::telemetry::OpenedWarpAISource;
use crate::terminal::model::terminal_model::BlockIndex;

pub mod execution_context;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AskAIType {
    /// Covers all possible origins of text selection, including the block list terminal,
    /// the alt-screen terminal, and the input area. Not all instances will require
    /// `populate_input_box`, which determines whether we should automatically render
    /// something like "Explain the following" within the user's input box.
    FromTextSelection {
        text: Arc<String>,
        populate_input_box: bool,
    },
    /// Data about a block to inform Agent Mode.
    FromBlock {
        input: Arc<String>,
        output: Arc<String>,
        exit_code: ExitCode,
        block_index: BlockIndex,
    },
    /// Which blocks to attach to a block list AI query.
    FromBlocks { block_indices: HashSet<BlockIndex> },
}

impl From<&AskAIType> for OpenedWarpAISource {
    fn from(value: &AskAIType) -> Self {
        match value {
            AskAIType::FromBlock { .. } | AskAIType::FromBlocks { .. } => {
                OpenedWarpAISource::HelpWithBlock
            }
            AskAIType::FromTextSelection { .. } => OpenedWarpAISource::HelpWithTextSelection,
        }
    }
}

impl From<RequestLimitRefreshDurationGraphql> for RequestLimitRefreshDuration {
    fn from(value: RequestLimitRefreshDurationGraphql) -> Self {
        match value {
            RequestLimitRefreshDurationGraphql::Monthly => RequestLimitRefreshDuration::Monthly,
            RequestLimitRefreshDurationGraphql::Weekly => RequestLimitRefreshDuration::Weekly,
            RequestLimitRefreshDurationGraphql::EveryTwoWeeks => {
                RequestLimitRefreshDuration::EveryTwoWeeks
            }
        }
    }
}

impl From<RequestLimitInfoGraphql> for RequestLimitInfo {
    fn from(value: RequestLimitInfoGraphql) -> Self {
        RequestLimitInfo {
            is_unlimited: value.is_unlimited,
            limit: value.request_limit as usize,
            num_requests_used_since_refresh: value.requests_used_since_last_refresh as usize,
            included_usage_cents: value.included_usage_cents,
            usage_cents_used_since_last_refresh: value.usage_cents_used_since_last_refresh,
            next_refresh_time: value.next_refresh_time,
            request_limit_refresh_duration: value.request_limit_refresh_duration.into(),
            is_unlimited_voice: value.is_unlimited_voice,
            voice_request_limit: value.voice_request_limit as usize,
            voice_requests_used_since_last_refresh: value.voice_requests_used_since_last_refresh
                as usize,
            is_unlimited_codebase_indices: value.is_unlimited_codebase_indices,
            max_codebase_indices: value.max_codebase_indices as usize,
            max_files_per_repo: value.max_files_per_repo as usize,
            embedding_generation_batch_size: value.embedding_generation_batch_size as usize,
        }
    }
}
