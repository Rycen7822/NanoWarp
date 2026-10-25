use crate::palette::PaletteSource;
pub mod auth_manager;
mod auth_override_warning_body;
pub mod auth_override_warning_modal;
mod auth_view_body;
pub mod auth_view_modal;
mod auth_view_shared_helpers;
mod login_error_modal;
mod login_failure_notification;
pub mod needs_sso_link_view;
pub use warp_server_auth::{auth_state, credentials, user, user_uid};
#[cfg(target_family = "wasm")]
pub mod web_handoff;

use ::settings::{Setting, ToggleableSetting};
pub use auth_manager::AuthManager;
pub use auth_state::AuthStateProvider;
use itertools::Itertools;
pub use user_uid::UserUid;
use warp_core::channel::ChannelState;
use warp_errors::report_if_error;
use warpui::modals::{AlertDialogWithCallbacks, ModalButton};
use warpui::{AppContext, SingletonEntity};

use crate::code::editor_management::{CodeEditorStatus, CodeEditorSummary};
use crate::palette::PaletteMode;
use crate::root_view::RootView;

use crate::session_management::{RunningSessionSummary, SessionNavigationData};
use crate::settings::PrivacySettings;
use crate::terminal::general_settings::GeneralSettings;
use crate::workspace::{Workspace, WorkspaceAction};
use crate::{
    GlobalResourceHandlesProvider, focus_running_window_and_show_native_modal, persistence,
};

pub fn init(app: &mut AppContext) {
    auth_view_modal::init(app);
    auth_view_body::init(app);
    auth_override_warning_body::init(app);
}

/// Returns the configured Warp web logout URL.
///
/// Keep this derived from the channel's server root so local and non-production
/// builds log out of the same web session they use for authentication.
pub fn web_logout_url() -> String {
    format!(
        "{}/logout",
        ChannelState::server_root_url().trim_end_matches('/')
    )
}

/// If the app has running processes or dirty objects, we'll show a confirmation modal before logging out.
/// If the user aborts, the user will not be logged out.
pub fn maybe_log_out(app: &mut AppContext) {
    let sessions = SessionNavigationData::all_sessions(app).collect_vec();
    let num_long_running_commands = RunningSessionSummary::new(&sessions)
        .long_running_cmds
        .len();

    let code_editors = CodeEditorStatus::all_editors(app).collect_vec();
    let code_editor_summary = CodeEditorSummary::new(&code_editors);

    let num_unsaved_files = code_editor_summary.unsaved_changes.len();

    let show_warning_before_log_out = *GeneralSettings::as_ref(app)
        .show_warning_before_quitting
        .value();
    if show_warning_before_log_out && (num_long_running_commands > 0 || num_unsaved_files > 0) {
        let mut button_data = vec![ModalButton::for_app("Yes, log out", |ctx| {
            log_out_and_open_web(ctx);
        })];

        let mut info_text_vec: Vec<String> = vec![];
        if num_long_running_commands > 0 {
            let plural = if num_long_running_commands > 1 {
                "processes"
            } else {
                "process"
            };
            info_text_vec.push(format!(
                "You have {num_long_running_commands} {plural} running."
            ));

            button_data.push(ModalButton::for_app("Show running processes", move |ctx| {
                let windowing_model = ctx.windows();
                let window_id = if let Some(active_window_id) = windowing_model.active_window() {
                    active_window_id
                } else if let Some(window_id) = ctx.window_ids().collect_vec().first() {
                    let window_id = *window_id;
                    windowing_model.show_window_and_focus_app(window_id);
                    window_id
                } else {
                    return;
                };

                if let Some(workspaces) = ctx.views_of_type::<Workspace>(window_id)
                    && let Some(handle) = workspaces.first()
                {
                    ctx.dispatch_typed_action_for_view(
                        window_id,
                        handle.id(),
                        &WorkspaceAction::OpenPalette {
                            mode: PaletteMode::Navigation,
                            source: PaletteSource::LogOutModal,
                            query: Some("running".to_owned()),
                        },
                    );
                }
            }))
        }

        if num_unsaved_files > 0 {
            let plural = if num_unsaved_files > 1 {
                "files"
            } else {
                "file"
            };
            info_text_vec.push(format!(
                "You have {num_unsaved_files} unsaved {plural}. \
            Logging out will cause you to lose the {plural}."
            ));
        }

        button_data.push(ModalButton::for_app("Cancel", move |_| {}));

        let alert_data = AlertDialogWithCallbacks::for_app(
            "Log out?",
            info_text_vec.join("\n"),
            button_data,
            move |ctx| {
                GeneralSettings::handle(ctx).update(ctx, |general_settings, ctx| {
                    report_if_error!(
                        general_settings
                            .show_warning_before_quitting
                            .toggle_and_save_value(ctx)
                    );
                });
            },
        );

        // On mac, we show the native platform modal. On platforms that don't support a native modal,
        // we show the custom warp modal.
        if cfg!(all(not(target_family = "wasm"), target_os = "macos")) {
            app.show_native_platform_modal(alert_data);
        } else {
            let sessions = SessionNavigationData::all_sessions(app).collect_vec();
            let sessions_summary = RunningSessionSummary::new(&sessions);
            focus_running_window_and_show_native_modal(sessions_summary, alert_data, app);
        }
    } else {
        log_out_and_open_web(app);
    }
}

/// Logs out locally and sends the user to Warp web's logout endpoint.
///
/// This is intentionally separate from [`log_out`], which is also used for
/// non-user-initiated auth recovery paths where opening a browser would be
/// surprising.
pub fn log_out_and_open_web(app: &mut AppContext) {
    log_out(app);
    let logout_url = web_logout_url();
    app.open_url(&logout_url);
}

// Log out the user, clears workspace state, stops running processes, and deletes database.
pub fn log_out(app: &mut AppContext) {
    let global_resource_handles = GlobalResourceHandlesProvider::as_ref(app).get();

    // As part of Logout v0, we remove sqlite3 so sessions and cloud objects don't persist between accounts.
    // TODO: Implement per-user scoping of sqlite3.
    persistence::remove(&global_resource_handles.model_event_sender);

    AuthManager::handle(app).update(app, |auth_manager, ctx| {
        auth_manager.log_out(ctx);
    });
    remove_cloud_persisted_settings(app);

    // Dispatch the GUI root-view action on every open GUI window so its state can be updated
    // correctly. Other front-ends, such as the TUI, manage their logout transition separately.
    let window_ids = app.window_ids().collect_vec();
    for window_id in window_ids {
        if let Some(root_views) = app.views_of_type::<RootView>(window_id) {
            let Some(root_view) = root_views.first() else {
                continue;
            };
            app.dispatch_action(
                window_id,
                &[root_view.id()],
                "root_view:log_out",
                &(),
                log::Level::Info,
            );
        }
    }

    #[cfg(target_family = "wasm")]
    crate::platform::wasm::emit_event(crate::platform::wasm::WarpEvent::LoggedOut);
}

// Remove the cloud persisted settings from user defaults.
// When a user signs out, we remove cloud persisted settings of their account.
// This is so they do not experience the old settings when they log in with a different account.
// Partial deletion of user defaults is a stopgap for Logout v0. The correct solution is:
fn remove_cloud_persisted_settings(app: &mut AppContext) {
    // Reset the Privacy Settings in the login screen to default values.
    PrivacySettings::handle(app).update(app, |privacy_settings, _| {
        privacy_settings.refresh_to_default();
    });
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
