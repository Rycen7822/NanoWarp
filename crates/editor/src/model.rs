use std::ops::Range;

use itertools::{Either, Itertools};
use line_ending::LineEnding;
use string_offset::{ByteOffset, CharOffset};
use vec1::{Vec1, vec1};
use warpui_core::clipboard::ClipboardContent;
use warpui_core::{AppContext, Entity, ModelAsRef, ModelContext, ModelHandle};

use crate::content::buffer::{
    AutoScrollBehavior, Buffer, BufferEditAction, BufferSelectAction, EditOrigin, SelectionOffsets,
    ShouldAutoscroll, ToBufferCharOffset,
};
use crate::content::selection_model::BufferSelectionModel;
use crate::content::text::TextStyles;
use crate::content::version::BufferVersion;
use crate::render::model::RenderState;
use crate::selection::{SelectionMode, SelectionModel, TextDirection, TextUnit};

/// A wrapper for a buffer that provides access to its internal update_content method.
/// It's important this is only returned from `CoreEditorModel::update_content` method
/// so that we can ensure `on_buffer_version_updated` is always invoked on buffer content
/// changes synchronously.
///
/// This should NEVER be constructed manually outside of this crate.
pub struct BufferUpdateWrapper<'a> {
    buffer: &'a mut Buffer,
}

impl BufferUpdateWrapper<'_> {
    pub fn apply_edit(
        &mut self,
        action: BufferEditAction,
        origin: EditOrigin,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Buffer>,
    ) {
        self.buffer
            .update_content(action, origin, selection_model, ctx);
    }

    pub fn apply_edit_with_autoscroll(
        &mut self,
        action: BufferEditAction,
        origin: EditOrigin,
        should_autoscroll: ShouldAutoscroll,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Buffer>,
    ) {
        self.buffer.update_content_with_autoscroll(
            action,
            origin,
            should_autoscroll,
            selection_model,
            ctx,
        );
    }

    pub fn buffer(&mut self) -> &mut Buffer {
        self.buffer
    }
}

pub trait CoreEditorModel: Entity {
    type T: Entity;

    fn content(&self) -> &ModelHandle<Buffer>;
    fn buffer_selection_model(&self) -> &ModelHandle<BufferSelectionModel>;
    fn selection_model(&self) -> &ModelHandle<SelectionModel>;
    fn render_state(&self) -> &ModelHandle<RenderState>;
    fn validate(&self, ctx: &impl ModelAsRef);
    fn active_text_style(&self) -> TextStyles;

    fn buffer_version(&self, ctx: &AppContext) -> BufferVersion {
        self.content().as_ref(ctx).buffer_version()
    }

    /// This callback is always invoked _synchronously_ after a buffer update is applied.
    fn on_buffer_version_updated(
        &self,
        _buffer_version: BufferVersion,
        _ctx: &mut ModelContext<Self::T>,
    ) {
    }

    fn update_content(
        &self,
        action: impl FnOnce(BufferUpdateWrapper, &mut ModelContext<Buffer>),
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.content().update(ctx, |buffer, ctx| {
            action(BufferUpdateWrapper { buffer }, ctx)
        });
        self.on_buffer_version_updated(self.content().as_ref(ctx).buffer_version(), ctx);
    }

    /// Requests a complete rebuild of the layout state. This is expensive, but necessary when
    /// layout-affecting state like the font size changes.
    fn rebuild_layout(&self, ctx: &mut ModelContext<Self::T>) {
        log::debug!("Rebuilding layout state");
        let delta = self.content().as_ref(ctx).invalidate_layout();
        let buffer_version = self.content().as_ref(ctx).buffer_version();
        self.render_state().update(ctx, move |render_state, _ctx| {
            let scroll_position = render_state.snapshot_scroll_position();
            render_state.add_pending_edit(delta, buffer_version);
            render_state.scroll_to(scroll_position);
        });
    }

    /// Sets the document path for resolving relative paths (e.g., relative image paths in markdown).
    fn set_document_path(&self, path: Option<std::path::PathBuf>, ctx: &mut ModelContext<Self::T>) {
        self.render_state().update(ctx, |render_state, _| {
            render_state.set_document_path(path);
        });
    }

    fn user_insert(&mut self, text: &str, ctx: &mut ModelContext<Self::T>) {
        self.insert(text, EditOrigin::UserTyped, ctx);
    }

    fn insert(&mut self, text: &str, origin: EditOrigin, ctx: &mut ModelContext<Self::T>) {
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    BufferEditAction::Insert {
                        text,
                        style: self.active_text_style(),
                        override_text_style: None,
                    },
                    origin,
                    self.buffer_selection_model().clone(),
                    ctx,
                );
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn system_insert_autoscroll_vertical_only(
        &mut self,
        text: &str,
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.update_content(
            |mut content, ctx| {
                content.apply_edit_with_autoscroll(
                    BufferEditAction::Insert {
                        text,
                        style: self.active_text_style(),
                        override_text_style: None,
                    },
                    EditOrigin::SystemEdit,
                    ShouldAutoscroll::VerticalOnly,
                    self.buffer_selection_model().clone(),
                    ctx,
                );
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn replace_first_n_characters(
        &mut self,
        n: CharOffset,
        text: &str,
        ctx: &mut ModelContext<Self::T>,
    ) {
        let selection_model = self.buffer_selection_model().clone();
        self.update_content(
            |mut content, ctx| {
                let start = CharOffset::from(1);
                let end = std::cmp::min(start + n.as_usize(), content.buffer().max_charoffset());
                content.apply_edit(
                    BufferEditAction::InsertAtCharOffsetRanges {
                        edits: &vec1![(text.to_owned(), start..end)],
                    },
                    EditOrigin::UserInitiated,
                    selection_model,
                    ctx,
                );
            },
            ctx,
        );
        self.validate(ctx);
    }

    /// Truncate the buffer to the given length.
    fn truncate(&mut self, len: usize, ctx: &mut ModelContext<Self::T>) {
        self.update_content(
            |mut content, ctx| {
                let byte_offset: ByteOffset = (len + 1).into(); // TODO(CLD-558)
                let char_offset = byte_offset.to_buffer_char_offset(content.buffer());
                let max_offset = content.buffer().max_charoffset();
                if char_offset < max_offset {
                    // Use InsertAtCharOffsetRanges with an empty string to delete the range.
                    // This uses insert_on_selection: false, which clamps anchors instead of
                    // invalidating them. This is important for system edits like streaming
                    // updates where we don't want to invalidate selection anchors.
                    content.apply_edit(
                        BufferEditAction::InsertAtCharOffsetRanges {
                            edits: &vec1![(String::new(), char_offset..max_offset)],
                        },
                        EditOrigin::SystemEdit,
                        self.buffer_selection_model().clone(),
                        ctx,
                    );
                }
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn backspace(&mut self, ctx: &mut ModelContext<Self::T>) {
        // Edit the internal content model.
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    BufferEditAction::Backspace,
                    EditOrigin::UserInitiated,
                    self.buffer_selection_model().clone(),
                    ctx,
                )
            },
            ctx,
        );

        self.validate(ctx);
    }

    /// Compute the ranges that would be deleted for a given direction and unit.
    /// Returns None if all selections are not single cursors (i.e., there are ranges).
    fn replacement_range_for_deletion(
        &self,
        direction: TextDirection,
        unit: TextUnit,
        ctx: &mut ModelContext<Self::T>,
    ) -> Option<Vec1<Range<CharOffset>>> {
        // If any selection is a range, and not a cursor, return None
        if !self
            .buffer_selection_model()
            .as_ref(ctx)
            .all_single_cursors()
        {
            return None;
        }

        let selection = self.selection_model().as_ref(ctx);
        let selections = selection.cursors(ctx);
        let x_goals = selection
            .goal_xs
            .as_ref()
            .map(|goal| Either::Left(goal.into_iter().map(|x| Some(*x))))
            .unwrap_or_else(|| Either::Right(std::iter::repeat_n(None, selections.len())));

        let ranges = Vec1::try_from_vec(
            selections
                .into_iter()
                .zip(x_goals)
                .map(|(start, goal_x)| {
                    let end = selection
                        .navigate(start, direction, unit.clone(), 1, goal_x, ctx)
                        .offset;
                    start.min(end)..start.max(end)
                })
                .collect_vec(),
        )
        .expect("Vec1 should not be empty because input Vec1 cannot be empty.");

        Some(ranges)
    }

    // Internal implementation of the deletion logic. For different editor modes, they should implement their own
    // delete function as the detail of what content should be written to the clipboard might be slightly different.
    fn delete_internal<B>(
        &mut self,
        direction: TextDirection,
        unit: TextUnit,
        cut: bool,
        write_to_clipboard: B,
        ctx: &mut ModelContext<Self::T>,
    ) where
        B: FnOnce(
            &ModelHandle<Buffer>,
            &ModelHandle<BufferSelectionModel>,
            Option<Vec1<Range<CharOffset>>>,
            &mut ModelContext<Self::T>,
        ),
    {
        // If any selection is a range, and not a cursor, then treat this as a backspace.
        // Otherwise, navigate and delete from each cursor.
        if let Some(ranges) = self.replacement_range_for_deletion(direction, unit, ctx) {
            // We don't update the selection before deleting, which means we can't use
            // `read_selected_text_as_clipboard_content`. That's because undo/redo captures the
            // selection prior to editing, and we want to keep the user's original cursor.
            if cut {
                let content = self.content();
                write_to_clipboard(
                    content,
                    self.buffer_selection_model(),
                    Some(ranges.clone()), /* override range */
                    ctx,
                );
            }

            self.update_content(
                |mut content, ctx| {
                    content.apply_edit(
                        BufferEditAction::Delete(ranges),
                        EditOrigin::UserInitiated,
                        self.buffer_selection_model().clone(),
                        ctx,
                    );
                },
                ctx,
            )
        } else {
            // If there's already a selection range, treat this as backspace instead of applying
            // the specific delete action.
            if cut {
                let content = self.content();
                write_to_clipboard(content, self.buffer_selection_model(), None, ctx);
            }
            self.backspace(ctx);
        }

        self.validate(ctx);
    }

    fn indent(&mut self, shift: bool, ctx: &mut ModelContext<Self::T>) {
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    // By default apply only 1 unit for indentation.
                    BufferEditAction::Indent { num_unit: 1, shift },
                    EditOrigin::UserInitiated,
                    self.buffer_selection_model().clone(),
                    ctx,
                );
            },
            ctx,
        );
        self.validate(ctx)
    }

    fn clear_buffer(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_selection(BufferSelectAction::SelectAll, AutoScrollBehavior::None, ctx)
        });
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    BufferEditAction::Backspace,
                    EditOrigin::SystemEdit,
                    self.buffer_selection_model().clone(),
                    ctx,
                );
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn undo(&mut self, ctx: &mut ModelContext<Self::T>) {
        // Edit the internal content model.
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    BufferEditAction::Undo,
                    EditOrigin::UserInitiated,
                    self.buffer_selection_model().clone(),
                    ctx,
                )
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn redo(&mut self, ctx: &mut ModelContext<Self::T>) {
        // Edit the internal content model.
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    BufferEditAction::Redo,
                    EditOrigin::UserInitiated,
                    self.buffer_selection_model().clone(),
                    ctx,
                )
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn select_up(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.extend_selection(TextDirection::Backwards, TextUnit::Line, ctx);
        });
        self.validate(ctx);
    }

    fn select_down(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.extend_selection(TextDirection::Forwards, TextUnit::Line, ctx);
        });
        self.validate(ctx);
    }

    fn select_left(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_selection(
                BufferSelectAction::ExtendLeft,
                AutoScrollBehavior::Selection,
                ctx,
            )
        });
        self.validate(ctx);
    }

    fn select_right(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_selection(
                BufferSelectAction::ExtendRight,
                AutoScrollBehavior::Selection,
                ctx,
            )
        });
        self.validate(ctx);
    }

    /// Extends the selection to the start of the soft-wrapped line the cursor is on.
    fn select_to_line_start(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.extend_selection(TextDirection::Backwards, TextUnit::LineBoundary, ctx);
        });
        self.validate(ctx);
    }

    /// Extends the selection to the end of the soft-wrapped line the cursor is on.
    fn select_to_line_end(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.extend_selection(TextDirection::Forwards, TextUnit::LineBoundary, ctx);
        });
        self.validate(ctx);
    }

    /// Extends the selection to the start of the paragraph that the cursor is on.
    fn select_to_paragraph_start(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.extend_selection(TextDirection::Backwards, TextUnit::ParagraphBoundary, ctx)
        });
        self.validate(ctx);
    }

    /// Extends the selection to the end of the paragraph that the cursor is on.
    fn select_to_paragraph_end(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.extend_selection(TextDirection::Forwards, TextUnit::ParagraphBoundary, ctx)
        });
        self.validate(ctx);
    }

    fn move_up(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.move_selection(TextDirection::Backwards, TextUnit::Line, ctx);
        });
        self.validate(ctx);
    }

    fn move_down(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.move_selection(TextDirection::Forwards, TextUnit::Line, ctx);
        });
        self.validate(ctx);
    }

    fn move_left(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_selection(
                BufferSelectAction::MoveLeft,
                AutoScrollBehavior::Selection,
                ctx,
            )
        });
        self.validate(ctx);
    }

    fn move_right(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_selection(
                BufferSelectAction::MoveRight,
                AutoScrollBehavior::Selection,
                ctx,
            )
        });
        self.validate(ctx);
    }

    /// Moves the cursor to the start of the soft-wrapped line it's on.
    fn move_to_line_start(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.move_selection(TextDirection::Backwards, TextUnit::LineBoundary, ctx);
        });
        self.validate(ctx);
    }

    /// Moves the cursor to the end of the soft-wrapped line it's on.
    fn move_to_line_end(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.move_selection(TextDirection::Forwards, TextUnit::LineBoundary, ctx);
        });
        self.validate(ctx);
    }

    /// Moves the cursor to the start of the paragraph it's on.
    fn move_to_paragraph_start(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.move_selection(TextDirection::Backwards, TextUnit::ParagraphBoundary, ctx);
        });
        self.validate(ctx);
    }

    /// Moves the cursor to the end of the paragraph it's on.
    fn move_to_paragraph_end(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.move_selection(TextDirection::Forwards, TextUnit::ParagraphBoundary, ctx);
        });
        self.validate(ctx);
    }

    /// Moves to first nonwhitespace character, optionally keeping selection.
    fn vim_move_to_first_nonwhitespace(
        &mut self,
        keep_selection: bool,
        ctx: &mut ModelContext<Self::T>,
    ) {
        let current_selections = self.selections(ctx);
        let content = self.content().as_ref(ctx);

        let new_selections = current_selections.mapped(|selection_offset| {
            let cursor_offset = selection_offset.head;
            let first_nonwhitespace = content.containing_line_first_nonwhitespace(cursor_offset);

            SelectionOffsets {
                head: first_nonwhitespace,
                tail: if keep_selection {
                    selection_offset.tail
                } else {
                    first_nonwhitespace
                },
            }
        });

        self.update_selection(
            BufferSelectAction::SetSelectionOffsets {
                selections: new_selections,
            },
            AutoScrollBehavior::Selection,
            ctx,
        );
    }

    fn select_all(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.update_selection(BufferSelectAction::SelectAll, AutoScrollBehavior::None, ctx);
    }

    /// Moves the selection forward to the next word ending. If `select` is true, this extends the
    /// selection to that location. Otherwise, it moves the cursor.
    fn forward_word_with_unit(
        &mut self,
        select: bool,
        unit: TextUnit,
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.selection_model().update(ctx, |selection, ctx| {
            if select {
                selection.extend_selection(TextDirection::Forwards, unit, ctx);
            } else {
                selection.move_selection(TextDirection::Forwards, unit, ctx);
            }
        });
    }

    /// Moves the selection back to the previous word start. If `select` is true, this extends the
    /// selection to that location. Otherwise, it moves the cursor.
    fn backward_word_with_unit(
        &mut self,
        select: bool,
        unit: TextUnit,
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.selection_model().update(ctx, |selection, ctx| {
            if select {
                selection.extend_selection(TextDirection::Backwards, unit, ctx);
            } else {
                selection.move_selection(TextDirection::Backwards, unit, ctx);
            }
        });
    }

    fn selections(&self, ctx: &AppContext) -> Vec1<SelectionOffsets> {
        self.buffer_selection_model()
            .as_ref(ctx)
            .selection_offsets()
    }

    /// Return whether there is currently a single selection, either a single cursor or a single range.
    fn has_single_selection(&self, ctx: &AppContext) -> bool {
        self.buffer_selection_model().as_ref(ctx).selections().len() == 1
    }

    fn selection_is_single_cursor(&self, ctx: &AppContext) -> bool {
        let selection_model = self.buffer_selection_model().as_ref(ctx);
        selection_model.selections().len() == 1
            && selection_model.first_selection_is_single_cursor()
    }

    /// Return whether there is currently a single selection, and that selection is a single range, not
    /// just a cursor
    fn selection_is_single_range(&self, ctx: &AppContext) -> bool {
        let selection_model = self.buffer_selection_model().as_ref(ctx);
        selection_model.selections().len() == 1
            && !selection_model.first_selection_is_single_cursor()
    }

    fn selection_head(&self, ctx: &AppContext) -> CharOffset {
        // TODO(CLD-558): This matches how we shift the selection by 1.
        self.buffer_selection_model()
            .as_ref(ctx)
            .first_selection_head()
            - 1
    }

    fn logical_line_start(&self, offset: CharOffset, ctx: &AppContext) -> CharOffset {
        // TODO(CLD-558)
        self.content().as_ref(ctx).containing_line_start(offset) - 1
    }

    fn logical_line_end(&self, offset: CharOffset, ctx: &AppContext) -> CharOffset {
        // TODO(CLD-558)
        self.content().as_ref(ctx).containing_line_end(offset) - 1
    }

    /// Move the cursor to an exact content location.
    fn cursor_at(&self, offset: CharOffset, ctx: &mut ModelContext<Self::T>) {
        self.selection_model()
            .update(ctx, |selection, ctx| selection.set_cursor(offset, ctx));
        self.validate(ctx);
    }

    fn add_cursor_at(&self, offset: CharOffset, ctx: &mut ModelContext<Self::T>) {
        self.selection_model()
            .update(ctx, |selection, ctx| selection.add_cursor(offset, ctx));
        self.validate(ctx);
    }

    fn set_last_selection_head(&mut self, offset: CharOffset, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.set_last_head(offset, ctx);
        });
        self.validate(ctx);
    }

    /// Update the in-progress selection with a new cursor location (usually the result of dragging).
    fn update_pending_selection(&mut self, offset: CharOffset, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_pending_selection(offset, ctx);
        });
        self.validate(ctx);
    }

    /// Begin selecting at `offset`. The initial selection depends on the given mode.
    fn begin_selection(
        &mut self,
        offset: CharOffset,
        mode: SelectionMode,
        clear_selections: bool,
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.begin_selection(offset, mode, clear_selections, ctx)
        });

        self.validate(ctx);
    }

    /// End any ongoing selection actions. When dragging to select, this should be called when the
    /// mouse is released.
    fn end_selection(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.end_selection(ctx);
        });
        self.validate(ctx);
    }

    /// This is a helper for semantic-selection actions.
    fn update_selection(
        &mut self,
        action: BufferSelectAction,
        autoscroll: AutoScrollBehavior,
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.selection_model().update(ctx, |selection, ctx| {
            selection.update_selection(action, autoscroll, ctx);
        });
        self.validate(ctx);
    }
}

pub trait PlainTextEditorModel: CoreEditorModel {
    fn enter(&mut self, ctx: &mut ModelContext<Self::T>) {
        self.update_content(
            |mut content, ctx| {
                content.apply_edit(
                    BufferEditAction::Enter {
                        style: self.active_text_style(),
                        force_newline: false,
                    },
                    EditOrigin::UserTyped,
                    self.buffer_selection_model().clone(),
                    ctx,
                );
            },
            ctx,
        );
        self.validate(ctx);
    }

    fn read_text_as_clipboard_content(
        &self,
        range: Range<CharOffset>,
        ctx: &AppContext,
    ) -> ClipboardContent {
        let buffer = self.content().as_ref(ctx);
        ClipboardContent::plain_text(buffer.text_in_range(range).into_string())
    }

    fn read_selected_text_as_clipboard_content(&self, ctx: &AppContext) -> ClipboardContent {
        let buffer = self.content().as_ref(ctx);
        ClipboardContent::plain_text(
            buffer
                .selected_text_as_plain_text(self.buffer_selection_model().clone(), ctx)
                .into_string(),
        )
    }

    fn copy_all(&self, ctx: &mut ModelContext<Self::T>) {
        let end = self.content().as_ref(ctx).max_charoffset();
        let clipboard = self.read_text_as_clipboard_content(CharOffset::from(1)..end, ctx);
        ctx.clipboard().write(clipboard);
    }

    fn delete(
        &mut self,
        direction: TextDirection,
        unit: TextUnit,
        cut: bool,
        ctx: &mut ModelContext<Self::T>,
    ) {
        self.delete_internal(
            direction,
            unit,
            cut,
            |buffer, selection_model, override_range, ctx| {
                let buffer = buffer.as_ref(ctx);
                let ranges = match override_range {
                    Some(ranges) => ranges,
                    None => selection_model.as_ref(ctx).selections_to_offset_ranges(),
                };

                // TODO: this should allow passing in the line ending mode.
                let content = ClipboardContent::plain_text(
                    buffer.text_in_ranges(ranges, LineEnding::LF).into_string(),
                );
                ctx.clipboard().write(content);
            },
            ctx,
        );
    }
}
