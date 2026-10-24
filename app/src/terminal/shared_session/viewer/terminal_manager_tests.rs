use warpui::App;

use super::*;
use crate::ai::blocklist::QueuedQueryModel;
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
                let id = history.start_new_conversation(terminal_view_id, false, false, ctx);
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
