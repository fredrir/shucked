use super::*;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};

fn complete(
    dialect: &str,
    marked: &str,
    snippets: bool,
    insert_replace: bool,
) -> Vec<types::CompletionItem> {
    let root = tempfile::tempdir().unwrap();
    let cursor = marked.find('¦').unwrap();
    let source = marked.replacen('¦', "", 1);
    let (sender, _) = crossbeam::channel::unbounded();
    let (client_sender, _) = crossbeam::channel::unbounded();
    let client = Client::new(sender, client_sender);
    let uri = types::Url::from_file_path(root.path().join(format!("script.{dialect}"))).unwrap();
    let workspaces = Workspaces::new(vec![Workspace::default(
        types::Url::from_file_path(root.path()).unwrap(),
    )]);
    let capabilities = serde_json::from_value(serde_json::json!({
        "textDocument": {"completion": {"completionItem": {
            "snippetSupport": snippets,
            "insertReplaceSupport": insert_replace,
            "insertTextModeSupport": {"valueSet": [1, 2]}
        }}}
    }))
    .unwrap();
    let mut session = Session::new(
        &capabilities,
        PositionEncoding::UTF16,
        GlobalOptions::default().into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.clone(), 1).with_language_id(if dialect == "fish" {
            "fish"
        } else {
            "shellscript"
        }),
    );
    let snapshot = session.take_snapshot(uri.clone()).unwrap();
    let position = crate::edit::offset_to_position(
        &source,
        snapshot.query().document().index(),
        cursor,
        snapshot.encoding(),
    );
    let response = crate::handlers::editor_features::completion(
        snapshot,
        &client,
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
    .unwrap();
    match response {
        Some(types::CompletionResponse::List(list)) => list.items,
        None => Vec::new(),
        _ => panic!("expected completion list"),
    }
}

fn insertion(item: &types::CompletionItem) -> &str {
    match item.text_edit.as_ref().unwrap() {
        types::CompletionTextEdit::Edit(edit) => &edit.new_text,
        types::CompletionTextEdit::InsertAndReplace(edit) => &edit.new_text,
    }
}

#[test]
fn shell_structures_offer_editable_dialect_specific_bodies() {
    for dialect in ["sh", "bash", "zsh", "fish"] {
        for (prefix, label) in [
            ("i", "if"),
            ("fo", "for"),
            ("wh", "while"),
            ("fun", "function"),
            ("ca", "case"),
        ] {
            let items = complete(dialect, &format!("{prefix}¦"), true, false);
            let item = items
                .iter()
                .find(|item| item.label == label)
                .unwrap_or_else(|| panic!("{dialect} {label}: {items:?}"));
            let text = insertion(item);
            assert_eq!(
                item.insert_text_format,
                Some(types::InsertTextFormat::SNIPPET)
            );
            assert_eq!(
                item.insert_text_mode,
                Some(types::InsertTextMode::ADJUST_INDENTATION)
            );
            assert!(text.contains("${1:"), "{text}");
            assert!(text.contains("\n\t"), "{text}");
            assert!(text.ends_with("\n$0"), "{text}");
            if dialect == "fish" {
                assert!(!text.contains("; then") && !text.contains("; do"), "{text}");
                if label != "case" {
                    assert!(text.contains("\nend\n"), "{text}");
                }
            } else if label == "if" {
                assert!(
                    text.contains("; then\n") && text.contains("\nfi\n"),
                    "{text}"
                );
            }
        }
    }
}

#[test]
fn clients_without_snippet_support_receive_plain_keywords() {
    for dialect in ["bash", "fish"] {
        let items = complete(dialect, "i¦", false, false);
        let item = items.iter().find(|item| item.label == "if").unwrap();
        assert_eq!(insertion(item), "if");
        assert_ne!(
            item.insert_text_format,
            Some(types::InsertTextFormat::SNIPPET)
        );
        assert_eq!(item.kind, Some(types::CompletionItemKind::KEYWORD));
    }
}

#[test]
fn fully_typed_if_keeps_the_block_as_the_first_choice() {
    for dialect in ["bash", "zsh", "fish"] {
        let items = complete(dialect, "if¦", true, false);
        let first = items.first().expect("keyword completion");
        assert_eq!(first.label, "if", "{dialect}: {items:?}");
        assert_eq!(
            first.insert_text_format,
            Some(types::InsertTextFormat::SNIPPET)
        );
    }
}

#[test]
fn snippets_preserve_full_token_replace_and_prefix_insert_ranges() {
    for dialect in ["bash", "fish"] {
        let items = complete(dialect, "    i¦wrong", true, true);
        let item = items.iter().find(|item| item.label == "if").unwrap();
        let Some(types::CompletionTextEdit::InsertAndReplace(edit)) = &item.text_edit else {
            panic!("insert/replace edit")
        };
        assert_eq!(
            edit.insert,
            types::Range::new(types::Position::new(0, 4), types::Position::new(0, 5))
        );
        assert_eq!(
            edit.replace,
            types::Range::new(types::Position::new(0, 4), types::Position::new(0, 10))
        );
        assert!(edit.new_text.starts_with("if ${1:condition}"));
    }
}

#[test]
fn arguments_comments_and_quoted_commands_do_not_expand_structures() {
    for dialect in ["bash", "zsh", "fish"] {
        for marked in [
            "echo i¦",
            "# i¦",
            "echo ok # i¦",
            "'i¦'",
            "\"i¦\"",
            "echo $i¦",
        ] {
            let items = complete(dialect, marked, true, false);
            assert!(
                items
                    .iter()
                    .all(|item| item.insert_text_format != Some(types::InsertTextFormat::SNIPPET)),
                "{dialect} {marked}: {items:?}"
            );
        }
    }
}

#[test]
fn continuation_clauses_leave_existing_closing_keywords_in_place() {
    let items = complete("bash", "if true; then\n    :\nel¦\nfi", true, false);
    let item = items.iter().find(|item| item.label == "elif").unwrap();
    assert_eq!(insertion(item), "elif ${1:condition}; then\n\t${2::}\n$0");
    let items = complete("fish", "switch value\nca¦\nend", true, false);
    let item = items.iter().find(|item| item.label == "case").unwrap();
    assert_eq!(insertion(item), "case ${1:pattern}\n\t${2:command}\n$0");
}
