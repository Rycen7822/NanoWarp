use std::ops::Range;

use string_offset::CharOffset;
use warp_editor::content::buffer::Buffer;
use warpui::text::word_boundaries::WordBoundariesPolicy;

/// Returns the range of the word surrounding the given offset.
pub(crate) fn get_word_range_at_offset(
    buffer: &Buffer,
    offset: CharOffset,
    word_boundary_policy: Option<WordBoundariesPolicy>,
) -> Option<Range<CharOffset>> {
    use warp_editor::content::buffer::{ToBufferCharOffset, ToBufferPoint};
    use warpui::text::TextBuffer;
    use warpui::text::words::is_default_word_boundary;

    let word_boundary_policy = word_boundary_policy.unwrap_or(WordBoundariesPolicy::Default);
    let mut word_found_at: Option<CharOffset> = None;
    let mut cursor_offset = offset;

    if let Ok(chars) = buffer.chars_at(offset) {
        for c in chars {
            if c == '\n' {
                // Do not cross line boundaries when searching for the nearest word
                break;
            }
            if !is_default_word_boundary(c) {
                word_found_at = Some(cursor_offset);
                break;
            }
            // advance one character
            cursor_offset += 1;
        }
    }

    let found_offset = word_found_at?;
    let found_point = found_offset.to_buffer_point(buffer);

    let word_start_point = buffer
        .word_starts_backward_from_offset_inclusive(found_point)
        .ok()
        .map(|iter| iter.with_policy(&word_boundary_policy))
        .and_then(|mut iter| iter.next())
        .unwrap_or(found_point);

    let word_end_point = buffer
        .word_ends_from_offset_exclusive(found_point)
        .ok()
        .map(|iter| iter.with_policy(&word_boundary_policy))
        .and_then(|mut iter| iter.next())
        .unwrap_or(found_point);

    let word_start = word_start_point.to_buffer_char_offset(buffer);
    let word_end = word_end_point.to_buffer_char_offset(buffer);

    if word_start < word_end {
        Some(word_start..word_end)
    } else {
        None
    }
}
