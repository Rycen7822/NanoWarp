use super::*;

#[test]
fn replacement_join_returns_without_waiting_for_a_stuck_loop() {
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        release_rx.recv().unwrap();
        mio_channel::channel().1
    });
    let started = Instant::now();
    let result = warpui::r#async::block_on(join_event_loop(handle, Duration::ZERO));
    assert!(result.err().unwrap().to_string().contains("timeout"));
    assert!(started.elapsed() < Duration::from_secs(1));
    release_tx.send(()).unwrap();
}

#[test]
fn replacement_join_returns_the_original_channel() {
    let (sender, receiver) = mio_channel::channel();
    let handle = std::thread::spawn(move || receiver);
    while !handle.is_finished() {
        std::thread::yield_now();
    }
    let receiver = warpui::r#async::block_on(join_event_loop(handle, Duration::ZERO)).unwrap();
    sender.send(Message::ShutdownForReplacement).unwrap();
    assert!(matches!(
        receiver.try_recv(),
        Ok(Message::ShutdownForReplacement)
    ));
}
