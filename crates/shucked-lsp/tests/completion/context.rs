use super::*;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};
use lsp_types::{ClientCapabilities, Url};

fn complete(
    root: &Path,
    marked: &str,
    options: serde_json::Value,
    insert_replace: bool,
) -> Vec<types::CompletionItem> {
    completion_list(root, marked, options, insert_replace).items
}

fn completion_list(
    root: &Path,
    marked: &str,
    options: serde_json::Value,
    insert_replace: bool,
) -> types::CompletionList {
    let cursor = marked.find('¦').expect("cursor marker");
    let source = marked.replacen('¦', "", 1);
    let (sender, _) = crossbeam::channel::unbounded();
    let (client_sender, _) = crossbeam::channel::unbounded();
    let client = Client::new(sender, client_sender);
    let uri = Url::from_file_path(root.join("script.sh")).unwrap();
    let workspaces = Workspaces::new(vec![Workspace::default(Url::from_file_path(root).unwrap())]);
    let capabilities = serde_json::from_value::<ClientCapabilities>(serde_json::json!({
        "textDocument": { "completion": { "completionItem": { "insertReplaceSupport": insert_replace } } }
    })).unwrap();
    let global: GlobalOptions = serde_json::from_value(options).unwrap();
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
    let result = crate::editor_features::completion_with_environment(
        snapshot,
        &client,
        params,
        Some((&environment, &RequestCancellationToken::default())),
        |_, _| Vec::new(),
    )
    .unwrap();
    match result {
        Some(types::CompletionResponse::List(list)) => list,
        None => types::CompletionList {
            is_incomplete: false,
            items: Vec::new(),
        },
        _ => panic!("expected completion list"),
    }
}

fn edit(item: &types::CompletionItem) -> &types::TextEdit {
    match item.text_edit.as_ref().unwrap() {
        types::CompletionTextEdit::Edit(edit) => edit,
        _ => panic!("expected replacement edit"),
    }
}

#[test]
fn paths_preserve_quoting_expansions_and_replace_suffixes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("some folder")).unwrap();
    std::fs::write(root.path().join("some folder/file name.txt"), "").unwrap();
    for (source, expected) in [
        ("cat some\\ folder/fi¦le", "file\\ name.txt"),
        ("cat 'some folder/fi¦le'", "file name.txt"),
        ("cat \"some folder/fi¦le\"", "file name.txt"),
        ("cat ~/some\\ folder/fi¦le", "file\\ name.txt"),
        ("cat \"$HOME/some folder/fi¦le\"", "file name.txt"),
        ("cat \"${HOME}/some folder/fi¦le\"", "file name.txt"),
    ] {
        let items = complete(root.path(), source, serde_json::json!({}), false);
        let candidate = items
            .iter()
            .find(|item| item.label == "file name.txt")
            .unwrap_or_else(|| panic!("missing file in {source}: {items:?}"));
        assert_eq!(edit(candidate).new_text, expected, "{source}");
        assert_eq!(
            edit(candidate).range.end.character - edit(candidate).range.start.character,
            4,
            "replace entire basename: {source}"
        );
    }
}

#[test]
fn directories_redirects_hidden_files_and_literal_dollars() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder")).unwrap();
    std::fs::write(root.path().join("file"), "").unwrap();
    std::fs::write(root.path().join(".hidden"), "").unwrap();
    std::fs::write(root.path().join("$literal"), "").unwrap();
    assert_eq!(
        complete(root.path(), "cd f¦", serde_json::json!({}), false)
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["folder/"]
    );
    let items = complete(root.path(), "echo hi > f¦", serde_json::json!({}), false);
    assert!(items.iter().any(|item| item.label == "file"));
    assert!(!items.iter().any(|item| item.label == ".hidden"));
    assert!(
        complete(root.path(), "cat .¦", serde_json::json!({}), false)
            .iter()
            .any(|item| item.label == ".hidden")
    );
    assert!(
        complete(root.path(), "cat '$li¦'", serde_json::json!({}), false)
            .iter()
            .any(|item| item.label == "$literal")
    );
}

#[test]
fn unavailable_providers_do_not_invent_command_arguments() {
    let root = tempfile::tempdir().unwrap();
    for source in ["git --no-¦", "docker compose u¦", "unregistered --¦"] {
        let items = complete(root.path(), source, serde_json::json!({}), false);
        assert!(
            items
                .iter()
                .all(|item| item.kind != Some(types::CompletionItemKind::FIELD)),
            "{source}"
        );
    }
    for source in [
        "git status -- --por¦",
        "git commit -m --am¦",
        "echo 'git status --por¦'",
        "# git --no-¦",
        "cat <<EOF\ngit --no-¦\nEOF\n",
    ] {
        assert!(
            !complete(root.path(), source, serde_json::json!({}), false)
                .iter()
                .any(|item| item.kind == Some(types::CompletionItemKind::FIELD)),
            "{source}"
        );
    }
}

#[test]
fn symbols_after_assignments_and_pipelines_replace_remaining_name() {
    let root = tempfile::tempdir().unwrap();
    for source in [
        "build_project() { :; }\nFOO=1 bu¦ild",
        "build_project() { :; }\nprintf x | bu¦ild",
        "build_project() { :; }\necho $(bu¦ild)",
    ] {
        let items = complete(root.path(), source, serde_json::json!({}), false);
        let candidate = items
            .iter()
            .find(|item| item.label == "build_project")
            .unwrap();
        assert_eq!(edit(candidate).new_text, "build_project");
        assert_eq!(
            edit(candidate).range.end.character - edit(candidate).range.start.character,
            5
        );
    }
    let items = complete(
        root.path(),
        "value=1\necho $va¦lue",
        serde_json::json!({}),
        true,
    );
    let candidate = items.iter().find(|item| item.label == "value").unwrap();
    let Some(types::CompletionTextEdit::InsertAndReplace(edit)) = &candidate.text_edit else {
        panic!("expected insert/replace edit")
    };
    assert_eq!(
        edit.insert,
        types::Range::new(types::Position::new(1, 6), types::Position::new(1, 8))
    );
    assert_eq!(edit.replace.end.character, 11);
}

#[test]
fn environment_variables_can_be_disabled_and_results_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let items = complete(
        root.path(),
        "echo $SHUCKED_TEST_¦",
        serde_json::json!({}),
        false,
    );
    assert!(
        items
            .iter()
            .any(|item| item.label == "SHUCKED_TEST_VARIABLE")
    );
    let items = complete(
        root.path(),
        "echo $SHUCKED_TEST_¦",
        serde_json::json!({"server": {"completion": {"includeEnvironment": false}}}),
        false,
    );
    assert!(items.is_empty());
    for name in ["one", "two", "three"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    let list = completion_list(
        root.path(),
        "cd ¦",
        serde_json::json!({"server": {"completion": {"maxItems": 2}}}),
        false,
    );
    assert_eq!(list.items.len(), 2);
    assert!(list.is_incomplete);
}

#[test]
fn assignments_and_equals_options_complete_paths() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file name"), "").unwrap();
    for source in [
        "OUTPUT=fi¦",
        "OUTPUT=\"fi¦\"",
        "curl --output=fi¦",
        "curl --output=\"fi¦\"",
        "curl --output='fi¦'",
    ] {
        let items = complete(root.path(), source, serde_json::json!({}), false);
        assert!(
            items.iter().any(|item| item.label == "file name"),
            "{source}: {items:?}"
        );
    }
    let items = complete(
        root.path(),
        "OUTPUT=\"$SHUCKED_TEST_¦\"",
        serde_json::json!({}),
        false,
    );
    assert!(
        items
            .iter()
            .any(|item| item.label == "SHUCKED_TEST_VARIABLE")
    );
}

#[test]
fn escaped_dollars_do_not_complete_variables_and_quoted_tildes_are_literal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("~")).unwrap();
    std::fs::write(root.path().join("~/literal"), "").unwrap();
    for source in [r"echo \$SHUCKED_TEST_¦", "echo '$SHUCKED_TEST_¦'"] {
        assert!(
            !complete(root.path(), source, serde_json::json!({}), false)
                .iter()
                .any(|item| item.label == "SHUCKED_TEST_VARIABLE"),
            "{source}"
        );
    }
    assert!(
        complete(root.path(), "cat \"~/li¦\"", serde_json::json!({}), false)
            .iter()
            .any(|item| item.label == "literal")
    );
}

#[test]
fn unicode_paths_use_utf16_edit_ranges() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("😀file"), "").unwrap();
    let items = complete(root.path(), "cat 😀f¦oo", serde_json::json!({}), false);
    let candidate = items.iter().find(|item| item.label == "😀file").unwrap();
    assert_eq!(
        edit(candidate).range,
        types::Range::new(types::Position::new(0, 4), types::Position::new(0, 9))
    );
}

#[test]
fn wrappers_preserve_command_context_and_option_values() {
    let root = tempfile::tempdir().unwrap();
    for source in [
        "build_project() { :; }\nsudo -u root bu¦",
        "build_project() { :; }\nenv -u SECRET FOO=bar bu¦",
        "build_project() { :; }\ncommand -v bu¦",
    ] {
        assert!(
            complete(root.path(), source, serde_json::json!({}), false)
                .iter()
                .any(|item| item.label == "build_project"),
            "{source}"
        );
    }
    let items = complete(root.path(), "sudo -u ro¦", serde_json::json!({}), false);
    assert!(
        !items
            .iter()
            .any(|item| item.kind == Some(types::CompletionItemKind::FUNCTION))
    );
}

#[test]
fn fuzzy_symbols_rank_prefix_matches_before_subsequences() {
    let root = tempfile::tempdir().unwrap();
    let items = complete(
        root.path(),
        "build_project() { :; }\nbpr_exact() { :; }\nbpr¦",
        serde_json::json!({}),
        false,
    );
    let names = items
        .iter()
        .map(|item| item.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["bpr_exact", "build_project"]);
    let items = complete(
        root.path(),
        "long_variable_name=1\necho $lvn¦",
        serde_json::json!({}),
        false,
    );
    assert!(items.iter().any(|item| item.label == "long_variable_name"));
}

#[test]
fn multiple_spaces_keep_function_and_dynamic_environment_context() {
    let root = tempfile::tempdir().unwrap();
    for source in [
        "git() { :; }\ngit   ¦",
        "PATH=\"$UNKNOWN_PATH\"\ngit   ¦",
        "git() { :; }\necho $(git   ¦)",
    ] {
        let items = complete(root.path(), source, serde_json::json!({}), false);
        assert!(
            !items.iter().any(|item| item.label == "status"),
            "unrelated Git grammar in {source}"
        );
    }
}

#[test]
fn attached_command_candidates_include_session_symbols_only_in_live_session_mode() {
    let mut context = shucked_command::ExecutionContext::default();
    let mut environment = shucked_command::EnvironmentSnapshot::empty(&context);
    environment
        .aliases
        .insert("personal_ls".into(), shucked_command::Alias::default());
    environment.functions.insert("personal_build".into());
    assert!(!command_names(&context, &environment).contains("personal_ls"));
    context.mode = shucked_command::ExecutionMode::InteractiveSession;
    let names = command_names(&context, &environment);
    assert!(names.contains("personal_ls"));
    assert!(names.contains("personal_build"));
    environment.fresh = false;
    assert!(!command_names(&context, &environment).contains("personal_build"));
}

#[test]
fn source_alias_names_are_completed_only_when_shell_will_expand_them() {
    let root = tempfile::tempdir().unwrap();
    for (source, expected) in [
        ("#!/bin/zsh\nalias myls=eza\nmy¦", true),
        ("#!/bin/zsh\nalias myls=eza; my¦", false),
        ("#!/bin/zsh\nalias myls=eza\nunalias myls\nmy¦", false),
        ("#!/bin/zsh\nalias myls=eza\n'my¦'", false),
        ("#!/bin/bash\nalias myls=eza\nmy¦", false),
        (
            "#!/bin/bash\nshopt -s expand_aliases\nalias myls=eza\nmy¦",
            true,
        ),
    ] {
        let items = complete(root.path(), source, serde_json::json!({}), false);
        assert_eq!(
            items
                .iter()
                .any(|item| item.label == "myls" && item.detail.as_deref() == Some("Source alias")),
            expected,
            "{source}"
        );
    }
}

#[test]
fn providers_receive_exact_option_values_suffixes_and_argument_boundaries() {
    for (marked, prefix, suffix, words, replacement) in [
        (
            "eza --color=al¦ways",
            "--color=al",
            "ways",
            vec!["eza"],
            "--color=always",
        ),
        (
            "eza --color=\"al¦ways\"",
            "--color=al",
            "ways",
            vec!["eza"],
            "--color=\"always\"",
        ),
        (
            "docker compose --profile te¦st",
            "te",
            "st",
            vec!["docker", "compose", "--profile"],
            "always",
        ),
        ("eza - ¦", "", "", vec!["eza", "-"], "always"),
    ] {
        let cursor = marked.find('¦').unwrap();
        let source = marked.replacen('¦', "", 1);
        let parsed = shucked_parser::parser::Parser::new(&source).parse();
        let index = shucked_indexer::Indexer::new(&source, &parsed);
        let site = context::at(&source, &index, cursor).unwrap();
        assert_eq!(site.prefix, prefix, "{marked}");
        assert_eq!(site.suffix, suffix, "{marked}");
        assert_eq!(site.words, words, "{marked}");
        assert!(
            !site.redirect,
            "option values remain provider contexts: {marked}"
        );
        let candidate = if site.option.is_some() {
            "--color=always"
        } else {
            "always"
        };
        assert_eq!(site.insert(candidate), replacement, "{marked}");
    }
}

#[test]
fn key_value_arguments_keep_provider_context_and_quoted_values() {
    for marked in ["dd if=pa¦th", "docker run --mount type=\"bi¦nd\""] {
        let cursor = marked.find('¦').unwrap();
        let source = marked.replacen('¦', "", 1);
        let parsed = shucked_parser::parser::Parser::new(&source).parse();
        let index = shucked_indexer::Indexer::new(&source, &parsed);
        let site = context::at(&source, &index, cursor).unwrap();
        assert!(!site.redirect, "{marked}");
        assert!(!site.command, "{marked}");
        assert_eq!(site.range.end, source.len());
        if marked.starts_with("dd") {
            assert_eq!(site.prefix, "if=pa");
            assert_eq!(site.insert("if=path"), "if=path");
        } else {
            assert_eq!(site.prefix, "type=bi");
            assert_eq!(site.insert("type=bind"), "type=\"bind\"");
        }
    }
}
