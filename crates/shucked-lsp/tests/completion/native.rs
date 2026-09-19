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

#[test]
fn private_helper_executables_never_extend_target_resolution_or_wrapper_candidates() {
    let root = tempfile::tempdir().unwrap();
    let provider = root.path().join("providers");
    let helpers = provider.join("runtime/helpers/bin");
    std::fs::create_dir_all(&helpers).unwrap();
    for name in ["helper-only-tool", "target-tool", "printf"] {
        std::fs::write(helpers.join(name), "private executable").unwrap();
    }
    executable(root.path(), "target-tool", "exit 0");
    let environment = Environment::fixture(root.path());
    assert!(
        environment
            .executable_path("helper-only-tool", root.path())
            .is_none()
    );
    // Wrapper providers such as env/sudo return executable names as ordinary candidates.
    let private_path = helpers.join("helper-only-tool");
    let candidates: Vec<_> = [
        "helper-only-tool",
        "target-tool",
        "printf",
        private_path.to_str().unwrap(),
    ]
    .into_iter()
    .map(|text| Candidate {
        text: text.into(),
        description: "command argument".into(),
    })
    .collect();
    let filtered = filter_private_candidates(
        &provider,
        &environment,
        root.path(),
        "bash",
        true,
        &candidates,
    );
    assert_eq!(
        filtered
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["target-tool", "printf"]
    );
    let original = environment.execution_path().unwrap();
    let mut command = std::process::Command::new("unused");
    assets::configure_worker_path(&mut command, &provider, Some(&original));
    let worker_path = command
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .unwrap()
        .1
        .unwrap();
    assert!(std::env::split_paths(worker_path).any(|path| path == helpers));
    assert_eq!(environment.execution_path().unwrap(), original);
}

#[test]
fn managed_grammars_work_when_primary_tool_is_bound_to_an_absolute_path() {
    let root = tempfile::tempdir().unwrap();
    let git = executable(
        root.path(),
        "git",
        "case \"$1\" in --list-cmds=*) printf 'checkout\\n' ;; --version) printf 'git version 2.50.0\\n' ;; *) exit 1 ;; esac",
    );
    let mut environment = Environment::detect(true);
    environment.cwd = root.path().to_owned();
    environment.native = Arc::new(Native::detect());
    for dialect in ["bash", "zsh", "fish"] {
        let available = match dialect {
            "bash" => environment.native.bash.is_some(),
            "fish" => environment.native.fish.is_some(),
            _ => environment.native.zsh.is_some(),
        };
        if !available {
            continue;
        }
        let entries = environment
            .native
            .complete(
                &environment,
                &[git.to_string_lossy().into_owned()],
                "chec",
                root.path(),
                &RequestCancellationToken::default(),
                false,
                dialect,
            )
            .unwrap_or_else(|| panic!("missing {dialect} completion for a target-bound Git path"));
        assert!(
            entries
                .iter()
                .any(|entry| entry.text.trim_end() == "checkout"),
            "{dialect}: {entries:?}"
        );
    }
}

#[test]
fn ordinary_arguments_named_like_helpers_are_retained() {
    let root = tempfile::tempdir().unwrap();
    let helpers = root.path().join("runtime/helpers/bin");
    std::fs::create_dir_all(&helpers).unwrap();
    std::fs::write(helpers.join("grep"), "private executable").unwrap();
    let environment = Environment::fixture(root.path());
    let entries = vec![Candidate {
        text: "grep".into(),
        description: "Git branch".into(),
    }];
    let words = vec!["git".into(), "checkout".into()];
    assert!(!wrapper_command_position(&words));
    assert_eq!(
        filter_private_candidates(
            root.path(),
            &environment,
            root.path(),
            "bash",
            wrapper_command_position(&words),
            &entries
        )
        .len(),
        1
    );
    assert!(wrapper_command_position(&[
        "/usr/bin/env".into(),
        "LANG=C".into()
    ]));
    assert!(!wrapper_command_position(&[
        "sudo".into(),
        "git".into(),
        "checkout".into()
    ]));
    assert!(!wrapper_command_position(&["sudo".into(), "-u".into()]));
    assert!(wrapper_command_position(&[
        "sudo".into(),
        "-u".into(),
        "root".into()
    ]));
    assert!(!wrapper_command_position(&["xargs".into(), "-I".into()]));
}

#[test]
fn dynamic_provider_queries_use_selected_primary_instead_of_path_shadow() {
    let root = tempfile::tempdir().unwrap();
    let chosen_root = root.path().join("chosen");
    let chosen = executable(
        &chosen_root,
        "git",
        "printf selected >> selected-marker\nexit 1",
    );
    executable(root.path(), "git", "printf shadow >> shadow-marker\nexit 1");
    let mut paths = vec![root.path().join("bin")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).unwrap();
    let provider = Native::detect();
    let words = [chosen.to_string_lossy().into_owned(), "checkout".to_owned()];
    for dialect in ["bash", "fish", "zsh"] {
        let cancellation = RequestCancellationToken::default();
        match dialect {
            "bash" | "fish" => {
                let shell = if dialect == "bash" {
                    &provider.bash
                } else {
                    &provider.fish
                };
                let Some(shell) = shell else { continue };
                shell.complete(&words, "fixture", root.path(), &cancellation, Some(&path));
            }
            _ => {
                let Some(shell) = &provider.zsh else { continue };
                shell.complete(
                    &words,
                    "fixture",
                    root.path(),
                    1500,
                    false,
                    &cancellation,
                    Some(&path),
                );
            }
        }
        assert!(
            root.path().join("selected-marker").exists(),
            "{dialect} did not query the selected primary"
        );
        assert!(
            !root.path().join("shadow-marker").exists(),
            "{dialect} queried a different PATH executable"
        );
        std::fs::remove_file(root.path().join("selected-marker")).unwrap();
    }
}
