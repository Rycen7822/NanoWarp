mod auth_secret_ftux_dropdown;
mod auth_secret_ftux_view;
pub(crate) mod auth_secret_selector;
mod block;
mod delete_auth_secret_confirmation_dialog;
mod footer;
mod harness_selector;
mod host_selector;
mod loading_screen;
mod model;
mod progress;
mod progress_ui_state;
mod team_required;
mod view_impl;

pub use auth_secret_ftux_view::{
    AuthSecretFtuxAction, AuthSecretFtuxView, AuthSecretFtuxViewEvent,
};
pub use auth_secret_selector::{
    AuthSecretSelector, AuthSecretSelectorAction, AuthSecretSelectorEvent,
};
pub use block::*;
pub use footer::{render_error_footer, render_loading_footer};
pub use harness_selector::{HarnessSelector, HarnessSelectorAction, HarnessSelectorEvent};
pub use host_selector::{
    Host, HostSelector, HostSelectorAction, HostSelectorEvent, NakedHeaderButtonTheme,
};
pub use loading_screen::{render_cloud_mode_error_screen, render_cloud_mode_loading_screen};
pub use model::{AgentProgress, AmbientAgentViewModel, AmbientAgentViewModelEvent, Status};
pub use progress::{ProgressProps, ProgressStep, ProgressStepState, render_progress};
pub use progress_ui_state::AmbientAgentProgressUIState;
pub use team_required::CloudAgentTeamRequiredView;
pub(crate) use team_required::{
    CLOUD_AGENT_TEAM_REQUIRED_MESSAGE,
    should_render as should_render_cloud_agent_team_required_view,
};
use warp_core::features::FeatureFlag;
use warpui::geometry::vector::Vector2F;
use warpui::{AppContext, ModelHandle, ViewHandle, WindowId};

use crate::ai::blocklist::agent_view::{AgentViewController, AgentViewState};
use crate::pane_group::TerminalViewResources;
use crate::terminal::{TerminalManager, TerminalModel, TerminalView, shared_session};

/// Creates a cloud mode terminal view and manager for ambient agent sessions.
/// See `viewer::TerminalManager::enable_orchestration_polling` for the flag.
pub fn create_cloud_mode_view(
    resources: TerminalViewResources,
    view_bounds_size: Vector2F,
    window_id: WindowId,
    enable_orchestration_polling: bool,
    ctx: &mut AppContext,
) -> (
    ViewHandle<TerminalView>,
    ModelHandle<Box<dyn TerminalManager>>,
) {
    // In Cloud Mode, ambient agent prompts are composed in an uninitialized session-sharing
    // viewer pane. This lets us reuse the terminal input without a backing session, and
    // then join the ambient agent session once it's ready.
    let terminal_init = shared_session::viewer::TerminalManager::new_deferred(
        resources,
        view_bounds_size,
        window_id,
        enable_orchestration_polling,
        ctx,
    );
    let viewer_manager = terminal_init.manager;
    let terminal_view = terminal_init.view;
    let terminal_manager: ModelHandle<Box<dyn TerminalManager>> =
        ctx.add_model(|_ctx| Box::new(viewer_manager) as Box<dyn TerminalManager>);

    // Subscribe to the ambient agent view model to join the session once it's ready.
    // This ensures that we use the manager corresponding to this specific view.
    let Some(view_model) = terminal_view
        .as_ref(ctx)
        .ambient_agent_view_model()
        .cloned()
    else {
        log::warn!("Cloud mode view was created without an ambient agent view model");
        return (terminal_view, terminal_manager);
    };
    wire_ambient_agent_session_events(&terminal_manager, &view_model, ctx);

    (terminal_view, terminal_manager)
}

/// Wires an [`AmbientAgentViewModel`]'s session lifecycle events to the viewer
/// [`shared_session::viewer::TerminalManager`] so the viewer connects/attaches to
/// the right shared session as runs come and go:
/// - [`AmbientAgentViewModelEvent::SessionReady`]: a freshly spawned run's session is
///   ready, so connect the viewer to it (initial run).
/// - [`AmbientAgentViewModelEvent::ExecutionSessionReady`]: a follow-up run started on a
///   new VM after the previous one ended, so re-attach the viewer to the new session.
///
/// Shared by the compose path ([`create_cloud_mode_view`]) and the shared-session viewer
/// path (`PaneGroup::create_shared_session_viewer`) so every ambient viewer re-attaches on
/// follow-up runs — including a raw `shared_session` link join whose model is created lazily
/// at `SessionJoined`.
pub fn wire_ambient_agent_session_events(
    terminal_manager: &ModelHandle<Box<dyn TerminalManager>>,
    view_model: &ModelHandle<AmbientAgentViewModel>,
    ctx: &mut AppContext,
) {
    let view_model = view_model.clone();
    terminal_manager.update(ctx, |_, ctx| {
        ctx.subscribe_to_model(&view_model, move |manager, _view_model, event, ctx| {
            let Some(manager) = manager
                .as_any_mut()
                .downcast_mut::<shared_session::viewer::TerminalManager>()
            else {
                return;
            };
            match event {
                AmbientAgentViewModelEvent::SessionReady { session_id } => {
                    if manager.connect_to_session(*session_id, false, ctx) {
                        manager.start_cloud_mode_setup_command_tracking();
                    }
                }
                AmbientAgentViewModelEvent::ExecutionSessionReady { session_id } => {
                    // Returns false when the viewer is mid-connect, in which case the pane stays
                    // on its current session. Recoverable, but silent otherwise: the attach is
                    // driven by an event, so the caller that requested it has already returned.
                    if !manager.attach_execution_session(*session_id, ctx) {
                        log::warn!(
                            "Ambient viewer could not re-attach to execution session {session_id}"
                        );
                    }
                }
                AmbientAgentViewModelEvent::DispatchedAgent
                | AmbientAgentViewModelEvent::FollowupDispatched
                | AmbientAgentViewModelEvent::ProgressUpdated
                | AmbientAgentViewModelEvent::EnvironmentSelected
                | AmbientAgentViewModelEvent::Failed { .. }
                | AmbientAgentViewModelEvent::ShowCloudAgentCapacityModal
                | AmbientAgentViewModelEvent::ShowAICreditModal
                | AmbientAgentViewModelEvent::NeedsGithubAuth
                | AmbientAgentViewModelEvent::Cancelled
                | AmbientAgentViewModelEvent::HarnessSelected
                | AmbientAgentViewModelEvent::ViewerHarnessResolved
                | AmbientAgentViewModelEvent::HostSelected
                | AmbientAgentViewModelEvent::HarnessModelSelected
                | AmbientAgentViewModelEvent::HarnessCommandStarted { .. }
                | AmbientAgentViewModelEvent::UpdatedSetupCommandVisibility
                | AmbientAgentViewModelEvent::AuthSecretSelected
                | AmbientAgentViewModelEvent::RunLifecycleChanged
                | AmbientAgentViewModelEvent::FollowupSubmissionFailed { .. } => {}
            }
        });
    });
}

/// Returns `true` when a cloud agent shared session is in any pre-first-exchange phase —
/// either still spawning (loading screen) or running setup commands before the first
/// agent turn. In this state, we hide the interactive input and render a loading footer.
pub fn is_cloud_agent_pre_first_exchange(
    ambient_agent_view_model: Option<&ModelHandle<AmbientAgentViewModel>>,
    agent_view_controller: &ModelHandle<AgentViewController>,
    terminal_model: &TerminalModel,
    app: &AppContext,
) -> bool {
    if !(FeatureFlag::CloudMode.is_enabled() && FeatureFlag::AgentView.is_enabled()) {
        return false;
    }

    let Some(ambient_agent_view_model) = ambient_agent_view_model else {
        return false;
    };

    let view_model = ambient_agent_view_model.as_ref(app);

    let is_in_pre_first_exchange_status = matches!(
        view_model.status(),
        Status::WaitingForSession { .. } | Status::AgentRunning
    );
    if !is_in_pre_first_exchange_status {
        return false;
    }

    let agent_view_state = agent_view_controller.as_ref(app).agent_view_state().clone();
    let AgentViewState::Active { origin, .. } = agent_view_state else {
        return false;
    };

    // Shared-session viewers enter through SharedSessionSelection / ThirdPartyCloudAgent.
    // Keep cloud follow-up setup progress and prompt queuing active for these viewers.
    if !origin.is_cloud_agent() && !terminal_model.is_shared_ambient_agent_session() {
        return false;
    }

    // For non-oz harness runs, there is no Oz `AppendedExchange` to key off of, so we also
    // exit the pre-first-exchange phase when the harness CLI (e.g. `claude`, `gemini`) has
    // been detected. See `mark_harness_command_started`.
    if view_model.harness_command_started() {
        return false;
    }

    // Loading phase (`WaitingForSession`): no setup commands have started yet, but we're
    // still pre-first-exchange. Skip the block-list flag check.
    if matches!(view_model.status(), Status::WaitingForSession { .. }) {
        return true;
    }

    terminal_model
        .block_list()
        .is_executing_oz_environment_startup_commands()
}
