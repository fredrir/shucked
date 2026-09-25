//! Offline argument completion: bundled grammars bound like validation, and
//! native enrichment of the items they offer.
use super::super::environment::Environment;
use super::super::grammar;
use super::*;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};
use lsp_types::{ClientCapabilities, Url};
use std::path::{Path, PathBuf};

fn executable(directory: &Path, name: &str, body: &str) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn host(
    root: &Path,
    native: bool,
) -> (
    shucked_command::ExecutionContext,
    shucked_command::EnvironmentSnapshot,
) {
    let context = shucked_command::ExecutionContext {
        cwd: Some(root.to_owned()),
        cwd_known: true,
        native_execution_allowed: native,
        ..Default::default()
    };
    let environment = shucked_command::host::capture(&context, vec![root.join("bin")], 1);
    (context, environment)
}

fn resolved(
    context: &shucked_command::ExecutionContext,
    environment: &shucked_command::EnvironmentSnapshot,
    name: &str,
) -> shucked_command::ResolvedCommand {
    match shucked_command::resolve(
        context,
        environment,
        &shucked_command::CommandSite::literal(name),
    ) {
        shucked_command::CommandResolution::Resolved(command) => command,
        other => panic!("{name} did not resolve: {other:?}"),
    }
}

fn labels(candidates: &[Candidate]) -> Vec<&str> {
    candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect()
}

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn grammar_candidates_follow_the_cursor_word() {
    let (version, ls) = shucked_command::newest_tool_grammar("gnu-ls").unwrap();
    let binding = grammar::Binding {
        grammar: std::sync::Arc::new(ls.grammar),
        provider: format!("gnu-ls {version}"),
    };
    let all = grammar::candidates(&binding, &words(&["ls"]), "-");
    let long = grammar::candidates(&binding, &words(&["ls"]), "--co");
    assert_eq!(labels(&long), ["--color", "--context"]);
    assert_eq!(
        long[0].description,
        "Colorize output: always, auto or never"
    );
    assert_eq!(long[0].provider, "gnu-ls 9.7");
    let dash_l = all.iter().find(|candidate| candidate.text == "-l").unwrap();
    assert_eq!(dash_l.description, "List in long format");
    assert!(all.iter().any(|candidate| candidate.text == "--all"));
    // A short cluster grows by one more letter, described by that letter.
    let cluster = grammar::candidates(&binding, &words(&["ls"]), "-la");
    assert!(labels(&cluster).contains(&"-lah"));
    assert!(!labels(&cluster).contains(&"-lal"), "{cluster:?}");
    assert_eq!(
        cluster
            .iter()
            .find(|candidate| candidate.text == "-lah")
            .unwrap()
            .description,
        "Show sizes in human-readable units such as 1K or 234M"
    );
    // A cluster containing a flag that takes a value cannot grow.
    assert!(grammar::candidates(&binding, &words(&["ls"]), "-lw").is_empty());
    // Nothing after the end of options, and nothing for a file operand.
    assert!(grammar::candidates(&binding, &words(&["ls", "--"]), "-").is_empty());
    assert!(grammar::candidates(&binding, &words(&["ls"]), "").is_empty());
    assert!(grammar::candidates(&binding, &words(&["ls"]), "src").is_empty());

    let (_, docker) = shucked_command::newest_tool_grammar("docker").unwrap();
    let binding = grammar::Binding {
        grammar: std::sync::Arc::new(docker.grammar),
        provider: "docker".into(),
    };
    let subcommands = grammar::candidates(&binding, &words(&["docker"]), "");
    assert!(labels(&subcommands).contains(&"run"));
    assert!(
        !labels(&subcommands)
            .iter()
            .any(|name| name.starts_with("__"))
    );
    assert_eq!(
        labels(&grammar::candidates(&binding, &words(&["docker"]), "ru")),
        ["run"]
    );
    // A global option that takes a value does not select a subcommand.
    assert!(
        labels(&grammar::candidates(
            &binding,
            &words(&["docker", "--context", "x"]),
            "r"
        ))
        .contains(&"run")
    );
    assert!(
        labels(&grammar::candidates(&binding, &words(&["docker"]), "--")).contains(&"--context")
    );
    // Child arguments are outside the bundled grammar.
    assert!(grammar::candidates(&binding, &words(&["docker", "run"]), "-").is_empty());
    assert!(grammar::candidates(&binding, &words(&["docker", "unknown-plugin"]), "-").is_empty());
}

#[cfg(unix)]
#[test]
fn bindings_follow_the_identified_version_and_fall_back_when_unverified() {
    grammar::invalidate();
    let root = tempfile::tempdir().unwrap();
    let runs = root.path().join("runs");
    executable(
        &root.path().join("bin"),
        "ls",
        &format!(
            "printf x >> '{}'\n[ \"$*\" = --version ] || exit 9\nprintf 'ls (GNU coreutils) 9.7\\n'",
            runs.display()
        ),
    );
    let (untrusted, environment) = host(root.path(), false);
    let command = resolved(&untrusted, &environment, "ls");
    let binding =
        grammar::bind_resolved(&untrusted, &environment, &command, false, &|| false).unwrap();
    assert!(
        binding.provider.ends_with("(unverified)"),
        "{}",
        binding.provider
    );
    assert!(!runs.exists(), "an untrusted workspace never runs the tool");

    let (trusted, environment) = host(root.path(), true);
    let command = resolved(&trusted, &environment, "ls");
    let binding =
        grammar::bind_resolved(&trusted, &environment, &command, true, &|| false).unwrap();
    assert_eq!(binding.provider, "gnu-ls 9.7");
    assert_eq!(
        binding.grammar.flags["-l"].description.as_deref(),
        Some("List in long format")
    );
    assert_eq!(std::fs::read(&runs).unwrap().len(), 1);
    // The binding is cached by identity until the environment is invalidated.
    let again = grammar::bind_resolved(&trusted, &environment, &command, true, &|| false).unwrap();
    assert!(std::sync::Arc::ptr_eq(&binding, &again));
    assert_eq!(std::fs::read(&runs).unwrap().len(), 1);
    grammar::invalidate();
    grammar::bind_resolved(&trusted, &environment, &command, true, &|| false).unwrap();
    assert!(!std::fs::read(&runs).unwrap().is_empty());

    // A covered tool with an uncovered release is offered unverified.
    grammar::invalidate();
    executable(
        &root.path().join("bin"),
        "ls",
        "[ \"$*\" = --version ] || exit 9\nprintf 'ls (GNU coreutils) 8.32\\n'",
    );
    let (trusted, environment) = host(root.path(), true);
    let command = resolved(&trusted, &environment, "ls");
    let binding =
        grammar::bind_resolved(&trusted, &environment, &command, true, &|| false).unwrap();
    assert_eq!(binding.provider, "gnu-ls 9.7 (unverified)");

    // brew and git have inventories rather than option grammars.
    executable(&root.path().join("bin"), "git", "exit 0");
    let (trusted, environment) = host(root.path(), true);
    let command = resolved(&trusted, &environment, "git");
    assert!(grammar::bind_resolved(&trusted, &environment, &command, true, &|| false).is_none());
    grammar::invalidate();
}

fn complete(root: &Path, marked: &str, native: bool) -> types::CompletionList {
    let cursor = marked.find('¦').expect("cursor marker");
    let source = marked.replacen('¦', "", 1);
    let (sender, _) = crossbeam::channel::unbounded();
    let (client_sender, _) = crossbeam::channel::unbounded();
    let client = Client::new(sender, client_sender);
    let uri = Url::from_file_path(root.join("script.sh")).unwrap();
    let workspaces = Workspaces::new(vec![Workspace::default(Url::from_file_path(root).unwrap())]);
    let capabilities = serde_json::from_value::<ClientCapabilities>(serde_json::json!({})).unwrap();
    let global: GlobalOptions =
        serde_json::from_value(serde_json::json!({"nativeExecutionAllowed": native})).unwrap();
    let mut session = Session::new(
        &capabilities,
        PositionEncoding::UTF16,
        global.into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.clone(), 1).with_language_id("shellscript"),
    );
    let mut snapshot = session.take_snapshot(uri.clone()).unwrap();
    snapshot.command_service =
        std::sync::Arc::new(crate::handlers::commands::CommandService::fixture(vec![
            root.join("bin"),
        ]));
    let position = types::Position::new(
        source[..cursor]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count() as u32,
        source[..cursor]
            .rsplit('\n')
            .next()
            .unwrap()
            .encode_utf16()
            .count() as u32,
    );
    let params = types::CompletionParams {
        text_document_position: types::TextDocumentPositionParams {
            text_document: types::TextDocumentIdentifier { uri },
            position,
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    };
    let environment = Environment::fixture(root);
    match crate::editor_features::completion_with_environment(
        snapshot,
        &client,
        params,
        Some((&environment, &RequestCancellationToken::default())),
        |_, _| Vec::new(),
    )
    .unwrap()
    {
        Some(types::CompletionResponse::List(list)) => list,
        None => types::CompletionList {
            is_incomplete: false,
            items: Vec::new(),
        },
        _ => panic!("expected completion list"),
    }
}

fn item<'a>(list: &'a types::CompletionList, label: &str) -> &'a types::CompletionItem {
    list.items
        .iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| {
            panic!(
                "{label} missing from {:?}",
                list.items
                    .iter()
                    .map(|item| &item.label)
                    .collect::<Vec<_>>()
            )
        })
}

#[cfg(unix)]
#[test]
fn flags_complete_from_the_bundled_grammar_without_any_shell() {
    grammar::invalidate();
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("bin"), "ls", "exit 0");
    executable(&root.path().join("bin"), "docker", "exit 0");
    std::fs::write(root.path().join("-lfile"), "").unwrap();
    // The fixture environment permits no execution, so the newest grammar
    // answers as an unverified suggestion; no engine is installed here anyway.
    let list = complete(root.path(), "ls -¦", false);
    let long = item(&list, "-l");
    assert_eq!(long.kind, Some(types::CompletionItemKind::FIELD));
    assert_eq!(
        long.detail.as_deref(),
        Some("List in long format · gnu-ls 9.7 (unverified)")
    );
    assert_eq!(long.commit_characters, Some(vec![" ".into()]));
    assert!(
        !list.items.iter().any(|item| item.label == "-lfile"),
        "a grammar answer owns the position; no path fallback"
    );
    let list = complete(root.path(), "ls --col¦", false);
    assert_eq!(
        item(&list, "--color").detail.as_deref(),
        Some("Colorize output: always, auto or never · gnu-ls 9.7 (unverified)")
    );
    match item(&list, "--color").text_edit.as_ref().unwrap() {
        types::CompletionTextEdit::Edit(edit) => assert_eq!(edit.new_text, "--color"),
        other => panic!("{other:?}"),
    }
    let list = complete(root.path(), "ls -la¦", false);
    assert!(
        item(&list, "-lah")
            .detail
            .as_deref()
            .unwrap()
            .starts_with("Show sizes")
    );
    // After `--` only operands follow.
    let list = complete(root.path(), "ls -- -¦", false);
    assert!(
        !list
            .items
            .iter()
            .any(|item| item.kind == Some(types::CompletionItemKind::FIELD))
    );
    // A subcommand position lists the grammar's commands.
    let list = complete(root.path(), "docker ru¦", false);
    assert_eq!(
        item(&list, "run").kind,
        Some(types::CompletionItemKind::VALUE)
    );
    // Unknown commands and shadowed names get nothing.
    let list = complete(root.path(), "unknown-tool -¦", false);
    assert!(
        !list
            .items
            .iter()
            .any(|item| item.kind == Some(types::CompletionItemKind::FIELD))
    );
    let list = complete(root.path(), "ls() { :; }\nls -¦", false);
    assert!(!list.items.iter().any(|item| item.label == "-l"));
    grammar::invalidate();
}

#[test]
fn native_descriptions_enrich_offline_items() {
    // A native candidate for a label the grammar already offered enriches it.
    let mut items = vec![super::super::item(
        "-l",
        types::CompletionItemKind::FIELD,
        "List in long format · gnu-ls 9.7",
        "-l".into(),
        types::Range::default(),
        1,
    )];
    let offline = super::super::offline::Offline::from_items(&items);
    let native = Candidate {
        text: "-l".into(),
        description: "long listing".into(),
        provider: "zsh".into(),
        ..Default::default()
    };
    assert!(offline.enrich(&mut items, &native));
    assert_eq!(items[0].detail.as_deref(), Some("long listing · zsh"));
    let silent = Candidate {
        text: "-l".into(),
        ..Default::default()
    };
    assert!(offline.enrich(&mut items, &silent));
    assert_eq!(items[0].detail.as_deref(), Some("long listing · zsh"));
    let other = Candidate {
        text: "-a".into(),
        ..Default::default()
    };
    assert!(!offline.enrich(&mut items, &other));
}
