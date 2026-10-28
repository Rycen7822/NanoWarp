use std::collections::VecDeque;

use markdown_parser::FormattedTextFragment;

use super::buffer::{StyledBufferBlock, StyledBufferBlocks, StyledBufferRun};
use super::core::EditorLine;

/// Convert editor paragraphs into the line states used by undo.
pub struct BufferToEditorLines<'a> {
    blocks: StyledBufferBlocks<'a>,
}
impl<'a> BufferToEditorLines<'a> {
    pub fn new(blocks: StyledBufferBlocks<'a>) -> Self {
        Self { blocks }
    }
    pub fn to_editor_lines(self) -> VecDeque<EditorLine> {
        let mut lines = VecDeque::new();
        let mut trailing_new_line = false;
        for item in self.blocks {
            let StyledBufferBlock::Text(text_block) = item;
            trailing_new_line = false;
            lines.push_back(EditorLine::Line(
                text_block
                    .block
                    .into_iter()
                    .map(|run| {
                        trailing_new_line = run.run.ends_with('\n');
                        run.to_formatted_text_fragment()
                    })
                    .collect(),
            ));
        }
        if trailing_new_line {
            lines.push_back(EditorLine::LineBreak);
        }
        lines
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
