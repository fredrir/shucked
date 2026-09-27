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

#[test]
fn startup_file_outside_the_roots_that_loads_the_workspace_supplies_definitions() {
    // `HOME` is pinned per thread instead of through the environment: the
    // test runner is parallel and the process environment is shared.
    let home = tempfile::tempdir().unwrap();
    let home_path = std::fs::canonicalize(home.path()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let lib = root_path.join("lib.zsh");
    let other = root_path.join("other.zsh");
    std::fs::write(&lib, "greet world\n").unwrap();
    std::fs::write(&other, "greet nobody\n").unwrap();
    let zshrc = home_path.join(".zshrc");
    std::fs::write(
        &zshrc,
        format!(
            "greet() {{ print \"hi $1\"; }}\nsource {}/lib.zsh\ngreet again\n",
            root_path.display()
        ),
    )
    .unwrap();
    crate::handlers::workspace_functions::with_test_home_dir(&home_path, || {
        let (mut session, client, messages) = session(&root_path);
        let lib_uri = open(&mut session, &lib, "greet world\n");
        let other_uri = open(&mut session, &other, "greet nobody\n");
        assert!(is_function(&session, &lib_uri, "greet"));
        assert!(!is_function(&session, &other_uri, "greet"));

        let selected = position(&lib_uri, 0, 2);
        let definitions = definition(&session, &client, selected.clone());
        assert_eq!(definitions.len(), 1, "{definitions:?}");
        assert_eq!(definitions[0].uri.to_file_path().unwrap(), zshrc);
        assert_eq!(definitions[0].range.start.line, 0);

        let details = hover(&session, &client, selected.clone());
        let text = markdown(&details);
        assert!(text.contains(".zshrc:1"), "{text}");
        assert!(text.contains("Workspace call sites: 2"), "{text}");
        assert!(!text.contains("Incomplete results"), "{text}");

        let mut references = references(&session, &client, selected, false)
            .into_iter()
            .map(|location| {
                (
                    location.uri.to_file_path().unwrap(),
                    location.range.start.line,
                )
            })
            .collect::<Vec<_>>();
        references.sort();
        let mut expected = vec![(zshrc.clone(), 2), (lib.clone(), 0)];
        expected.sort();
        assert_eq!(references, expected);
        // Nothing was incomplete, so no notice was shown.
        assert!(
            !messages
                .try_iter()
                .any(|message| matches!(message, lsp_server::Message::Notification(n) if n.method == "window/showMessage"))
        );

        // The unsourced file keeps its own, unresolved, view.
        assert!(definition(&session, &client, position(&other_uri, 0, 2)).is_empty());
    });
}

fn implementation(
    session: &Session,
    client: &Client,
    position: types::TextDocumentPositionParams,
) -> Vec<types::Location> {
    use crate::server::api::requests::Implementation;
    let params = types::GotoDefinitionParams {
        text_document_position_params: position,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let snapshot =
        Implementation::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    match Implementation::run_with_snapshot(snapshot, client, params).unwrap() {
        Some(types::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(types::GotoDefinitionResponse::Array(locations)) => locations,
        None => Vec::new(),
        _ => panic!("unexpected implementation response"),
    }
}

fn shown_messages(messages: &crossbeam::channel::Receiver<lsp_server::Message>) -> Vec<String> {
    messages
        .try_iter()
        .filter_map(|message| match message {
            lsp_server::Message::Notification(notification)
                if notification.method == "window/showMessage" =>
            {
                Some(notification.params.to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn oh_my_zsh_bootstrap_resolves_the_framework_lib_and_plugin_files() {
    let home = tempfile::tempdir().unwrap();
    let home_path = std::fs::canonicalize(home.path()).unwrap();
    let omz = home_path.join(".oh-my-zsh");
    std::fs::create_dir_all(omz.join("lib")).unwrap();
    std::fs::create_dir_all(omz.join("plugins/git")).unwrap();
    std::fs::create_dir_all(omz.join("plugins/docker")).unwrap();
    std::fs::create_dir_all(omz.join("custom/plugins")).unwrap();
    // The real bootstrap globs its lib directory and iterates `$plugins`;
    // neither is statically followable, which is what the framework
    // contract stands in for.
    std::fs::write(
        omz.join("oh-my-zsh.sh"),
        "for config_file (\"$ZSH\"/lib/*.zsh); do\n  source \"$config_file\"\ndone\n\
         for plugin ($plugins); do\n  if [[ -f \"$ZSH_CUSTOM/plugins/$plugin/$plugin.plugin.zsh\" ]]; then\n    source \"$ZSH_CUSTOM/plugins/$plugin/$plugin.plugin.zsh\"\n  elif [[ -f \"$ZSH/plugins/$plugin/$plugin.plugin.zsh\" ]]; then\n    source \"$ZSH/plugins/$plugin/$plugin.plugin.zsh\"\n  fi\ndone\n",
    )
    .unwrap();
    std::fs::write(omz.join("lib/git.zsh"), "git_current_branch() { :; }\n").unwrap();
    std::fs::write(
        omz.join("plugins/git/git.plugin.zsh"),
        "gst() { git status; }\n",
    )
    .unwrap();
    std::fs::write(
        omz.join("plugins/docker/docker.plugin.zsh"),
        "dps() { docker ps; }\n",
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let zshrc = root_path.join(".zshrc");
    let source = "export EDITOR=vim\nexport ZSH=\"$HOME/.oh-my-zsh\"\nplugins=(git)\nsource $ZSH/oh-my-zsh.sh\ngst\ngit_current_branch\ndps\necho \"$EDITOR\"\n";
    std::fs::write(&zshrc, source).unwrap();
    crate::handlers::workspace_functions::with_test_home_dir(&home_path, || {
        let (mut session, client, messages) = session(&root_path);
        let uri = open(&mut session, &zshrc, source);
        assert!(is_function(&session, &uri, "gst"));
        assert!(is_function(&session, &uri, "git_current_branch"));
        // A plugin that is installed but not selected is not loaded.
        assert!(!is_function(&session, &uri, "dps"));

        let definitions = definition(&session, &client, position(&uri, 4, 1));
        assert_eq!(definitions.len(), 1, "{definitions:?}");
        assert_eq!(
            definitions[0].uri.to_file_path().unwrap(),
            omz.join("plugins/git/git.plugin.zsh")
        );
        let definitions = definition(&session, &client, position(&uri, 5, 3));
        assert_eq!(definitions.len(), 1, "{definitions:?}");
        assert_eq!(
            definitions[0].uri.to_file_path().unwrap(),
            omz.join("lib/git.zsh")
        );

        let text = hover(&session, &client, position(&uri, 4, 1));
        let text = markdown(&text);
        assert!(text.contains("git.plugin.zsh:1"), "{text}");
        assert!(text.contains("Loaded through"), "{text}");
        assert!(text.contains(".zshrc"), "{text}");
        assert!(!text.contains("Incomplete results"), "{text}");

        // The bootstrap operand opens the whole framework sequence, bootstrap first.
        let loaded = implementation(&session, &client, position(&uri, 3, 12))
            .into_iter()
            .map(|location| location.uri.to_file_path().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            loaded,
            vec![
                omz.join("oh-my-zsh.sh"),
                omz.join("lib/git.zsh"),
                omz.join("plugins/git/git.plugin.zsh"),
            ]
        );
        let text = hover(&session, &client, position(&uri, 3, 12));
        assert!(
            markdown(&text).contains("Files loaded through oh-my-zsh"),
            "{}",
            markdown(&text)
        );

        // A value visible across the bootstrap: the framework load is
        // followed, so nothing is reported as incomplete.
        let uses = references(&session, &client, position(&uri, 0, 8), false);
        assert_eq!(uses.len(), 1, "{uses:?}");
        assert_eq!(uses[0].range.start.line, 7);
        assert!(
            shown_messages(&messages).is_empty(),
            "{:?}",
            shown_messages(&messages)
        );
    });
}

#[test]
fn prezto_module_loads_resolve_to_module_init_files() {
    let home = tempfile::tempdir().unwrap();
    let home_path = std::fs::canonicalize(home.path()).unwrap();
    let prezto = home_path.join(".zprezto");
    std::fs::create_dir_all(prezto.join("modules/utility")).unwrap();
    std::fs::create_dir_all(prezto.join("runcoms")).unwrap();
    std::fs::write(
        prezto.join("init.zsh"),
        "zstyle -a ':prezto:load' pmodule 'pmodules'\n\
         for pmodule in \"$pmodules[@]\"; do\n  source \"${ZDOTDIR:-$HOME}/.zprezto/modules/$pmodule/init.zsh\"\ndone\n",
    )
    .unwrap();
    std::fs::write(
        prezto.join("modules/utility/init.zsh"),
        "mkdcd() { mkdir -p \"$1\" && cd \"$1\"; }\n",
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    // A dotfiles-repository layout: `zshrc` is still zsh, but unlike a file
    // named `.zshrc` it does not make its own directory the `ZDOTDIR`.
    let zshrc = root_path.join("zshrc");
    let source = "zstyle ':prezto:load' pmodule 'environment' 'utility'\nsource \"${ZDOTDIR:-$HOME}/.zprezto/init.zsh\"\nmkdcd build\n";
    std::fs::write(&zshrc, source).unwrap();
    crate::handlers::workspace_functions::with_test_home_dir(&home_path, || {
        let (mut session, client, messages) = session(&root_path);
        let uri = open(&mut session, &zshrc, source);
        assert!(is_function(&session, &uri, "mkdcd"));
        let definitions = definition(&session, &client, position(&uri, 2, 1));
        assert_eq!(definitions.len(), 1, "{definitions:?}");
        assert_eq!(
            definitions[0].uri.to_file_path().unwrap(),
            prezto.join("modules/utility/init.zsh")
        );
        let text = hover(&session, &client, position(&uri, 2, 1));
        assert!(
            !markdown(&text).contains("Incomplete results"),
            "{}",
            markdown(&text)
        );
        // The module statement itself leads to the installed module; the
        // missing `environment` module is simply absent.
        let loaded = implementation(&session, &client, position(&uri, 0, 3))
            .into_iter()
            .map(|location| location.uri.to_file_path().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(loaded, vec![prezto.join("modules/utility/init.zsh")]);
        assert!(shown_messages(&messages).is_empty());
    });
}

fn declaration(
    session: &Session,
    client: &Client,
    position: types::TextDocumentPositionParams,
) -> Vec<types::Location> {
    use crate::server::api::requests::Declaration;
    let params = types::GotoDefinitionParams {
        text_document_position_params: position,
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let snapshot =
        Declaration::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    match Declaration::run_with_snapshot(snapshot, client, params).unwrap() {
        Some(types::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(types::GotoDefinitionResponse::Array(locations)) => locations,
        None => Vec::new(),
        _ => panic!("unexpected declaration response"),
    }
}

fn paths(locations: &[types::Location]) -> Vec<(std::path::PathBuf, u32)> {
    locations
        .iter()
        .map(|location| {
            (
                location.uri.to_file_path().unwrap(),
                location.range.start.line,
            )
        })
        .collect()
}

#[test]
fn autoload_declarations_navigate_to_the_function_file_on_fpath() {
    let home = tempfile::tempdir().unwrap();
    let home_path = std::fs::canonicalize(home.path()).unwrap();
    let functions = home_path.join("functions");
    std::fs::create_dir_all(&functions).unwrap();
    std::fs::write(
        functions.join("myfunc"),
        "# myfunc: an autoloadable function\nprint hi\n",
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let zshrc = root_path.join(".zshrc");
    let source = "fpath=($HOME/functions $fpath)\nautoload -Uz myfunc\nautoload -Uz missing_fn\nmyfunc\nmissing_fn\n";
    std::fs::write(&zshrc, source).unwrap();
    crate::handlers::workspace_functions::with_test_home_dir(&home_path, || {
        let (mut session, client, _messages) = session(&root_path);
        let uri = open(&mut session, &zshrc, source);
        // Declared functions are commands, whether or not their file exists.
        assert!(is_function(&session, &uri, "myfunc"));
        assert!(is_function(&session, &uri, "missing_fn"));
        assert!(!missing(&session, &uri, "myfunc"));
        assert!(!missing(&session, &uri, "missing_fn"));

        let file = functions.join("myfunc");
        // From the call site.
        assert_eq!(
            paths(&definition(&session, &client, position(&uri, 3, 2))),
            vec![(file.clone(), 0)]
        );
        assert_eq!(
            paths(&implementation(&session, &client, position(&uri, 3, 2))),
            vec![(file.clone(), 0)]
        );
        // The declaration is the `autoload` line.
        assert_eq!(
            paths(&declaration(&session, &client, position(&uri, 3, 2))),
            vec![(zshrc.clone(), 1)]
        );
        // From the `autoload` operand.
        assert_eq!(
            paths(&definition(&session, &client, position(&uri, 1, 15))),
            vec![(file.clone(), 0)]
        );
        assert_eq!(
            paths(&implementation(&session, &client, position(&uri, 1, 15))),
            vec![(file.clone(), 0)]
        );
        // Without a file on the search path, the declaration itself is the target.
        assert_eq!(
            paths(&definition(&session, &client, position(&uri, 4, 2))),
            vec![(zshrc.clone(), 2)]
        );

        let text = hover(&session, &client, position(&uri, 3, 2));
        let text = markdown(&text);
        assert!(text.contains("(autoload)"), "{text}");
        assert!(text.contains("loaded from"), "{text}");
        assert!(text.contains("functions/myfunc"), "{text}");
        let text = hover(&session, &client, position(&uri, 4, 2));
        assert!(
            markdown(&text).contains("no file with this name"),
            "{}",
            markdown(&text)
        );
    });
}

#[test]
fn bindkey_widget_names_lead_to_the_registration_and_its_function() {
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let zshrc = root_path.join(".zshrc");
    let source = "my_widget_fn() { zle reset-prompt; }\nzle -N my-widget my_widget_fn\nbindkey '^X^E' my-widget\nbindkey -M viins '^R' other-widget\n";
    std::fs::write(&zshrc, source).unwrap();
    let (mut session, client, _messages) = session(&root_path);
    let uri = open(&mut session, &zshrc, source);
    let targets = paths(&definition(&session, &client, position(&uri, 2, 17)));
    assert_eq!(targets, vec![(zshrc.clone(), 1), (zshrc.clone(), 0)]);
    assert_eq!(
        paths(&implementation(&session, &client, position(&uri, 2, 17))),
        targets
    );
    // An unregistered widget has nothing to offer.
    assert!(definition(&session, &client, position(&uri, 3, 24)).is_empty());
}
