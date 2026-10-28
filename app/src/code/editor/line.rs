use std::ops::Range;

use warp_editor::render::model::LineCount;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorLineLocation {
    Collapsed {
        line_range: Range<LineCount>,
    },
    Current {
        line_number: LineCount,
        line_range: Range<LineCount>,
    },
    Removed {
        line_number: LineCount,
        line_range: Range<LineCount>,
        // There can be many deleted lines in a removal hunk, so we track the index of this line within the hunk.
        index: usize,
    },
}

impl EditorLineLocation {
    pub fn line_range(&self) -> &Range<LineCount> {
        match self {
            EditorLineLocation::Current { line_range, .. } => line_range,
            EditorLineLocation::Removed { line_range, .. } => line_range,
            EditorLineLocation::Collapsed { line_range } => line_range,
        }
    }

    /// Check if this line location represents the same line as another.
    /// This is not the same as equality, as the line range of the diff hunk may differ.
    pub fn is_same_line(&self, other: &Self) -> bool {
        match (self, other) {
            (
                EditorLineLocation::Current { line_number: a, .. },
                EditorLineLocation::Current { line_number: b, .. },
            ) if a == b => true,
            (
                EditorLineLocation::Removed {
                    line_number: al,
                    index: ai,
                    ..
                },
                EditorLineLocation::Removed {
                    line_number: bl,
                    index: bi,
                    ..
                },
            ) if al == bl && ai == bi => true,
            _ => false,
        }
    }
}
