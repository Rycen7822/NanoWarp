//! Hit-testing implementation for the rendering model.

use string_offset::CharOffset;
use sum_tree::SeekBias;
use warpui_core::units::{IntoPixels, Pixels};

use super::positioned::{Positioned, PositionedCursor};
use super::{BlockItem, Height, LayoutSummary, RenderState};

#[cfg(test)]
#[path = "location_tests.rs"]
mod tests;

/// A location within the editor, as resolved by hit-testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// An exact location within the content space.
    Text {
        /// Offset of the hit character.
        char_offset: CharOffset,
        /// Whether or not we clamped to this location (for example, if the position
        /// was after the end of text on a line, or after the end of all text
        /// in the editor).
        clamped: bool,
        /// Cursor disposition for soft-wrapped text.
        wrap_direction: WrapDirection,
        /// The starting offset of the block that contains the hit character.
        block_start: CharOffset,
        link: Option<String>,
    },
}

impl Location {
    /// The starting [`CharOffset`] of the block that was hit. All hits are within a single block.
    pub fn block_start(&self) -> CharOffset {
        match self {
            Location::Text { block_start, .. } => *block_start,
        }
    }
}

/// With soft-wrapping, the end of one line and the start of the next have the
/// same character offset. The `WrapDirection` indicates which one a location is
/// at.
///
/// Suppose we have the line `longword`, soft-wrapped to
/// ```text
/// long
/// word
/// ```
///
/// Visually, after `g` and before `w` are two distinct locations. However, they
/// have the same character offset, 4.
///
/// To represent the first location, we wrap up:
/// ```text
/// long|
/// word
/// ```
///
/// To represent the second, we wrap down:
/// ```text
/// long
/// |word
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WrapDirection {
    /// Place the cursor at the end of the previous line.
    Up,
    /// Place the cursor at the start of the next line.
    #[default]
    Down,
}

impl RenderState {
    /// Performs hit-testing on coordinates relative to the content origin
    /// (that is, non-viewported).
    pub fn render_coordinates_to_location(&self, x: Pixels, y: Pixels) -> Location {
        let content = self.content.borrow();
        let mut block_cursor = content.cursor::<Height, LayoutSummary>();
        block_cursor.seek(&y.into(), SeekBias::Left);

        let Some(block) = block_cursor.positioned_item() else {
            // If we're at the end of the editor, bias towards placing new text
            // on a new line.
            let char_offset = self.max_offset();
            log::debug!("Clamped to end: {char_offset}");
            return Location::Text {
                char_offset,
                clamped: true,
                wrap_direction: WrapDirection::Down,
                block_start: char_offset,
                link: None,
            };
        };

        block.coordinates_to_location(x, y)
    }

    /// Performs hit-testing on coordinates relative to the viewport origin.
    pub fn viewport_coordinates_to_location(&self, x: Pixels, y: Pixels) -> Location {
        self.render_coordinates_to_location(
            (x + self.viewport.scroll_left()).max(Pixels::zero()),
            (y + self.viewport.scroll_top()).max(Pixels::zero()),
        )
    }
}

impl<'a> Positioned<'a, BlockItem> {
    /// Resolve coordinates to a location, assuming they're within this block.
    /// The coordinates are all relative to the content origin.
    fn coordinates_to_location(&self, x: Pixels, y: Pixels) -> Location {
        match self.item {
            BlockItem::Paragraph(paragraph) => self
                .paragraph(paragraph)
                .coordinate_to_location(self.unpad_x(x), y),
            BlockItem::TrailingNewLine(_)
            | BlockItem::TemporaryBlock { .. }
            | BlockItem::Hidden { .. } => Location::Text {
                char_offset: self.start_char_offset,
                clamped: true,
                wrap_direction: WrapDirection::Down,
                block_start: self.start_char_offset,
                link: None,
            },
        }
    }

    /// Remove horizontal padding from an x-coordinate in order to hit-test within a padded
    /// paragraph.
    fn unpad_x(&self, x: Pixels) -> Pixels {
        (x - self.content_origin().x().into_pixels()).max(Pixels::zero())
    }
}
