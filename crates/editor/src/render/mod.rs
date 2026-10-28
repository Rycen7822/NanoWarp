//! Rich Text Editor rendering layer - model and UI element for rendering
//! marked-up rich text.

pub mod element;
pub mod layout;
pub mod model;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
