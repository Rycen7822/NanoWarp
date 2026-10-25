use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use parking_lot::FairMutex;
use session_sharing_protocol::common::{
    ActivePrompt, CommandExecutionFailureReason, WriteToPtyFailureReason,
};
use session_sharing_protocol::sharer::{
    AddGuestsResponse, FailedToInitializeSessionReason, Lifetime, LinkAccessLevelUpdateResponse,
    QuotaType, RemoveGuestResponse, SessionEndedReason, SessionSourceType,
    TeamAccessLevelUpdateResponse, UpdatePendingUserRoleResponse,
};
use warp_errors::report_error;
use warpui::{AppContext, ModelContext, ModelHandle, SingletonEntity, ViewHandle, WindowId};

use super::terminal_manager::{TerminalManager, TerminalSurfaceInit, TerminalSurfaceResult};
use crate::NetworkStatus;
use crate::context_chips::current_prompt::CurrentPrompt;
use crate::context_chips::prompt_snapshot::PromptSnapshot;
use crate::context_chips::prompt_type::PromptType;
use crate::editor::CrdtOperation;
use crate::features::FeatureFlag;
use crate::network::{NetworkStatusEvent, NetworkStatusKind};
use crate::pane_group::TerminalViewResources;
use crate::persistence::ModelEvent;
use crate::terminal::safe_mode_settings::get_secret_obfuscation_mode;
use crate::terminal::session_settings::{SessionSettings, SessionSettingsChangedEvent};
use crate::terminal::shared_session::manager::Manager;
use crate::terminal::shared_session::permissions_manager::SessionPermissionsManager;
use crate::terminal::shared_session::presence_manager::PresenceManager;
use crate::terminal::shared_session::sharer::network::{
    Network, NetworkEvent, failed_to_add_guests_user_error,
    failed_to_initialize_session_user_error, session_terminated_reason_string,
};
use crate::terminal::shared_session::{
    SharedSessionActionSource, SharedSessionScrollbackType, SharedSessionStatus, max_session_size,
};
use crate::terminal::view::Event as TerminalViewEvent;
use crate::terminal::writeable_pty::terminal_manager_util::wire_up_remote_server_controller_with_view;
use crate::terminal::{TerminalManager as TerminalManagerTrait, TerminalModel, TerminalView};
use crate::view_components::ToastFlavor;
#[cfg(not(any(test, feature = "integration_tests")))]
use crate::workspaces::user_workspaces::{ResolvedTeamScope, TeamScope, UserWorkspaces};

const ACL_UPDATE_FAILURE_RESPONSE: &str = "Something went wrong. Please try again.";

/// Configuration for constructing the GUI terminal surface.
pub(crate) struct TerminalViewSurfaceConfig {
    pub(crate) resources: TerminalViewResources,
    pub(crate) model_event_sender: Option<SyncSender<ModelEvent>>,
    pub(crate) window_id: WindowId,
    pub(crate) has_restored_command_blocks: bool,
}

/// Creates the GUI terminal surface and its manager-owned post-wiring closure.
pub(crate) fn create_terminal_view_surface(
    config: TerminalViewSurfaceConfig,
    surface_init: TerminalSurfaceInit,
    ctx: &mut AppContext,
) -> TerminalSurfaceResult<
    TerminalView,
    impl FnOnce(&mut TerminalManager<TerminalView>, &ViewHandle<TerminalView>, &mut AppContext) + use<>,
> {
    let TerminalSurfaceInit {
        wakeups_rx,
        model_events,
        model,
        sessions,
        size_info,
        colors,
        inactive_pty_reads_rx,
    } = surface_init;
    let TerminalViewSurfaceConfig {
        resources,
        model_event_sender,
        window_id,
        has_restored_command_blocks,
    } = config;
    let current_prompt = ctx.add_model(|ctx| {
        CurrentPrompt::new_with_model_events(
            sessions.clone(),
            Some((&model_events, model.clone())),
            ctx,
        )
    });
    let prompt_type = ctx.add_model(|ctx| PromptType::new_dynamic(current_prompt.clone(), ctx));
    let view = ctx.add_typed_action_view(window_id, |ctx| {
        TerminalView::new(
            resources,
            wakeups_rx,
            model_events,
            model,
            sessions,
            size_info,
            colors,
            model_event_sender,
            prompt_type.clone(),
            Some(inactive_pty_reads_rx),
            ctx,
        )
    });

    TerminalSurfaceResult {
        surface: view,
        post_wire: move |terminal_manager: &mut TerminalManager<TerminalView>,
                         view: &ViewHandle<TerminalView>,
                         ctx: &mut AppContext| {
            // Append the session restoration separator when command blocks were restored.
            if has_restored_command_blocks {
                terminal_manager
                    .model()
                    .lock()
                    .block_list_mut()
                    .append_session_restoration_separator_to_block_list();
            }

            // In unit tests, we know we aren't going to bootstrap a shell
            // so if we're waiting on starting a shared session until bootstrapped,
            // just attempt to start it now.
            #[cfg(test)]
            if matches!(
                terminal_manager.model().lock().shared_session_status(),
                SharedSessionStatus::SharePendingPreBootstrap { .. }
            ) {
                view.update(ctx, |view, ctx| {
                    view.attempt_to_share_session(
                        SharedSessionScrollbackType::All,
                        None,
                        SessionSourceType::User,
                        ctx,
                    )
                });
            }

            wire_up_remote_server_controller_with_view(
                &terminal_manager.remote_server_controller(),
                view,
                ctx,
            );

            // Wire up TerminalView-specific session sharing (sharer setup, prompt/presence/
            // input-mode/conversation broadcasts, agent-view registration, network status).
            terminal_manager.session_sharer = wire_up_terminal_view_session_sharing(
                view,
                current_prompt,
                prompt_type,
                terminal_manager.model(),
                window_id,
                ctx,
            );
        },
    }
}

/// Wires up `TerminalView`-specific session sharing: the local sharer (`Network`),
/// prompt/presence broadcasts and
/// network-status reconnection handling. Returns the sharer cell the manager stores.
///
/// This is the GUI session-sharing boundary. The reusable protocol/model pieces
/// (`shared_session::sharer::Network`, ordered terminal event flow, shared handlers)
/// remain unchanged; this helper groups the `TerminalView`-dependent wiring so it is
/// easy to identify and work on separately from the generic manager.
#[allow(clippy::too_many_arguments)]
fn wire_up_terminal_view_session_sharing(
    view: &ViewHandle<TerminalView>,
    current_prompt: ModelHandle<CurrentPrompt>,
    prompt_type: ModelHandle<PromptType>,
    model: Arc<FairMutex<TerminalModel>>,
    window_id: WindowId,
    ctx: &mut AppContext,
) -> Rc<RefCell<Option<ModelHandle<Network>>>> {
    let session_sharer: Rc<RefCell<Option<ModelHandle<Network>>>> = Rc::new(RefCell::new(None));
    let session_sharer_clone = session_sharer.clone();

    // Send warp prompt updates.
    ctx.observe_model(&current_prompt, move |current_prompt, ctx| {
        // If for some reason ctx.notify() was called on the warp prompt but we're using ps1, do nothing.
        if *SessionSettings::as_ref(ctx).honor_ps1 {
            return
        }
        let prompt_snapshot = current_prompt.read(ctx, |current_prompt, ctx| {
            PromptSnapshot::from_current_prompt(current_prompt, ctx)
        });
        if let Some(network) = session_sharer_clone.borrow().as_ref() {
            let Ok(serialized_prompt) = serde_json::to_string(&prompt_snapshot) else {
                report_error!("Failed to serialize prompt snapshot to send active prompt update to shared session server");
                return
            };
            network.update(ctx, |network, _| {
                network.send_active_prompt_update_if_changed(session_sharing_protocol::common::ActivePrompt::WarpPrompt(serialized_prompt))
            });
        }
    });

    let session_sharer_clone = session_sharer.clone();
    ctx.subscribe_to_model(&SessionSettings::handle(ctx), move |_, event, ctx| {
        if let SessionSettingsChangedEvent::HonorPS1 { .. } = event {
            if !*SessionSettings::as_ref(ctx).honor_ps1 {
                // We don't need to send a WarpPrompt message here when turning off PS1 because this will be sent
                // as part of observing the warp prompt and sending messages on updates.
                return;
            }
            if let Some(network) = session_sharer_clone.borrow().as_ref() {
                network.update(ctx, |network, _| {
                    network.send_active_prompt_update_if_changed(
                        session_sharing_protocol::common::ActivePrompt::PS1,
                    )
                });
            }
        }
    });

    // Always wire up the model but check the flag when a share is attempted.
    TerminalManager::<TerminalView>::wire_up_session_sharer_with_view(
        view,
        prompt_type,
        session_sharer.clone(),
        model.clone(),
        window_id,
        ctx,
    );

    TerminalManager::<TerminalView>::handle_network_status_events(
        view,
        session_sharer.clone(),
        ctx,
    );

    session_sharer
}

impl TerminalManager<TerminalView> {
    #[allow(clippy::too_many_arguments)]
    fn start_sharing_session(
        terminal_view: ViewHandle<TerminalView>,
        prompt_type: ModelHandle<PromptType>,
        shared_session_model: Rc<RefCell<Option<ModelHandle<Network>>>>,
        scrollback_type: SharedSessionScrollbackType,
        lifetime: Lifetime,
        source: SessionSourceType,
        model: Arc<FairMutex<TerminalModel>>,
        window_id: WindowId,
        ctx: &mut AppContext,
    ) {
        let mut session_sharer = shared_session_model.borrow_mut();

        // If it's already being shared, then this should no-op.
        // In practice, this event shouldn't even be emitted if that's the case.
        if session_sharer.is_some() {
            log::warn!("Tried to share a session that's already being shared.");
            return;
        }
        if matches!(source, SessionSourceType::AmbientAgent { .. }) {
            model
                .lock()
                .set_shared_session_status(SharedSessionStatus::NotShared);
            Manager::handle(ctx).update(ctx, |manager, ctx| {
                manager.share_failed(window_id, ctx);
            });
            terminal_view.update(ctx, |view, ctx| {
                let reason = "Cloud agent sessions are no longer supported.".to_owned();
                view.notify_shared_session_link_changed(ctx);
                view.show_persistent_toast(reason.clone(), ToastFlavor::Error, ctx);
                ctx.emit(TerminalViewEvent::FailedToShareSession {
                    reason,
                    cause: None,
                });
            });
            return;
        }
        log::info!("Starting shared session");

        // Record the source on the model so we can distinguish ambient agent
        // sessions from user-initiated shared sessions in the UI logic, and so
        // the orchestrator task id is discoverable regardless of which variant
        // the share is.
        model.lock().set_shared_session_source(source.clone());
        Self::log_shared_session_lifecycle(
            &terminal_view,
            &model,
            "start_requested",
            "trigger=terminal_view_start_sharing",
            ctx,
        );
        let active_prompt = if *SessionSettings::as_ref(ctx).honor_ps1 {
            ActivePrompt::PS1
        } else {
            let current_prompt_snapshot = prompt_type.as_ref(ctx).snapshot(ctx);
            let Ok(serialized_prompt) = serde_json::to_string(&current_prompt_snapshot) else {
                report_error!(
                    "Failed to serialize prompt snapshot to send active prompt update to shared session server"
                );
                return;
            };
            ActivePrompt::WarpPrompt(serialized_prompt)
        };

        let selection = terminal_view.read(ctx, |view, ctx| {
            view.get_shared_session_presence_selection(ctx)
        });

        let (events_tx, events_rx) = async_channel::unbounded();

        let scrollback_first_block_index = scrollback_type.first_block_index(&model.lock());
        let max_session_size = max_session_size(window_id, ctx);

        // TODO: rather than picking which constructor we use here,
        // we might want to use a dedicated terminal manager for tests.
        cfg_if::cfg_if! {
            if #[cfg(any(test, feature = "integration_tests"))] {
                let _ = lifetime;
                let network = ctx.add_model(|ctx| Network::new_for_test(
                    model.clone(),
                    events_rx,
                    active_prompt,
                    selection,
                    max_session_size,
                    ctx,
                ));
            } else {
                let input_replica_id = terminal_view
                    .as_ref(ctx)
                    .input()
                    .as_ref(ctx)
                    .editor()
                    .as_ref(ctx)
                    .replica_id(ctx);
                let team_uid = ResolvedTeamScope::from_scope(
                    &UserWorkspaces::as_ref(ctx).team_context_for_window(window_id),
                )
                .team_uid();
                let network = ctx.add_model(|ctx| {
                    Network::new(
                        model.clone(),
                        events_rx,
                        scrollback_type,
                        active_prompt,
                        selection,
                        input_replica_id,
                        team_uid,
                        lifetime,
                        source.clone(),
                        max_session_size,
                        ctx,
                    )
                });
            }
        }

        // Secret redaction relies on a lookback, so it can't work with
        // real-time session sharing.
        model
            .lock()
            .disable_secret_obfuscation_for_shared_sesson_creator(scrollback_first_block_index);

        // Set the event sender on the model for ordered terminal events.
        model
            .lock()
            .set_ordered_terminal_events_for_shared_session_tx(events_tx);

        let shared_session_model_clone = shared_session_model.clone();
        ctx.subscribe_to_model(&network, move |network, event, ctx| match event {
            NetworkEvent::SharedSessionCreatedSuccessfully {
                session_id,
                sharer_id,
                sharer_firebase_uid,
            } => {
                // Change the status of the session to reflect that the share is now active.
                model
                    .lock()
                    .set_shared_session_status(SharedSessionStatus::ActiveSharer);

                // Let the terminal view know the share is active so it can reflect that in its view.
                terminal_view.update(ctx, |view, ctx| {
                    view.on_session_share_started(
                        sharer_id.clone(),
                        *sharer_firebase_uid,
                        scrollback_type,
                        *session_id,
                        ctx,
                    );
                });
                Self::log_shared_session_lifecycle(
                    &terminal_view,
                    &model,
                    "session_established",
                    "outcome=active_sharer",
                    ctx,
                );

                // Let the manager know the share is active with the relevant metadata.
                Manager::handle(ctx).update(ctx, |manager, ctx| {
                    manager.started_share(terminal_view.downgrade(), *session_id, window_id, ctx);
                });

                // Flush the initial input operations that the sharer performed
                // in the latest buffer before the share was started.
                let init_input_ops: Vec<CrdtOperation> = terminal_view
                    .as_ref(ctx)
                    .input()
                    .as_ref(ctx)
                    .latest_buffer_operations()
                    .cloned()
                    .collect();
                if !init_input_ops.is_empty() {
                    network.update(ctx, |network, _ctx| {
                        network.send_input_update(
                            model.lock().block_list().active_block_id(),
                            init_input_ops.iter(),
                        );
                    });
                }
            }
            NetworkEvent::FailedToCreateSharedSession { reason, cause } => {
                log::warn!("Failed to create shared session: reason={reason:?}, cause={cause:?}");

                model
                    .lock()
                    .set_shared_session_status(SharedSessionStatus::NotShared);

                Manager::handle(ctx).update(ctx, |manager, ctx| {
                    manager.share_failed(window_id, ctx);
                });

                terminal_view.update(ctx, |view, ctx| {
                    view.notify_shared_session_link_changed(ctx);
                    let reason_string = failed_to_initialize_session_user_error(reason);

                    if matches!(
                        reason,
                        FailedToInitializeSessionReason::NoUserQuotaRemaining {
                            quota_type: QuotaType::SessionsCreated
                        }
                    ) {
                        view.open_share_session_denied_modal(ctx);
                    } else {
                        view.show_persistent_toast(reason_string.clone(), ToastFlavor::Error, ctx);
                    }

                    ctx.emit(TerminalViewEvent::FailedToShareSession {
                        reason: reason_string,
                        cause: cause.clone(),
                    });
                });

                // Drop the network so we can create a new one when trying again.
                shared_session_model_clone.borrow_mut().take();
            }
            NetworkEvent::SessionTerminated { reason } => {
                Self::shared_session_terminated(
                    &terminal_view,
                    shared_session_model_clone.clone(),
                    model.clone(),
                    ctx,
                );

                let max_session_size = network.as_ref(ctx).max_session_size();
                terminal_view.update(ctx, |view, ctx| {
                    let reason_string = session_terminated_reason_string(reason, max_session_size);
                    view.show_persistent_toast(reason_string, ToastFlavor::Error, ctx);
                });
            }
            NetworkEvent::Reconnecting => {
                // TODO(roland): add some limiting in a time frame to avoid possible infinite retry in this case:
                // Server disconnects
                // ---- begin loop
                // We reconnect here, and it's successful
                // The server immediately replies with a retryable error, or terminates the connection unexpectedly
                // We emit an event and attempt to reconnect immediately
                // ---- end loop
                terminal_view.update(ctx, |view, ctx| {
                    view.on_shared_session_reconnection_status_changed(true, ctx)
                });
            }
            NetworkEvent::ReconnectedSuccessfully => {
                terminal_view.update(ctx, |view, ctx| {
                    view.on_shared_session_reconnection_status_changed(false, ctx)
                });
            }
            NetworkEvent::FailedToReconnect => {
                Self::shared_session_terminated(
                    &terminal_view,
                    shared_session_model_clone.clone(),
                    model.clone(),
                    ctx,
                );

                terminal_view.update(ctx, |view, ctx| {
                    view.show_persistent_toast(
                        "Something went wrong. Please try sharing again.".to_string(),
                        ToastFlavor::Error,
                        ctx,
                    );
                });
            }
            NetworkEvent::ReconnectLimitReached { reason } => {
                Self::shared_session_terminated(
                    &terminal_view,
                    shared_session_model_clone.clone(),
                    model.clone(),
                    ctx,
                );

                terminal_view.update(ctx, |view, ctx| {
                    view.show_persistent_toast(reason.to_string(), ToastFlavor::Error, ctx);
                    ctx.emit(TerminalViewEvent::SharedSessionFailed {
                        reason: reason.to_string(),
                    });
                });
            }
            NetworkEvent::ParticipantListUpdated(participant_list) => {
                let was_viewer_driven_sizing_eligible = terminal_view.update(ctx, |view, ctx| {
                    view.is_viewer_driven_sizing_eligible(true, ctx)
                });

                if let Some(presence_manager) =
                    terminal_view.as_ref(ctx).shared_session_presence_manager()
                {
                    presence_manager.update(ctx, |presence_manager, ctx| {
                        presence_manager.update_participants(*participant_list.clone(), ctx)
                    });
                }

                // Check eligibility from the incoming participant list directly,
                // since the presence manager processes new viewers asynchronously.
                if was_viewer_driven_sizing_eligible {
                    let sharer_uid = participant_list
                        .sharer
                        .info
                        .profile_data
                        .firebase_uid
                        .as_str();
                    let still_eligible =
                        PresenceManager::single_distinct_present_viewer_uid_from_viewers(
                            participant_list.viewers.iter(),
                        )
                        .is_some_and(|viewer_uid| viewer_uid == sharer_uid);
                    if !still_eligible {
                        terminal_view.update(ctx, |view, ctx| {
                            view.restore_pty_to_sharer_size(ctx);
                        });
                    }
                }

                if let Some(session_id) = terminal_view.as_ref(ctx).shared_session_id().cloned() {
                    SessionPermissionsManager::handle(ctx).update(
                        ctx,
                        |permissions_manager, ctx| {
                            permissions_manager.updated_guests(
                                ctx,
                                session_id,
                                participant_list.guests.clone(),
                                participant_list.pending_guests.clone(),
                            );
                        },
                    );
                }
            }
            NetworkEvent::ParticipantPresenceUpdated(update) => {
                terminal_view.update(ctx, |view, ctx| {
                    view.on_participant_presence_updated(update, ctx);
                });
            }
            NetworkEvent::RoleRequested {
                participant_id,
                role_request_id,
                role,
            } => {
                terminal_view.update(ctx, |view, ctx| {
                    view.on_role_requested(
                        participant_id.clone(),
                        role_request_id.clone(),
                        *role,
                        ctx,
                    );
                });
            }
            NetworkEvent::RoleRequestCancelled {
                participant_id,
                role_request_id,
            } => {
                terminal_view.update(ctx, |view, ctx| {
                    view.on_role_request_cancelled(
                        participant_id.clone(),
                        role_request_id.clone(),
                        ctx,
                    );
                });
            }
            NetworkEvent::ParticipantRoleChanged {
                participant_id,
                role,
            } => {
                terminal_view.update(ctx, |view, ctx| {
                    view.on_participant_role_changed(participant_id, *role, ctx);
                });
            }
            NetworkEvent::InputUpdated {
                block_id,
                operations,
            } => {
                // For the sharer, we're always up to speed so if this block ID
                // is not the latest, then it's an old block ID and we don't need
                // these operations.
                if model.lock().block_list().active_block_id() != block_id {
                    return;
                }

                terminal_view.update(ctx, |view, ctx| {
                    view.input().update(ctx, |input, ctx| {
                        input.process_remote_edits(block_id, operations.clone(), ctx);
                    });
                });
            }
            NetworkEvent::CommandExecutionRequested {
                id,
                participant_id,
                block_id,
                command,
            } => {
                let (is_block_id_latest, is_currently_long_running) = {
                    let model = model.lock();
                    let active_block = model.block_list().active_block();
                    (
                        active_block.id() == block_id,
                        active_block.is_active_and_long_running(),
                    )
                };

                // If the viewer is trying to execute for an old block ID (they can never be ahead)
                // or the active block is long running, we need to reject this request.
                if !is_block_id_latest || is_currently_long_running {
                    network.update(ctx, |network, _ctx| {
                        network.send_command_execution_rejection(
                            id.clone(),
                            participant_id.clone(),
                            CommandExecutionFailureReason::StaleBuffer,
                        );
                    });
                    return;
                }

                // If the viewer is no longer an executor, we need to reject the request.
                let Some(viewer_role) = terminal_view
                    .as_ref(ctx)
                    .shared_session_presence_manager()
                    .and_then(|manager| manager.as_ref(ctx).viewer_role(participant_id))
                else {
                    log::warn!("Failed to get viewer's role during command");
                    return;
                };
                if !viewer_role.can_execute() {
                    network.update(ctx, |network, _ctx| {
                        network.send_command_execution_rejection(
                            id.clone(),
                            participant_id.clone(),
                            CommandExecutionFailureReason::InsufficientPermissions,
                        );
                    });
                    return;
                }

                terminal_view.update(ctx, |view, ctx| {
                    view.input().update(ctx, |input, ctx| {
                        input.try_execute_command_on_behalf_of_shared_session_participant(
                            command,
                            participant_id.clone(),
                            ctx,
                        );
                    });
                });
            }
            NetworkEvent::WriteToPtyRequested { id, bytes } => {
                if !FeatureFlag::SharedSessionWriteToLongRunningCommands.is_enabled() {
                    return;
                }

                let is_currently_long_running = {
                    let model = model.lock();
                    model
                        .block_list()
                        .active_block()
                        .is_active_and_long_running()
                };
                if !is_currently_long_running {
                    network.update(ctx, |network, _ctx| {
                        network.send_write_to_pty_rejection(
                            id.clone(),
                            WriteToPtyFailureReason::StaleBuffer,
                        );
                    });
                    return;
                }

                // If the viewer is no longer an executor, we need to reject the request.
                let Some(viewer_role) = terminal_view
                    .as_ref(ctx)
                    .shared_session_presence_manager()
                    .and_then(|manager| manager.as_ref(ctx).viewer_role(&id.participant_id))
                else {
                    log::warn!("Failed to get viewer's role during write to pty requested");
                    return;
                };
                if !viewer_role.can_execute() {
                    network.update(ctx, |network, _ctx| {
                        network.send_write_to_pty_rejection(
                            id.clone(),
                            WriteToPtyFailureReason::InsufficientPermissions,
                        );
                    });
                    return;
                }

                terminal_view.update(ctx, |view, ctx| {
                    view.write_viewer_bytes_to_pty(bytes.clone(), ctx);
                });
            }
            NetworkEvent::LinkAccessLevelUpdateResponse { response } => {
                terminal_view.update(ctx, |view, ctx| match response {
                    LinkAccessLevelUpdateResponse::Ok { role } => {
                        let Some(session_id) = view.shared_session_id() else {
                            return;
                        };
                        SessionPermissionsManager::handle(ctx).update(
                            ctx,
                            |permissions_manager, ctx| {
                                permissions_manager.updated_link_permissions(
                                    *session_id,
                                    *role,
                                    ctx,
                                );
                            },
                        );
                    }
                    LinkAccessLevelUpdateResponse::Error => {
                        let reason_string =
                            "Failed to update permissions for shared session".to_owned();
                        view.show_persistent_toast(reason_string, ToastFlavor::Error, ctx);
                    }
                });
            }
            NetworkEvent::TeamAccessLevelUpdateResponse { response } => {
                terminal_view.update(ctx, |view, ctx| match response {
                    TeamAccessLevelUpdateResponse::Success { team_acl, .. } => {
                        let Some(session_id) = view.shared_session_id() else {
                            return;
                        };
                        SessionPermissionsManager::handle(ctx).update(
                            ctx,
                            |permissions_manager, ctx| {
                                permissions_manager.updated_team_permissions(
                                    *session_id,
                                    team_acl.clone(),
                                    ctx,
                                );
                            },
                        );
                    }
                    TeamAccessLevelUpdateResponse::Error(_) => {
                        view.show_persistent_toast(
                            ACL_UPDATE_FAILURE_RESPONSE.to_owned(),
                            crate::view_components::ToastFlavor::Error,
                            ctx,
                        );
                    }
                });
            }
            NetworkEvent::AddGuestsResponse { response } => {
                if let AddGuestsResponse::Error(reason) = response {
                    terminal_view.update(ctx, |view, ctx| {
                        let reason_string = failed_to_add_guests_user_error(reason);
                        view.show_persistent_toast(reason_string, ToastFlavor::Error, ctx);
                    });
                }
            }
            NetworkEvent::RemoveGuestResponse { response } => {
                if let RemoveGuestResponse::Error(_) = response {
                    terminal_view.update(ctx, |view, ctx| {
                        view.show_persistent_toast(
                            ACL_UPDATE_FAILURE_RESPONSE.to_owned(),
                            crate::view_components::ToastFlavor::Error,
                            ctx,
                        );
                    });
                }
            }
            NetworkEvent::UpdatePendingUserRoleResponse { response } => {
                if let UpdatePendingUserRoleResponse::Error(_) = response {
                    terminal_view.update(ctx, |view, ctx| {
                        view.show_persistent_toast(
                            ACL_UPDATE_FAILURE_RESPONSE.to_owned(),
                            crate::view_components::ToastFlavor::Error,
                            ctx,
                        );
                    });
                }
            }
            NetworkEvent::ViewerTerminalSizeReported { window_size } => {
                if !*SharedSessionSettings::as_ref(ctx).viewer_driven_sizing_enabled {
                    return;
                }
                let eligible = terminal_view.update(ctx, |view, ctx| {
                    view.is_viewer_driven_sizing_eligible(true, ctx)
                });
                if eligible {
                    terminal_view.update(ctx, |view, ctx| {
                        view.resize_from_viewer_report(*window_size, ctx);
                    });
                }
            }
        });

        *session_sharer = Some(network);
    }

    fn log_shared_session_lifecycle(
        terminal_view: &ViewHandle<TerminalView>,
        model: &Arc<FairMutex<TerminalModel>>,
        event: &'static str,
        details: impl std::fmt::Display,
        ctx: &AppContext,
    ) {
        let session_id = terminal_view.as_ref(ctx).shared_session_id().cloned();
        let source_type = match model.lock().shared_session_source() {
            Some(SessionSourceType::User) => "user",
            Some(SessionSourceType::AmbientAgent { .. }) => "ambient_agent",
            None => "unknown",
        };
        log::info!(
            "Shared session local lifecycle: event={event} session_id={session_id:?} source_type={source_type} {details}"
        );
    }

    /// Contains necessary logic for stopping the current shared session.
    fn cleanup_shared_session(
        terminal_view: &ViewHandle<TerminalView>,
        model: Arc<FairMutex<TerminalModel>>,
        ctx: &mut AppContext,
    ) {
        let mut model_lock = model.lock();
        if !model_lock.shared_session_status().is_sharer() {
            log::warn!("Attempted to stop sharing current session that is not being shared");
            return;
        }

        // Change the status of the session to unshared.
        model_lock.set_shared_session_status(SharedSessionStatus::NotShared);
        model_lock.set_obfuscate_secrets(get_secret_obfuscation_mode(ctx));
        model_lock.clear_ordered_terminal_events_for_shared_session_tx();

        // Drop the lock so that it can be taken by the other entities that
        // need to do cleanup.
        drop(model_lock);

        // Let the manager know we've stopped sharing.
        Manager::handle(ctx).update(ctx, |manager, ctx| {
            manager.stopped_share(terminal_view.id(), ctx);
        });

        terminal_view.update(ctx, |view, ctx| {
            view.on_session_share_ended(ctx);
        });
    }

    /// Called when the server terminates the current session.
    fn shared_session_terminated(
        terminal_view: &ViewHandle<TerminalView>,
        session_sharer: Rc<RefCell<Option<ModelHandle<Network>>>>,
        model: Arc<FairMutex<TerminalModel>>,
        ctx: &mut AppContext,
    ) {
        Self::cleanup_shared_session(terminal_view, model, ctx);
        // Drop the ModelHandle<Network> and set session_sharer to None.
        session_sharer.borrow_mut().take();
    }

    /// Called when the client explicitly wants to end the current session.
    /// Guarantees we also notify viewers of a session ended reason.
    fn end_shared_session(
        terminal_view: &ViewHandle<TerminalView>,
        session_sharer: Rc<RefCell<Option<ModelHandle<Network>>>>,
        reason: SessionEndedReason,
        model: Arc<FairMutex<TerminalModel>>,
        ctx: &mut AppContext,
    ) {
        Self::log_shared_session_lifecycle(
            terminal_view,
            &model,
            "end_requested",
            format_args!("reason={reason:?}"),
            ctx,
        );
        Self::cleanup_shared_session(terminal_view, model, ctx);

        // Drop the ModelHandle<Network> and set session_sharer to None.
        if let Some(network_handle) = session_sharer.borrow_mut().take() {
            // Dropping the ModelHandle<Network> may not necessarily drop the Network within if there are other references to it, so we explicitly close the websocket just in case.
            // We also notify viewers with the given reason.
            network_handle.update(ctx, |network, _| network.end_session(reason));
        }
    }

    fn wire_up_session_sharer_with_view(
        terminal_view: &ViewHandle<TerminalView>,
        prompt_type: ModelHandle<PromptType>,
        shared_session_model: Rc<RefCell<Option<ModelHandle<Network>>>>,
        model: Arc<FairMutex<TerminalModel>>,
        window_id: WindowId,
        ctx: &mut AppContext,
    ) {
        let session_sharer = shared_session_model.clone();
        let model = model.clone();

        ctx.subscribe_to_view(terminal_view, move |view, event, ctx| match event {
            TerminalViewEvent::StartSharingCurrentSession {
                scrollback_type,
                source,
            } if FeatureFlag::CreatingSharedSessions.is_enabled() => {
                Self::start_sharing_session(
                    view.clone(),
                    prompt_type.clone(),
                    session_sharer.clone(),
                    *scrollback_type,
                    Lifetime::Ephemeral,
                    source.clone(),
                    model.clone(),
                    window_id,
                    ctx,
                );
            }
            TerminalViewEvent::StartSharingCurrentSession { .. } => {
                log::warn!(
                    "Ignoring request to start sharing current session because \
                     CreatingSharedSessions is disabled"
                );
            }
            TerminalViewEvent::StopSharingCurrentSession { reason } => {
                Self::end_shared_session(&view, session_sharer.clone(), *reason, model.clone(), ctx)
            }
            TerminalViewEvent::SelectedBlocksChanged | TerminalViewEvent::SelectedTextChanged => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    let selection = view.read(ctx, |view, ctx| {
                        view.get_shared_session_presence_selection(ctx)
                    });
                    network.update(ctx, |network, _| {
                        network.send_presence_selection_if_changed(selection);
                    });
                }
            }
            TerminalViewEvent::UpdateRole {
                participant_id,
                role,
            } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_role_update(participant_id.clone(), *role);
                    });
                }
            }
            TerminalViewEvent::UpdateUserRole { user_uid, role } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_user_role_update(*user_uid, *role);
                    });
                }
            }
            TerminalViewEvent::UpdatePendingUserRole { email, role } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_pending_user_role_update(email.clone(), *role);
                    });
                }
            }
            TerminalViewEvent::AddGuests { emails, role } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_add_guests(emails.clone(), *role);
                    });
                }
            }
            TerminalViewEvent::RemoveGuest { user_uid } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_remove_guest(*user_uid);
                    });
                }
            }
            TerminalViewEvent::RemovePendingGuest { email } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_remove_pending_guest(email.clone());
                    });
                }
            }
            TerminalViewEvent::MakeAllParticipantsReaders { reason } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_make_all_participants_readers(*reason);
                    });
                }
            }
            TerminalViewEvent::RespondToRoleRequest {
                participant_id,
                role_request_id,
                response,
            } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_role_request_response(
                            participant_id.clone(),
                            role_request_id.clone(),
                            response.clone(),
                        );
                    });
                }
            }
            TerminalViewEvent::InputEditorUpdated {
                block_id,
                operations,
            } => {
                // If the block ID has become stale by the time we get here,
                // we don't need to send this update to the server.
                if model.lock().block_list().active_block_id() != block_id {
                    return;
                }

                let filtered: Vec<_> = operations.iter().collect();
                if filtered.is_empty() {
                    return;
                }
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_input_update(block_id, filtered.into_iter());
                    });
                }
            }
            TerminalViewEvent::UpdateSessionLinkPermissions { role } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_link_permission_update(*role);
                    });
                }
            }
            TerminalViewEvent::UpdateSessionTeamPermissions { role, team_uid } => {
                if let Some(network) = session_sharer.borrow().as_ref() {
                    network.update(ctx, |network, _| {
                        network.send_team_permission_update(*role, team_uid.clone());
                    });
                }
            }
            _ => (),
        });
    }

    fn handle_network_status_events(
        view: &ViewHandle<TerminalView>,
        session_sharer: Rc<RefCell<Option<ModelHandle<Network>>>>,
        ctx: &mut AppContext,
    ) {
        let weak_view_handle = view.downgrade();
        let network_status = NetworkStatus::handle(ctx);

        ctx.subscribe_to_model(&network_status, move |_, event, ctx| {
            let binding = session_sharer.borrow();
            let Some(network) = binding.as_ref() else {
                return;
            };
            let Some(view) = weak_view_handle.upgrade(ctx) else {
                return;
            };
            let NetworkStatusEvent::NetworkStatusChanged { new_status } = event;
            match new_status {
                NetworkStatusKind::Online => {
                    if network.as_ref(ctx).is_connected() {
                        view.update(ctx, |view, ctx| {
                            view.on_shared_session_reconnection_status_changed(false, ctx)
                        });
                    }
                }
                NetworkStatusKind::Offline => {
                    view.update(ctx, |view, ctx| {
                        view.on_shared_session_reconnection_status_changed(true, ctx)
                    });
                }
            }
        });
    }

    #[cfg(test)]
    pub fn session_sharer(&self) -> Rc<RefCell<Option<ModelHandle<Network>>>> {
        self.session_sharer.clone()
    }

    /// Returns the PTY process id, for integration tests.
    #[cfg(feature = "integration_tests")]
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl TerminalManagerTrait for TerminalManager<TerminalView> {
    fn model(&self) -> Arc<FairMutex<TerminalModel>> {
        self.model.clone()
    }
    fn replace_shell(
        &mut self,
        request: crate::terminal::shell_recovery::ShellReplacementRequest,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) -> anyhow::Result<()> {
        TerminalManager::replace_shell(self, request, ctx)
    }

    fn on_session_bootstrapped(
        &mut self,
        session_id: crate::terminal::model::session::SessionId,
        ctx: &mut ModelContext<Box<dyn TerminalManagerTrait>>,
    ) {
        TerminalManager::on_session_bootstrapped(self, session_id, ctx);
    }

    fn on_view_detached(
        &self,
        // The detach type is intentionally ignored: a sharer always stops sharing immediately,
        // even on a reversible `HiddenForClose` detach. This is desirable for security — a sharer
        // should not continue accepting commands from viewers while the session is not visible.
        detach_type: crate::pane_group::pane::DetachType,
        app: &mut AppContext,
    ) {
        let shared_session_status = self.model.lock().shared_session_status().clone();
        if shared_session_status.is_sharer() {
            Self::log_shared_session_lifecycle(
                &self.view,
                &self.model,
                "view_detached",
                format_args!("detach_type={detach_type:?}"),
                app,
            );
            let is_confirm_close_session =
                *SessionSettings::as_ref(app).should_confirm_close_session;
            self.view.update(app, |terminal_view, ctx| {
                // This emits an event that is handled in [`Self::end_shared_session`].
                // We still need to call this in order to emit a telemetry event.
                terminal_view.stop_sharing_session(
                    SharedSessionActionSource::Closed {
                        is_confirm_close_session,
                    },
                    ctx,
                )
            });
            // The window could close before the event from above is processed, so directly stop sharing here.
            Self::end_shared_session(
                &self.view,
                self.session_sharer.clone(),
                SessionEndedReason::EndedBySharer,
                self.model.clone(),
                app,
            )
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Send a Shutdown event to each PTY's event loop and waits for the
/// event loop to terminate.
/// This is needed on Windows to ensure all OpenConsole processes are
/// cleaned up before the main thread exits.
#[cfg(windows)]
pub fn shutdown_all_pty_event_loops(ctx: &mut AppContext) {
    let terminal_managers: Vec<ModelHandle<Box<dyn TerminalManagerTrait>>> = ctx.models_of_type();
    terminal_managers.into_iter().for_each(|terminal_manager| {
        terminal_manager.update(ctx, |terminal_manager, _ctx| {
            if let Some(manager) = terminal_manager
                .as_any_mut()
                .downcast_mut::<TerminalManager<TerminalView>>()
            {
                manager.shutdown_event_loop();
            }
        })
    })
}
use crate::terminal::shared_session::settings::SharedSessionSettings;
