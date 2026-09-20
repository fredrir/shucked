use super::*;

#[test]
fn bash_pack_completes_git_subcommands_without_startup_configuration() {
    let Some(provider) = ManagedShell::detect("bash") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let result = provider
        .complete(
            &["git".to_owned()],
            "chec",
            root.path(),
            &RequestCancellationToken::default(),
            None,
        )
        .expect("managed Bash completion");
    assert!(
        result.iter().any(|item| item.text.trim_end() == "checkout"),
        "{result:?}"
    );
}

#[test]
fn fish_pack_completes_described_git_subcommands() {
    let Some(provider) = ManagedShell::detect("fish") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let result = provider
        .complete(
            &["git".to_owned()],
            "chec",
            root.path(),
            &RequestCancellationToken::default(),
            None,
        )
        .expect("managed Fish completion");
    assert!(
        result
            .iter()
            .any(|item| item.text.trim_end() == "checkout" && !item.description.is_empty()),
        "{result:?}"
    );
}

#[test]
fn managed_shells_do_not_execute_editor_substitutions() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    for name in ["bash", "fish"] {
        let Some(provider) = ManagedShell::detect(name) else {
            continue;
        };
        let words = ["git".to_owned(), format!("$(touch {})", marker.display())];
        provider.complete(
            &words,
            "",
            root.path(),
            &RequestCancellationToken::default(),
            None,
        );
        assert!(!marker.exists(), "{name} executed edited text");
    }
}

#[test]
fn personal_startup_and_completion_configuration_is_not_loaded() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("startup-executed");
    let payload = format!("touch {}; exit 1\n", marker.display());
    for filename in [".bashrc", ".bash_profile", ".profile", ".bash_completion"] {
        std::fs::write(root.path().join(filename), &payload).unwrap();
    }
    std::fs::create_dir_all(root.path().join(".config/fish")).unwrap();
    std::fs::write(root.path().join(".config/fish/config.fish"), &payload).unwrap();
    for name in ["bash", "fish"] {
        let Some(mut provider) = ManagedShell::detect(name) else {
            continue;
        };
        provider.home = Some(root.path().to_owned());
        let result = provider
            .complete(
                &["git".to_owned()],
                "chec",
                root.path(),
                &RequestCancellationToken::default(),
                None,
            )
            .expect("managed completion with hostile startup files");
        assert!(
            result.iter().any(|item| item.text.trim_end() == "checkout"),
            "{name}: {result:?}"
        );
        assert!(!marker.exists(), "{name} loaded personal startup files");
    }
}

#[cfg(unix)]
#[test]
fn invalidation_during_completion_cannot_repopulate_the_cache() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("worker");
    std::fs::write(&executable, "#!/bin/sh\nprintf ready > started\nwhile [ ! -f release ]; do /bin/sleep 0.01; done\nprintf 'P\\0001\\000M\\000candidate\\000description\\000E\\000'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let provider = Arc::new(ManagedShell {
        name: "bash",
        executable,
        root: root.path().to_owned(),
        cache: Mutex::default(),
        worker: crate::handlers::completion::native_process::Persistent::default(),
        generation: AtomicU64::new(0),
        home: None,
    });
    let worker_provider = provider.clone();
    let directory = root.path().to_owned();
    let worker = std::thread::spawn(move || {
        worker_provider.complete(
            &["tool".into()],
            "",
            &directory,
            &RequestCancellationToken::default(),
            None,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while !root.path().join("started").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(root.path().join("started").exists());
    provider.invalidate();
    std::fs::write(root.path().join("release"), "").unwrap();
    let result = worker.join().unwrap().unwrap();
    assert_eq!(result[0].text, "candidate");
    assert!(provider.cache.lock().unwrap().is_empty());
}

#[test]
fn bash_git_flags_drop_completion_delimiters_but_tracked_filename_spaces_survive() {
    let Some(provider) = ManagedShell::detect("bash") else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(root.path().join("file "), "fixture\n").unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["add", "--", "file "])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Shucked Fixture",
                "-c",
                "user.email=fixture@invalid",
                "-c",
                "commit.gpgSign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--quiet",
                "-m",
                "fixture"
            ])
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(root.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(root.path().join("file "), "modified fixture\n").unwrap();
    let flags = provider
        .complete(
            &["git".into(), "checkout".into()],
            "--",
            root.path(),
            &RequestCancellationToken::default(),
            None,
        )
        .unwrap();
    assert!(
        flags.iter().any(|candidate| candidate.text == "--detach"),
        "{flags:?}"
    );
    assert!(!flags.iter().any(|candidate| candidate.text == "--detach "));
    let paths = provider
        .complete(
            &["git".into(), "add".into()],
            "file",
            root.path(),
            &RequestCancellationToken::default(),
            None,
        )
        .unwrap();
    assert!(
        paths.iter().any(|candidate| candidate.text == "file "),
        "{paths:?}"
    );
}

#[test]
fn bash_callback_options_preserve_raw_names_and_decode_explicit_quoting() {
    for (options, callback) in [
        ("-o filenames", "COMPREPLY=('file ');"),
        ("-o filenames -o noquote", "COMPREPLY=(\"'file '\");"),
        ("", "compopt -o filenames +o nospace; COMPREPLY=('file ');"),
        (
            "-o filenames",
            "compopt +o filenames; COMPREPLY=(\"'file '\");",
        ),
    ] {
        let Some(mut provider) = ManagedShell::detect("bash") else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let pack = root.path().join("packs/bash-completion");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("bash_completion"), format!("_comp_load() {{ complete {options} -F fixture printf; }}\nfixture() {{ {callback} }}\n")).unwrap();
        provider.root = root.path().to_owned();
        let result = provider
            .complete(
                &["printf".into()],
                "file",
                root.path(),
                &RequestCancellationToken::default(),
                None,
            )
            .unwrap();
        assert!(
            result.iter().any(|candidate| candidate.text == "file "),
            "{options}: {result:?}"
        );
    }
}
