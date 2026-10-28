use sum_tree::SumTree;

use super::text::{BufferBlockStyle, BufferSummary, BufferText, MarkerDir};
use crate::content::cursor::BufferSumTree;

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;

/// Validates a [`SumTree`] of content, panicking if it is not valid.
pub fn validate_content(content: &SumTree<BufferText>) {
    let mut cursor = content.cursor::<(), BufferSummary>();
    cursor.descend_to_first_item(content, |_| true);

    let mut active_block_style: Option<BufferBlockStyle> = None;

    while let Some(item) = cursor.item() {
        let start_summary = cursor.start();
        let char_offset = start_summary.text.chars;
        match item {
            BufferText::Text { .. } => {}
            BufferText::Marker { marker_type, dir } => {
                let style_depth = start_summary.style_summary().style_counter(marker_type);
                match dir {
                    MarkerDir::Start => {
                        assert!(
                            style_depth == 0,
                            "{char_offset}: Found {marker_type:?} start marker, but style was already active\nBuffer: {}",
                            content.debug()
                        );
                    }
                    MarkerDir::End => {
                        assert!(
                            style_depth == 1,
                            "{char_offset}: Found {marker_type:?} end marker, but style was not active\nBuffer: {}",
                            content.debug()
                        );
                    }
                }
            }
            BufferText::Newline => (),
            BufferText::BlockMarker { marker_type } => {
                if Some(BufferBlockStyle::PlainText) == active_block_style {
                    assert!(
                        marker_type != &BufferBlockStyle::PlainText,
                        "{char_offset}: Found plain text marker when the active style is plain text\nBuffer: {}",
                        content.debug()
                    );
                }

                active_block_style = Some(marker_type.clone());
            }
            BufferText::Placeholder { .. } | BufferText::Link(_) => {}
        }

        assert!(
            active_block_style.is_some(),
            "{char_offset}: Buffer doesn't have an active block style.\nBuffer: {}",
            content.debug()
        );
        cursor.next();
    }

    // Buffers must end as plain text.
    assert!(
        active_block_style
            .clone()
            .is_some_and(|style| style == BufferBlockStyle::PlainText),
        "Buffer ends as {active_block_style:?}, not plain text.\nBuffer: {}",
        content.debug()
    );
}
