use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use warp_core::ui::appearance::Appearance;
use warp_editor::content::buffer::InitialBufferState;
use warp_util::file::{FileLoadError, FileSaveError};
use warpui::elements::MouseStateHandle;
use warpui::{AppContext, Element, Entity, TypedActionView, View, ViewContext, ViewHandle};

use super::ImmediateSaveError;
use super::editor::view::CodeEditorView;

#[derive(Debug)]
pub enum LocalCodeEditorEvent {
    #[allow(dead_code)]
    FileLoaded,
    #[allow(dead_code)]
    FailedToLoad { error: Rc<FileLoadError> },
    #[allow(dead_code)]
    FileSaved { auto_saved: bool },
    #[allow(dead_code)]
    FailedToSave { error: Arc<FileSaveError> },
    #[allow(dead_code)]
    DiffStatusUpdated,
    #[allow(dead_code)]
    DiscardUnsavedChanges { path: PathBuf },
    #[allow(dead_code)]
    ViewportUpdated,
    #[allow(dead_code)]
    DelayedRenderingFlushed,
    #[allow(dead_code)]
    LayoutInvalidated,
}

pub struct LocalCodeEditorView {
    editor: ViewHandle<CodeEditorView>,
}

impl LocalCodeEditorView {
    pub fn new(editor: ViewHandle<CodeEditorView>, _ctx: &mut ViewContext<Self>) -> Self {
        Self { editor }
    }

    pub fn reset_with_state(&mut self, _state: InitialBufferState, _ctx: &mut ViewContext<Self>) {}

    pub fn editor(&self) -> &ViewHandle<CodeEditorView> {
        &self.editor
    }

    pub fn save_local(&self, _ctx: &mut ViewContext<Self>) -> Result<(), ImmediateSaveError> {
        Err(ImmediateSaveError::NoFileId)
    }

    pub fn has_unsaved_changes(&self, _ctx: &AppContext) -> bool {
        false
    }

    /// Stub: the WASM editor tracks no auto-save marker, so this is a no-op.
    pub fn mark_next_save_as_auto_save(&mut self) {}

    /// Stub: the WASM editor has no backing file, so nothing can be auto-saved.
    pub fn can_auto_save(&self, _app: &AppContext) -> bool {
        false
    }
}

impl Entity for LocalCodeEditorView {
    type Event = LocalCodeEditorEvent;
}

impl View for LocalCodeEditorView {
    fn ui_name() -> &'static str {
        "LocalCodeEditorView"
    }
    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        warpui::elements::Empty::new().finish()
    }
}

impl TypedActionView for LocalCodeEditorView {
    type Action = ();
}

pub fn render_unsaved_circle_with_tooltip(
    _mouse_state: MouseStateHandle,
    _tooltip_text: String,
    _size: f32,
    _right_margin: f32,
    _appearance: &Appearance,
) -> Box<dyn Element> {
    warpui::elements::Empty::new().finish()
}
