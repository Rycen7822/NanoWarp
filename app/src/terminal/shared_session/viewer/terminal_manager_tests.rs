use async_broadcast::broadcast;
use warpui::App;

use super::*;
use crate::ai::blocklist::QueuedQueryModel;
use crate::pane_group::PaneConfigurationEvent;
use crate::test_util::add_window_with_terminal;
use crate::test_util::terminal::initialize_app_for_terminal_view;
use crate::workspace::ToastStack;

#[test]
fn command_execution_request_failed_clears_queued_command_in_flight() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(|_| ToastStack);

        let terminal = add_window_with_terminal(&mut app, None);
        let terminal_view_id = terminal.id();
        let conversation_id =
            BlocklistAIHistoryModel::handle(&app).update(&mut app, |history, ctx| {
                let id = history.start_new_conversation(terminal_view_id, false, false, false, ctx);
                history.set_active_conversation_id(id, terminal_view_id, ctx);
                id
            });
        QueuedQueryModel::handle(&app).update(&mut app, |model, _ctx| {
            model.arm_command_in_flight(conversation_id);
        });

        terminal.update(&mut app, |view, ctx| {
            TerminalManager::handle_command_execution_request_failed(
                view,
                &CommandExecutionFailureReason::StaleBuffer,
                ctx,
            );
        });

        QueuedQueryModel::handle(&app).read(&app, |model, _ctx| {
            assert!(!model.has_command_in_flight(conversation_id));
        });
    });
}
#[test]
fn handle_viewer_session_end_ignores_stale_ambient_end() {
    // A stale ambient end (the ended network is no longer the current one) must
    // be ignored: `handle_viewer_session_end` routes ambient panes through
    // `end_current_ambient_session`, whose current-network guard bails, so the
    // helper returns `false` and performs no teardown.
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);

        let terminal_view = add_window_with_terminal(&mut app, None);
        let model = Arc::new(FairMutex::new(TerminalModel::mock(None, None)));

        let (wakeups_tx, _wakeups_rx) = async_channel::unbounded();
        let (events_tx, _events_rx) = async_channel::unbounded::<crate::terminal::event::Event>();
        let (pty_reads_tx, pty_reads_rx) = broadcast(8);
        let _inactive_pty_reads_rx = pty_reads_rx.deactivate();
        let channel_event_proxy = ChannelEventListener::new(wakeups_tx, events_tx, pty_reads_tx);
        let (_write_to_pty_tx, write_to_pty_rx) = async_channel::unbounded();

        let ended_network = app.add_model(|ctx| {
            Network::new_for_test(
                channel_event_proxy,
                terminal_view.downgrade(),
                model.clone(),
                write_to_pty_rx,
                RemoteUpdateGuard::new(),
                ctx,
            )
        });

        // Empty `current_network` => the ended network is stale.
        let current_network = Arc::new(FairMutex::new(None));

        let mut handled = true;
        app.update(|ctx| {
            handled = TerminalManager::handle_viewer_session_end(
                &terminal_view,
                model.clone(),
                &current_network,
                &ended_network,
                /* is_ambient_agent */ true,
                ctx,
            );
        });

        assert!(
            !handled,
            "a stale ambient end (ended network != current) must be ignored"
        );
        assert!(
            !model.lock().shared_session_status().is_finished_viewer(),
            "an ignored stale ambient end must not finish the viewer"
        );
    });
}

#[test]
fn ending_ambient_session_refreshes_shared_session_link_surfaces() {
    let _handoff_flag = FeatureFlag::HandoffCloudCloud.override_enabled(false);

    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(Manager::new);

        let terminal_view = add_window_with_terminal(&mut app, None);
        let model = terminal_view.read(&app, |view, _| view.model.clone());
        model
            .lock()
            .set_shared_session_status(SharedSessionStatus::ActiveViewer {
                role: Default::default(),
            });

        let (wakeups_tx, _wakeups_rx) = async_channel::unbounded();
        let (events_tx, _events_rx) = async_channel::unbounded::<crate::terminal::event::Event>();
        let (pty_reads_tx, pty_reads_rx) = broadcast(8);
        let _inactive_pty_reads_rx = pty_reads_rx.deactivate();
        let channel_event_proxy = ChannelEventListener::new(wakeups_tx, events_tx, pty_reads_tx);
        let (_write_to_pty_tx, write_to_pty_rx) = async_channel::unbounded();
        let ended_network = app.add_model(|ctx| {
            Network::new_for_test(
                channel_event_proxy,
                terminal_view.downgrade(),
                model.clone(),
                write_to_pty_rx,
                RemoteUpdateGuard::new(),
                ctx,
            )
        });
        let ended_session_id = ended_network.read(&app, |network, _| network.session_id());
        Manager::handle(&app).update(&mut app, |manager, ctx| {
            manager.joined_share(terminal_view.downgrade(), ended_session_id, ctx);
        });

        let link_change_events = Arc::new(FairMutex::new(0));
        let link_change_events_for_subscription = link_change_events.clone();
        let pane_configuration =
            terminal_view.read(&app, |view, _| view.pane_configuration().clone());
        app.update(|ctx| {
            ctx.subscribe_to_model(&pane_configuration, move |_, event, _| {
                if matches!(event, PaneConfigurationEvent::SharedSessionLinkChanged) {
                    *link_change_events_for_subscription.lock() += 1;
                }
            });
        });

        let current_network = Arc::new(FairMutex::new(Some(ended_network.clone())));
        let handled = app.update(|ctx| {
            TerminalManager::end_current_ambient_session(
                &terminal_view,
                model.clone(),
                &current_network,
                &ended_network,
                ctx,
            )
        });

        assert!(handled);
        assert_eq!(*link_change_events.lock(), 1);
        terminal_view.read(&app, |view, ctx| {
            let status = view.model.lock().shared_session_status().clone();
            assert_eq!(
                Manager::as_ref(ctx).session_id_for_link(&view.id(), &status),
                None,
                "an ended id must not stay exposed while the status is still ActiveViewer"
            );
        });
    });
}
