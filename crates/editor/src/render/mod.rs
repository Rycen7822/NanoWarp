//! Rich Text Editor rendering layer - model and UI element for rendering
//! marked-up rich text.

pub mod element;
pub mod layout;
pub mod model;

/// The size for icon buttons within the rich-text editor. This is needed for both layout and
/// painting, so it's defined here.
pub const ICON_BUTTON_SIZE: f32 = 24.;
pub const CODE_BLOCK_BOTTOM_PADDING: f32 = 42.;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
