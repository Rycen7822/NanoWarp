//! UI state for the ambient agent progress/loading screen.

use warpui::elements::shimmering_text::ShimmeringTextStateHandle;
use warpui::elements::{MouseStateHandle, SelectionHandle};

/// UI state for rendering the ambient agent progress screen (loading or error).
/// This keeps all cloud mode UI handles together and separates them from the main TerminalView.
#[derive(Default)]
pub struct AmbientAgentProgressUIState {
    /// State handle for the shimmering text animation in the cloud mode loading screen.
    pub loading_shimmer_handle: ShimmeringTextStateHandle,

    /// Selection handle for making error text selectable in the cloud mode error screen.
    pub error_selection_handle: SelectionHandle,

    /// Stores selected text from the cloud mode error screen for copying.
    pub error_selected_text: std::rc::Rc<parking_lot::RwLock<Option<String>>>,

    /// Mouse state handle for the authenticate button in the GitHub auth screen.
    pub auth_button_mouse_state: MouseStateHandle,
}
