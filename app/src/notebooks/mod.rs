mod context_menu;
pub mod editor;
pub mod file;
pub mod link;
mod styles;
pub mod telemetry;

use serde::{Deserialize, Serialize};
use warpui::AppContext;

/// The backing file location of a notebook viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum NotebookLocation {
    LocalFile,
    RemoteFile,
}

/// Initialize file-viewer and shared editor keybindings.
pub fn init(app: &mut AppContext) {
    self::file::init(app);
    self::editor::view::init(app);
}
