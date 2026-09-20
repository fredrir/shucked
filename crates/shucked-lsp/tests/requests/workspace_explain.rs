use std::path::Path;

use crossbeam::channel::{self, Receiver};
use lsp_types as types;

use super::super::traits::BackgroundRequestHandler;
use super::{Hover, References};
use crate::session::RequestCancellationToken;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};

fn session(root: &Path) -> (Session, Client, Receiver<lsp_server::Message>) {
    let (events, _) = channel::unbounded();
    let (messages, receiver) = channel::unbounded();
    let client = Client::new(events, messages);
    let workspaces = Workspaces::new(vec![Workspace::new(
        types::Url::from_file_path(root).unwrap(),
    )]);
    let session = Session::new(
        &types::ClientCapabilities::default(),
        PositionEncoding::UTF16,
        GlobalOptions::default().into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap();
    (session, client, receiver)
}

fn open(session: &mut Session, path: &Path, source: &str) -> types::Url {
    let uri = types::Url::from_file_path(path).unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.into(), 1).with_language_id("shellscript"),
    );
    uri
}

fn position(uri: &types::Url, line: u32, character: u32) -> types::TextDocumentPositionParams {
    types::TextDocumentPositionParams {
        text_document: types::TextDocumentIdentifier { uri: uri.clone() },
        position: types::Position::new(line, character),
    }
}

fn hover(
    session: &Session,
    client: &Client,
    position: types::TextDocumentPositionParams,
) -> types::Hover {
    let params = types::HoverParams {
        text_document_position_params: position,
        work_done_progress_params: Default::default(),
    };
    let snapshot = Hover::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    Hover::run_with_snapshot(snapshot, client, params)
        .unwrap()
        .unwrap()
}

fn markdown(hover: &types::Hover) -> &str {
    let types::HoverContents::Markup(content) = &hover.contents else {
        panic!("expected markup");
    };
    assert_eq!(content.kind, types::MarkupKind::Markdown);
    &content.value
}

fn references(
    session: &Session,
    client: &Client,
    position: types::TextDocumentPositionParams,
    declarations: bool,
) -> Vec<types::Location> {
    let params = types::ReferenceParams {
        text_document_position: position,
        context: types::ReferenceContext {
            include_declaration: declarations,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let snapshot =
        References::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    References::run_with_snapshot(snapshot, client, params)
        .unwrap()
        .unwrap_or_default()
}

#[test]
fn hover_links_origins_and_consumers_and_references_select_the_exact_assignment() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper.sh");
    std::fs::write(&helper, "VALUE=old\nVALUE=shared\n").unwrap();
    let main = root.path().join("main.sh");
    let source = "source helper.sh\necho \"🦀$VALUE\"\nVALUE=own\necho \"$VALUE\"\n";
    std::fs::write(&main, source).unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(&mut session, &helper, "VALUE=old\nVALUE=shared\n");
    let selected = position(&uri, 1, 2);
    let details = hover(&session, &client, selected.clone());
    let text = markdown(&details);
    assert!(text.contains("helper.sh:2"), "{text}");
    assert!(text.contains("main.sh:2"), "{text}");
    assert!(text.contains("Go to References"));
    let uses = references(&session, &client, selected.clone(), false);
    assert_eq!(uses.len(), 1);
    assert_eq!(
        uses[0].uri,
        types::Url::from_file_path(std::fs::canonicalize(&main).unwrap()).unwrap()
    );
    assert_eq!(uses[0].range.start, types::Position::new(1, 9));
    assert_eq!(uses[0].range.end, types::Position::new(1, 14));
    let with_declarations = references(&session, &client, selected, true);
    assert_eq!(with_declarations.len(), 2);
    assert!(
        with_declarations
            .iter()
            .any(|location| location.uri == uri && location.range.start.line == 1)
    );
    assert!(references(&session, &client, position(&uri, 0, 2), false).is_empty());
}

#[test]
fn conditional_imports_expose_possible_origins_and_read_only_references() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.sh"), "VALUE=a\n").unwrap();
    std::fs::write(root.path().join("b.sh"), "VALUE=b\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    let source = "source a.sh\nif ready; then source b.sh; fi\necho \"$VALUE\"\n";
    let uri = open(&mut session, &root.path().join("main.sh"), source);
    let selected = position(&uri, 2, 9);
    let details = hover(&session, &client, selected.clone());
    let text = markdown(&details);
    assert!(text.contains("a.sh:1") && text.contains("b.sh:1"), "{text}");
    assert!(text.contains("conditional execution"));
    assert!(!text.contains("Incomplete"), "{text}");
    assert_eq!(references(&session, &client, selected, true).len(), 3);
}

#[test]
fn source_hover_explains_missing_paths_unknown_values_and_directives() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source missing.sh\nsource \"$UNKNOWN/helper.sh\"\n# shucked: source=/dev/null\nsource \"$IGNORED\"\n",
    );
    let missing = hover(&session, &client, position(&uri, 0, 10));
    assert!(markdown(&missing).contains("Source file not found."));
    assert!(markdown(&missing).contains("Searched:"));
    assert!(markdown(&missing).contains(&root.path().join("missing.sh").display().to_string()));
    assert_eq!(missing.range.unwrap().start, types::Position::new(0, 7));
    let unknown = hover(&session, &client, position(&uri, 1, 15));
    assert!(
        markdown(&unknown).contains("unknown or conflicting values"),
        "{}",
        markdown(&unknown)
    );
    let ignored = hover(&session, &client, position(&uri, 2, 21));
    assert!(
        markdown(&ignored).contains("disabled by the /dev/null directive"),
        "{}",
        markdown(&ignored)
    );
}

#[test]
fn source_hover_uses_unsaved_helpers_for_derived_paths() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("paths.sh");
    std::fs::write(&helper, "DIR=missing\n").unwrap();
    std::fs::create_dir(root.path().join("actual")).unwrap();
    let target = root.path().join("actual/values.sh");
    std::fs::write(&target, "VALUE=shared\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    open(
        &mut session,
        &helper,
        &format!("DIR=\"{}\"\n", root.path().join("actual").display()),
    );
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source paths.sh\nsource \"$DIR/values.sh\"\necho \"$VALUE\"\n",
    );
    let result = hover(&session, &client, position(&uri, 1, 15));
    assert!(
        markdown(&result).contains("Resolved source:"),
        "{}",
        markdown(&result)
    );
    assert!(markdown(&result).contains("actual/values.sh"));
    let origin = hover(&session, &client, position(&uri, 2, 9));
    assert!(markdown(&origin).contains("actual/values.sh:1"));
}

#[test]
fn file_limit_keeps_known_references_and_reports_incomplete_discovery() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("closed.sh"), "VALUE=other\n").unwrap();
    let (mut session, client, messages) = session(root.path());
    let mut options = crate::ClientOptions::default();
    options.server.call_hierarchy.max_files = 1;
    session.update_client_options(options);
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "VALUE=known\necho \"$VALUE\"\n",
    );
    let selected = position(&uri, 0, 2);
    let result = hover(&session, &client, selected.clone());
    assert!(markdown(&result).contains("Incomplete results"));
    assert!(markdown(&result).contains("workspace file limit (1) reached"));
    assert_eq!(references(&session, &client, selected, false).len(), 1);
    assert!(messages.try_iter().any(|message| matches!(message, lsp_server::Message::Notification(notification)
        if notification.method == "window/showMessage" && notification.params["message"].as_str().is_some_and(|text| text.contains("workspace file limit (1) reached")))));
}

#[test]
fn unresolved_source_notifies_that_references_are_partial() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "VALUE=known\necho \"$VALUE\"\nsource \"$DYNAMIC\"\necho \"$VALUE\"\n",
    );
    assert_eq!(
        references(&session, &client, position(&uri, 0, 2), false).len(),
        2
    );
    assert!(messages.try_iter().any(|message| matches!(message, lsp_server::Message::Notification(notification)
        if notification.method == "window/showMessage" && notification.params["message"].as_str().is_some_and(|text| text.contains("source effects could not be followed")))));
}

#[test]
fn source_depth_limit_is_visible_even_when_the_immediate_target_resolves() {
    let root = tempfile::tempdir().unwrap();
    for n in 0..35 {
        std::fs::write(
            root.path().join(format!("helper{n}.sh")),
            if n == 34 {
                "VALUE=deep\n".into()
            } else {
                format!("source helper{}.sh\n", n + 1)
            },
        )
        .unwrap();
    }
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source helper0.sh\nsource \"$VALUE/end.sh\"\n",
    );
    let result = hover(&session, &client, position(&uri, 0, 10));
    let text = markdown(&result);
    assert!(text.contains("Resolved source:"), "{text}");
    assert!(text.contains("Incomplete workspace discovery"), "{text}");
    assert!(
        text.contains("source analysis reached a file, depth, size, or work limit"),
        "{text}"
    );
}

#[test]
fn watched_file_creation_and_deletion_refresh_resolution_explanations() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source helper.sh\n",
    );
    let target = root.path().join("helper.sh");
    let selected = position(&uri, 0, 10);
    assert!(
        markdown(&hover(&session, &client, selected.clone())).contains("Source file not found.")
    );
    std::fs::write(&target, "VALUE=created\n").unwrap();
    session.reload_settings(
        &[types::FileEvent {
            uri: types::Url::from_file_path(&target).unwrap(),
            typ: types::FileChangeType::CREATED,
        }],
        &client,
    );
    assert!(markdown(&hover(&session, &client, selected.clone())).contains("Resolved source:"));
    std::fs::remove_file(&target).unwrap();
    session.reload_settings(
        &[types::FileEvent {
            uri: types::Url::from_file_path(&target).unwrap(),
            typ: types::FileChangeType::DELETED,
        }],
        &client,
    );
    assert!(markdown(&hover(&session, &client, selected)).contains("Source file not found."));
}

#[test]
fn open_document_aliases_are_preserved_in_references_and_hover_links() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, _messages) = session(root.path());
    let helper = root.path().join("helper [one].sh");
    let helper_uri = open(&mut session, &helper, "VALUE=shared\n");
    let consumer_uri = open(
        &mut session,
        &root.path().join("consumer.sh"),
        "source 'helper [one].sh'\necho \"$VALUE\"\n",
    );
    let uses = references(&session, &client, position(&helper_uri, 0, 2), true);
    assert!(uses.iter().any(|location| location.uri == helper_uri));
    assert!(uses.iter().any(|location| location.uri == consumer_uri));
    let result = hover(&session, &client, position(&consumer_uri, 1, 9));
    assert!(
        markdown(&result).contains("helper \\[one\\].sh:1"),
        "{}",
        markdown(&result)
    );
    let mut linked_uri = helper_uri;
    linked_uri.set_fragment(Some("L1"));
    assert!(markdown(&result).contains(&format!("(<{linked_uri}>)")));
}

#[test]
fn unknown_variable_path_is_distinct_from_an_unsupported_expression() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("variable.sh"),
        "source \"$UNKNOWN/helper.sh\"\n",
    );
    let result = hover(&session, &client, position(&uri, 0, 15));
    assert!(
        markdown(&result).contains("unknown or conflicting values"),
        "{}",
        markdown(&result)
    );
    let uri = open(
        &mut session,
        &root.path().join("expression.sh"),
        "source \"$(get_path)\"\n",
    );
    let result = hover(&session, &client, position(&uri, 0, 12));
    assert!(
        markdown(&result).contains("unsupported runtime expression"),
        "{}",
        markdown(&result)
    );
}

#[test]
fn unindexed_source_reports_the_file_limit_instead_of_silently_losing_details() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, _messages) = session(root.path());
    let mut options = crate::ClientOptions::default();
    options.server.call_hierarchy.max_files = 0;
    session.update_client_options(options);
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source helper.sh\n",
    );
    let result = hover(&session, &client, position(&uri, 0, 10));
    assert!(markdown(&result).contains("workspace file limit (0) reached"));
}

#[test]
fn cancelled_reference_requests_return_no_locations_or_partial_result_warning() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, messages) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "VALUE=known\nsource \"$DYNAMIC\"\necho \"$VALUE\"\n",
    );
    let params = types::ReferenceParams {
        text_document_position: position(&uri, 0, 2),
        context: types::ReferenceContext {
            include_declaration: true,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let cancellation = RequestCancellationToken::default();
    let snapshot = References::snapshot(&session, &params, cancellation.clone()).unwrap();
    cancellation.cancel();
    assert!(
        References::run_with_snapshot(snapshot, &client, params)
            .unwrap()
            .is_none()
    );
    assert!(!messages.try_iter().any(|message| matches!(message, lsp_server::Message::Notification(notification)
        if notification.method == "window/showMessage" && notification.params["message"].as_str().is_some_and(|text| text.contains("Workspace references are incomplete")))));
}

#[test]
fn zsh_startup_glob_connects_numbered_modules_in_load_order() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("modules");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        root.path().join(".zshenv"),
        format!("export ZCONF=\"{}\"\n", directory.display()),
    )
    .unwrap();
    let loader = "if [[ -n $AGENT_SHELL ]]; then\n source \"$ZCONF/02-utils.zsh\"\n return 0\nfi\nfor module in \"$ZCONF\"/{0[2-9],[1-9][0-9]}-*.zsh(N); do\n source \"$module\"\ndone\n";
    std::fs::write(root.path().join(".zshrc"), loader).unwrap();
    let utility = directory.join("02-utils.zsh");
    let utility_source = "[[ $OSTYPE == linux* ]] && LINUX=1\nUNUSED=1\n";
    std::fs::write(&utility, utility_source).unwrap();
    std::fs::write(
        directory.join("05-plugins.zsh"),
        "source \"$UNKNOWN_PLUGIN\"\n",
    )
    .unwrap();
    let aliases = directory.join("30-aliases.zsh");
    std::fs::write(
        &aliases,
        "if [[ -n $LINUX ]]; then alias tool='linux-tool'; fi\n",
    )
    .unwrap();
    // Matching names in another directory must not join this source environment.
    std::fs::write(root.path().join("unrelated.zsh"), "echo $UNUSED\n").unwrap();
    let (mut session, client, _messages) = session(root.path());
    let uri = open(&mut session, &utility, utility_source);
    let details = hover(&session, &client, position(&uri, 0, 28));
    assert!(
        markdown(&details).contains("30-aliases.zsh:1"),
        "{}",
        markdown(&details)
    );
    let uses = references(&session, &client, position(&uri, 0, 28), false);
    assert_eq!(uses.len(), 1, "{uses:?}");
    assert_eq!(
        uses[0].uri,
        types::Url::from_file_path(std::fs::canonicalize(&aliases).unwrap()).unwrap()
    );
    let diagnostics = crate::generate_diagnostics(&session.take_snapshot(uri.clone()).unwrap());
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic.code
            == Some(types::NumberOrString::String("C001".into()))
            && diagnostic.range.start.line == 0),
        "{diagnostics:?}"
    );
    assert!(diagnostics.iter().any(|diagnostic| diagnostic.code
        == Some(types::NumberOrString::String("C001".into()))
        && diagnostic.range.start.line == 1));
    let loader_uri = open(&mut session, &root.path().join(".zshrc"), loader);
    let details = hover(&session, &client, position(&loader_uri, 5, 11));
    assert!(
        markdown(&details).contains("Files matched by the source loop"),
        "{}",
        markdown(&details)
    );
}

#[test]
fn module_globs_refresh_for_unsaved_created_deleted_and_renamed_consumers() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("02-utils.zsh");
    std::fs::write(&helper, "LINUX=1\n").unwrap();
    let loader = format!(
        "ROOT=\"{}\"\nfor module in \"$ROOT\"/[0-9][0-9]-*.zsh(N); do source \"$module\"; done\n",
        root.path().display()
    );
    std::fs::write(root.path().join("init.zsh"), loader).unwrap();
    let (mut session, client, _messages) = session(root.path());
    let helper_uri = open(&mut session, &helper, "LINUX=1\n");
    let selected = position(&helper_uri, 0, 2);
    assert!(references(&session, &client, selected.clone(), false).is_empty());
    let consumer = root.path().join("30-aliases.zsh");
    let consumer_uri = open(&mut session, &consumer, "echo $LINUX\n");
    let uses = references(&session, &client, selected.clone(), false);
    assert_eq!(uses.len(), 1);
    assert_eq!(uses[0].uri, consumer_uri);
    // A renamed module outside the glob must no longer consume the binding.
    let key = session.key_from_url(consumer_uri);
    session.close_document(&key).unwrap();
    std::fs::write(&consumer, "echo $LINUX\n").unwrap();
    let event = |path: &Path, typ| types::FileEvent {
        uri: types::Url::from_file_path(path).unwrap(),
        typ,
    };
    session.reload_settings(&[event(&consumer, types::FileChangeType::CREATED)], &client);
    assert_eq!(
        references(&session, &client, selected.clone(), false).len(),
        1
    );
    let renamed = root.path().join("aliases.zsh");
    std::fs::rename(&consumer, &renamed).unwrap();
    session.reload_settings(
        &[
            event(&consumer, types::FileChangeType::DELETED),
            event(&renamed, types::FileChangeType::CREATED),
        ],
        &client,
    );
    assert!(references(&session, &client, selected.clone(), false).is_empty());
    std::fs::rename(&renamed, &consumer).unwrap();
    session.reload_settings(&[event(&consumer, types::FileChangeType::CREATED)], &client);
    assert_eq!(
        references(&session, &client, selected.clone(), false).len(),
        1
    );
    std::fs::remove_file(&consumer).unwrap();
    session.reload_settings(&[event(&consumer, types::FileChangeType::DELETED)], &client);
    assert!(references(&session, &client, selected, false).is_empty());
}

#[test]
fn unsaved_startup_path_changes_retarget_module_consumers() {
    let root = tempfile::tempdir().unwrap();
    for name in ["one", "two"] {
        let directory = root.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("02-utils.zsh"), "LINUX=1\n").unwrap();
        std::fs::write(directory.join("30-aliases.zsh"), "echo $LINUX\n").unwrap();
    }
    let globals = root.path().join(".zshenv");
    let source = |name| format!("export ZCONF='{}'\n", root.path().join(name).display());
    std::fs::write(&globals, source("one")).unwrap();
    std::fs::write(
        root.path().join(".zshrc"),
        "for module in \"$ZCONF\"/[0-9][0-9]-*.zsh(N); do source \"$module\"; done\n",
    )
    .unwrap();
    let (mut session, client, _messages) = session(root.path());
    let first = open(
        &mut session,
        &root.path().join("one/02-utils.zsh"),
        "LINUX=1\n",
    );
    let second = open(
        &mut session,
        &root.path().join("two/02-utils.zsh"),
        "LINUX=1\n",
    );
    assert_eq!(
        references(&session, &client, position(&first, 0, 2), false).len(),
        1
    );
    assert!(references(&session, &client, position(&second, 0, 2), false).is_empty());
    open(&mut session, &globals, &source("two"));
    assert!(references(&session, &client, position(&first, 0, 2), false).is_empty());
    assert_eq!(
        references(&session, &client, position(&second, 0, 2), false).len(),
        1
    );
}
