use std::collections::VecDeque;

use markdown_parser::{
    CodeBlockText, FormattedIndentTextInline, FormattedTaskList, FormattedText,
    FormattedTextFragment, FormattedTextHeader, FormattedTextLine,
    OrderedFormattedIndentTextInline,
};

use super::buffer::{StyledBufferBlock, StyledBufferBlocks, StyledBufferRun};
use super::text::BufferBlockStyle;

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
