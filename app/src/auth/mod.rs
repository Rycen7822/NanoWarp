pub mod auth_manager;
mod auth_override_warning_body;
pub mod auth_override_warning_modal;
mod auth_view_body;
pub mod auth_view_modal;
mod auth_view_shared_helpers;
mod login_failure_notification;
pub use warp_server_auth::{auth_state, credentials, user, user_uid};

pub use auth_state::AuthStateProvider;
pub use user_uid::UserUid;
use warpui::AppContext;
use warpui::elements::Border;

use crate::themes::theme::{Blend, Fill};

pub fn init(app: &mut AppContext) {
    auth_view_modal::init(app);
    auth_view_body::init(app);
    auth_override_warning_body::init(app);
}

/// Border for the auth modals, which use their own colors instead of the chosen theme.
pub(crate) fn unthemed_window_border() -> Border {
    if cfg!(all(not(target_os = "macos"), not(target_family = "wasm"))) {
        Border::all(1.).with_border_fill(Fill::black().blend(&Fill::white().with_opacity(15)))
    } else {
        Border::all(1.).with_border_fill(Fill::black().with_opacity(0))
    }
}
