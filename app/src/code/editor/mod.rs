#![cfg_attr(target_family = "wasm", allow(dead_code, unused_imports))]

pub(crate) mod comment_editor;
mod comments;
pub(super) mod diff;
mod element;
pub mod embedded_comment;
pub mod find;
pub mod goto_line;
pub mod line;
mod line_iterator;
pub mod model;
mod nav_bar;
pub mod scroll;
pub mod view;

pub use comment_editor::{CommentEditor, CommentEditorEvent};
pub use comments::{EditorCommentsModel, EditorReviewComment};
pub(crate) use diff::{add_color, remove_color};
pub use element::GutterHoverTarget;

/// Visual representation of a single diff hunk.
#[derive(Clone, PartialEq, Eq)]
pub struct DiffDelta {
    pub replacement_line_range: std::ops::Range<usize>,
    pub insertion: String,
}

impl std::fmt::Debug for DiffDelta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if cfg!(debug_assertions) {
            write!(
                f,
                "DiffDelta {{\nreplacement_line_range: {:?},",
                &self.replacement_line_range
            )?;
            f.write_str("\n--insertion--\n")?;
            f.write_str(&self.insertion)?;
            f.write_str("\n}")
        } else {
            Ok(())
        }
    }
}
