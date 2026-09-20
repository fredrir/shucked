use super::*;
use crate::server::api::requests::{Completion, Definition};
use shucked_command::{CommandKind, CommandResolution};

fn definition(
    session: &Session,
    client: &Client,
    position: types::TextDocumentPositionParams,
) -> Vec<types::Location> {
    let params = types::GotoDefinitionParams {
        text_document_position_params: position,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let snapshot =
        Definition::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    match Definition::run_with_snapshot(snapshot, client, params).unwrap() {
        Some(types::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(types::GotoDefinitionResponse::Array(locations)) => locations,
        None => Vec::new(),
        _ => panic!("unexpected definition response"),
    }
}
fn is_function(session: &Session, uri: &types::Url, name: &str) -> bool {
    let snapshot = session.take_snapshot(uri.clone()).unwrap();
    snapshot.command_service.analysis(&snapshot).sites.iter().any(|(site, resolution)| site.name() == Some(name) && matches!(resolution, CommandResolution::Resolved(command) if command.kind == CommandKind::Function))
}
fn missing(session: &Session, uri: &types::Url, name: &str) -> bool {
    let snapshot = session.take_snapshot(uri.clone()).unwrap();
    snapshot
        .command_service
        .analysis(&snapshot)
        .sites
        .iter()
        .any(|(site, resolution)| {
            site.name() == Some(name) && matches!(resolution, CommandResolution::Missing(_))
        })
}

#[test]
fn module_loader_connects_commands_navigation_references_completion_and_hierarchy() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("02-utils.zsh");
    std::fs::write(&helper, "add_path() { :; }\nhas_cmd() { :; }\n").unwrap();
    let caller = root.path().join("03-paths.zsh");
    std::fs::write(&caller, "add_path /bin\nhas_cmd editor\n").unwrap();
    std::fs::write(root.path().join("init.zsh"), format!("if [[ -n $AGENT ]]; then source {0}/02-utils.zsh; source {0}/03-paths.zsh; return; fi\nfor module in {0}/0[2-9]-*.zsh(N); do source \"$module\"; done\n", root.path().display())).unwrap();
    let (mut session, client, _messages) = session(root.path());
    let caller_uri = open(
        &mut session,
        &caller,
        &std::fs::read_to_string(&caller).unwrap(),
    );
    let helper_uri = open(
        &mut session,
        &helper,
        &std::fs::read_to_string(&helper).unwrap(),
    );
    assert!(is_function(&session, &caller_uri, "add_path"));
    assert!(is_function(&session, &caller_uri, "has_cmd"));
    let selected = position(&caller_uri, 0, 2);
    let definitions = definition(&session, &client, selected.clone());
    assert_eq!(definitions.len(), 1);
    assert!(definitions[0].uri.path().ends_with("02-utils.zsh"));
    assert_eq!(
        references(&session, &client, position(&helper_uri, 0, 2), false).len(),
        1
    );
    let details = hover(&session, &client, selected.clone());
    assert!(markdown(&details).contains("init.zsh"));
    assert!(markdown(&details).contains("Workspace call sites: 1"));
    let params = types::CompletionParams {
        text_document_position: position(&caller_uri, 0, 3),
        context: None,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let snapshot =
        Completion::snapshot(&session, &params, RequestCancellationToken::default()).unwrap();
    let completions = Completion::run_with_snapshot(snapshot, &client, params)
        .unwrap()
        .unwrap();
    let items = match completions {
        types::CompletionResponse::Array(items) => items,
        types::CompletionResponse::List(list) => list.items,
    };
    assert!(items.iter().any(|item| item.label == "add_path"));
    let context = session.workspace_function_context(RequestCancellationToken::default());
    let items = crate::call_hierarchy::prepare_call_hierarchy(
        context.clone(),
        session.take_snapshot(caller_uri).unwrap(),
        &client,
        types::CallHierarchyPrepareParams {
            text_document_position_params: selected,
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    let incoming = crate::call_hierarchy::incoming_calls(
        context,
        types::CallHierarchyIncomingCallsParams {
            item: items[0].clone(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].from_ranges[0].start.line, 0);
}

#[test]
fn unsaved_helper_edits_refresh_command_results_without_touching_caller() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper.sh");
    std::fs::write(&helper, "workspace_helper() { :; }\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source ./helper.sh\nworkspace_helper\n",
    );
    let helper_uri = open(&mut session, &helper, "workspace_helper() { :; }\n");
    assert!(is_function(&session, &uri, "workspace_helper"));
    let generation = session
        .take_snapshot(uri.clone())
        .unwrap()
        .environment_generation();
    session
        .update_text_document(
            &session.key_from_url(helper_uri.clone()),
            vec![types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "other_helper() { :; }\n".into(),
            }],
            2,
        )
        .unwrap();
    assert!(!is_function(&session, &uri, "workspace_helper"));
    assert!(definition(&session, &client, position(&uri, 1, 2)).is_empty());
    assert_eq!(
        session
            .take_snapshot(uri.clone())
            .unwrap()
            .environment_generation(),
        generation,
        "source edits must retain host inventory"
    );
    session
        .close_document(&session.key_from_url(helper_uri))
        .unwrap();
    assert!(is_function(&session, &uri, "workspace_helper"));
}

#[test]
fn unknown_sources_keep_navigation_candidates_without_claiming_exact_bindings() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper.sh");
    std::fs::write(&helper, "workspace_helper() { :; }\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source ./helper.sh\nsource \"$PLUGIN\"\nworkspace_helper\n",
    );
    assert!(!missing(&session, &uri, "workspace_helper"));
    assert!(!is_function(&session, &uri, "workspace_helper"));
    assert_eq!(definition(&session, &client, position(&uri, 2, 2)).len(), 1);
    let details = hover(&session, &client, position(&uri, 2, 2));
    assert!(markdown(&details).contains("Incomplete results"));
}

#[test]
fn module_creation_deletion_and_move_refresh_bindings_and_preserve_source_order() {
    let root = tempfile::tempdir().unwrap();
    let caller = root.path().join("03-caller.zsh");
    std::fs::write(&caller, "workspace_helper\n").unwrap();
    std::fs::write(
        root.path().join("init.zsh"),
        format!(
            "for module in {}/0*-*.zsh(N); do source \"$module\"; done\n",
            root.path().display()
        ),
    )
    .unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(&mut session, &caller, "workspace_helper\n");
    assert!(!is_function(&session, &uri, "workspace_helper"));
    let helper = root.path().join("02-helper.zsh");
    std::fs::write(&helper, "workspace_helper() { :; }\n").unwrap();
    let event = |path: &Path, typ| types::FileEvent {
        uri: types::Url::from_file_path(path).unwrap(),
        typ,
    };
    session.reload_settings(&[event(&helper, types::FileChangeType::CREATED)], &client);
    assert!(is_function(&session, &uri, "workspace_helper"));
    let later = root.path().join("04-helper.zsh");
    std::fs::rename(&helper, &later).unwrap();
    session.reload_settings(
        &[
            event(&helper, types::FileChangeType::DELETED),
            event(&later, types::FileChangeType::CREATED),
        ],
        &client,
    );
    assert!(!is_function(&session, &uri, "workspace_helper"));
    assert!(definition(&session, &client, position(&uri, 0, 2)).is_empty());
    std::fs::remove_file(&later).unwrap();
    session.reload_settings(&[event(&later, types::FileChangeType::DELETED)], &client);
    assert!(!is_function(&session, &uri, "workspace_helper"));
}

#[test]
fn conditional_definitions_return_all_candidates_and_keep_rename_strict() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.sh"), "helper() { :; }\n").unwrap();
    std::fs::write(root.path().join("b.sh"), "helper() { :; }\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "if ready; then source ./a.sh; else source ./b.sh; fi\nhelper\n",
    );
    let selected = position(&uri, 1, 2);
    assert_eq!(definition(&session, &client, selected.clone()).len(), 2);
    assert!(!missing(&session, &uri, "helper"));
    let params = types::TextDocumentPositionParams { ..selected.clone() };
    let snapshot = session.take_snapshot(uri.clone()).unwrap();
    let analysis = snapshot.analysis().unwrap();
    let call = analysis
        .semantic()
        .all_call_sites()
        .find(|call| call.callee.as_str() == "helper")
        .unwrap();
    let context = session.workspace_function_context(RequestCancellationToken::default());
    let index = crate::workspace_functions::workspace_function_index(&context).unwrap();
    let path = crate::workspace_functions::canonical_path(&uri.to_file_path().unwrap());
    assert!(
        index
            .resolve_call_site_exact(&path, call.name_span, &context.cancellation)
            .is_none()
    );
    let items = crate::call_hierarchy::prepare_call_hierarchy(
        context,
        snapshot,
        &client,
        types::CallHierarchyPrepareParams {
            text_document_position_params: params,
            work_done_progress_params: Default::default(),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(items.len(), 2);
    assert!(
        items
            .iter()
            .all(|item| item.detail.as_deref() == Some("Possible workspace binding"))
    );
}

#[test]
fn function_lookup_respects_command_builtin_and_env_namespaces() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, _client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "unique_workspace_helper() { :; }\nunique_workspace_helper\ncommand unique_workspace_helper\nbuiltin unique_workspace_helper\nenv unique_workspace_helper\n",
    );
    let snapshot = session.take_snapshot(uri).unwrap();
    let analysis = snapshot.command_service.analysis(&snapshot);
    let calls = analysis
        .sites
        .iter()
        .filter(|(site, _)| site.name() == Some("unique_workspace_helper"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 4);
    assert!(
        matches!(&calls[0].1, CommandResolution::Resolved(command) if command.kind == CommandKind::Function)
    );
    assert!(calls[1..].iter().all(|(_, resolution)| !matches!(resolution, CommandResolution::Resolved(command) if command.kind == CommandKind::Function)));
}

#[test]
fn file_limit_keeps_known_function_references_and_explains_partial_discovery() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("closed.sh"), "other_helper() { :; }\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    let mut options = crate::ClientOptions::default();
    options.server.call_hierarchy.max_files = 1;
    session.update_client_options(options);
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "helper() { :; }\nhelper\n",
    );
    let details = hover(&session, &client, position(&uri, 0, 2));
    assert!(markdown(&details).contains("workspace file limit (1) reached"));
    assert_eq!(
        references(&session, &client, position(&uri, 0, 2), false).len(),
        1
    );
    assert!(is_function(&session, &uri, "helper"));
}
