use super::*;
use crate::{GlobalOptions, PositionEncoding, Session, TextDocument, Workspaces};

fn snapshot(version: i32) -> DocumentSnapshot {
    let (events, _) = crossbeam::channel::unbounded();
    let (messages, _) = crossbeam::channel::unbounded();
    let client = Client::new(events, messages);
    let options = GlobalOptions::default().into_settings(client.clone());
    let mut session = Session::new(
        &Default::default(),
        PositionEncoding::UTF16,
        options,
        &Workspaces::new(vec![]),
        &client,
    )
    .unwrap();
    let uri = Url::parse("file:///tmp/shucked-worker-test.sh").unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new("echo hello\n".to_owned(), version),
    );
    session.take_snapshot(uri).unwrap()
}

#[test]
fn static_feedback_is_ready_before_the_environment_debounce() {
    let mut queue = Queue::default();
    queue.schedule(snapshot(1));
    let now = Instant::now();
    assert!(queue.next(false, now).unwrap().is_some());
    assert!(queue.next(true, now).is_err());
    assert!(
        queue
            .next(true, now + Duration::from_secs(1))
            .unwrap()
            .is_some()
    );
}

#[test]
fn replacing_a_document_cancels_its_running_environment_work() {
    let mut queue = Queue::default();
    queue.schedule(snapshot(1));
    let old = queue
        .next(true, Instant::now() + Duration::from_secs(1))
        .unwrap()
        .unwrap();
    queue.schedule(snapshot(2));
    assert!(old.analysis_cancellation().is_cancelled());
    let new = queue
        .next(true, Instant::now() + Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(new.query().document().version(), 2);
    assert!(!new.analysis_cancellation().is_cancelled());
}

#[test]
fn closing_a_document_cancels_running_work_and_discards_pending_work() {
    let mut queue = Queue::default();
    queue.schedule(snapshot(1));
    let running = queue.next(false, Instant::now()).unwrap().unwrap();
    queue.cancel(running.query().file_url());
    assert!(running.analysis_cancellation().is_cancelled());
    assert!(
        queue
            .next(true, Instant::now() + Duration::from_secs(1))
            .unwrap()
            .is_none()
    );
}
