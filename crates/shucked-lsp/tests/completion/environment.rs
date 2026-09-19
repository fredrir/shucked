use super::*;

#[test]
fn discovers_executables_and_refreshes_after_invalidation() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let executable = bin.join("remote-command.exe");
    std::fs::write(&executable, "not executed").unwrap();
    std::fs::write(bin.join("ordinary-file"), "text").unwrap();
    std::fs::create_dir(bin.join("directory.exe")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let environment = Environment::fixture(root.path());
    let cancellation = RequestCancellationToken::default();
    let (commands, incomplete) = environment.commands("", &cancellation);
    assert!(!incomplete);
    assert_eq!(commands.keys().collect::<Vec<_>>(), ["remote-command.exe"]);
    std::fs::remove_file(executable).unwrap();
    environment.invalidate();
    assert!(environment.commands("", &cancellation).0.is_empty());
}

#[test]
fn cancelled_scans_do_not_poison_the_cache() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("visible"), "").unwrap();
    let environment = Environment::fixture(root.path());
    let cancellation = RequestCancellationToken::default();
    cancellation.cancel();
    assert!(environment.directory(root.path(), &cancellation).incomplete);
    assert_eq!(
        environment
            .directory(root.path(), &RequestCancellationToken::default())
            .entries
            .len(),
        1
    );
}

#[test]
fn first_path_entry_wins_and_missing_directories_are_ignored() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first");
    let second = root.path().join("second");
    for directory in [&first, &second] {
        std::fs::create_dir(directory).unwrap();
        let executable = directory.join("tool.exe");
        std::fs::write(&executable, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let mut environment = Environment::fixture(root.path());
    environment.path = vec![root.path().join("missing"), first.clone(), second];
    let (commands, _) = environment.commands("tool", &RequestCancellationToken::default());
    assert_eq!(commands.len(), 1);
    assert_eq!(commands["tool.exe"], first.join("tool.exe"));
}

#[test]
fn scoped_requests_share_directory_cache_and_invalidation() {
    let root = tempfile::tempdir().unwrap();
    let environment = Environment::fixture(root.path());
    let context = shucked_command::ExecutionContext::default();
    let snapshot = shucked_command::EnvironmentSnapshot::empty(&context);
    let cancellation = RequestCancellationToken::default();
    let first = environment
        .scoped(&context, &snapshot)
        .directory(root.path(), &cancellation);
    let second = environment
        .scoped(&context, &snapshot)
        .directory(root.path(), &cancellation);
    assert!(
        Arc::ptr_eq(&first, &second),
        "requests should reuse the bounded directory listing"
    );
    std::fs::write(root.path().join("new-file"), "").unwrap();
    environment.invalidate();
    let refreshed = environment
        .scoped(&context, &snapshot)
        .directory(root.path(), &cancellation);
    assert_eq!(refreshed.entries[0].name, "new-file");
}
