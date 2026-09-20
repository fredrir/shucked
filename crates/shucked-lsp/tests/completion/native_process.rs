use super::*;
use std::sync::Arc;

fn host(script: &str) -> Command {
    let mut command = Command::new("/bin/bash");
    command.args(["--noprofile", "--norc", "-c", script]);
    command
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
        let candidates = crate::handlers::completion::native_zsh::parse_output(&output).unwrap();
        assert_eq!(candidates[0].text, format!("{}:{input}", index + 1));
        assert_eq!(candidates[1].text, "E");
        assert_eq!(candidates[1].description, "terminator is a candidate");
    }
}

#[test]
fn timed_out_worker_kills_helpers_and_next_request_restarts_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let worker = Persistent::default();
    let script = r#"
while IFS= read -r -d '' value; do
    printf 'P\0000\000'
    if [[ $value == slow ]]; then
        (/bin/sleep 0.25; printf leaked > leaked) &
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
    let candidates = crate::handlers::completion::native_zsh::parse_output(&output).unwrap();
    assert_eq!(candidates[0].text, "recovered");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!root.path().join("leaked").exists());
}

#[test]
fn cancellation_interrupts_a_worker_waiting_for_a_result() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("started");
    let worker = Arc::new(Persistent::default());
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
    assert!(task.join().unwrap().is_none());
}
