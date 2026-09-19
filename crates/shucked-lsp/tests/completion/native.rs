use super::*;
use std::os::unix::fs::PermissionsExt;

fn executable(root: &Path, name: &str, body: &str) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn complete(environment: &Environment, words: &[&str], prefix: &str) -> Vec<Candidate> {
    environment
        .native
        .complete(
            environment,
            &words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>(),
            prefix,
            &environment.cwd,
            &RequestCancellationToken::default(),
            false,
            "zsh",
        )
        .unwrap()
        .as_ref()
        .clone()
}

#[test]
fn package_queries_use_host_databases_without_forwarding_document_arguments() {
    let root = tempfile::tempdir().unwrap();
    executable(
        root.path(),
        "pacman",
        "case \"$*\" in '-Slq') printf 'available-package\\n' ;; '-Qq') printf 'installed-package\\n' ;; *) exit 9 ;; esac",
    );
    executable(
        root.path(),
        "brew",
        "case \"$*\" in formulae) printf 'formula-name\\n' ;; casks) printf 'cask-name\\n' ;; 'list --formula -1') printf 'installed-formula\\n' ;; 'list --cask -1') printf 'installed-cask\\n' ;; *) exit 9 ;; esac",
    );
    let environment = Environment::fixture(root.path());
    assert_eq!(
        complete(&environment, &["pacman", "-S", "$(touch marker)"], "")[0].text,
        "available-package"
    );
    assert_eq!(
        complete(&environment, &["pacman", "-R"], "")[0].text,
        "installed-package"
    );
    assert_eq!(
        complete(&environment, &["brew", "install", "--cask"], "")[0].text,
        "cask-name"
    );
    assert_eq!(
        complete(&environment, &["brew", "install", "--formula"], "")[0].text,
        "formula-name"
    );
    assert_eq!(
        complete(&environment, &["brew", "uninstall", "--formula"], "")[0].text,
        "installed-formula"
    );
    assert_eq!(complete(&environment, &["brew", "install"], "").len(), 2);
    assert!(!root.path().join("marker").exists());
}

#[test]
fn native_flags_keep_descriptions_and_cached_queries_refresh() {
    let root = tempfile::tempdir().unwrap();
    let command = executable(
        root.path(),
        "eza",
        "[ \"$*\" = '--help' ] || exit 9; printf '  -a, --all  Include hidden entries\\n  --sort=FIELD  Order entries\\n'",
    );
    let environment = Environment::fixture(root.path());
    let entries = complete(&environment, &["eza"], "--a");
    assert!(
        entries
            .iter()
            .any(|entry| entry.text == "--all" && entry.description == "Include hidden entries")
    );
    assert!(entries.iter().any(|entry| entry.text == "--sort"));
    std::fs::write(command, "#!/bin/sh\nexit 1\n").unwrap();
    assert_eq!(
        complete(&environment, &["eza"], "--al").len(),
        entries.len()
    );
    environment.invalidate();
    assert!(
        environment
            .native
            .complete(
                &environment,
                &["eza".to_owned()],
                "-",
                root.path(),
                &RequestCancellationToken::default(),
                false,
                "zsh",
            )
            .is_none()
    );
}

#[test]
fn cancelled_and_slow_queries_return_without_poisoning_future_queries() {
    let root = tempfile::tempdir().unwrap();
    let path = executable(root.path(), "brew", "while :; do :; done");
    let token = RequestCancellationToken::default();
    let cancel = token.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel.cancel();
    });
    let started = Instant::now();
    let result = capture(
        &mut std::process::Command::new(&path),
        Duration::from_secs(5),
        &token,
        false,
    );
    thread.join().unwrap();
    assert!(result.is_none());
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        capture(
            &mut std::process::Command::new(path),
            Duration::from_millis(40),
            &RequestCancellationToken::default(),
            false
        )
        .is_none()
    );
}

#[test]
fn does_not_run_unknown_help_interfaces_or_package_install_operations() {
    let root = tempfile::tempdir().unwrap();
    executable(root.path(), "custom", "touch must-not-exist");
    let environment = Environment::fixture(root.path());
    assert!(
        environment
            .native
            .complete(
                &environment,
                &["custom".to_owned()],
                "--",
                root.path(),
                &RequestCancellationToken::default(),
                false,
                "zsh",
            )
            .is_none()
    );
    assert!(!root.path().join("must-not-exist").exists());
    for words in [
        vec!["pacman", "-U"],
        vec!["pacman", "-S", "--config", "custom"],
        vec!["brew", "tap"],
    ] {
        assert!(
            package_queries(
                words[0],
                &words
                    .iter()
                    .map(|word| (*word).to_owned())
                    .collect::<Vec<_>>(),
                ""
            )
            .is_none()
        );
    }
}

#[test]
fn timeout_stops_completion_helpers_as_well_as_the_parent() {
    let root = tempfile::tempdir().unwrap();
    let path = executable(
        root.path(),
        "helper",
        "(/bin/sleep 0.3; printf leaked > leaked) &\nwait",
    );
    assert!(
        capture(
            std::process::Command::new(path).current_dir(root.path()),
            Duration::from_millis(40),
            &RequestCancellationToken::default(),
            false
        )
        .is_none()
    );
    std::thread::sleep(Duration::from_millis(400));
    assert!(!root.path().join("leaked").exists());
}

#[test]
fn multiline_native_help_keeps_descriptions_for_short_and_long_flags() {
    let entries = help_candidates(
        "OPTIONS\n    -., --hidden\n        Include hidden entries.\n\n    --sort <FIELD>\n        Choose the ordering field.\n",
    );
    for flag in ["-.", "--hidden"] {
        assert!(
            entries
                .iter()
                .any(|entry| entry.text == flag && entry.description == "Include hidden entries.")
        );
    }
    assert!(
        entries.iter().any(
            |entry| entry.text == "--sort" && entry.description == "Choose the ordering field."
        )
    );
}

#[test]
fn pacman_file_and_group_operations_do_not_suggest_package_names() {
    for flag in ["-Qo", "-Qp", "-Sg", "-Sl", "-F", "-U"] {
        assert!(
            package_queries("pacman", &["pacman".to_owned(), flag.to_owned()], "").is_none(),
            "{flag}"
        );
    }
}
