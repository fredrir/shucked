use super::*;
use crate::session::{Client, RequestCancellationToken};
use crate::{GlobalOptions, PositionEncoding, Session, TextDocument, Workspaces};
use std::os::unix::fs::PermissionsExt;

fn fixture(
    root: &std::path::Path,
    filename: &str,
    text: &str,
    options: Option<crate::session::environment_options::EnvironmentOptions>,
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
    let uri = types::Url::from_file_path(root.join(filename)).unwrap();
    session.open_text_document(uri.clone(), TextDocument::new(text.into(), 1));
    if let Some(options) = options {
        session.select_environment(uri.clone(), Some(options));
    }
    let mut snapshot = session.take_snapshot(uri).unwrap();
    let mut service = CommandService::new(true);
    service.path = Some(vec![root.into()]);
    snapshot.environment_generation = service.generation();
    snapshot.command_service = Arc::new(service);
    snapshot
}

fn executable(path: &std::path::Path, text: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{text}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn cheap_command_resolution_never_runs_metadata_programs() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("metadata-ran");
    executable(
        &root.path().join("git"),
        &format!("echo yes > '{}'", marker.display()),
    );
    let snapshot = fixture(root.path(), "script.sh", "git invalid-command\n", None);
    let analysis = snapshot.command_service.analysis(&snapshot);
    assert!(matches!(
        analysis.sites[0].1,
        CommandResolution::Resolved(_)
    ));
    assert!(
        !marker.exists(),
        "resolving a completion/hover/token snapshot ran a program"
    );
    assert!(analysis.validation.lock().unwrap().is_none());
}

#[test]
fn cancelling_metadata_kills_the_query_and_does_not_cache_partial_validation() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("started");
    executable(
        &root.path().join("git"),
        &format!(
            "echo ready > '{}'; /bin/sleep 10; printf 'git version 2.50.0\\n'",
            marker.display()
        ),
    );
    let token = RequestCancellationToken::default();
    let snapshot = fixture(root.path(), "script.sh", "git invalid-command\n", None)
        .with_analysis_cancellation(token.clone());
    let analysis = snapshot.command_service.analysis(&snapshot);
    let worker_snapshot = snapshot.clone();
    let worker_analysis = analysis.clone();
    let (done, completion) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        validation(&worker_snapshot, &worker_analysis);
        done.send(()).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(marker.exists(), "metadata query did not start");
    token.cancel();
    completion
        .recv_timeout(Duration::from_millis(800))
        .expect("query must react to cancellation before its normal timeout");
    worker.join().unwrap();
    assert!(analysis.validation.lock().unwrap().is_none());
}

#[test]
fn attached_startup_file_does_not_use_post_startup_path_or_directory() {
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("only_after_startup"), "exit 0");
    let options = crate::session::environment_options::EnvironmentOptions {
        session_id: Some("terminal".into()),
        ..Default::default()
    };
    let snapshot = fixture(root.path(), ".zshrc", "only_after_startup\n", Some(options));
    snapshot.command_service.update_session(ShellSessionState {
        id: "terminal".into(),
        generation: 1,
        cwd: root.path().into(),
        path: vec![root.path().into()],
        aliases: BTreeMap::new(),
        functions: BTreeSet::new(),
        connected: true,
        shell: Some("zsh".into()),
        options: BTreeMap::new(),
    });
    let analysis = snapshot.command_service.analysis(&snapshot);
    assert!(matches!(analysis.sites[0].1, CommandResolution::Unknown(_)));
    assert!(!analysis.environment.path_known);
    assert!(analysis.context.cwd.is_none());
    assert!(!analysis.local_environment);
}

#[test]
fn captured_inventory_preserves_document_language_and_portable_policy() {
    let root = tempfile::tempdir().unwrap();
    let context = ExecutionContext {
        target_id: "offline-host".into(),
        dialect: ShellDialect::Fish,
        mode: shucked_command::ExecutionMode::InteractiveSession,
        ..Default::default()
    };
    let environment = EnvironmentSnapshot::empty(&context);
    let inventory = shucked_command::TargetInventory::capture("test host", &context, &environment);
    let path = root.path().join("target.json");
    std::fs::write(&path, inventory.to_json().unwrap()).unwrap();
    let options = crate::session::environment_options::EnvironmentOptions {
        policy: Some("portable".into()),
        target_inventory: Some(path),
        ..Default::default()
    };
    let snapshot = fixture(root.path(), ".zshrc", "setopt promptsubst\n", Some(options));
    let analysis = snapshot.command_service.analysis(&snapshot);
    assert_eq!(analysis.context.dialect, ShellDialect::Zsh);
    assert_eq!(
        analysis.context.mode,
        shucked_command::ExecutionMode::StartupFile
    );
    assert_eq!(analysis.context.policy, ValidationPolicy::Portable);
    assert_eq!(analysis.context.target_id, "offline-host");
    assert!(!analysis.context.native_execution_allowed);
    assert!(!analysis.local_environment);
    assert!(!analysis.environment.builtins_complete);
}

#[test]
fn special_files_cannot_block_target_inventory_loading() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("inventory.fifo");
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let options = crate::session::environment_options::EnvironmentOptions {
        target_inventory: Some(path),
        ..Default::default()
    };
    let snapshot = fixture(root.path(), "script.sh", "echo hello\n", Some(options));
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = snapshot.command_service.analysis(&snapshot);
        send.send(result.failure.clone()).unwrap();
    });
    let failure = receive
        .recv_timeout(Duration::from_secs(1))
        .expect("opening a named pipe must not wait for a writer");
    worker.join().unwrap();
    assert!(failure.unwrap().contains("regular file"));
}

#[test]
fn pull_preserves_completed_environment_feedback_until_snapshot_changes() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = fixture(
        root.path(),
        "script.sh",
        "absent_command_in_this_fixture\n",
        None,
    );
    assert!(
        cached_diagnostics(&snapshot).is_empty(),
        "pull must wait for the debounced worker"
    );
    let completed = diagnostics(&snapshot);
    assert_eq!(completed.len(), 1);
    assert_eq!(
        completed[0].code,
        Some(types::NumberOrString::String("ENV001".into()))
    );
    // Simulate expiry/eviction of the cheap resolver cache between editor pulls.
    snapshot.command_service.cache.lock().unwrap().clear();
    snapshot.command_service.analysis(&snapshot);
    assert_eq!(cached_diagnostics(&snapshot), completed);
    snapshot.command_service.invalidate();
    assert!(cached_diagnostics(&snapshot).is_empty());
}

#[test]
fn empty_editor_target_settings_use_workspace_resolution() {
    let root = tempfile::tempdir().unwrap();
    let options = serde_json::from_value(serde_json::json!({
        "policy": "workspace", "cwd": "", "targetInventory": "", "sessionId": ""
    }))
    .unwrap();
    let snapshot = fixture(
        root.path(),
        "script.sh",
        "definitely_missing_tool\n",
        Some(options),
    );
    let analysis = snapshot.command_service.analysis(&snapshot);
    assert_eq!(analysis.context.policy, ValidationPolicy::Workspace);
    assert!(analysis.local_environment);
    assert!(matches!(analysis.sites[0].1, CommandResolution::Missing(_)));
    assert!(analysis.failure.is_none());
}

fn live_session(id: &str, directory: &std::path::Path) -> ShellSessionState {
    ShellSessionState {
        id: id.into(),
        generation: 1,
        cwd: directory.into(),
        path: vec![],
        aliases: BTreeMap::new(),
        functions: BTreeSet::new(),
        connected: true,
        shell: Some("zsh".into()),
        options: BTreeMap::new(),
    }
}

#[test]
fn retired_sessions_do_not_exhaust_lifetime_attachment_capacity() {
    let root = tempfile::tempdir().unwrap();
    let service = CommandService::new(true);
    for index in 0..80 {
        let mut state = live_session(&format!("terminal-{index}"), root.path());
        service.update_session(state.clone());
        assert!(service.session(&state.id).unwrap().connected);
        state.connected = false;
        state.generation += 1;
        service.update_session(state);
    }
    service.update_session(live_session("new-terminal", root.path()));
    assert!(service.session("new-terminal").unwrap().connected);
}

#[test]
fn live_session_resolves_relative_executables_and_alias_targets() {
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("tool"), "exit 0");
    let options = crate::session::environment_options::EnvironmentOptions {
        session_id: Some("terminal".into()),
        ..Default::default()
    };
    let mut snapshot = fixture(root.path(), "script.zsh", "./tool\nshort\n", Some(options));
    let mut state = live_session("terminal", root.path());
    state.aliases.insert("short".into(), vec!["./tool".into()]);
    snapshot.command_service.update_session(state);
    snapshot.environment_generation = snapshot.command_service.generation();
    let analysis = snapshot.command_service.analysis(&snapshot);
    assert_eq!(analysis.sites.len(), 2);
    for (_, resolution) in &analysis.sites {
        assert!(
            matches!(resolution, CommandResolution::Resolved(_)),
            "{resolution:?}"
        );
    }
    let mut disconnected = live_session("terminal", root.path());
    disconnected.connected = false;
    disconnected.generation = 2;
    snapshot.command_service.update_session(disconnected);
    snapshot.environment_generation = snapshot.command_service.generation();
    let stale = snapshot.command_service.analysis(&snapshot);
    assert!(
        stale
            .sites
            .iter()
            .all(|(_, resolution)| matches!(resolution, CommandResolution::Unknown(_)))
    );
}

#[test]
fn edited_documents_reuse_path_inventory_but_refresh_exact_command_names() {
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("original_tool"), "exit 0");
    let first = fixture(root.path(), "script.sh", "original_tool\n", None);
    let initial = first.command_service.analysis(&first);
    assert!(
        initial.environment.search_path[0]
            .commands
            .contains_key("original_tool")
    );
    executable(&root.path().join("new_tool"), "exit 0");
    let mut edited = fixture(root.path(), "script.sh", "new_tool\n", None);
    edited.command_service = first.command_service.clone();
    let updated = edited.command_service.analysis(&edited);
    assert!(
        !updated.environment.search_path[0]
            .commands
            .contains_key("new_tool"),
        "PATH listing should be reused across source edits"
    );
    assert!(
        matches!(updated.sites[0].1, CommandResolution::Resolved(_)),
        "new invocation needs a fresh point lookup"
    );
    assert!(matches!(
        updated.environment.exact_lookups.get("new_tool"),
        Some(shucked_command::LookupEvidence::Present(_))
    ));
    edited.command_service.invalidate();
    edited.environment_generation = edited.command_service.generation();
    let refreshed = edited.command_service.analysis(&edited);
    assert!(
        refreshed.environment.search_path[0]
            .commands
            .contains_key("new_tool")
    );
}

#[test]
fn inventory_reuse_separates_launch_directory_confidence_and_path_order() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = fixture(root.path(), "script.sh", "echo hi\n", None);
    let mut context = ExecutionContext {
        cwd: Some(root.path().into()),
        ..Default::default()
    };
    let unknown = snapshot
        .command_service
        .capture_host(&context, &[PathBuf::new()], &snapshot);
    assert!(!unknown.is_complete());
    context.cwd_known = true;
    executable(&root.path().join("local_tool"), "exit 0");
    let known = snapshot
        .command_service
        .capture_host(&context, &[PathBuf::new()], &snapshot);
    assert!(known.search_path[0].commands.contains_key("local_tool"));
    let other =
        snapshot
            .command_service
            .capture_host(&context, &[root.path().join("missing")], &snapshot);
    assert!(!other.command_names().contains("local_tool"));
}

#[test]
fn local_function_before_later_source_survives_unavailable_workspace_index() {
    let root = tempfile::tempdir().unwrap();
    let mut snapshot = fixture(
        root.path(),
        "script.sh",
        "local_fn() { :; }\nlocal_fn\n. \"$LATER\"\nlocal_fn\n",
        None,
    );
    snapshot.workspace_functions = None;
    let analysis = snapshot.command_service.analysis(&snapshot);
    let calls = analysis
        .sites
        .iter()
        .filter(|(facts, _)| facts.name() == Some("local_fn"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert!(
        matches!(&calls[0].1, CommandResolution::Resolved(command) if command.kind == shucked_command::CommandKind::Function)
    );
    assert!(matches!(&calls[1].1, CommandResolution::Unknown(_)));
}
