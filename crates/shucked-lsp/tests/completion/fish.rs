use super::*;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};
use lsp_types::{ClientCapabilities, Url};

fn fixture(root: &std::path::Path, marked: &str) -> (DocumentSnapshot, types::CompletionParams) {
    let cursor = marked.find('¦').unwrap();
    let source = marked.replacen('¦', "", 1);
    let (sender, _) = crossbeam::channel::unbounded();
    let (client_sender, _) = crossbeam::channel::unbounded();
    let client = Client::new(sender, client_sender);
    let uri = Url::from_file_path(root.join("script.fish")).unwrap();
    let workspaces = Workspaces::new(vec![Workspace::default(Url::from_file_path(root).unwrap())]);
    let mut session = Session::new(
        &ClientCapabilities::default(),
        PositionEncoding::UTF16,
        GlobalOptions::default().into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.clone(), 1).with_language_id("fish"),
    );
    let snapshot = session.take_snapshot(uri.clone()).unwrap();
    let position = offset_to_position(
        &source,
        snapshot.query().document().index(),
        cursor,
        snapshot.encoding(),
    );
    (
        snapshot,
        types::CompletionParams {
            text_document_position: types::TextDocumentPositionParams {
                text_document: types::TextDocumentIdentifier { uri },
                position,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        },
    )
}
fn items(root: &std::path::Path, marked: &str) -> Vec<types::CompletionItem> {
    let (snapshot, params) = fixture(root, marked);
    let env = Environment::fixture(root);
    match complete(
        &snapshot,
        &params,
        Some((&env, &RequestCancellationToken::default())),
        None,
    )
    .unwrap()
    {
        types::CompletionResponse::List(list) => list.items,
        _ => panic!("list"),
    }
}
#[test]
fn fish_function_and_builtin_completion_does_not_need_bash_analysis() {
    let root = tempfile::tempdir().unwrap();
    let (snapshot, _) = fixture(root.path(), "function greet\n echo hi\nend\ngre¦");
    assert!(snapshot.analysis().is_none());
    assert!(
        items(root.path(), "function greet\n echo hi\nend\ngre¦")
            .iter()
            .any(|i| i.label == "greet")
    );
    assert!(
        items(root.path(), "str¦")
            .iter()
            .any(|i| i.label == "string")
    );
}

#[test]
fn fish_blank_arguments_do_not_guess_files_and_cd_only_lists_directories() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("workspace-noise"), "").unwrap();
    std::fs::create_dir(root.path().join("real-directory")).unwrap();
    assert!(items(root.path(), "unregistered ¦").is_empty());
    let completed = items(root.path(), "cd ¦");
    assert_eq!(
        completed
            .iter()
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>(),
        ["real-directory/"]
    );
    assert!(
        items(root.path(), "unregistered ./¦")
            .iter()
            .any(|item| item.label == "./workspace-noise")
    );
}
#[test]
fn fish_path_edit_replaces_entire_quoted_word_with_correct_utf16_range() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file name"), "").unwrap();
    let items = items(root.path(), "echo ø 'fi¦le'");
    let item = items.iter().find(|i| i.label == "file name").unwrap();
    let Some(types::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("edit")
    };
    assert_eq!(edit.new_text, "'file name'");
    assert_eq!(edit.range.start.character, 7);
    assert_eq!(edit.range.end.character, 13);
}
#[test]
fn fish_tokens_include_source_functions_without_bash_fallback() {
    let root = tempfile::tempdir().unwrap();
    let (snapshot, _) = fixture(root.path(), "function greet\n echo 'hi'\nend\ngreet¦");
    let tokens = crate::handlers::semantic_tokens_fish::full(&snapshot);
    assert!(tokens.data.iter().any(|token| token.token_type == 1));
    assert!(tokens.data.iter().any(|token| token.token_type == 4));
}

#[test]
fn fish_comments_do_not_offer_command_or_path_completions() {
    let root = tempfile::tempdir().unwrap();
    for source in ["# str¦", "echo hi # str¦"] {
        let (snapshot, params) = fixture(root.path(), source);
        assert!(complete(&snapshot, &params, None, None).is_none());
    }
    assert!(
        items(root.path(), "echo (str¦")
            .iter()
            .any(|item| item.label == "string")
    );
}

#[test]
fn fish_tilde_paths_preserve_expansion_and_quoted_tildes_stay_literal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("~")).unwrap();
    std::fs::write(root.path().join("~/literal file"), "").unwrap();
    std::fs::write(root.path().join("home file"), "").unwrap();
    let quoted = items(root.path(), "cat '~/li¦'");
    assert!(quoted.iter().any(|item| item.label == "~/literal file"));
    let unquoted = items(root.path(), "cat ~/ho¦");
    let item = unquoted
        .iter()
        .find(|item| item.label == "~/home file")
        .unwrap();
    let Some(types::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("edit")
    };
    assert_eq!(edit.new_text, "~/'home file'");
}

#[test]
fn fish_variable_completion_keeps_surrounding_double_quotes() {
    let root = tempfile::tempdir().unwrap();
    let items = items(root.path(), "echo \"$SHUCKED_TEST_¦\"");
    let item = items
        .iter()
        .find(|item| item.label == "$SHUCKED_TEST_VARIABLE")
        .unwrap();
    let Some(types::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
        panic!("edit")
    };
    assert_eq!(edit.new_text, "$SHUCKED_TEST_VARIABLE");
    assert_eq!(edit.range.start.character, 6);
    assert_eq!(edit.range.end.character, 20);
}
