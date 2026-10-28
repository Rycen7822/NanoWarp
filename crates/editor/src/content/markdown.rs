use std::collections::VecDeque;
use std::fmt::Write;
use std::ops::Range;

use markdown_parser::{
    CodeBlockText, FormattedIndentTextInline, FormattedTaskList, FormattedText,
    FormattedTextFragment, FormattedTextHeader, FormattedTextLine,
    OrderedFormattedIndentTextInline,
};
use string_offset::CharOffset;
use warpui_core::elements::{ListIndentLevel, ListNumbering};
use warpui_core::{AppContext, ModelContext, ModelHandle};

use super::buffer::{
    ActionWithSelectionDelta, Buffer, EditOrigin, EditResult, StyledBufferBlock,
    StyledBufferBlocks, StyledBufferRun,
};
use super::core::{CoreEditorAction, CoreEditorActionType};
use super::text::{BlockHeaderSize, BlockType, BufferBlockStyle, TextStylesWithMetadata};
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

/// Parser that takes a slice of content model with StyledBufferBlocks and returns
/// the formatted markdown in string.
pub struct BufferMarkdownParser<'a> {
    blocks: StyledBufferBlocks<'a>,
    style: MarkdownStyle<'a>,
}

impl<'a> BufferMarkdownParser<'a> {
    pub fn new(style: MarkdownStyle<'a>, blocks: StyledBufferBlocks<'a>) -> Self {
        Self { blocks, style }
    }

    // Consumes the iterator and returns the corresponding markdown string.
    pub fn to_markdown(&mut self) -> String {
        let mut res = String::new();
        let mut ordered_list_numbering = ListNumbering::new();

        for block in self.blocks.by_ref() {
            match block {
                StyledBufferBlock::Item(item) => {
                    res.push_str(&item.as_markdown(self.style));
                    res.push('\n');
                }
                StyledBufferBlock::Text(text_block) => {
                    // Push the prefix for the active block.
                    match text_block.style.clone() {
                        BufferBlockStyle::Header { header_size } => {
                            res.push_str(&"#".repeat(header_size.into()));
                            res.push(' ');
                        }
                        BufferBlockStyle::UnorderedList { indent_level } => {
                            res.push_str("    ".repeat(indent_level.as_usize()).as_str());
                            res.push_str("* ")
                        }
                        BufferBlockStyle::OrderedList {
                            indent_level,
                            number,
                        } => {
                            let indent = indent_level.as_usize();
                            res.push_str("    ".repeat(indent).as_str());

                            let _ = write!(
                                &mut res,
                                "{}. ",
                                ordered_list_numbering.advance(indent, number).label_index
                            );
                        }
                        BufferBlockStyle::CodeBlock { code_block_type } => res.push_str(&format!(
                            "```{}\n",
                            &code_block_type.to_markdown_representation(self.style)
                        )),
                        BufferBlockStyle::TaskList {
                            indent_level,
                            complete,
                        } => {
                            res.push_str("    ".repeat(indent_level.as_usize()).as_str());
                            if complete {
                                res.push_str("- [x] ");
                            } else {
                                res.push_str("- [ ] ");
                            }
                        }
                        BufferBlockStyle::PlainText => (),
                    };

                    if !matches!(text_block.style, BufferBlockStyle::OrderedList { .. }) {
                        ordered_list_numbering.reset();
                    }

                    let block_escapes = !matches!(
                        self.style,
                        MarkdownStyle::Export {
                            should_not_escape_markdown_punctuation: true,
                            ..
                        }
                    ) && text_block.style.escape_markdown_punctuation();
                    Self::append_block_content(&text_block.block, block_escapes, &mut res);

                    // Push the suffix for the active block.
                    match text_block.style {
                        BufferBlockStyle::CodeBlock { .. } => res.push_str("```\n"),
                        BufferBlockStyle::Header { .. }
                        | BufferBlockStyle::PlainText
                        | BufferBlockStyle::UnorderedList { .. }
                        | BufferBlockStyle::OrderedList { .. }
                        | BufferBlockStyle::TaskList { .. } => (),
                    }
                }
            }
        }

        res
    }

    /// Emits Markdown formatting markers for changing from `prev_styles` to `next_styles`.
    ///
    /// This will omit formatting for unchanged styles, which is required to produce valid Markdown
    /// (for example, according to [this](https://spec.commonmark.org/0.30/#example-411),
    /// `*abc**def*`, parses to `<i_s>abc**def<i_e>`, not `<i_s>abcdef<i_e>`).
    ///
    /// If the text that `next_styles` applies to (`next_content`) starts with whitespace,
    /// the formatting markers might be reordered after the first character to comply
    /// with [these rules](https://spec.commonmark.org/0.30/#left-flanking-delimiter-run).
    /// Likewise, if the last character of `buf` is whitespace, the formatting markers may be
    /// reordered before it.
    ///
    /// This alters the user content slightly, but will parse back to what they expect, rather than
    /// something un-representable. In this case, the returned `&str` is the shifted
    /// `next_content`.
    fn append_formatting<'b>(
        prev_styles: &TextStylesWithMetadata,
        next_styles: &TextStylesWithMetadata,
        mut next_content: &'b str,
        buf: &mut String,
    ) -> &'b str {
        // Because of how links start a new formatting scope, we have to change styles in a particular
        // order, from the inside out:
        // 1. End inline code (since other style markers aren't interpreted within code spans)
        // 2. End other inline styles
        // 3. End and then start links
        // 4. Start inline styles
        // 5. Start inline code

        if prev_styles.is_inline_code() && !next_styles.is_inline_code() {
            buf.push('`')
        }

        let end_bold = prev_styles.is_at_least_bold() && !next_styles.is_at_least_bold();
        let end_italic = prev_styles.is_italic() && !next_styles.is_italic();
        let end_strikethrough = prev_styles.is_strikethrough() && !next_styles.is_strikethrough();
        let end_underline = prev_styles.is_underlined() && !next_styles.is_underlined();

        // This, and the parallel logic below, is an unfortunate workaround for the CommonMark
        // rules about space before/after bold and italic formatting markers
        // (https://spec.commonmark.org/0.30/#left-flanking-delimiter-run). Like Notion, we
        // rearrange the user content slightly to match the spec, in order to be compatible with
        // GitHub and other parsers.
        // In this case, if the current buffer ends in whitespace, we remove it, write the
        // end formatting markers, and then re-add it.
        let swapped_content = buf
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_whitespace())
            .last()
            // Panic safety: iterating over `char_indices` means that `idx` must be in-bounds and a
            // code point boundary.
            .map(|(idx, _)| buf.split_off(idx));

        if end_underline {
            buf.push_str("</u>")
        }

        if end_strikethrough {
            buf.push_str("~~");
        }

        if end_bold {
            buf.push_str("**");
        }

        if end_italic {
            buf.push('*');
        }

        buf.extend(swapped_content);

        let prev_link = prev_styles.link_content();
        let next_link = next_styles.link_content();

        // Open/close links if:
        // * A link is beginning or ending.
        // * There are two adjacent, different, links.
        if prev_link != next_link {
            if let Some(closing_url) = prev_link {
                // The `String` implementation of `fmt::Write` never returns `Err`.
                let _ = write!(buf, "]({closing_url})");
            }

            if next_link.is_some() {
                buf.push('[');
            }
        }

        let start_bold = !prev_styles.is_at_least_bold() && next_styles.is_at_least_bold();
        let start_italic = !prev_styles.is_italic() && next_styles.is_italic();
        let start_strikethrough = !prev_styles.is_strikethrough() && next_styles.is_strikethrough();
        let start_underline = !prev_styles.is_underlined() && next_styles.is_underlined();

        // In order to parse correctly, Markdown emphasis markers must be after whitespace. If the
        // next run of content starts with whitespace, we write that and then the markers, and then
        // let the outer loop write the rest of the content.
        if (start_bold || start_italic || start_strikethrough || start_underline)
            && let Some(idx) = next_content.find(|c: char| !c.is_whitespace())
        {
            // Panic safety: if the pattern matches, `find` returns the byte index of the first
            // matching character, so it will be a code point boundary.
            let (whitespace, rest) = next_content.split_at(idx);
            buf.push_str(whitespace);
            next_content = rest;
        }

        if start_bold {
            buf.push_str("**");
        }

        if start_italic {
            buf.push('*');
        }

        if start_strikethrough {
            buf.push_str("~~");
        }

        if start_underline {
            buf.push_str("<u>");
        }

        if !prev_styles.is_inline_code() && next_styles.is_inline_code() {
            buf.push('`');
        }

        next_content
    }

    /// Appends text content to a Markdown buffer, escaping any special characters as needed.
    fn append_content(text: &str, should_escape: bool, buf: &mut String) {
        if should_escape {
            buf.reserve(text.len());
            for ch in text.chars() {
                // Only escape punctuation characters that have special meaning in Markdown
                if Self::is_markdown_special_char(ch) {
                    buf.push('\\');
                }
                buf.push(ch);
            }
        } else {
            buf.push_str(text);
        }
    }

    fn append_block_content(runs: &[StyledBufferRun], block_escapes: bool, buf: &mut String) {
        let mut prev_styles = TextStylesWithMetadata::default();
        for run in runs {
            if run.run.is_empty() || run.text_styles.is_placeholder() {
                continue;
            }
            let (content, has_trailing_newline) = match run.run.strip_suffix('\n') {
                Some(without_newline) => (without_newline, true),
                _ => (run.run.as_str(), false),
            };
            let content = Self::append_formatting(&prev_styles, &run.text_styles, content, buf);
            prev_styles.clone_from(&run.text_styles);
            let should_escape = block_escapes && run.text_styles.escape_markdown_punctuation();
            Self::append_content(content, should_escape, buf);
            if has_trailing_newline {
                Self::append_formatting(
                    &run.text_styles,
                    &TextStylesWithMetadata::default(),
                    "",
                    buf,
                );
                prev_styles = TextStylesWithMetadata::default();
                buf.push('\n');
            }
        }
        Self::append_formatting(&prev_styles, &TextStylesWithMetadata::default(), "", buf);
    }

    /// Returns true if the character has special meaning in Markdown and should be escaped.
    fn is_markdown_special_char(ch: char) -> bool {
        matches!(
            ch,
            '\\' | // Backslash - used for escaping other characters
            '`' | // Backtick - used for inline code
            '*' | // Asterisk - used for emphasis and unordered lists
            '_' | // Underscore - used for emphasis
            '{' | // Left curly brace - used in some Markdown extensions
            '}' | // Right curly brace - used in some Markdown extensions
            '[' | // Left square bracket - used for links and images
            ']' | // Right square bracket - used for links and images
            '(' | // Left parenthesis - used for links and images
            ')' | // Right parenthesis - used for links and images
            '#' | // Hash - used for headings
            '+' | // Plus - used for unordered lists
            '-' | // Minus - used for unordered lists and horizontal rules
            '.' | // Period - used for ordered lists
            '!' // Exclamation mark - used for images
        )
    }
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

impl BufferBlockStyle {
    /// Whether or not punctuation that may be misinterpreted as Markdown formatting should be
    /// escaped. This is true for _most_ block types, but some do not allow escapes.
    fn escape_markdown_punctuation(&self) -> bool {
        !matches!(self, BufferBlockStyle::CodeBlock { .. })
    }
}

impl TextStylesWithMetadata {
    fn escape_markdown_punctuation(&self) -> bool {
        !self.is_inline_code()
    }
}

impl Buffer {
    pub(super) fn replace_with_formatted_text(
        &mut self,
        range: Range<CharOffset>,
        text: FormattedText,
        origin: EditOrigin,
        selection_model: ModelHandle<BufferSelectionModel>,
        ctx: &mut ModelContext<Self>,
    ) -> EditResult {
        selection_model.update(ctx, |selection_model, _| {
            selection_model.truncate();
        });
        let edit = self.edits_for_formatted_text(range, text, origin);
        self.modify_first_selection(selection_model, edit, ctx)
    }

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

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
