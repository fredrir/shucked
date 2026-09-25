use super::*;
use crate::handlers::commands::{CommandService, EnvironmentSource};
use crate::session::{Client, DocumentSnapshot, environment_options::EnvironmentOptions};
use crate::{GlobalOptions, PositionEncoding, Session, TextDocument, Workspaces};
use std::os::unix::fs::PermissionsExt;

/// A fake login shell: a POSIX script that ignores `-l -i -c <script>` and
/// prints a record stream. `$HOME/alias-target` decides what `short` expands to.
fn fake_shell(home: &Path, name: &str, body: &str) -> PathBuf {
    let shell = home.join(name);
    std::fs::write(&shell, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    shell
}

const REPORT: &str = r#"printf 'cwd\0%s\0' "$PWD"
printf 'searchpath\0%s\0' "$HOME/bin:/usr/bin"
printf 'alias\0short=%s\0' "$(cat "$HOME/alias-target" 2>/dev/null || echo printf)"
printf "alias\0ll='ls -l'\0"
printf "alias\0complex='cd /tmp && ls'\0"
printf 'function\0greet\0'
printf 'function\0__shucked_capture\0'
printf 'option\0expand_aliases=1\0'
printf 'option\0capture=%s\0' "${SHUCKED_CAPTURE-unset}"
printf 'option\0term=%s\0' "${TERM-unset}"
printf 'end\0shucked-login-shell\0'"#;

fn service(
    home: &Path,
    shell: &Path,
    native_allowed: bool,
    timeout: Duration,
) -> (Arc<LoginShellService>, TestClient) {
    let (events, event_receiver) = crossbeam::channel::unbounded();
    let (messages, message_receiver) = crossbeam::channel::unbounded();
    let client = Client::new(events, messages);
    let service = Arc::new(
        LoginShellService::new(native_allowed, Some(client)).with_host(
            StartupLocations::private(home),
            Some(shell.to_path_buf()),
            timeout,
        ),
    );
    (
        service,
        TestClient {
            events: event_receiver,
            messages: message_receiver,
        },
    )
}

struct TestClient {
    events: crossbeam::channel::Receiver<crate::server::Event>,
    messages: crossbeam::channel::Receiver<lsp_server::Message>,
}

impl TestClient {
    fn shown_messages(&self) -> Vec<String> {
        self.messages
            .try_iter()
            .filter_map(|message| match message {
                lsp_server::Message::Notification(notification)
                    if notification.method == "window/showMessage" =>
                {
                    notification.params["message"].as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }
}

fn wait_settled(service: &Arc<LoginShellService>, shell: &Path) -> LoginShellStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = service.status(shell);
        if status != LoginShellStatus::Pending || Instant::now() > deadline {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn record_stream_parses_like_the_terminal_hook_payload() {
    let stream = b"cwd\0/home/me\0path\0/opt/bin\0path\0\0alias\0ls=eza --icons\0alias\0gs='git status'\0alias\0dangerous=touch x; rm y\0function\0deploy\0function\0__shucked_capture\0function\0bad name\0option\0aliases=on\0end\0shucked-login-shell\0";
    let parsed = parse_records(stream).unwrap();
    assert_eq!(parsed.cwd.as_deref(), Some(Path::new("/home/me")));
    assert_eq!(parsed.path, vec![PathBuf::from("/opt/bin"), PathBuf::new()]);
    assert_eq!(parsed.aliases["ls"], vec!["eza", "--icons"]);
    assert!(
        !parsed.aliases.contains_key("gs"),
        "value-bearing arguments stay private"
    );
    assert!(parsed.functions.contains("gs"));
    assert!(!parsed.aliases.contains_key("dangerous"));
    assert!(
        parsed.functions.contains("dangerous"),
        "opaque aliases are names only"
    );
    assert!(parsed.functions.contains("deploy"));
    assert!(!parsed.functions.contains("__shucked_capture"));
    assert!(!parsed.functions.contains("bad name"));
    assert_eq!(parsed.options["aliases"], "on");
}

#[test]
fn bash_alias_lines_and_missing_end_marker_are_handled() {
    let stream = b"searchpath\0/a:/b\0alias\0alias ll='ls -l'\0alias\0alias q='it'\"'\"'s'\0end\0shucked-login-shell\0";
    let parsed = parse_records(stream).unwrap();
    assert_eq!(parsed.path, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    assert_eq!(parsed.aliases["ll"], vec!["ls", "-l"]);
    assert!(
        parsed.functions.contains("q"),
        "quotes inside values are opaque"
    );
    let error = parse_records(b"cwd\0/x\0").unwrap_err();
    assert!(error.contains("did not finish"), "{error}");
    assert!(parse_records(&vec![b'x'; MAX_OUTPUT_BYTES + 1]).is_err());
}

#[test]
fn shell_kind_and_default_resolution_follow_the_executable_name() {
    assert_eq!(
        ShellKind::from_path(Path::new("/opt/homebrew/bin/zsh")),
        ShellKind::Zsh
    );
    assert_eq!(
        ShellKind::from_path(Path::new("/bin/bash")),
        ShellKind::Bash
    );
    assert_eq!(
        ShellKind::from_path(Path::new("/usr/local/bin/fish")),
        ShellKind::Fish
    );
    assert_eq!(
        ShellKind::from_path(Path::new("/bin/dash")),
        ShellKind::Posix
    );
    assert_eq!(
        resolve_shell(Some(Path::new("/x/zsh")), Some(Path::new("/bin/bash"))),
        PathBuf::from("/x/zsh")
    );
    assert_eq!(
        resolve_shell(Some(Path::new("")), Some(Path::new("/bin/bash"))),
        PathBuf::from("/bin/bash")
    );
    let fallback = resolve_shell(None, None);
    assert!(fallback == Path::new("/bin/zsh") || fallback == Path::new("/bin/bash"));
    let files = startup_files(ShellKind::Zsh, &StartupLocations::private(Path::new("/h")));
    assert!(files.contains(&PathBuf::from("/h/.zprofile")));
    assert!(files.contains(&PathBuf::from("/h/.zshrc")));
}

#[test]
fn capture_success_populates_path_aliases_and_functions() {
    let home = tempfile::tempdir().unwrap();
    let shell = fake_shell(home.path(), "bash", REPORT);
    let (service, client) = service(home.path(), &shell, true, Duration::from_secs(5));
    assert_eq!(service.lookup(&shell), LoginShellStatus::Pending);
    let LoginShellStatus::Ready(state) = wait_settled(&service, &shell) else {
        panic!("capture did not succeed: {:?}", service.status(&shell));
    };
    assert_eq!(state.id, SESSION_ID);
    assert_eq!(state.shell.as_deref(), Some("bash"));
    assert_eq!(
        state.path,
        vec![home.path().join("bin"), PathBuf::from("/usr/bin")]
    );
    assert_eq!(state.aliases["short"], vec!["printf"]);
    assert_eq!(state.aliases["ll"], vec!["ls", "-l"]);
    assert!(!state.aliases.contains_key("complex"));
    assert!(state.functions.contains("complex"));
    assert!(state.functions.contains("greet"));
    assert!(!state.functions.contains("__shucked_capture"));
    assert_eq!(
        state.options["capture"], "1",
        "SHUCKED_CAPTURE guards slow startup code"
    );
    assert_eq!(state.options["term"], "dumb");
    assert!(state.connected);
    assert_eq!(service.captures_started(), 1);
    assert!(client.shown_messages().is_empty(), "success is silent");
    assert!(
        client
            .events
            .try_iter()
            .any(|event| matches!(event, crate::server::Event::EnvironmentTick)),
        "completion must schedule an environment refresh"
    );
    // The second lookup and an unchanged refresh reuse the capture.
    assert!(matches!(service.lookup(&shell), LoginShellStatus::Ready(_)));
    service.refresh();
    assert_eq!(service.captures_started(), 1);
    assert!(
        service
            .watch_directories()
            .contains(&home.path().join("bin"))
    );
}

#[test]
fn timeout_falls_back_and_reports_once() {
    let home = tempfile::tempdir().unwrap();
    let shell = fake_shell(
        home.path(),
        "zsh",
        "printf 'cwd\\0%s\\0' \"$PWD\"; /bin/sleep 30",
    );
    let (service, client) = service(home.path(), &shell, true, Duration::from_millis(300));
    let started = Instant::now();
    service.lookup(&shell);
    let status = wait_settled(&service, &shell);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timeout must kill the shell"
    );
    let LoginShellStatus::Failed(reason) = status else {
        panic!("expected a timeout failure, got {status:?}");
    };
    assert!(reason.contains("timed out"), "{reason}");
    let shown = client.shown_messages();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains("timed out"));
    // Failures back off: a refresh right away does not start another shell.
    service.refresh();
    assert_eq!(service.captures_started(), 1);
    // Re-selecting the context retries immediately, and the notice is not repeated.
    service.retry_failed();
    assert_eq!(service.captures_started(), 2);
    wait_settled(&service, &shell);
    assert!(
        client.shown_messages().is_empty(),
        "the notice is shown once per shell"
    );
}

#[test]
fn untrusted_workspaces_never_start_the_login_shell() {
    let home = tempfile::tempdir().unwrap();
    let marker = home.path().join("ran");
    let shell = fake_shell(
        home.path(),
        "bash",
        &format!("touch '{}'; {REPORT}", marker.display()),
    );
    let (service, _client) = service(home.path(), &shell, false, Duration::from_secs(5));
    assert!(matches!(
        service.lookup(&shell),
        LoginShellStatus::Disabled(_)
    ));
    service.refresh();
    service.retry_failed();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(service.captures_started(), 0);
    assert!(
        !marker.exists(),
        "an untrusted workspace ran the login shell"
    );
}

#[test]
fn refresh_recaptures_after_the_startup_fingerprint_changes() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("alias-target"), "printf\n").unwrap();
    let shell = fake_shell(home.path(), "bash", REPORT);
    let (service, _client) = service(home.path(), &shell, true, Duration::from_secs(5));
    service.lookup(&shell);
    let LoginShellStatus::Ready(first) = wait_settled(&service, &shell) else {
        panic!("first capture failed");
    };
    assert_eq!(first.aliases["short"], vec!["printf"]);
    std::fs::write(home.path().join("alias-target"), "echo\n").unwrap();
    service.refresh();
    assert_eq!(
        service.captures_started(),
        1,
        "unchanged startup files keep the capture"
    );
    // A new ~/.bashrc changes the fingerprint of a bash login shell.
    std::fs::write(home.path().join(".bashrc"), "alias short=echo\n").unwrap();
    service.refresh();
    assert_eq!(service.captures_started(), 2);
    let LoginShellStatus::Ready(second) = wait_settled(&service, &shell) else {
        panic!("second capture failed");
    };
    assert_eq!(second.aliases["short"], vec!["echo"]);
    assert!(second.generation > first.generation);
}

fn document(
    root: &Path,
    text: &str,
    options: EnvironmentOptions,
    command_service: Arc<CommandService>,
) -> DocumentSnapshot {
    let (events, _) = crossbeam::channel::unbounded();
    let (messages, _) = crossbeam::channel::unbounded();
    let client = Client::new(events, messages);
    let global_settings = GlobalOptions::default().into_settings(client.clone());
    let mut session = Session::new(
        &Default::default(),
        PositionEncoding::UTF16,
        global_settings,
        &Workspaces::new(vec![]),
        &client,
    )
    .unwrap();
    let uri = lsp_types::Url::from_file_path(root.join("script.sh")).unwrap();
    session.open_text_document(uri.clone(), TextDocument::new(text.into(), 1));
    session.select_environment(uri.clone(), Some(options));
    let mut snapshot = session.take_snapshot(uri).unwrap();
    snapshot.environment_generation = command_service.generation();
    snapshot.command_service = command_service;
    snapshot
}

#[test]
fn login_shell_policy_feeds_analysis_through_the_session_path() {
    let home = tempfile::tempdir().unwrap();
    let shell = fake_shell(home.path(), "bash", REPORT);
    std::fs::create_dir_all(home.path().join("bin")).unwrap();
    fake_shell(&home.path().join("bin"), "only_in_login_path", "exit 0");
    let (service, _client) = service(home.path(), &shell, true, Duration::from_secs(5));
    let commands = Arc::new(CommandService::new(true));
    commands.install_login_shell(service.clone());
    let options = EnvironmentOptions {
        policy: Some(POLICY.into()),
        login_shell: Some(shell.clone()),
        ..Default::default()
    };
    let text = "short hello\nonly_in_login_path\ngreet\n";
    let pending = document(home.path(), text, options.clone(), commands.clone());
    let first = pending.command_service.analysis(&pending);
    assert!(matches!(
        first.source,
        EnvironmentSource::LoginShellFallback { .. }
    ));
    assert!(first.failure.as_deref().unwrap().contains("in progress"));
    assert!(first.local_environment);
    assert!(matches!(
        wait_settled(&service, &shell),
        LoginShellStatus::Ready(_)
    ));
    commands.invalidate();
    let ready = document(home.path(), text, options, commands.clone());
    let analysis = ready.command_service.analysis(&ready);
    assert_eq!(
        analysis.source,
        EnvironmentSource::LoginShell {
            shell: shell.clone()
        }
    );
    assert!(analysis.failure.is_none());
    assert_eq!(analysis.context.target_id, SESSION_ID);
    assert_eq!(
        analysis.context.policy,
        shucked_command::ValidationPolicy::Session
    );
    assert_eq!(
        analysis.context.mode,
        shucked_command::ExecutionMode::InteractiveSession
    );
    assert_eq!(analysis.sites.len(), 3);
    for (site, resolution) in &analysis.sites {
        assert!(
            matches!(resolution, shucked_command::CommandResolution::Resolved(_)),
            "{:?}: {resolution:?}",
            site.name()
        );
    }
    let hover = crate::handlers::commands::hover(&ready, 0).unwrap();
    let lsp_types::HoverContents::Markup(content) = hover.contents else {
        panic!("expected markup hover");
    };
    assert!(
        content.value.contains("Environment: login shell ("),
        "{}",
        content.value
    );
    let host = crate::handlers::commands::HostDetails::detect(true);
    let details = crate::handlers::commands::environment_details(&ready, Some(0), &host);
    assert!(details.contains("Login shell:"), "{details}");
    assert!(
        details.contains("captured (2 PATH entries, 2 aliases"),
        "{details}"
    );
    assert!(details.contains("- Name: `short`"), "{details}");
    assert!(details.contains("Alias chain"), "{details}");
}

#[test]
fn oversized_terminal_state_is_truncated_not_dropped() {
    let commands = CommandService::new(true);
    let mut state = super::super::commands::ShellSessionState {
        id: "terminal".into(),
        generation: 1,
        cwd: PathBuf::from("/"),
        path: vec![],
        aliases: (0..(super::super::commands::MAX_SESSION_NAMES + 10))
            .map(|index| (format!("a{index:06}"), vec!["printf".to_owned()]))
            .collect(),
        functions: (0..(super::super::commands::MAX_SESSION_NAMES + 10))
            .map(|index| format!("f{index:06}"))
            .collect(),
        connected: true,
        live_completion: false,
        shell: Some("zsh".into()),
        options: Default::default(),
    };
    commands.update_session(state.clone());
    let stored = commands
        .session("terminal")
        .expect("truncated state is kept");
    assert_eq!(
        stored.aliases.len(),
        super::super::commands::MAX_SESSION_NAMES
    );
    assert_eq!(
        stored.functions.len(),
        super::super::commands::MAX_SESSION_NAMES
    );
    assert!(stored.aliases.contains_key("a000000"));
    // A client cannot impersonate the login-shell state.
    state.id = SESSION_ID.into();
    state.generation = 2;
    commands.update_session(state);
    assert!(commands.session(SESSION_ID).is_none());
}
