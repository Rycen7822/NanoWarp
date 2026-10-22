use super::{SetupStep, SetupTracer};
use futures::executor::block_on;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use warpui::r#async::executor::Background;

fn background() -> Arc<Background> {
    Arc::new(Background::new(1, |_| "setup-scope-test".to_string()))
}

fn assert_steps_execute(reporter: &SetupTracer, steps: impl IntoIterator<Item = SetupStep>) {
    for step in steps {
        assert_eq!(
            block_on(reporter.record_result(step, async { Ok::<_, std::io::Error>(42) })).unwrap(),
            42,
        );

        let (tx, rx) = mpsc::channel();
        reporter.record_value_detached(step, async move {
            tx.send(42).unwrap();
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), 42);
    }
}

#[test]
fn setup_tracer_executes_environment_steps() {
    let reporter = SetupTracer::new(background());
    assert_steps_execute(&reporter, enum_iterator::all::<SetupStep>());
}
