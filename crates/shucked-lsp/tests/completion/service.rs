use super::*;

fn item() -> Output {
    Output::Candidates(Arc::new(vec![Candidate {
        text: "--flag".into(),
        ..Default::default()
    }]))
}

fn listener() -> (Notice, crossbeam::channel::Receiver<lsp_server::Message>) {
    let (main, _) = crossbeam::channel::unbounded();
    let (client, messages) = crossbeam::channel::unbounded();
    (
        Notice {
            client: Client::new(main, client),
            uri: Url::parse("file:///completion.sh").unwrap(),
            version: 1,
            position: Position::new(0, 5),
        },
        messages,
    )
}

#[test]
fn slow_provider_returns_immediately_and_identical_requests_share_work() {
    let service = Service::default();
    let (started, start) = bounded(1);
    let (release, wait) = bounded(1);
    let (notice, messages) = listener();
    let key = Key::Native("command -".into());
    let (first, pending) = service.query(key.clone(), Some(notice.clone()), move |_| {
        started.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        Some(item())
    });
    assert!(first.is_none() && pending);
    start.recv_timeout(Duration::from_secs(1)).unwrap();
    let (second, pending) = service.query(key.clone(), Some(notice.clone()), |_| {
        panic!("duplicate work")
    });
    assert!(second.is_none() && pending);
    release.send(()).unwrap();
    let lsp_server::Message::Notification(ready) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("notification")
    };
    assert_eq!(ready.method, "shucked/completionReady");
    assert_eq!(ready.params["version"], 1);
    assert_eq!(ready.params["candidateCount"], 1);
    let (cached, pending) = service.query(key, Some(notice), |_| panic!("cache miss"));
    assert_eq!(cached.unwrap().len(), 1);
    assert!(!pending);
    assert!(
        messages.try_recv().is_err(),
        "coalesced listeners receive one refresh"
    );
}

#[test]
fn blocked_preparation_returns_immediately_and_notifies_without_candidates() {
    let service = Service::default();
    let (started, start) = bounded(1);
    let (release, wait) = bounded(1);
    let (notice, messages) = listener();
    let key = Key::Preparation("workspace version and epoch".into());
    let (output, pending) = service.query(key.clone(), Some(notice.clone()), move |_| {
        started.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        Some(Output::Prepared)
    });
    assert!(output.is_none() && pending);
    start.recv_timeout(Duration::from_secs(1)).unwrap();
    let (output, pending) = service.query(key.clone(), Some(notice), |_| {
        panic!("duplicate preparation")
    });
    assert!(output.is_none() && pending);
    assert!(messages.try_recv().is_err());
    release.send(()).unwrap();
    let lsp_server::Message::Notification(ready) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("analysis readiness notification");
    };
    assert_eq!(ready.method, "shucked/completionReady");
    assert_eq!(ready.params["reason"], "analysisReady");
    assert_eq!(ready.params["candidateCount"], 0);
    assert!(
        messages.try_recv().is_err(),
        "one readiness event per cursor"
    );
    assert!(
        service
            .state
            .lock()
            .unwrap()
            .cache
            .iter()
            .all(|entry| entry.key != key),
        "preparation must recheck shared caches after eviction"
    );
}

#[test]
fn invalidation_cancels_work_and_prevents_stale_cache_publication() {
    let service = Service::default();
    let (started, start) = bounded(1);
    let (release, wait) = bounded(1);
    let (done, finish) = bounded(1);
    let key = Key::Native("old environment".into());
    service.query(key, None, move |cancel| {
        started.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(cancel.is_cancelled());
        done.send(()).unwrap();
        Some(item())
    });
    start.recv_timeout(Duration::from_secs(1)).unwrap();
    service.invalidate();
    release.send(()).unwrap();
    finish.recv_timeout(Duration::from_secs(1)).unwrap();
    let state = service.state.lock().unwrap();
    assert!(state.pending.is_empty());
    assert!(state.cache.is_empty());
}

#[test]
fn moving_the_cursor_retires_only_obsolete_listeners() {
    let service = Service::default();
    let (started, start) = bounded(1);
    let (release, wait) = bounded(1);
    let (old, messages) = listener();
    let mut current = old.clone();
    current.position.character += 1;
    let key = Key::Native("shared provider request".into());
    service.query(key.clone(), Some(old.clone()), move |_| {
        started.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        Some(item())
    });
    start.recv_timeout(Duration::from_secs(1)).unwrap();
    service.query(key, Some(current.clone()), |_| panic!("duplicate"));
    service.cancel_document(&old.uri, Some((current.version, current.position)));
    release.send(()).unwrap();
    let lsp_server::Message::Notification(ready) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("notification")
    };
    assert_eq!(
        ready.params["position"]["character"],
        current.position.character
    );
    assert!(messages.try_recv().is_err());
}

#[test]
fn directory_results_do_not_wait_for_busy_shell_providers() {
    let service = Service::default();
    let (started, start) = bounded(2);
    let (release, wait) = bounded(2);
    for index in 0..2 {
        let started = started.clone();
        let wait = wait.clone();
        service.query(Key::Native(format!("busy-{index}")), None, move |_| {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            Some(item())
        });
    }
    for _ in 0..2 {
        start.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    let (notice, messages) = listener();
    service.query(
        Key::Directory("/some/directory".into()),
        Some(notice),
        |_| Some(item()),
    );
    // Neither native worker has been released: the directory lane must make progress independently.
    let result = messages.recv_timeout(Duration::from_secs(1));
    for _ in 0..2 {
        release.send(()).unwrap();
    }
    assert!(result.is_ok());
}

#[test]
fn a_requested_prewarm_is_promoted_and_cancelled_with_its_document() {
    let service = Service::default();
    let (started, start) = bounded(1);
    let (release, wait) = bounded(1);
    let key = Key::Native("next argument".into());
    service.query(key.clone(), None, move |cancel| {
        started.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(cancel.is_cancelled());
        Some(item())
    });
    start.recv_timeout(Duration::from_secs(1)).unwrap();
    let (notice, _) = listener();
    service.query(key, Some(notice.clone()), |_| panic!("duplicate prewarm"));
    service.cancel_document(&notice.uri, None);
    assert!(service.state.lock().unwrap().pending.is_empty());
    release.send(()).unwrap();
}

#[test]
fn invalidated_results_request_a_retry_without_poisoning_the_next_query() {
    let service = Service::default();
    let (notice, messages) = listener();
    let key = Key::Native("environment changed before dispatch".into());
    service.query(key.clone(), Some(notice.clone()), |_| {
        Some(Output::Invalidated)
    });
    let lsp_server::Message::Notification(invalidated) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("retry notification");
    };
    assert_eq!(invalidated.method, "shucked/completionReady");
    assert_eq!(invalidated.params["reason"], "environmentChanged");
    assert_eq!(invalidated.params["candidateCount"], 0);
    let (cached, pending) = service.query(key.clone(), Some(notice.clone()), |_| Some(item()));
    assert!(
        cached.is_none() && pending,
        "an invalid environment must not become an empty cached answer"
    );
    let lsp_server::Message::Notification(ready) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("fresh result notification");
    };
    assert_eq!(ready.params["candidateCount"], 1);
    let (cached, pending) =
        service.query(key, Some(notice), |_| panic!("fresh result was not cached"));
    assert_eq!(cached.unwrap().len(), 1);
    assert!(!pending);
}

#[test]
fn invalidation_notifies_each_cursor_once_and_late_results_cannot_replace_new_data() {
    let service = Service::default();
    let (notice, messages) = listener();
    let (started, start) = bounded(2);
    let (release, wait) = bounded(2);
    let (done, finish) = bounded(2);
    let key = Key::Native("first provider".into());
    for key in [key.clone(), Key::Native("second provider".into())] {
        let started = started.clone();
        let wait = wait.clone();
        let done = done.clone();
        service.query(key, Some(notice.clone()), move |cancel| {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(cancel.is_cancelled());
            done.send(()).unwrap();
            Some(Output::Candidates(Arc::new(vec![Candidate {
                text: "stale".into(),
                ..Default::default()
            }])))
        });
    }
    for _ in 0..2 {
        start.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    service.invalidate();
    let lsp_server::Message::Notification(ready) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("invalidation notification");
    };
    assert_eq!(ready.params["reason"], "environmentChanged");
    assert_eq!(
        ready.params["position"]["character"],
        notice.position.character
    );
    assert!(
        messages.try_recv().is_err(),
        "one cursor should receive one invalidation refresh"
    );
    service.query(key.clone(), Some(notice.clone()), |_| Some(item()));
    for _ in 0..2 {
        release.send(()).unwrap();
    }
    for _ in 0..2 {
        finish.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    let lsp_server::Message::Notification(ready) =
        messages.recv_timeout(Duration::from_secs(1)).unwrap()
    else {
        panic!("fresh result notification");
    };
    assert_eq!(ready.params["candidateCount"], 1);
    let (Some(Output::Candidates(cached)), pending) =
        service.query(key, Some(notice), |_| panic!("fresh cache lost"))
    else {
        panic!("fresh candidates");
    };
    assert_eq!(cached[0].text, "--flag");
    assert!(!pending);
    assert!(
        messages.try_recv().is_err(),
        "cancelled workers must not publish stale refreshes"
    );
}

#[test]
fn watched_directories_survive_result_eviction_and_environment_invalidation() {
    let service = Service::default();
    let (notice, messages) = listener();
    let directory = std::path::PathBuf::from("/completion-fixture");
    service.query(
        Key::Directory(directory.clone()),
        Some(notice.clone()),
        |_| {
            Some(Output::Directory(Arc::new(Directory {
                entries: vec![super::super::environment::Entry {
                    name: "entry".into(),
                    directory: true,
                    executable: false,
                }],
                incomplete: false,
            })))
        },
    );
    messages.recv_timeout(Duration::from_secs(1)).unwrap();
    for index in 0..MAX_CACHE {
        service.query(
            Key::Native(format!("provider-{index}")),
            Some(notice.clone()),
            |_| Some(item()),
        );
        messages.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    assert_eq!(service.watch_directories(), vec![directory.clone()]);
    service.invalidate();
    assert_eq!(service.watch_directories(), vec![directory]);
}

#[test]
fn recent_contexts_are_bounded_survive_invalidation_and_retire_with_the_cursor() {
    let service = Service::default();
    let (notice, messages) = listener();
    for index in 0..10 {
        let mut notice = notice.clone();
        notice.uri = Url::parse(&format!("file:///script-{index}.sh")).unwrap();
        service.remember(notice.clone());
        service.query(Key::Native(format!("recent-{index}")), Some(notice), |_| {
            Some(item())
        });
        messages.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    let recent = service.recent();
    assert_eq!(recent.len(), 8);
    assert!(
        recent
            .iter()
            .all(|notice| !notice.uri.as_str().ends_with("script-0.sh")
                && !notice.uri.as_str().ends_with("script-1.sh"))
    );
    service.invalidate();
    assert_eq!(service.recent().len(), 8);
    let current = recent.last().unwrap();
    service.cancel_document(&current.uri, Some((current.version, current.position)));
    assert_eq!(service.recent().len(), 8);
    service.cancel_document(&current.uri, None);
    assert_eq!(service.recent().len(), 7);
}
