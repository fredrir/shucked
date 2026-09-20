use super::super::native_process::capture;
use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn executable(root: &Path, name: &str, body: &str) -> PathBuf {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
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
        ..Default::default()
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
        ..Default::default()
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

#[test]
fn installed_completion_drives_unknown_command_in_every_script_dialect() {
    if NativeZsh::detect().is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    executable(root.path(), "shucked-fixture", "touch must-not-execute");
    let completions = root.path().join("share/zsh/site-functions");
    std::fs::create_dir_all(&completions).unwrap();
    std::fs::write(completions.join("_shucked_fixture"), "#compdef shucked-fixture\n_arguments '1:operation:(start stop)' '--mode=[Output mode]:mode:(wide compact)' '-v[Verbose output]' '--absolute[Absolute path]'\n").unwrap();
    let mut environment = Environment::fixture(root.path());
    environment.native = Arc::new(Native::detect());
    for dialect in ["bash", "sh", "zsh", "fish"] {
        for (words, prefix, expected) in [
            (vec!["shucked-fixture".into()], "st", "start"),
            (vec!["shucked-fixture".into()], "--mode=w", "--mode=wide"),
            (vec!["shucked-fixture".into()], "-v", "-v"),
        ] {
            let result = environment
                .native
                .complete(
                    &environment,
                    &words,
                    prefix,
                    root.path(),
                    &RequestCancellationToken::default(),
                    false,
                    dialect,
                )
                .unwrap_or_else(|| panic!("installed completion {dialect}: {prefix}"));
            assert!(
                result.iter().any(|item| item.text == expected),
                "{dialect} {prefix}: {result:?}"
            );
            assert!(result.iter().all(|item| item.provider == "installed · zsh"));
        }
    }
    let midword = environment
        .native
        .complete_at(
            &environment,
            &["shucked-fixture".into()],
            "--abs",
            root.path(),
            &RequestCancellationToken::default(),
            false,
            "bash",
            "uffix",
        )
        .unwrap();
    assert!(
        midword.iter().any(|item| item.text == "--absolute"),
        "{midword:?}"
    );
    assert!(!root.path().join("must-not-execute").exists());
}

#[test]
fn unregistered_commands_do_not_get_executed_for_help() {
    let root = tempfile::tempdir().unwrap();
    executable(
        root.path(),
        "shucked-unregistered",
        "touch must-not-execute",
    );
    let mut environment = Environment::fixture(root.path());
    environment.native = Arc::new(Native::detect());
    assert!(
        environment
            .native
            .complete(
                &environment,
                &["shucked-unregistered".into()],
                "--",
                root.path(),
                &RequestCancellationToken::default(),
                false,
                "bash"
            )
            .is_none()
    );
    assert!(!root.path().join("must-not-execute").exists());
}

#[test]
fn zsh_spacing_metadata_preserves_filename_spaces_without_inserting_delimiter_spaces() {
    if NativeZsh::detect().is_none() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    executable(root.path(), "shucked-spacing", "exit 9");
    std::fs::write(root.path().join("file "), "fixture").unwrap();
    let completions = root.path().join("share/zsh/site-functions");
    std::fs::create_dir_all(&completions).unwrap();
    std::fs::write(completions.join("_shucked_spacing"), "#compdef shucked-spacing\ncompadd -S ' ' -- --flag\ncompadd -S '' -- --prefix=\ncompadd -f -- 'file '\ncompadd -Q -S '' -- '--detach ' 'escaped\\ ' \"'quoted '\"\n").unwrap();
    let mut environment = Environment::fixture(root.path());
    environment.native = Arc::new(Native::detect());
    let result = environment
        .native
        .complete(
            &environment,
            &["shucked-spacing".into()],
            "",
            root.path(),
            &RequestCancellationToken::default(),
            false,
            "bash",
        )
        .unwrap();
    assert!(
        result
            .iter()
            .any(|item| item.text == "--flag" && !item.no_space),
        "{result:?}"
    );
    assert!(
        result
            .iter()
            .any(|item| item.text == "--prefix=" && item.no_space),
        "{result:?}"
    );
    assert!(result.iter().any(|item| item.text == "file "), "{result:?}");
    assert!(!result.iter().any(|item| item.text == "--flag "));
    assert!(
        result
            .iter()
            .any(|item| item.text == "--detach" && !item.no_space),
        "{result:?}"
    );
    assert!(
        result
            .iter()
            .any(|item| item.text == "escaped " && item.no_space),
        "{result:?}"
    );
    assert!(
        result
            .iter()
            .any(|item| item.text == "quoted " && item.no_space),
        "{result:?}"
    );
}
