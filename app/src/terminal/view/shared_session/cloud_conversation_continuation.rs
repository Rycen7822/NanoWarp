use crate::terminal::TerminalModel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AIQueryRouting {
    LiveRemoteVm { is_executor: bool },
    UnconnectedReadOnly,
    Local,
}

pub(crate) fn resolve_ai_query_routing(terminal_model: &TerminalModel) -> AIQueryRouting {
    let status = terminal_model.shared_session_status();
    if status.is_active_viewer() {
        AIQueryRouting::LiveRemoteVm {
            is_executor: status.is_executor(),
        }
    } else if terminal_model.is_conversation_transcript_viewer() || status.is_finished_viewer() {
        AIQueryRouting::UnconnectedReadOnly
    } else {
        AIQueryRouting::Local
    }
}

#[cfg(test)]
#[path = "cloud_conversation_continuation_tests.rs"]
mod tests;
