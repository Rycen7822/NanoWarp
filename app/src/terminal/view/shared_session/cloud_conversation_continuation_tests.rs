use super::*;
use crate::terminal::shared_session::SharedSessionStatus;
use warpui::App;

#[test]
fn routing_is_local_for_non_cloud_pane() {
    App::test((), |mut app| async move {
        let model = TerminalModel::mock(None, None);
        app.update(|_| {
            let routing = resolve_ai_query_routing(&model);
            assert_eq!(routing, AIQueryRouting::Local);
        });
    });
}

#[test]
fn routing_allows_live_input_only_for_executable_shared_session_role() {
    let mut reader = TerminalModel::mock(None, None);
    reader.set_shared_session_status(SharedSessionStatus::reader());
    let mut executor = TerminalModel::mock(None, None);
    executor.set_shared_session_status(SharedSessionStatus::executor());
    assert_eq!(
        resolve_ai_query_routing(&reader),
        AIQueryRouting::LiveRemoteVm { is_executor: false }
    );
    assert_eq!(
        resolve_ai_query_routing(&executor),
        AIQueryRouting::LiveRemoteVm { is_executor: true }
    );
}
