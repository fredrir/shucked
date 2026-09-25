use super::*;
use crate::handlers::completion::native_zsh::Candidate;
use std::sync::Arc;

fn host(script: &str) -> Command {
    let mut command = Command::new("/bin/bash");
    command.args(["--noprofile", "--norc", "-c", script]);
    command
}

/// One frame per request, numbered so the serving process is observable; a
/// `slow` request finishes after a delay, `hang` never finishes.
const COUNTING: &str = r#"
count=0
while IFS= read -r -d '' value; do
    ((count+=1))
    printf 'P\0000\000'
    if [[ $value == slow ]]; then
        /bin/sleep 0.2
    elif [[ $value == hang ]]; then
        while :; do /bin/sleep 1; done
    fi
    printf 'M\000%s:%s\000description\000E\000' "$count" "$value"
done
"#;

fn candidates(output: &[u8]) -> Vec<Candidate> {
    crate::handlers::completion::native_zsh::parse_output(output).unwrap()
}

#[test]
fn initialized_worker_survives_context_changes_and_preserves_frame_boundaries() {
    let worker = Persistent::default();
    let script = r#"
count=0
while IFS= read -r -d '' value; do
    ((count+=1))
    printf 'P\0000\000M\000%s:%s\000description\000M\000E\000terminator is a candidate\000E\000' "$count" "$value"
done
"#;
    for (index, input) in ["first", "two words", "$(touch must-not-execute)"]
        .iter()
        .enumerate()
    {
        let output = worker
            .request(
                &mut host(script),
                "fixture".into(),
                &[(*input).into()],
                Duration::from_secs(1),
                Duration::from_secs(1),
                &RequestCancellationToken::default(),
            )
            .unwrap();
        let candidates = candidates(&output);
        assert_eq!(candidates[0].text, format!("{}:{input}", index + 1));
        assert_eq!(candidates[1].text, "E");
        assert_eq!(candidates[1].description, "terminator is a candidate");
    }
}

#[test]
fn timed_out_request_keeps_the_worker_warm_for_the_next_request() {
    let worker = Persistent::default();
    assert!(
        worker
            .request(
                &mut host(COUNTING),
                "fixture".into(),
                &["slow".into()],
                Duration::from_millis(50),
                Duration::from_millis(50),
                &RequestCancellationToken::default()
            )
            .is_none()
    );
    assert!(worker.busy(), "the abandoned response is still arriving");
    // The follow-up waits for the abandoned response, then reuses the process.
    let output = worker
        .request(
            &mut host(COUNTING),
            "fixture".into(),
            &["fast".into()],
            Duration::from_secs(1),
            Duration::from_secs(1),
            &RequestCancellationToken::default(),
        )
        .unwrap();
    assert_eq!(candidates(&output)[0].text, "2:fast");
    assert!(!worker.busy());
}

#[test]
fn identical_request_adopts_the_abandoned_response_instead_of_rerunning() {
    let worker = Persistent::default();
    let token = RequestCancellationToken::default();
    let cancel = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        cancel.cancel();
    });
    assert!(
        worker
            .request(
                &mut host(COUNTING),
                "fixture".into(),
                &["slow".into()],
                Duration::from_secs(2),
                Duration::from_secs(2),
                &token
            )
            .is_none()
    );
    let output = worker
        .request(
            &mut host(COUNTING),
            "fixture".into(),
            &["slow".into()],
            Duration::from_secs(1),
            Duration::from_secs(1),
            &RequestCancellationToken::default(),
        )
        .unwrap();
    assert_eq!(
        candidates(&output)[0].text,
        "1:slow",
        "the first run's response was reused"
    );
    let output = worker
        .request(
            &mut host(COUNTING),
            "fixture".into(),
            &["slow".into()],
            Duration::from_secs(1),
            Duration::from_secs(1),
            &RequestCancellationToken::default(),
        )
        .unwrap();
    assert_eq!(
        candidates(&output)[0].text,
        "2:slow",
        "a late response is adopted once"
    );
}

#[test]
fn a_request_arriving_during_a_long_drain_reports_busy_without_failing_the_worker() {
    let worker = Persistent::default();
    assert!(
        worker
            .request(
                &mut host(COUNTING),
                "fixture".into(),
                &["slow".into()],
                Duration::from_millis(20),
                Duration::from_millis(20),
                &RequestCancellationToken::default()
            )
            .is_none()
    );
    // Too short to outlast the drain: nothing is sent and the worker survives.
    assert!(
        worker
            .request(
                &mut host(COUNTING),
                "fixture".into(),
                &["fast".into()],
                Duration::from_millis(20),
                Duration::from_millis(20),
                &RequestCancellationToken::default()
            )
            .is_none()
    );
    assert!(worker.busy());
    let output = worker
        .request(
            &mut host(COUNTING),
            "fixture".into(),
            &["fast".into()],
            Duration::from_secs(1),
            Duration::from_secs(1),
            &RequestCancellationToken::default(),
        )
        .unwrap();
    assert_eq!(candidates(&output)[0].text, "2:fast");
}

#[test]
fn stuck_worker_is_killed_with_its_helpers_after_the_drain_cap() {
    let root = tempfile::tempdir().unwrap();
    let worker = Persistent::with_drain_cap(Duration::from_millis(100));
    let script = r#"
while IFS= read -r -d '' value; do
    printf 'P\0000\000'
    if [[ $value == slow ]]; then
        (/bin/sleep 0.4; printf leaked > leaked) &
        wait
    else
        printf 'M\000recovered\000\000E\000'
    fi
done
"#;
    let mut slow = host(script);
    slow.current_dir(root.path());
    assert!(
        worker
            .request(
                &mut slow,
                "fixture".into(),
                &["slow".into()],
                Duration::from_millis(50),
                Duration::from_millis(50),
                &RequestCancellationToken::default()
            )
            .is_none()
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(!worker.busy(), "the drain cap ended the abandoned response");
    let mut next = host(script);
    next.current_dir(root.path());
    let output = worker
        .request(
            &mut next,
            "fixture".into(),
            &["fast".into()],
            Duration::from_secs(1),
            Duration::from_secs(1),
            &RequestCancellationToken::default(),
        )
        .unwrap();
    assert_eq!(candidates(&output)[0].text, "recovered");
    std::thread::sleep(Duration::from_millis(400));
    assert!(!root.path().join("leaked").exists());
}

#[test]
fn cancellation_interrupts_a_worker_waiting_for_a_result() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("started");
    let worker = Arc::new(Persistent::with_drain_cap(Duration::from_millis(200)));
    let token = RequestCancellationToken::default();
    let background_token = token.clone();
    let background_worker = worker.clone();
    let directory = root.path().to_owned();
    let task = std::thread::spawn(move || {
        let mut command = host(
            "IFS= read -r -d '' value; printf ready > started; while :; do /bin/sleep 1; done",
        );
        command.current_dir(directory);
        background_worker.request(
            &mut command,
            "fixture".into(),
            &["wait".into()],
            Duration::from_secs(5),
            Duration::from_secs(5),
            &background_token,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(marker.exists(), "worker never received its request");
    token.cancel();
    let started = Instant::now();
    assert!(task.join().unwrap().is_none());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        worker.busy(),
        "the cancelled request left the worker draining"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !worker.busy(),
        "a worker that never answers is killed at the cap"
    );
}
