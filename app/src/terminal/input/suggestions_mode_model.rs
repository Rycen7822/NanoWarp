use warpui::{Entity, ModelContext};

use super::{DynamicEnumSuggestionStatus, InputSuggestionsMode};

/// Model responsible for managing the input suggestions mode state.
#[derive(Default)]
pub struct InputSuggestionsModeModel {
    mode: InputSuggestionsMode,
}

impl InputSuggestionsModeModel {
    pub fn mode(&self) -> &InputSuggestionsMode {
        &self.mode
    }

    pub fn set_mode(&mut self, mode: InputSuggestionsMode, ctx: &mut ModelContext<Self>) {
        if self.mode == mode {
            return;
        }

        // If we're setting a new non-closed mode while the current mode is also non-closed,
        // first emit a mode change for the implicit close before transitioning to the new mode.
        if self.is_visible() && !matches!(mode, InputSuggestionsMode::Closed) {
            self.mode = InputSuggestionsMode::Closed;
            ctx.emit(());
        }

        self.mode = mode;
        ctx.emit(());
    }

    pub fn set_dynamic_enum_status(
        &mut self,
        status: DynamicEnumSuggestionStatus,
        ctx: &mut ModelContext<Self>,
    ) {
        if let InputSuggestionsMode::DynamicWorkflowEnumSuggestions {
            dynamic_enum_status,
            ..
        } = &mut self.mode
        {
            *dynamic_enum_status = status;
            ctx.emit(());
        }
    }

    pub fn is_visible(&self) -> bool {
        self.mode.is_visible()
    }

    pub fn is_closed(&self) -> bool {
        matches!(self.mode, InputSuggestionsMode::Closed)
    }

    pub fn is_history_up(&self) -> bool {
        matches!(self.mode, InputSuggestionsMode::HistoryUp { .. })
    }

    pub fn is_completion_suggestions(&self) -> bool {
        matches!(
            self.mode,
            InputSuggestionsMode::CompletionSuggestions { .. }
        )
    }

    pub fn is_static_workflow_enum_suggestions(&self) -> bool {
        matches!(
            self.mode,
            InputSuggestionsMode::StaticWorkflowEnumSuggestions { .. }
        )
    }

    pub fn is_dynamic_workflow_enum_suggestions(&self) -> bool {
        matches!(
            self.mode,
            InputSuggestionsMode::DynamicWorkflowEnumSuggestions { .. }
        )
    }
}

impl Entity for InputSuggestionsModeModel {
    type Event = ();
}
