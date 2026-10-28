/// This module contains a model that can be used for loading and saving text files
/// and displaying them in a code editor.
/// It also handles applying an optional diff to the file content that will be applied
/// when the file is loaded.
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use remote_server::manager::RemoteServerManager;
use warp_core::r#async::debounce;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_util::content_version::ContentVersion;
use warp_util::file::{FileId, FileLoadError, FileSaveError};
use warp_util::path::LineAndColumnArg;
use warp_util::sync::Condition;
use warpui::elements::{
    ChildView, ConstrainedBox, Container, CrossAxisAlignment, Flex, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, Shrinkable, Text,
};
use warpui::platform::SaveFilePickerConfiguration;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::code::SaveOutcome;
use crate::code::buffer_location::LocalOrRemotePath as BufferFileLocation;
use crate::code::global_buffer_model::{BufferState, GlobalBufferModel, GlobalBufferModelEvent};
use crate::settings::CodeSettings;

/// How long to wait after the user stops typing before triggering a debounced
/// auto-save. Mirrors VS Code's default `files.autoSaveDelay` of 1000ms.
const AUTO_SAVE_DEBOUNCE_PERIOD: Duration = Duration::from_millis(1000);

use super::ImmediateSaveError;
use super::editor::scroll::ScrollTrigger;
use super::editor::view::{CodeEditorEvent, CodeEditorView};

type SaveCallback =
    Box<dyn FnOnce(SaveOutcome, &mut ViewContext<LocalCodeEditorView>) + Send + Sync + 'static>;

pub enum LocalCodeEditorEvent {
    FileLoaded,
    FailedToLoad {
        error: Rc<FileLoadError>,
    },
    FileSaved {
        /// Whether the save was triggered by auto-save. Used to suppress the
        /// "File saved." toast, which should only appear for manual (cmd-s) saves.
        auto_saved: bool,
    },
    FailedToSave {
        error: Arc<FileSaveError>,
    },
    /// Emitted when the diff status changes (e.g., line counts update).
    DiffStatusUpdated,
    DiscardUnsavedChanges {
        path: PathBuf,
    },
    /// Emitted when the viewport is updated after layout
    ViewportUpdated,
    /// Emitted when the render state layout has been updated.
    LayoutInvalidated,
    DelayedRenderingFlushed,
}

/// Metadata about a file that is opened in the code view.
#[derive(Debug, Clone)]
struct LoadedFileMetadata {
    id: FileId,
    location: BufferFileLocation,
}

use warp_errors::report_error;

#[derive(Debug, Clone)]
pub enum LocalCodeEditorAction {
    SaveFile,
    DiscardUnsavedChanges,
}

#[derive(Default)]
struct ConflictResolutionBannerMouseStates {
    discard_mouse_state: MouseStateHandle,
    overwrite_mouse_state: MouseStateHandle,
}

pub struct LocalCodeEditorView {
    pub(super) editor: ViewHandle<CodeEditorView>,
    metadata: Option<LoadedFileMetadata>,
    is_new_file: bool,
    /// A marker for when the backing file has first been loaded. This is used to prevent applying
    /// a diff before it can be properly calculated.
    file_loaded: Condition,
    /// Whether content was changed from its base.
    /// Content version of the base file state.
    base_content_version: Option<ContentVersion>,
    /// Set to `true` when a `RemoteBufferConflict` event fires for this
    /// editor's buffer. Cleared when the user discards or overwrites.
    has_remote_conflict: bool,
    conflict_banner_mouse_states: ConflictResolutionBannerMouseStates,
    /// Default directory to use for save dialogs when creating new files
    default_directory: Option<PathBuf>,
    /// Channel for debouncing auto-save requests while the user types.
    auto_save_debounce_tx: async_channel::Sender<()>,
    /// Set while an auto-save (debounced or focus-change) is in flight so the
    /// resulting `FileSaved` event can be marked as an auto-save and suppress
    /// the success toast.
    auto_save_in_flight: bool,
    /// Pending scroll position to apply after the file is loaded. This is used when
    /// `set_pending_scroll` is called before the file content has finished loading
    /// (e.g., in the GlobalBuffer path where content loads asynchronously).
    pending_scroll_on_load: Option<LineAndColumnArg>,
}

impl LocalCodeEditorView {
    pub fn new(editor: ViewHandle<CodeEditorView>, ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| match event {
            CodeEditorEvent::ContentChanged { origin, .. } => {
                if origin.from_user() {
                    // Queue a debounced auto-save while the user types. The
                    // debounced path saves the current buffer contents.
                    if *CodeSettings::as_ref(ctx).auto_save {
                        let _ = me.auto_save_debounce_tx.try_send(());
                    }
                }
            }
            CodeEditorEvent::DiffUpdated => {
                ctx.emit(LocalCodeEditorEvent::DiffStatusUpdated);
            }
            CodeEditorEvent::SelectionEnd => {
                ctx.notify();
            }
            CodeEditorEvent::ViewportUpdated => {
                ctx.emit(LocalCodeEditorEvent::ViewportUpdated);
            }
            CodeEditorEvent::LayoutInvalidated => {
                ctx.emit(LocalCodeEditorEvent::LayoutInvalidated);
            }
            CodeEditorEvent::DelayedRenderingFlushed => {
                ctx.emit(LocalCodeEditorEvent::DelayedRenderingFlushed);
            }
            _ => {}
        });

        // Set up debounce for auto-save while the user types.
        let (auto_save_debounce_tx, auto_save_debounce_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(
            debounce(AUTO_SAVE_DEBOUNCE_PERIOD, auto_save_debounce_rx),
            |me, (), ctx| me.auto_save_after_delay(ctx),
            |_, _| {},
        );

        // Auto-save when the editor's window is navigated away from (mirrors VS
        // Code's `files.autoSave: onWindowChange`).
        ctx.subscribe_to_model(
            &warpui::windowing::WindowManager::handle(ctx),
            Self::handle_window_focus_change,
        );

        Self {
            editor,
            is_new_file: false,
            metadata: None,
            file_loaded: Condition::new(),
            base_content_version: None,
            has_remote_conflict: false,
            conflict_banner_mouse_states: Default::default(),
            default_directory: None,
            auto_save_debounce_tx,
            auto_save_in_flight: false,
            pending_scroll_on_load: None,
        }
    }

    fn perform_save(&mut self, file_id: FileId, ctx: &mut ViewContext<Self>) {
        self.base_content_version = Some(self.editor.as_ref(ctx).version(ctx));

        let result = self.editor.update(ctx, |editor, ctx| {
            let content = editor.text(ctx);
            let buffer_version = editor.version(ctx);

            GlobalBufferModel::handle(ctx).update(ctx, move |model, ctx| {
                model.save(file_id, content.into_string(), buffer_version, ctx)
            })
        });

        if let Err(err) = result {
            // A synchronous save failure means no async `FileSaved` will arrive,
            // so clear the auto-save marker here.
            self.auto_save_in_flight = false;
            report_error!(&err);
            ctx.emit(LocalCodeEditorEvent::FailedToSave {
                error: Arc::new(err),
            });
        }
    }

    /// Auto-save triggered by the debounce timer after the user pauses typing.
    ///
    /// Saves an existing changed file after a brief pause. New/untitled
    /// files (no `file_id`) and disconnected remotes are skipped.
    fn auto_save_after_delay(&mut self, ctx: &mut ViewContext<Self>) {
        if !*CodeSettings::as_ref(ctx).auto_save {
            return;
        }

        if self.is_remote_disconnected(ctx) || !self.has_unsaved_changes(ctx) {
            return;
        }

        let Some(file_id) = self.file_id() else {
            return;
        };

        // Mark this as an
        // auto-save so the "File saved." toast is suppressed.
        self.auto_save_in_flight = true;
        self.perform_save(file_id, ctx);
    }

    /// Auto-save triggered when the editor loses focus (either another view in
    /// the app takes focus, or the editor's window is navigated away from).
    ///
    /// Mirrors VS Code's `files.autoSave: onFocusChange` / `onWindowChange`,
    /// which *do* honor format-on-save. We route through [`Self::save_local`] so
    /// the existing format-on-save setting is respected. `NoFileId` (untitled
    /// files) and `RemoteDisconnected` are expected no-ops; real save failures
    /// still surface via `FailedToSave` events.
    fn auto_save_on_focus_change(&mut self, ctx: &mut ViewContext<Self>) {
        if !*CodeSettings::as_ref(ctx).auto_save || !self.has_unsaved_changes(ctx) {
            return;
        }

        // Mark this as an auto-save so the "File saved." toast is suppressed.
        // If the save never starts (untitled file with no `file_id`, or a
        // disconnected remote), clear the marker so the next manual save still
        // shows its "File saved." toast.
        self.auto_save_in_flight = true;
        if self.save_local(ctx).is_err() {
            self.auto_save_in_flight = false;
        }
    }

    /// Handles application window focus changes. When the window containing this
    /// focused editor is navigated away from (the app is deactivated or another
    /// window becomes active), we auto-save — mirroring VS Code's
    /// `files.autoSave: onWindowChange`. In-app focus moves between views are
    /// handled separately by [`View::on_blur`].
    fn handle_window_focus_change(
        &mut self,
        _handle: ModelHandle<warpui::windowing::WindowManager>,
        event: &warpui::windowing::StateEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        let warpui::windowing::StateEvent::ValueChanged { current, previous } = event;
        let focused = ctx.is_self_or_child_focused();
        let was_window_focused = focused && previous.active_window == Some(ctx.window_id());
        let is_window_focused = focused && current.active_window == Some(ctx.window_id());

        if was_window_focused && !is_window_focused {
            self.auto_save_on_focus_change(ctx);
        }
    }

    /// Marks the next save as an auto-save so the resulting `FileSaved` event
    /// suppresses the "File saved." toast. Used by the close flow when auto-save
    /// is enabled, so closing a file flushes edits silently.
    pub fn mark_next_save_as_auto_save(&mut self) {
        self.auto_save_in_flight = true;
    }

    pub fn is_new_file(&self) -> bool {
        self.is_new_file
    }

    // This is a footgun - we should remove this codepath.
    pub fn set_new_file(&mut self, is_new: bool) {
        self.is_new_file = is_new;
    }

    pub fn set_default_directory(&mut self, directory: Option<PathBuf>) {
        self.default_directory = directory;
    }

    /// Whether the content of the source file this editor is based on has been loaded into the buffer.
    pub fn file_loaded(&self, ctx: &mut ViewContext<Self>) -> bool {
        // For global buffer, we could have utilized a shared buffer from another open editor. To avoid
        // any race condition, we directly check with the GlobalBufferModel rather than relying on self.base_content_version
        // which is updated via an async event.
        let Some(file_id) = self.file_id() else {
            return false;
        };

        GlobalBufferModel::as_ref(ctx).buffer_loaded(file_id)
    }

    /// Construct a new editor view with a shared buffer backed by the given location.
    ///
    /// Sets syntax highlighting from the local path or remote file extension.
    pub fn new_with_global_buffer<T>(
        location: BufferFileLocation,
        editor_constructor: T,
        ctx: &mut ViewContext<Self>,
    ) -> Self
    where
        T: FnOnce(BufferState, &mut ViewContext<Self>) -> ViewHandle<CodeEditorView>,
    {
        let buffer_state = GlobalBufferModel::handle(ctx)
            .update(ctx, |model, ctx| model.open(location.clone(), ctx));
        let file_id = buffer_state.file_id;
        let editor = editor_constructor(buffer_state, ctx);

        match &location {
            BufferFileLocation::Local(path) => {
                editor.update(ctx, |editor, ctx| {
                    editor.set_language_with_local_path(path, ctx);
                    editor.model.update(ctx, |model, ctx| {
                        model.rebuild_layout_with_syntax_highlighting(ctx)
                    });
                });
            }
            BufferFileLocation::Remote(remote_path) => {
                editor.update(ctx, |editor, ctx| {
                    editor.set_language_with_path(&remote_path.path, ctx);
                    editor.model.update(ctx, |model, ctx| {
                        model.rebuild_layout_with_syntax_highlighting(ctx)
                    });
                });
            }
        }

        let mut local_editor = Self::new(editor, ctx);

        local_editor.metadata = Some(LoadedFileMetadata {
            id: file_id,
            location,
        });

        Self::subscribe_to_global_buffer_events(file_id, ctx);

        local_editor
    }

    pub fn set_pending_scroll(&mut self, position: LineAndColumnArg, ctx: &mut ViewContext<Self>) {
        // If the file is already loaded, we can set the scroll trigger immediately with the
        // current buffer version. Otherwise, store it and apply when the file finishes loading.
        // This handles the race condition where set_pending_scroll is called before the file
        // content has been loaded (e.g., in the GlobalBuffer path).
        if self.file_loaded(ctx) {
            self.editor.update(ctx, |editor, ctx| {
                let version = editor.buffer_version(ctx);
                editor.set_pending_scroll(ScrollTrigger::new(position, version));
            });
        } else {
            self.pending_scroll_on_load = Some(position);
        }
    }

    fn on_file_loaded(&mut self, ctx: &mut ViewContext<Self>) {
        self.file_loaded.set();

        // Apply any pending scroll position that was set before the file finished loading.
        if let Some(position) = self.pending_scroll_on_load.take() {
            self.editor.update(ctx, |editor, ctx| {
                let version = editor.buffer_version(ctx);
                editor.set_pending_scroll(ScrollTrigger::new(position, version));
            });
        }
    }

    pub fn has_unsaved_changes(&self, ctx: &AppContext) -> bool {
        if self.is_new_file {
            let text = self.editor.as_ref(ctx).text(ctx);
            if text.as_str().is_empty() {
                return false;
            }
        }

        self.base_content_version
            .map(|base_version| !self.editor.as_ref(ctx).version_match(&base_version, ctx))
            .unwrap_or(false)
    }

    /// Unsubscribes from any existing GlobalBufferModel subscription and sets up a
    /// new one for the given `file_id`.  Handles BufferLoaded, FailedToLoad,
    /// BufferUpdatedFromFileEvent, FileSaved, and FailedToSave events.
    fn subscribe_to_global_buffer_events(file_id: FileId, ctx: &mut ViewContext<Self>) {
        ctx.unsubscribe_to_model(&GlobalBufferModel::handle(ctx));
        ctx.subscribe_to_model(&GlobalBufferModel::handle(ctx), move |me, _, event, ctx| {
            if event.file_id() != file_id {
                return;
            }
            match event {
                GlobalBufferModelEvent::BufferLoaded {
                    content_version, ..
                } => {
                    // For a reopen (discard), base_content_version is already
                    // set from the initial load. Accept the new version and
                    // clear any conflict flag.
                    me.has_remote_conflict = false;
                    if me.base_content_version.is_some() {
                        me.base_content_version = Some(*content_version);
                        ctx.notify();
                    } else {
                        me.base_content_version = Some(*content_version);
                        me.on_file_loaded(ctx);
                        ctx.emit(LocalCodeEditorEvent::FileLoaded);
                    }
                }
                GlobalBufferModelEvent::FailedToLoad { error, .. } => {
                    me.is_new_file = true;
                    me.on_file_loaded(ctx);
                    ctx.emit(LocalCodeEditorEvent::FailedToLoad {
                        error: error.clone(),
                    });
                }
                GlobalBufferModelEvent::BufferUpdatedFromFileEvent {
                    success,
                    content_version,
                    ..
                } => {
                    if !*success {
                        ctx.notify();
                    } else {
                        me.base_content_version = Some(*content_version);
                    }
                }
                GlobalBufferModelEvent::FileSaved {
                    content_version, ..
                } => {
                    // Consume the auto-save marker: suppress the toast for
                    // auto-saves, show it for manual (cmd-s) saves.
                    let auto_saved = std::mem::take(&mut me.auto_save_in_flight);
                    me.base_content_version = Some(*content_version);
                    me.has_remote_conflict = false;
                    ctx.emit(LocalCodeEditorEvent::FileSaved { auto_saved });
                }
                GlobalBufferModelEvent::FailedToSave { error, .. } => {
                    me.auto_save_in_flight = false;
                    me.base_content_version = GlobalBufferModel::as_ref(ctx).base_version(file_id);
                    ctx.emit(LocalCodeEditorEvent::FailedToSave {
                        error: error.clone(),
                    });
                }
                GlobalBufferModelEvent::RemoteBufferConflict { .. } => {
                    me.has_remote_conflict = true;
                    ctx.notify();
                }
                GlobalBufferModelEvent::ServerLocalBufferUpdated { .. } => {
                    // Not relevant for local code editors.
                }
            }
        });
    }

    pub fn has_version_conflicts(&self, app: &AppContext) -> bool {
        // Remote buffers use SyncClock for conflict detection.
        // The flag is set by the RemoteBufferConflict event handler.
        if matches!(self.file_location(), Some(BufferFileLocation::Remote(_))) {
            return self.has_remote_conflict;
        }
        let Some(file_id) = self.file_id() else {
            return false;
        };
        self.has_unsaved_changes(app)
            && self.base_content_version != GlobalBufferModel::as_ref(app).base_version(file_id)
    }

    /// Returns `true` when this editor is backed by a remote file whose
    /// host no longer has any connected session. Derived on-the-fly from
    /// `RemoteServerManager` so it is always in sync with actual
    /// connection state.
    pub fn is_remote_disconnected(&self, app: &AppContext) -> bool {
        let Some(BufferFileLocation::Remote(remote_path)) = self.file_location() else {
            return false;
        };
        RemoteServerManager::as_ref(app)
            .client_for_host(&remote_path.host_id)
            .is_none()
    }

    /// Whether auto-save can actually persist this editor's changes: it needs
    /// a backing file and, for remote files, a still-connected host. Untitled
    /// buffers (no `file_id`) and disconnected remotes return `false`.
    pub fn can_auto_save(&self, app: &AppContext) -> bool {
        self.file_id().is_some() && !self.is_remote_disconnected(app)
    }

    /// Save the file to the local file system (or remotely via the remote server).
    /// This will only return an error immediately if there is a failure in the sync part of the call.
    /// Other errors could be returned asynchronously via the FileModelEvent::FailedToSave event.
    pub fn save_local(&mut self, ctx: &mut ViewContext<Self>) -> Result<(), ImmediateSaveError> {
        if self.is_remote_disconnected(ctx) {
            return Err(ImmediateSaveError::RemoteDisconnected);
        }

        let Some(file_id) = self.file_id() else {
            return Err(ImmediateSaveError::NoFileId);
        };

        self.perform_save(file_id, ctx);
        Ok(())
    }

    /// Open a save dialog to save the file with a new name, optionally with a completion callback.
    pub fn save_as(&mut self, callback: Option<SaveCallback>, ctx: &mut ViewContext<Self>) {
        ctx.open_save_file_picker(
            move |path_opt, me, ctx| Self::handle_save_as(callback, path_opt, me, ctx),
            if let Some(default_dir) = &self.default_directory {
                SaveFilePickerConfiguration::new().with_default_directory(default_dir.clone())
            } else {
                SaveFilePickerConfiguration::new()
            },
        );
    }

    fn handle_save_as(
        callback: Option<SaveCallback>,
        path_opt: Option<String>,
        me: &mut Self,
        ctx: &mut ViewContext<Self>,
    ) {
        let callback = callback.unwrap_or(Box::new(|_, _| {}));
        let Some(path_str) = path_opt else {
            callback(SaveOutcome::Canceled, ctx);
            return;
        };
        let path = PathBuf::from(path_str);

        // Ensure parent directories exist before registering the file watcher.
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = std::fs::create_dir_all(parent);
        }

        let buffer = me.editor.as_ref(ctx).model.as_ref(ctx).buffer().clone();
        let buffer_state = GlobalBufferModel::handle(ctx)
            .update(ctx, |model, ctx| model.register(path.clone(), buffer, ctx));

        let file_id = buffer_state.file_id;
        me.metadata = Some(LoadedFileMetadata {
            id: file_id,
            location: BufferFileLocation::Local(path.clone()),
        });

        me.set_new_file(false);

        me.editor.update(ctx, |editor, ctx| {
            editor.set_language_with_local_path(&path, ctx);
        });

        let content = me.editor.as_ref(ctx).text(ctx).into_string();
        let buffer_version = me.editor.as_ref(ctx).version(ctx);

        me.base_content_version = Some(buffer_version);
        let save_outcome = match GlobalBufferModel::handle(ctx).update(ctx, move |model, ctx| {
            model.save(file_id, content, buffer_version, ctx)
        }) {
            Err(err) => {
                report_error!(&err);
                ctx.emit(LocalCodeEditorEvent::FailedToSave {
                    error: Arc::new(err),
                });
                SaveOutcome::Failed
            }
            _ => {
                Self::subscribe_to_global_buffer_events(file_id, ctx);
                SaveOutcome::Succeeded
            }
        };
        callback(save_outcome, ctx);
    }

    pub fn file_id(&self) -> Option<FileId> {
        self.metadata.as_ref().map(|m| m.id)
    }

    /// Returns the unified file location (local or remote).
    pub fn file_location(&self) -> Option<&BufferFileLocation> {
        self.metadata.as_ref().map(|m| &m.location)
    }

    /// Returns the local path if this editor is backed by a local file.
    /// Returns `None` for remote files. Used by local-only file operations.
    pub fn file_path(&self) -> Option<&Path> {
        self.file_location().and_then(|loc| loc.to_local_path())
    }

    pub fn editor(&self) -> &ViewHandle<CodeEditorView> {
        &self.editor
    }

    pub fn close_find_bar(&mut self, should_focus_editor: bool, ctx: &mut ViewContext<Self>) {
        self.editor.update(ctx, |editor, ctx| {
            editor.close_find_bar(should_focus_editor, ctx);
        });
    }
}

impl Entity for LocalCodeEditorView {
    type Event = LocalCodeEditorEvent;
}

impl View for LocalCodeEditorView {
    fn ui_name() -> &'static str {
        "LocalCodeEditorView"
    }

    fn on_focus(&mut self, focus_ctx: &warpui::FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.editor.update(ctx, |editor, ctx| editor.focus(ctx));
        }
    }

    fn on_blur(&mut self, _blur_ctx: &warpui::BlurContext, ctx: &mut ViewContext<Self>) {
        // When focus leaves this editor's subtree entirely, treat it as a
        // focus-change auto-save (mirrors VS Code's `files.autoSave:
        // onFocusChange`). Focus moving to a child view stays within the subtree, so
        // `is_self_or_child_focused` prevents saving on those transitions.
        if !ctx.is_self_or_child_focused() {
            self.auto_save_on_focus_change(ctx);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn warpui::Element> {
        if self.base_content_version.is_some() && self.is_remote_disconnected(app) {
            let appearance = Appearance::as_ref(app);
            let banner = render_remote_disconnected_banner(appearance);
            let mut col = Flex::column().with_child(banner);

            let editor_view = ChildView::new(&self.editor).finish();
            if self.editor.as_ref(app).needs_vertical_constraint() {
                col.add_child(Shrinkable::new(1., editor_view).finish());
            } else {
                col.add_child(editor_view);
            }
            col.finish()
        } else if self.has_version_conflicts(app) {
            let appearance = Appearance::as_ref(app);
            let banner = render_unsaved_changes_banner(
                appearance,
                self.conflict_banner_mouse_states
                    .discard_mouse_state
                    .clone(),
                self.conflict_banner_mouse_states
                    .overwrite_mouse_state
                    .clone(),
            );
            let mut col = Flex::column().with_child(banner);

            let editor_view = ChildView::new(&self.editor).finish();
            if self.editor.as_ref(app).needs_vertical_constraint() {
                col.add_child(Shrinkable::new(1., editor_view).finish());
            } else {
                col.add_child(editor_view);
            }
            col.finish()
        } else {
            ChildView::new(&self.editor).finish()
        }
    }
}

impl TypedActionView for LocalCodeEditorView {
    type Action = LocalCodeEditorAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            LocalCodeEditorAction::SaveFile => {
                if let Err(ImmediateSaveError::FailedToSave(err)) = self.save_local(ctx) {
                    report_error!(&err);
                    ctx.emit(LocalCodeEditorEvent::FailedToSave {
                        error: Arc::new(err),
                    });
                };
            }
            LocalCodeEditorAction::DiscardUnsavedChanges => {
                if let Some(path) = self.file_path().map(Path::to_path_buf) {
                    self.base_content_version = Some(self.editor().as_ref(ctx).version(ctx));
                    ctx.emit(LocalCodeEditorEvent::DiscardUnsavedChanges { path });
                } else if self.has_remote_conflict {
                    // Remote file: re-open the buffer from the server to get
                    // the latest on-disk content. The BufferLoaded event will
                    // clear has_remote_conflict and update base_content_version.
                    // If the re-open fails, has_remote_conflict stays true and
                    // the banner remains visible so the user can retry.
                    if let Some(file_id) = self.file_id() {
                        GlobalBufferModel::handle(ctx).update(ctx, |model, ctx| {
                            model.reopen_remote_buffer(file_id, ctx);
                        });
                    }
                }
            }
        }
    }
}

/// Renders a banner warning that the file has saved changes not reflected in the diff
pub fn render_unsaved_changes_banner(
    appearance: &Appearance,
    discard_mouse_state: MouseStateHandle,
    overwrite_mouse_state: MouseStateHandle,
) -> Box<dyn Element> {
    let left = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(
                ConstrainedBox::new(
                    Icon::Warning
                        .to_warpui_icon(appearance.theme().active_ui_text_color())
                        .finish(),
                )
                .with_height(16.)
                .with_width(16.)
                .finish(),
            )
            .with_margin_right(8.)
            .finish(),
        )
        .with_child(
            Shrinkable::new(
                1.,
                Text::new(
                    "This file has saved changes that are not reflected here.",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().active_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .finish(),
        )
        .finish();

    let right = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            appearance
                .ui_builder()
                .button(ButtonVariant::Text, discard_mouse_state)
                .with_text_label("Discard this version".into())
                .with_style(UiComponentStyles {
                    height: Some(24.),
                    padding: Some(Coords {
                        left: 8.,
                        right: 8.,
                        ..Default::default()
                    }),
                    font_color: Some(appearance.theme().active_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(LocalCodeEditorAction::DiscardUnsavedChanges)
                })
                .finish(),
        )
        .with_child(
            Container::new(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Outlined, overwrite_mouse_state)
                    .with_text_label("Overwrite".into())
                    .with_style(UiComponentStyles {
                        font_color: Some(appearance.theme().active_ui_text_color().into()),
                        ..Default::default()
                    })
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(LocalCodeEditorAction::SaveFile)
                    })
                    .finish(),
            )
            .with_margin_left(4.)
            .finish(),
        )
        .finish();

    Container::new(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Shrinkable::new(1., left).finish())
            .with_child(right)
            .finish(),
    )
    .with_background(appearance.theme().text_selection_as_context_color())
    .with_padding_top(4.)
    .with_padding_bottom(4.)
    .with_padding_left(12.)
    .with_padding_right(12.)
    .finish()
}

/// Renders a banner indicating that the remote SSH session is disconnected
/// and save / auto-reload are unavailable.
pub fn render_remote_disconnected_banner(appearance: &Appearance) -> Box<dyn Element> {
    let row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            Container::new(
                ConstrainedBox::new(
                    Icon::Warning
                        .to_warpui_icon(appearance.theme().active_ui_text_color())
                        .finish(),
                )
                .with_height(16.)
                .with_width(16.)
                .finish(),
            )
            .with_margin_right(8.)
            .finish(),
        )
        .with_child(
            Shrinkable::new(
                1.,
                Text::new(
                    "Remote host disconnected. You will not be able to see updates and save changes.",
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().active_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .finish(),
        )
        .finish();

    Container::new(row)
        .with_background(appearance.theme().text_selection_as_context_color())
        .with_padding_top(8.)
        .with_padding_bottom(8.)
        .with_padding_left(12.)
        .with_padding_right(12.)
        .finish()
}
