use std::ops::Range;

use rangemap::RangeSet;
use warp_editor::content::text::LineCount;

use crate::code::editor::DiffDelta;

/// The number of context lines to show before and after each change
const CONTEXT_LINES: usize = 4;

/// Calculate which lines should be hidden in a file after applying diffs.
///
/// This function takes a list of diff deltas and calculates which lines should be hidden
/// (everything except for CONTEXT_LINES before and after each change).
///
/// These are 0-indexed line numbers BEFORE the diffs are applied, so the first line is 0.
///
/// # Arguments
///
/// * `diffs` - The list of diff deltas that will be applied to the file
/// * `line_count` - The total number of lines in the file before any diffs are applied
///
/// # Returns
///
/// A `RangeSet<usize>` containing the line ranges that should be hidden (0-indexed).
///
/// Note that DiffDelta uses 1-indexed line ranges, so we convert them to 0-indexed
/// ```
pub fn calculate_hidden_lines(diffs: &[DiffDelta], line_count: usize) -> RangeSet<LineCount> {
    // Calculate the visible line ranges (with context)
    let mut visible_ranges: RangeSet<LineCount> = RangeSet::new();

    // Add ranges for diffs
    for diff in diffs {
        // Convert 1-indexed line ranges to 0-indexed
        let start_line = diff.replacement_line_range.start.saturating_sub(1);
        let end_line = diff.replacement_line_range.end.saturating_sub(1);

        let context_start = start_line.saturating_sub(CONTEXT_LINES);
        let context_end = end_line + CONTEXT_LINES;

        if context_start < context_end {
            visible_ranges.insert(context_start.into()..context_end.into());
        }
    }

    // Calculate hidden ranges as the complement of visible ranges
    let all_lines: Range<LineCount> = LineCount::from(0)..LineCount::from(line_count);

    // Find gaps in the visible ranges
    visible_ranges
        .gaps(&all_lines)
        .collect::<RangeSet<LineCount>>()
}

#[cfg(test)]
#[path = "hidden_lines_tests.rs"]
mod tests;
