//! Utilities for terminal keybindings.

use crate::util::bindings::{CustomAction, custom_tag_to_keystroke};

/// The keybinding label to display for a [`CustomAction`].
pub fn custom_action_to_display(action: CustomAction) -> Option<String> {
    custom_tag_to_keystroke(action.into()).map(|keystroke| keystroke.displayed())
}
