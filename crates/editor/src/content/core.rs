use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;

use enum_iterator::all;
use markdown_parser::FormattedTextFragment;
use string_offset::CharOffset;
use sum_tree::SumTree;

use super::buffer::{Buffer, EditOrigin, EditResult};
use super::cursor::BufferSumTree;
use super::edit::EditDelta;
use super::text::{
    BufferTextStyle, LinkCount, LinkMarker, MarkerDir, TextStyles, TextStylesWithMetadata,
};
use super::undo::{ReversibleEditorAction, UndoArg};
use crate::content::anchor::{Anchor, AnchorSide, AnchorUpdate};
use crate::content::buffer::{StyledBlockBoundaryBehavior, ToBufferByteOffset, ToBufferPoint};
use crate::content::cursor::BufferCursor;
use crate::content::edit::PreciseDelta;
use crate::content::text::{BufferBlockStyle, BufferText, StyleSummary};

/// Line states accepted by the editor, including inline formatting used by undo.
#[derive(Debug, Clone)]
pub enum EditorLine {
    Line(Vec<FormattedTextFragment>),
    LineBreak,
}

#[derive(Debug, Clone)]
pub struct CoreEditorAction {
    pub range: Range<CharOffset>,
    pub action: CoreEditorActionType,
    /// The anchor side for the start of the edit range. Setting it to AnchorSide::Left is useful
    /// when multiple actions need to be applied on the same range and this action should keep its
    /// start position.
    pub start_anchor_bias: AnchorSide,
    /// The anchor side for the end of the edit range. Setting it to AnchorSide::Right is
    /// useful when multiple actions need to be applied on the same range and one of the action
    /// might modify the size of the range.
    pub end_anchor_bias: AnchorSide,
}

impl CoreEditorAction {
    pub fn new(range: Range<CharOffset>, action: CoreEditorActionType) -> Self {
        Self {
            range,
            action,
            start_anchor_bias: AnchorSide::Right,
            end_anchor_bias: AnchorSide::Left,
        }
    }

    pub fn with_end_anchor_bias(mut self, anchor_side: AnchorSide) -> Self {
        self.end_anchor_bias = anchor_side;
        self
    }
}

#[derive(Debug, Clone)]
pub enum CoreEditorActionType {
    Insert {
        text: VecDeque<EditorLine>,
        // Source of the edit. This impacts the formatting behavior of the insertion.
        // For EditOrigin::UserTyped -- The source is a typed keystroke. Inherit styling for the first line if the edit is inline.
        // For EditOrigin::UserInitiated -- The source is a user initiated insertion (e.g. undo). Inherit styling for the first line if the edit is inline.
        // For EditOrigin::SystemEdit -- The source is a system inserted change (e.g. replacing editor content). Keep the full inserted styling.
        //
        // Note EditOrigin parameter here has a slightly different meaning from those in the high-level edit action. SystemEdit in the high-level edit action represents
        // actions that are completely initiated by the application and not the user (e.g. replacing the entire editor content by some pre-set message).
        source: EditOrigin,
        // Whether the insertion is applied on a selection offset set. This changes the behavior on how we are updating anchor states since if the selection is NOT applied
        // on an active selection, we want to clamp instead of invalidating anchors.
        insert_on_selection: bool,
    },
    StyleText(BufferTextStyle),
    UnstyleText(BufferTextStyle),
    /// Restores the initial plain text marker when an edit leaves the buffer empty.
    EnsurePlainTextMarker,
}

// Helper struct to keep track of a range's start
// and end anchor.
pub(super) struct RangeAnchors {
    pub(super) start: Anchor,
    pub(super) end: Anchor,
}

#[derive(Debug, Clone)]
pub struct ReplacementRange {
    pub new_range: Range<CharOffset>,
    pub old_range: Range<CharOffset>,
}

impl ReplacementRange {
    fn reversed(&self) -> Self {
        Self {
            new_range: self.old_range.clone(),
            old_range: self.new_range.clone(),
        }
    }
}

pub struct CoreEditorActionResult {
    /// The range of inserted characters after the core editor action.
    pub updated_range: Range<CharOffset>,
    /// The corresponding anchor update applied with the action.
    pub anchor_update: Option<AnchorUpdate>,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum CursorType {
    // Cursor is at the start of the buffer. We need to add a new starting
    // block marker.
    BufferStart,
    // Cursor is at the start of a newline.
    NewLineStart,
    // Cursor is inline.
    Inline,
}

impl Buffer {
    pub(super) fn apply_core_edit_actions(
        &mut self,
        actions: impl IntoIterator<Item = CoreEditorAction>,
    ) -> EditResult {
        // If there are no actions, return early.
        let mut actions_iter = actions.into_iter().peekable();
        if actions_iter.peek().is_none() {
            return EditResult::default();
        }

        log::trace!("Applying core editor actions");
        let mut action_and_anchors = Vec::new();
        let mut replaced_ranges: Option<Range<CharOffset>> = None;

        for action in actions_iter {
            let range = action.range.clone();

            let start_side = action.start_anchor_bias;

            // If the start of the range is the same as the end of the range, make sure their side is
            // consistent to avoid the case where start is after end offset after updates.
            // If the start has left bias, then the end will be >= the start, regardless of its
            // bias.
            let end_side = if range.start == range.end && start_side == AnchorSide::Right {
                AnchorSide::Right
            } else {
                action.end_anchor_bias
            };

            let anchors = RangeAnchors {
                start: self.internal_anchors.create_anchor(range.start, start_side),
                end: self.internal_anchors.create_anchor(range.end, end_side),
            };

            action_and_anchors.push((action, anchors));

            // For old replaced ranges we don't need to use anchors as they shouldn't be shifted by
            // the updates we apply.
            let replace_start = self.block_or_line_start(range.start);
            // block_or_line_end, containing_line_end, and containing_block_end all assume that
            // blocks end in a newline, block marker, or block item, so they unconditionally add 1
            // to get the exclusive end of a block (the first character after it).
            //
            // This assumption is true of all blocks except a plain-text block at the end of the
            // buffer, which generally will not end in a newline.
            //
            // We _could_ account for that case in those functions, by checking what the text at
            // the expected end offset is. However, various editing operations also rely on this
            // behavior (e.g. by subtracting 1 to get the offset of the ending marker/item), so
            // those functions need to consistently add 1.
            // Instead, handle it here by checking max_charoffset, so that the ranges for
            // re-rendering are as expected.
            let replace_end = self.block_or_line_end(range.end).min(self.max_charoffset());

            if let Some(replaced_ranges) = &mut replaced_ranges {
                replaced_ranges.start = replaced_ranges.start.min(replace_start);
                replaced_ranges.end = replaced_ranges.end.max(replace_end);
            } else {
                replaced_ranges = Some(replace_start..replace_end);
            }
        }

        let mut reverse_actions = Vec::new();
        let mut new_range_anchors = Vec::new();
        let mut precise_deltas = Vec::new();
        let mut anchor_updates = Vec::new();
        // Anchors tracking each delta's new content range, resolved after all edits
        // to get correct final-buffer coordinates.
        let mut new_content_range_anchors: Vec<RangeAnchors> = Vec::new();

        for (action, anchors) in action_and_anchors {
            // When applying the action, we need to read out its updated offset range from its anchors
            // since a previous action could shift the offset.
            let edit_start = self
                .internal_anchors
                .resolve(&anchors.start)
                .expect("Anchor should exist");
            let edit_end = self
                .internal_anchors
                .resolve(&anchors.end)
                .expect("Anchor should exist");
            log::trace!("Start anchor => {edit_start}, end anchor => {edit_end}");

            // Safety: if a previous edit in this batch caused the anchors for this
            // action to cross (start > end), skip the action rather than panicking.
            // This can happen when overlapping DiffDeltas slip through the diff
            // matching layer (see WARP-CLIENT-DEV-NYY).
            if edit_start > edit_end {
                log::warn!(
                    "Skipping edit action with inverted range {edit_start}..{edit_end} \
                     (anchors crossed after a prior edit in the same batch)"
                );
                continue;
            }

            let edit_range = edit_start..edit_end;
            let replaced_points = self.offset_range_to_point_range(edit_range.clone());

            // Compute pre-edit byte range from the correct intermediate buffer state.
            let old_byte_start = edit_range.start.to_buffer_byte_offset(self);
            let old_byte_end = edit_range.end.to_buffer_byte_offset(self);

            let reverse_action_type =
                self.reverse_core_edit_action(action.clone(), edit_range.clone());

            let result = self.apply_core_edit_action(CoreEditorAction::new(
                edit_range.clone(),
                action.action.clone(),
            ));

            if let Some(anchor_update) = result.anchor_update {
                anchor_updates.push(anchor_update);
            }
            let update_range = result.updated_range;
            log::trace!("=> Reverse action: {reverse_action_type:?}");

            // Compute post-edit byte length and end point from the correct intermediate
            // buffer state (after this edit, before subsequent edits).
            let new_byte_length = update_range
                .end
                .to_buffer_byte_offset(self)
                .as_usize()
                .saturating_sub(old_byte_start.as_usize());
            let new_end_point = update_range.end.to_buffer_point(self);

            // We could skip pushing no-op edits to precise delta.
            if !edit_range.is_empty() || !update_range.is_empty() {
                // Anchor the new content range so it can be resolved against the final
                // buffer state after all edits are applied.
                new_content_range_anchors.push(RangeAnchors {
                    start: self
                        .internal_anchors
                        .create_anchor(update_range.start, AnchorSide::Right),
                    end: self
                        .internal_anchors
                        .create_anchor(update_range.end, AnchorSide::Left),
                });
                precise_deltas.push(PreciseDelta {
                    replaced_range: edit_range,
                    replaced_points,
                    // Placeholder — resolved below after all edits are applied.
                    resolved_range: update_range.clone(),
                    replaced_byte_range: old_byte_start..old_byte_end,
                    new_byte_length,
                    new_end_point,
                });
            }

            let reverse_action = CoreEditorAction::new(update_range.clone(), reverse_action_type);
            reverse_actions.push(ReversibleEditorAction {
                next: reverse_action,
                reverse: action,
            });

            let new_range = self.block_or_line_start(update_range.start)
                ..self
                    .block_or_line_end(update_range.end)
                    // See the comment on replace_end above for why the max_offset check is needed.
                    .min(self.max_charoffset());
            log::trace!("=> New block range: {}..{}", new_range.start, new_range.end);

            let new_anchors = RangeAnchors {
                start: self
                    .internal_anchors
                    .create_anchor(new_range.start, AnchorSide::Right),
                end: self
                    .internal_anchors
                    .create_anchor(new_range.end, AnchorSide::Left),
            };

            new_range_anchors.push(new_anchors);
        }

        // If every action in the batch was skipped (e.g. all had inverted ranges),
        // there is nothing to commit — return early.
        if new_range_anchors.is_empty() {
            return EditResult::default();
        }

        // Resolve each delta's new content range anchors against the final buffer state.
        // If a later action in the batch deletes the content an earlier action inserted,
        // the earlier action's anchors will have been invalidated — drop those deltas.
        let precise_deltas: Vec<PreciseDelta> = precise_deltas
            .into_iter()
            .zip(new_content_range_anchors)
            .filter_map(|(mut delta, anchors)| {
                match (
                    self.internal_anchors.resolve(&anchors.start),
                    self.internal_anchors.resolve(&anchors.end),
                ) {
                    (Some(start), Some(end)) => {
                        delta.resolved_range = start..end;
                        Some(delta)
                    }
                    _ => None,
                }
            })
            .collect();

        let old_range = replaced_ranges.expect("Should have one range");
        reverse_actions.reverse();
        let replacement_range = ReplacementRange {
            old_range,
            new_range: self.anchors_to_range(new_range_anchors),
        };
        let undo_arg = UndoArg {
            actions: reverse_actions,
            replacement_range: replacement_range.reversed(),
        };

        log::debug!(
            "=> Overall previous range: {:?}",
            replacement_range.old_range
        );
        log::debug!("=> Overall new range: {:?}", replacement_range.new_range);

        let new_lines = Arc::new(self.styled_blocks_in_range(
            replacement_range.new_range,
            StyledBlockBoundaryBehavior::Exclusive,
        ));

        EditResult {
            undo_item: Some(undo_arg),
            delta: Some(EditDelta {
                precise_deltas,
                old_offset: replacement_range.old_range,
                new_lines,
            }),
            anchor_updates,
        }
    }

    /// If the active block style is plain text, return line start. Else, return block start.
    pub fn block_or_line_start(&self, offset: CharOffset) -> CharOffset {
        if offset == CharOffset::zero() {
            offset
        } else {
            self.containing_line_start(offset)
        }
    }

    /// If the active block style is plain text, return line end. Else, return block end.
    pub fn block_or_line_end(&self, offset: CharOffset) -> CharOffset {
        if offset == CharOffset::zero() {
            1.into()
        } else {
            self.containing_line_end(offset)
        }
    }

    // Find the minimal range of character offset that covers the list of range
    // represented by anchors.
    fn anchors_to_range(&self, anchors: Vec<RangeAnchors>) -> Range<CharOffset> {
        let range_start = anchors
            .iter()
            .filter_map(|range| self.internal_anchors.resolve(&range.start))
            .min();
        let range_end = anchors
            .iter()
            .filter_map(|range| self.internal_anchors.resolve(&range.end))
            .max();

        range_start.expect("Range start should be non-empty")
            ..range_end.expect("Range end should be non-empty")
    }

    pub(super) fn apply_core_edit_action(
        &mut self,
        action: CoreEditorAction,
    ) -> CoreEditorActionResult {
        log::trace!("Applying {:?}", action.action);
        log::trace!("=> Edit range: {:?}", action.range);
        log::trace!("=> Initial buffer: [{}]", self.debug());
        let result = match action.action {
            CoreEditorActionType::Insert {
                text,
                source,
                insert_on_selection: clamp_anchor,
            } => self.edit(action.range, text, clamp_anchor, source),
            CoreEditorActionType::StyleText(style) => self.style_text(action.range, style),
            CoreEditorActionType::UnstyleText(style) => self.unstyle_text(action.range, style),
            CoreEditorActionType::EnsurePlainTextMarker => self.ensure_plain_text(action.range),
        };
        log::trace!("=> Updated buffer: [{}]", self.debug());
        log::trace!("=> Update range: {:?}", result.updated_range);
        result
    }

    pub fn reverse_core_edit_action(
        &self,
        action: CoreEditorAction,
        range: Range<CharOffset>,
    ) -> CoreEditorActionType {
        match action.action {
            CoreEditorActionType::UnstyleText(style) => CoreEditorActionType::StyleText(style),
            CoreEditorActionType::StyleText(style) => CoreEditorActionType::UnstyleText(style),
            CoreEditorActionType::Insert { .. } => {
                let original_text =
                    self.range_to_formatted_text(range, StyledBlockBoundaryBehavior::Inclusive);
                CoreEditorActionType::Insert {
                    text: original_text,
                    source: EditOrigin::UserInitiated,
                    insert_on_selection: true,
                }
            }
            // EnsurePlainTextMarker captures whether or not a marker was added with its updated
            // range, so the reverse is to delete whatever was in that range.
            CoreEditorActionType::EnsurePlainTextMarker => CoreEditorActionType::Insert {
                text: VecDeque::new(),
                source: EditOrigin::SystemEdit,
                insert_on_selection: true,
            },
        }
    }

    fn edit(
        &mut self,
        range: Range<CharOffset>,
        text: VecDeque<EditorLine>,
        insert_on_selection: bool,
        source: EditOrigin,
    ) -> CoreEditorActionResult {
        debug_assert!(
            range.start <= range.end,
            "Invalid edit range {}..{}",
            range.start,
            range.end
        );
        debug_assert!(
            range.start <= self.max_charoffset(),
            "Edit starts at {}, but max char offset is {}",
            range.start,
            self.max_charoffset()
        );

        // Determine the edit range on the old content, before we modify it.
        let old_content = self.content.clone();
        let cursor = old_content.cursor::<CharOffset, StyleSummary>();
        let mut buffer_cursor = BufferCursor::new(cursor);
        let mut new_content = SumTree::new();

        new_content.push_tree(buffer_cursor.slice_to_offset_before_markers(range.start));
        let character_count_at_range_start = new_content.extent::<CharOffset>();

        let is_edit_inline = range.start != self.containing_line_start(range.start)
            || range.end != (self.containing_line_end(range.end) - 1);
        let mut edit_cursor = if range.start == CharOffset::zero() {
            CursorType::BufferStart
        } else if is_edit_inline {
            CursorType::Inline
        } else {
            CursorType::NewLineStart
        };

        let mut inherit_styling = source.from_user();
        for line in text {
            match line {
                EditorLine::LineBreak => {
                    end_all_active_text_styles(&mut new_content);
                    if edit_cursor == CursorType::BufferStart {
                        new_content.push(BufferText::BlockMarker {
                            marker_type: BufferBlockStyle::PlainText,
                        });
                    } else {
                        new_content.push(BufferText::Newline);
                    }
                }
                EditorLine::Line(line) => {
                    if inherit_styling && edit_cursor != CursorType::BufferStart {
                        update_content_with_text_fragments(&mut new_content, line);
                    } else {
                        end_all_active_text_styles(&mut new_content);
                        if edit_cursor == CursorType::BufferStart {
                            new_content.push(BufferText::BlockMarker {
                                marker_type: BufferBlockStyle::PlainText,
                            });
                        } else if edit_cursor == CursorType::NewLineStart {
                            new_content.push(BufferText::Newline);
                        }
                        update_content_with_text_fragments(&mut new_content, line);
                    }
                }
            }
            edit_cursor = CursorType::NewLineStart;
            inherit_styling = false;
        }

        let total_length = new_content.extent::<CharOffset>() - character_count_at_range_start;

        let active_style_summary = new_content.extent::<StyleSummary>();
        let active_text_style =
            active_text_styles_with_metadata_from_subtree(&new_content, active_style_summary);
        buffer_cursor.slice_to_offset_after_markers(range.end);

        // Make sure all style start/end markers are still matched after the insertion.
        let suffix_styling = *buffer_cursor.start();
        let suffix_text_styling =
            active_text_styles_with_metadata_from_subtree(&old_content, suffix_styling);
        let mut handled_weight = false;
        for style in all::<BufferTextStyle>() {
            let is_weight = style.has_custom_weight();
            if is_weight && handled_weight {
                continue;
            }
            if suffix_text_styling.exact_match_style(&style)
                && !active_text_style.exact_match_style(&style)
            {
                handled_weight |= is_weight;
                new_content.push(BufferText::Marker {
                    marker_type: style,
                    dir: MarkerDir::Start,
                });
            } else if !suffix_text_styling.exact_match_style(&style)
                && active_text_style.exact_match_style(&style)
            {
                handled_weight |= is_weight;
                new_content.push(BufferText::Marker {
                    marker_type: style,
                    dir: MarkerDir::End,
                });
            }
        }

        transition_styles_with_metadata(&active_text_style, &suffix_text_styling, &mut new_content);

        let end_of_line = self.containing_line_end(range.end);
        if end_of_line - 1 > range.end {
            new_content.push_tree(buffer_cursor.slice_to_offset_after_markers(end_of_line - 1));
        }
        let new_content_empty = new_content.last().is_none();
        match buffer_cursor.item() {
            Some(BufferText::BlockMarker { .. }) if !new_content_empty => {
                buffer_cursor.next();
                new_content.push(BufferText::Newline);
            }
            Some(BufferText::Newline) if new_content_empty => {
                buffer_cursor.next();
                new_content.push(BufferText::BlockMarker {
                    marker_type: BufferBlockStyle::PlainText,
                });
            }
            _ => (),
        }

        new_content.push_tree(buffer_cursor.suffix());
        drop(buffer_cursor);
        self.content = new_content;

        // If insertion is not on selection, we want to clamp instead of invalidating the anchors.
        let anchor_udpate = AnchorUpdate {
            start: range.start,
            old_character_count: (range.end - range.start).as_usize(),
            new_character_count: total_length.as_usize(),
            clamp: !insert_on_selection,
        };

        self.internal_anchors.update(anchor_udpate);

        CoreEditorActionResult {
            updated_range: range.start..range.start + total_length,
            anchor_update: Some(anchor_udpate),
        }
    }

    fn ensure_plain_text(&mut self, range: Range<CharOffset>) -> CoreEditorActionResult {
        let buffer_end = self.max_charoffset();
        let updated_range = if buffer_end == CharOffset::zero() {
            log::trace!("Inserting <text> marker at end of buffer");
            self.content.push(BufferText::BlockMarker {
                marker_type: BufferBlockStyle::PlainText,
            });

            range.end..range.end + 1
        } else {
            range.end..range.end
        };

        CoreEditorActionResult {
            updated_range,
            anchor_update: None,
        }
    }

    fn style_text(
        &mut self,
        range: Range<CharOffset>,
        text_style: BufferTextStyle,
    ) -> CoreEditorActionResult {
        let old_content = self.content.clone();
        let cursor = old_content.cursor::<CharOffset, StyleSummary>();
        let mut buffer_cursor = BufferCursor::new(cursor);
        let mut new_content = SumTree::new();

        new_content.push_tree(buffer_cursor.slice_to_offset_before_markers(range.start));

        // In the style markers right before range start, if there is no end marker, we will need to push a new start marker.
        // If there is an end marker, that means the range will be styled by simply removing that end marker. No need
        // to push a new start marker.
        let markers_before_range_start = buffer_cursor.slice_to_offset_after_markers(range.start);
        let mut found_ending_marker = false;

        let mut marker_cursor = markers_before_range_start.cursor::<(), ()>();
        marker_cursor.descend_to_first_item(&markers_before_range_start, |_| true);
        for item in marker_cursor {
            match item {
                BufferText::Marker {
                    marker_type,
                    dir: MarkerDir::End,
                } if *marker_type == text_style => {
                    found_ending_marker = true;
                }
                _ => new_content.push(item.clone()),
            }
        }

        if !found_ending_marker {
            new_content.push(BufferText::Marker {
                marker_type: text_style,
                dir: MarkerDir::Start,
            });
        }

        new_content.push_tree(buffer_cursor.slice_to_offset_before_markers(range.end));

        // In the style markers right before range end (exclusive), if there is no start marker, we will need to push a new end marker.
        // If there is an start marker, that means the range after was already styled. No need to push a new end marker.
        let markers_after_range_end = buffer_cursor.slice_to_offset_after_markers(range.end);
        let mut found_starting_marker = false;

        let mut marker_cursor = markers_after_range_end.cursor::<(), ()>();
        marker_cursor.descend_to_first_item(&markers_after_range_end, |_| true);
        for item in marker_cursor {
            match item {
                BufferText::Marker {
                    marker_type,
                    dir: MarkerDir::Start,
                } if *marker_type == text_style => {
                    found_starting_marker = true;
                }
                _ => new_content.push(item.clone()),
            }
        }

        if !found_starting_marker {
            new_content.push(BufferText::Marker {
                marker_type: text_style,
                dir: MarkerDir::End,
            });
        }

        new_content.push_tree(buffer_cursor.suffix());
        drop(buffer_cursor);
        self.content = new_content;

        CoreEditorActionResult {
            updated_range: range,
            anchor_update: None,
        }
    }

    fn unstyle_text(
        &mut self,
        range: Range<CharOffset>,
        text_style: BufferTextStyle,
    ) -> CoreEditorActionResult {
        let old_content = self.content.clone();
        let cursor = old_content.cursor::<CharOffset, StyleSummary>();
        let mut buffer_cursor = BufferCursor::new(cursor);
        let mut new_content = SumTree::new();

        new_content.push_tree(buffer_cursor.slice_to_offset_before_markers(range.start));

        // In the style markers right before range start, if there is no start marker, we will need to push a new end marker.
        // If there is a start marker, we just need to remove that start marker.
        let markers_before_range_start = buffer_cursor.slice_to_offset_after_markers(range.start);
        let mut found_starting_marker = false;

        let mut marker_cursor = markers_before_range_start.cursor::<(), ()>();
        marker_cursor.descend_to_first_item(&markers_before_range_start, |_| true);
        for item in marker_cursor {
            match item {
                BufferText::Marker {
                    marker_type,
                    dir: MarkerDir::Start,
                } if *marker_type == text_style => {
                    found_starting_marker = true;
                }
                _ => new_content.push(item.clone()),
            }
        }

        if !found_starting_marker {
            new_content.push(BufferText::Marker {
                marker_type: text_style,
                dir: MarkerDir::End,
            });
        }

        new_content.push_tree(buffer_cursor.slice_to_offset_before_markers(range.end));

        // In the style markers right before range end (exclusive), if there is no end marker, we will need to push a new start marker.
        // If there is a end marker, we just need to remove that end marker.
        let markers_after_range_end = buffer_cursor.slice_to_offset_after_markers(range.end);
        let mut found_ending_marker = false;

        let mut marker_cursor = markers_after_range_end.cursor::<(), ()>();
        marker_cursor.descend_to_first_item(&markers_after_range_end, |_| true);
        for item in marker_cursor {
            match item {
                BufferText::Marker {
                    marker_type,
                    dir: MarkerDir::End,
                } if *marker_type == text_style => {
                    found_ending_marker = true;
                }
                _ => new_content.push(item.clone()),
            }
        }

        if !found_ending_marker {
            new_content.push(BufferText::Marker {
                marker_type: text_style,
                dir: MarkerDir::Start,
            });
        }

        new_content.push_tree(buffer_cursor.suffix());
        drop(buffer_cursor);
        self.content = new_content;

        CoreEditorActionResult {
            updated_range: range,
            anchor_update: None,
        }
    }
}

/// Returns the active text style with metadata at the given subtree and a set StyleSummary.
fn active_text_styles_with_metadata_from_subtree(
    content_tree: &SumTree<BufferText>,
    style: StyleSummary,
) -> TextStylesWithMetadata {
    let text_style: TextStyles = style.into();

    TextStylesWithMetadata::from_text_styles(
        text_style,
        if text_style.is_link() {
            content_tree.url_at_link_count(&LinkCount(style.total_link_counter() as usize))
        } else {
            None
        },
        None,
    )
}

fn end_all_active_text_styles(new_content: &mut SumTree<BufferText>) {
    let active_style_summary = new_content.extent::<StyleSummary>();
    let active_text_style =
        active_text_styles_with_metadata_from_subtree(new_content, active_style_summary);
    let mut handled_weight = false;
    for style in all::<BufferTextStyle>() {
        let is_weight = style.has_custom_weight();
        if is_weight && handled_weight {
            continue;
        }
        if active_text_style.exact_match_style(&style) {
            handled_weight |= is_weight;
            new_content.push(BufferText::Marker {
                marker_type: style,
                dir: MarkerDir::End,
            });
        }
    }
    if active_text_style.link_content().is_some() {
        new_content.push(BufferText::Link(LinkMarker::End));
    }
}

fn transition_styles_with_metadata(
    prev_text_styles: &TextStylesWithMetadata,
    next_text_styles: &TextStylesWithMetadata,
    content_tree: &mut SumTree<BufferText>,
) {
    match (
        prev_text_styles.link_content(),
        next_text_styles.link_content(),
    ) {
        (Some(active_url), Some(new_url)) if active_url == new_url => (),
        (Some(_), Some(new_url)) => {
            content_tree.push(BufferText::Link(LinkMarker::End));
            content_tree.push(BufferText::Link(LinkMarker::Start(new_url)))
        }
        (Some(_), None) => content_tree.push(BufferText::Link(LinkMarker::End)),
        (None, Some(new_url)) => content_tree.push(BufferText::Link(LinkMarker::Start(new_url))),
        (None, None) => (),
    };
}

/// Updates the content subtree with formatted text fragments. The resulting content tree should be formatted with the
/// valid inline styles.
fn update_content_with_text_fragments(
    content_tree: &mut SumTree<BufferText>,
    fragments: Vec<FormattedTextFragment>,
) {
    for fragment in fragments {
        let style = content_tree.extent::<StyleSummary>();
        let active_text_style = active_text_styles_with_metadata_from_subtree(content_tree, style);

        let content = fragment.text;
        let text_style: TextStylesWithMetadata = fragment.styles.into();

        let mut handled_weight = false;
        for style in all::<BufferTextStyle>() {
            let is_weight = style.has_custom_weight();
            if is_weight && handled_weight {
                continue;
            }
            if active_text_style.exact_match_style(&style) && !text_style.exact_match_style(&style)
            {
                handled_weight |= is_weight;
                content_tree.push(BufferText::Marker {
                    marker_type: style,
                    dir: MarkerDir::End,
                });
            } else if !active_text_style.exact_match_style(&style)
                && text_style.exact_match_style(&style)
            {
                handled_weight |= is_weight;
                content_tree.push(BufferText::Marker {
                    marker_type: style,
                    dir: MarkerDir::Start,
                });
            }
        }

        transition_styles_with_metadata(&active_text_style, &text_style, content_tree);
        content_tree.append_str(&content);
    }
}
