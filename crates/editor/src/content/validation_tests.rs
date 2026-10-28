use sum_tree::SumTree;

use crate::content::cursor::BufferSumTree;
use crate::content::text::{BufferBlockStyle, BufferText, BufferTextStyle, MarkerDir};

#[test]
#[should_panic(
    expected = "2: Found Weight(Bold) end marker, but style was not active\nBuffer: <text>x<b_e>"
)]
fn test_validate_unmatched_style_end() {
    let mut tree = SumTree::new();
    tree.push(BufferText::BlockMarker {
        marker_type: BufferBlockStyle::PlainText,
    });
    tree.append_str("x");
    tree.push(BufferText::Marker {
        marker_type: BufferTextStyle::bold(),
        dir: MarkerDir::End,
    });

    super::validate_content(&tree);
}

#[test]
#[should_panic(
    expected = "3: Found Italic start marker, but style was already active\nBuffer: <text>x<i_s>y<i_s><i_e>"
)]
fn test_validate_unmatched_style_start() {
    let mut tree = SumTree::new();
    tree.push(BufferText::BlockMarker {
        marker_type: BufferBlockStyle::PlainText,
    });
    tree.append_str("x");
    tree.push(BufferText::Marker {
        marker_type: BufferTextStyle::Italic,
        dir: MarkerDir::Start,
    });
    tree.append_str("y");
    tree.push(BufferText::Marker {
        marker_type: BufferTextStyle::Italic,
        dir: MarkerDir::Start,
    });
    tree.push(BufferText::Marker {
        marker_type: BufferTextStyle::Italic,
        dir: MarkerDir::End,
    });

    super::validate_content(&tree);
}

#[test]
#[should_panic(expected = "0: Buffer doesn't have an active block style.\nBuffer: \\ny")]
fn test_validate_buffer_without_start_marker() {
    let mut tree = SumTree::new();
    tree.append_str("\ny");

    super::validate_content(&tree);
}

#[test]
#[should_panic(
    expected = "2: Found plain text marker when the active style is plain text\nBuffer: <text>y<text>x"
)]
fn test_validate_buffer_with_dup_text_marker() {
    let mut tree = SumTree::new();
    tree.push(BufferText::BlockMarker {
        marker_type: BufferBlockStyle::PlainText,
    });
    tree.append_str("y");
    tree.push(BufferText::BlockMarker {
        marker_type: BufferBlockStyle::PlainText,
    });
    tree.append_str("x");

    super::validate_content(&tree);
}

#[test]
fn test_validate_ok() {
    let mut tree = SumTree::new();
    tree.push(BufferText::BlockMarker {
        marker_type: BufferBlockStyle::PlainText,
    });
    tree.append_str("a");
    tree.push(BufferText::Marker {
        marker_type: BufferTextStyle::Italic,
        dir: MarkerDir::Start,
    });
    tree.append_str("i");
    tree.push(BufferText::Marker {
        marker_type: BufferTextStyle::Italic,
        dir: MarkerDir::End,
    });
    // This should not panic.
    super::validate_content(&tree);
}
