//! Source-facing helpers that centralize the derivation of the agent-icon shape
//! ([`IconWithStatusVariant`]) from the underlying state models. The invariant the
//! helpers enforce: any single logical agent run renders as the same brand color, glyph,
//! treatment regardless of which surface is rendering it (vertical
//! tabs, pane header, conversation list, notifications mailbox).
//!
//! Each helper is a thin adapter over one data source. Surfaces call the helper for
//! whichever source they hold and feed the resulting variant into
//! [`render_icon_with_status`]. The pure inner functions in this module are exercised
//! directly by the cross-surface consistency tests in `agent_icon_tests.rs`.
use warp_cli::agent::Harness;
use warpui::{AppContext, SingletonEntity};

use crate::ai::agent::conversation::ConversationStatus;
use crate::ai::agent_conversations_model::{
    AgentConversationEntry, AgentConversationsModel, AgentRunDisplayStatus,
};
use crate::terminal::CLIAgent;
use crate::terminal::view::TerminalView;
use crate::ui_components::icon_with_status::IconWithStatusVariant;

/// Returns the agent-icon variant for a live [`TerminalView`], or `None` when the terminal is
/// not an agent surface (plain terminal / shell / empty conversation).
///
/// Resolution order:
/// 1. A task-backed run uses task status and harness so the terminal chrome and the
///    matching conversation list card stay in lockstep.
/// 2. A selected local conversation falls through to the
///    no-task waterfall.
/// 3. Everything else returns `None` so the caller renders a plain-terminal indicator.
pub(crate) fn terminal_view_agent_icon_variant(
    terminal_view: &TerminalView,
    app: &AppContext,
) -> Option<IconWithStatusVariant> {
    let task_data = terminal_view
        .ambient_agent_task_id_for_details_panel(app)
        .and_then(|task_id| AgentConversationsModel::as_ref(app).get_task_data(&task_id));

    // Defer to the card helper when we have task data.
    if let Some(task) = task_data.as_ref() {
        let status = AgentRunDisplayStatus::from_task(task, app).to_conversation_status();
        let harness = task
            .agent_config_snapshot
            .as_ref()
            .and_then(|config| config.harness.as_ref())
            .map(|harness| harness.harness_type)
            .unwrap_or(Harness::Oz);
        return Some(agent_icon_variant_for_run(harness, status));
    }

    let inputs = TerminalIconInputs {
        selected_conversation_status: terminal_view.selected_conversation_status_for_display(app),
        has_selected_conversation: terminal_view
            .selected_conversation_display_title(app)
            .is_some(),
    };
    agent_icon_variant_from_terminal_inputs(&inputs)
}

pub(crate) fn agent_conversation_entry_icon_variant(
    entry: &AgentConversationEntry,
) -> IconWithStatusVariant {
    let status = entry.display.status.to_conversation_status();
    agent_icon_variant_for_run(entry.display.harness.unwrap_or(Harness::Oz), status)
}

/// Primitive inputs to the terminal-view waterfall, gathered once from the live
/// [`TerminalView`] / [`AppContext`].
struct TerminalIconInputs {
    /// The conversation status that the terminal view would surface in its status-icon slot.
    selected_conversation_status: Option<ConversationStatus>,
    /// Whether the terminal view currently has a selected conversation.
    has_selected_conversation: bool,
}

/// Pure waterfall from primitive inputs to an [`IconWithStatusVariant`]. Mirrors the
/// resolution order documented on [`terminal_view_agent_icon_variant`].
fn agent_icon_variant_from_terminal_inputs(
    inputs: &TerminalIconInputs,
) -> Option<IconWithStatusVariant> {
    if inputs.has_selected_conversation {
        return Some(IconWithStatusVariant::OzAgent {
            status: inputs.selected_conversation_status.clone(),
        });
    }

    None
}

/// Pure run-card logic: maps a [`Harness`] and status into an
/// [`IconWithStatusVariant`]. Falls back to the Oz variant for [`Harness::Oz`] and
/// [`Harness::Unknown`], the latter so a future-server harness this client doesn't
/// recognize doesn't render an unbranded gray circle.
pub(crate) fn agent_icon_variant_for_run(
    harness: Harness,
    status: ConversationStatus,
) -> IconWithStatusVariant {
    let cli_agent =
        CLIAgent::from_harness(harness).filter(|agent| !matches!(agent, CLIAgent::Unknown));
    match cli_agent {
        Some(agent) => IconWithStatusVariant::CLIAgent {
            agent,
            status: Some(status),
        },
        None => IconWithStatusVariant::OzAgent {
            status: Some(status),
        },
    }
}

#[cfg(test)]
#[path = "agent_icon_tests.rs"]
mod tests;
