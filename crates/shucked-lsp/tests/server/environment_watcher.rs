use super::*;

#[test]
fn missing_paths_watch_the_nearest_directory_without_watching_root() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        existing_directory(&temp.path().join("new/bin")),
        Some(temp.path().to_path_buf())
    );
    assert!(existing_directory(Path::new("/")).is_none());
}

#[test]
fn executable_install_and_removal_trigger_host_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let (main_sender, main_receiver) = crossbeam::channel::unbounded();
    let (lsp_sender, _lsp_receiver) = crossbeam::channel::unbounded();
    let watcher = EnvironmentWatcher::new(Client::new(main_sender, lsp_sender));
    watcher.update(vec![temp.path().to_path_buf()]);
    let candidate = temp.path().join("new-command");
    // Repeated writes also cover asynchronous watch registration without a fixed sleep.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        std::fs::write(&candidate, "#!/bin/sh\n").unwrap();
        if main_receiver
            .recv_timeout(Duration::from_millis(150))
            .is_ok()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "install did not refresh environment"
        );
    }
    while main_receiver.try_recv().is_ok() {}
    std::fs::remove_file(candidate).unwrap();
    assert!(
        main_receiver.recv_timeout(Duration::from_secs(15)).is_ok(),
        "removal did not refresh environment"
    );
}

#[test]
fn metadata_revalidation_detects_installation_and_removal_without_native_events() {
    let temp = tempfile::tempdir().unwrap();
    let candidate = temp.path().join("new-command");
    let targets = BTreeSet::from([temp.path().to_path_buf(), candidate.clone()]);
    let missing = fingerprints(&targets);
    std::fs::write(&candidate, "#!/bin/sh\n").unwrap();
    let installed = fingerprints(&targets);
    assert!(missing != installed);
    std::fs::remove_file(&candidate).unwrap();
    assert!(installed != fingerprints(&targets));
}
