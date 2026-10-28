use std::collections::VecDeque;
use std::ops::Range;

use markdown_parser::{
    CodeBlockText, FormattedIndentTextInline, FormattedTaskList, FormattedText,
    FormattedTextFragment, FormattedTextHeader, FormattedTextLine,
    OrderedFormattedIndentTextInline,
};
use string_offset::CharOffset;
use warpui_core::elements::ListIndentLevel;
use warpui_core::{AppContext, ModelContext, ModelHandle};

use super::buffer::{
    ActionWithSelectionDelta, Buffer, EditOrigin, EditResult, StyledBufferBlock,
    StyledBufferBlocks, StyledBufferRun,
};
use super::core::{CoreEditorAction, CoreEditorActionType};
use super::text::{BlockHeaderSize, BlockType, BufferBlockStyle};
use crate::content::anchor::AnchorSide;
use crate::content::selection_model::BufferSelectionModel;

/// A Markdown format to serialize a [`Buffer`] into.
#[derive(Clone, Copy)]
pub enum MarkdownStyle<'a> {
    /// The internal Markdown format used in Warp Drive. References are normalized, so the Markdown
    /// only refers to other objects by their IDs, with no other data.
    Internal,
    /// A Markdown format suitable for external use. If an [`AppContext`] is set, it may be used to
    /// enrich the exported Markdown (e.g. by expanding out embedded objects).
    Export {
        app_context: Option<&'a AppContext>,
        should_not_escape_markdown_punctuation: bool,
    },
}

/// Convert a slice of buffer model in StyledBufferBlocks into FormattedText.
pub struct BufferToFormattedText<'a> {
    blocks: StyledBufferBlocks<'a>,
}

impl<'a> BufferToFormattedText<'a> {
    pub fn new(blocks: StyledBufferBlocks<'a>) -> Self {
        Self { blocks }
    }

    pub fn to_formatted_text(self) -> FormattedText {
        let mut lines = VecDeque::new();
        let mut trailing_new_line = false;
        for item in self.blocks {
            match item {
                StyledBufferBlock::Item(item) => {
                    trailing_new_line = false;
                    lines.push_back(item.to_formatted_text_line());
                }
                StyledBufferBlock::Text(text_block) => {
                    lines.push_back(match text_block.style.clone() {
                        BufferBlockStyle::Header { header_size } => {
                            trailing_new_line = false;
                            FormattedTextLine::Heading(FormattedTextHeader {
                                heading_size: header_size.into(),
                                text: text_block
                                    .block
                                    .into_iter()
                                    .map(|run| run.to_formatted_text_fragment())
                                    .collect(),
                            })
                        }
                        BufferBlockStyle::TaskList {
                            indent_level,
                            complete,
                        } => {
                            trailing_new_line = false;
                            FormattedTextLine::TaskList(FormattedTaskList {
                                complete,
                                indent_level: indent_level.as_usize(),
                                text: text_block
                                    .block
                                    .into_iter()
                                    .map(|run| run.to_formatted_text_fragment())
                                    .collect(),
                            })
                        }
                        BufferBlockStyle::UnorderedList { indent_level } => {
                            trailing_new_line = false;
                            FormattedTextLine::UnorderedList(FormattedIndentTextInline {
                                indent_level: indent_level.as_usize(),
                                text: text_block
                                    .block
                                    .into_iter()
                                    .map(|run| run.to_formatted_text_fragment())
                                    .collect(),
                            })
                        }
                        BufferBlockStyle::OrderedList {
                            indent_level,
                            number,
                        } => {
                            trailing_new_line = false;
                            FormattedTextLine::OrderedList(OrderedFormattedIndentTextInline {
                                number,
                                indented_text: FormattedIndentTextInline {
                                    indent_level: indent_level.as_usize(),
                                    text: text_block
                                        .block
                                        .into_iter()
                                        .map(|run| run.to_formatted_text_fragment())
                                        .collect(),
                                },
                            })
                        }
                        BufferBlockStyle::CodeBlock { code_block_type } => {
                            trailing_new_line = false;
                            let text = text_block.block.into_iter().map(|run| run.run).collect();
                            FormattedTextLine::CodeBlock(CodeBlockText {
                                lang: code_block_type.to_string(),
                                code: text,
                            })
                        }
                        BufferBlockStyle::PlainText => {
                            trailing_new_line = false;
                            FormattedTextLine::Line(
                                text_block
                                    .block
                                    .into_iter()
                                    .map(|run| {
                                        trailing_new_line = run.run.ends_with('\n');
                                        run.to_formatted_text_fragment()
                                    })
                                    .collect(),
                            )
                        }
                    });
                }
            }
        }

        // Make sure trailing newline gets its own linebreak.
        if trailing_new_line {
            lines.push_back(FormattedTextLine::LineBreak);
        }

        FormattedText { lines }
    }
}

impl StyledBufferRun {
    fn to_formatted_text_fragment(&self) -> FormattedTextFragment {
        let text = match self.run.strip_suffix('\n') {
            Some(without_newline) => without_newline.to_string(),
            _ => self.run.clone(),
        };

        FormattedTextFragment {
            text,
            styles: self.text_styles.clone().into(),
        }
    }
}

impl Buffer {
    pub(super) fn insert_formatted_text_at_selections(
        &mut self,
        text: FormattedText,
        origin: EditOrigin,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Self>,
    ) -> EditResult {
        self.modify_each_selection(
            selection_model,
            |buffer, selection, selection_model, _index| {
                let range = selection_model.selection_to_offset_range(selection);
                let text = text.clone();
                buffer.edits_for_formatted_text(range, text, origin)
            },
            ctx,
        )
    }

    pub(super) fn edits_for_formatted_text(
        &mut self,
        range: Range<CharOffset>,
        text: FormattedText,
        origin: EditOrigin,
    ) -> ActionWithSelectionDelta {
        let mut editor_action_set = vec![];
        // When replacing text in-place for a subset of the document, we may
        // need to adjust the block style based on the first line of the
        // replacement. However, for full-buffer replacements (the entire
        // document body), we rely on the inserted formatted text alone and do
        // not emit a separate StyleBlock action, to avoid introducing extra
        // empty headings or markers.
        let is_full_buffer_replacement =
            range.start == CharOffset::from(1) && range.end == self.max_charoffset();

        if range.start != CharOffset::zero()
            && !is_full_buffer_replacement
            && range.start == self.containing_line_start(range.start)
            && range.end == self.containing_line_end(range.end) - 1
        {
            let new_block_style = text
                .lines
                .front()
                .and_then(formatted_text_line_to_block_style);

            let active_block_type = self.block_type_at_point(range.start);

            if let BlockType::Text(active_block_style) = active_block_type {
                match new_block_style {
                    Some(block_style)
                        if block_style != active_block_style
                            && active_block_style == BufferBlockStyle::PlainText =>
                    {
                        editor_action_set.push(CoreEditorAction::new(
                            range.clone(),
                            CoreEditorActionType::StyleBlock(block_style),
                        ));
                    }
                    _ => (),
                }
            }
        }

        editor_action_set.push(CoreEditorAction::new(
            range.clone(),
            CoreEditorActionType::Insert {
                text,
                source: origin,
                override_next_style: false,
                insert_on_selection: true,
            },
        ));
        editor_action_set.push(self.update_buffer_end(range.end));
        ActionWithSelectionDelta::new_with_offsets(
            editor_action_set,
            &mut self.internal_anchors,
            range.end,
            range.end,
            0,
            AnchorSide::Right,
        )
    }
}

fn formatted_text_line_to_block_style(line: &FormattedTextLine) -> Option<BufferBlockStyle> {
    match line {
        FormattedTextLine::Heading(header) => Some(BufferBlockStyle::Header {
            header_size: header
                .heading_size
                .try_into()
                .unwrap_or(BlockHeaderSize::Header1),
        }),
        FormattedTextLine::CodeBlock(text) => Some(BufferBlockStyle::CodeBlock {
            code_block_type: text.into(),
        }),
        FormattedTextLine::UnorderedList(list) => Some(BufferBlockStyle::UnorderedList {
            indent_level: ListIndentLevel::from_usize(list.indent_level),
        }),
        FormattedTextLine::TaskList(list) => Some(BufferBlockStyle::TaskList {
            indent_level: ListIndentLevel::from_usize(list.indent_level),
            complete: list.complete,
        }),
        FormattedTextLine::OrderedList(list) => Some(BufferBlockStyle::OrderedList {
            indent_level: ListIndentLevel::from_usize(list.indented_text.indent_level),
            number: list.number,
        }),
        FormattedTextLine::Line(_) => Some(BufferBlockStyle::PlainText),
        FormattedTextLine::LineBreak => None,
        FormattedTextLine::HorizontalRule => Some(BufferBlockStyle::PlainText),
        // TODO(kevin): handle embedded objects in buffer.
        FormattedTextLine::Embedded(_) => None,
        // Images are block items, not block styles
        FormattedTextLine::Image(_) => None,
    }
}
