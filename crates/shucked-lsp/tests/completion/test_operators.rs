use super::*;
use crate::session::RequestCancellationToken;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};

fn complete(marked: &str, with_environment: bool) -> types::CompletionList {
    let root = tempfile::tempdir().unwrap();
    let cursor = marked.find('¦').unwrap();
    let source = marked.replacen('¦', "", 1);
    let (sender, _) = crossbeam::channel::unbounded();
    let (client_sender, _) = crossbeam::channel::unbounded();
    let client = Client::new(sender, client_sender);
    let uri = types::Url::from_file_path(root.path().join("script.sh")).unwrap();
    let workspaces = Workspaces::new(vec![Workspace::default(
        types::Url::from_file_path(root.path()).unwrap(),
    )]);
    let capabilities = serde_json::from_value(serde_json::json!({})).unwrap();
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
        TextDocument::new(source.clone(), 1).with_language_id("shellscript"),
    );
    let environment = session.completion_environment.clone();
    let snapshot = session.take_snapshot(uri.clone()).unwrap();
    let position = crate::edit::offset_to_position(
        &source,
        snapshot.query().document().index(),
        cursor,
        snapshot.encoding(),
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
    let response = if with_environment {
        crate::handlers::editor_features::completion_with_environment(
            snapshot,
            &client,
            params,
            Some((&environment, &RequestCancellationToken::default())),
            |_, _| Vec::new(),
        )
    } else {
        crate::handlers::editor_features::completion(snapshot, &client, params)
    }
    .unwrap();
    match response {
        Some(types::CompletionResponse::List(list)) => list,
        None => types::CompletionList {
            is_incomplete: false,
            items: Vec::new(),
        },
        _ => panic!("expected completion list"),
    }
}

fn operators(list: &types::CompletionList) -> Vec<&str> {
    list.items
        .iter()
        .filter(|item| item.kind == Some(types::CompletionItemKind::OPERATOR))
        .map(|item| item.label.as_str())
        .collect()
}

fn insertion(item: &types::CompletionItem) -> &str {
    match item.text_edit.as_ref().unwrap() {
        types::CompletionTextEdit::Edit(edit) => &edit.new_text,
        types::CompletionTextEdit::InsertAndReplace(edit) => &edit.new_text,
    }
}

const UNARY_LABELS: &[&str] = &[
    "-a", "-b", "-c", "-d", "-e", "-f", "-G", "-g", "-h", "-k", "-L", "-n", "-N", "-o", "-O", "-p",
    "-r", "-s", "-S", "-t", "-u", "-v", "-w", "-x", "-z", "-R",
];

const BINARY_LABELS: &[&str] = &[
    "-nt", "-ot", "-ef", "-eq", "-ne", "-lt", "-le", "-gt", "-ge",
];

#[test]
fn dash_after_an_opener_lists_every_unary_operator_in_order() {
    for source in [
        "[[ -¦",
        "if [ -¦",
        "test -¦",
        "[[ ! -¦",
        "while [ -¦ ]; do :; done",
        "[[ -f x && -¦",
        "[ -f x ] || [ -¦",
        "[[ -f a &&\n    -¦",
        "case $x in\n  y) [ -¦\nesac",
    ] {
        let list = complete(source, false);
        assert_eq!(operators(&list), UNARY_LABELS, "{source}");
        assert!(!list.is_incomplete, "{source}");
        assert!(
            list.items
                .iter()
                .all(|item| item.kind == Some(types::CompletionItemKind::OPERATOR)),
            "{source}: {:?}",
            list.items
        );
    }
}

#[test]
fn operators_carry_short_descriptions() {
    let list = complete("[[ -¦", false);
    let by_label = |label: &str| {
        list.items
            .iter()
            .find(|item| item.label == label)
            .unwrap_or_else(|| panic!("missing {label}: {:?}", list.items))
    };
    assert_eq!(by_label("-z").detail.as_deref(), Some("empty string"));
    assert_eq!(by_label("-f").detail.as_deref(), Some("regular file"));
    assert_eq!(
        by_label("-N").detail.as_deref(),
        Some("modified since last read")
    );
    assert!(matches!(
        by_label("-N").documentation,
        Some(types::Documentation::String(ref doc)) if doc.contains("last read")
    ));
    assert_eq!(insertion(by_label("-z")), "-z");
    assert_eq!(
        by_label("-z").insert_text_format,
        Some(types::InsertTextFormat::PLAIN_TEXT)
    );
}

#[test]
fn dash_after_an_operand_lists_only_binary_operators() {
    for source in ["[[ $a -¦", "[ \"$#\" -¦", "test $(cmd) -¦"] {
        let list = complete(source, false);
        let labels = operators(&list);
        assert!(labels.starts_with(BINARY_LABELS), "{source}: {labels:?}");
        assert!(!labels.contains(&"-f"), "{source}: {labels:?}");
        assert!(!labels.contains(&"-z"), "{source}: {labels:?}");
        assert!(!list.is_incomplete, "{source}");
    }
    assert_eq!(operators(&complete("[[ $a -¦", false)), BINARY_LABELS);
    let list = complete("[ $a -¦", false);
    assert_eq!(
        operators(&list),
        [BINARY_LABELS, &["-a", "-o"]].concat(),
        "bracket tests also connect with -a/-o"
    );
}

#[test]
fn empty_prefix_after_an_operand_includes_string_operators_and_connectors() {
    let list = complete("[[ $a ¦", false);
    let labels = operators(&list);
    for label in ["-nt", "=", "==", "!=", "=~", "<", ">", "&&", "||"] {
        assert!(labels.contains(&label), "{label} missing from {labels:?}");
    }
    assert!(!labels.contains(&"-a"), "{labels:?}");

    let list = complete("[ $a ¦", false);
    let labels = operators(&list);
    for label in ["-nt", "=", "==", "!=", "<", ">", "-a", "-o"] {
        assert!(labels.contains(&label), "{label} missing from {labels:?}");
    }
    for label in ["=~", "&&", "||"] {
        assert!(!labels.contains(&label), "{label} present in {labels:?}");
    }
    let less = list.items.iter().find(|item| item.label == "<").unwrap();
    assert_eq!(insertion(less), "\\<", "string order needs escaping in [ ]");
    let equals = complete("[[ $a =¦", false);
    assert_eq!(operators(&equals), ["=", "==", "=~"]);
    let unequal = complete("[ $a !¦", false);
    assert_eq!(operators(&unequal), ["!="]);
}

#[test]
fn complete_tests_offer_connectors() {
    let list = complete("[ -f x -¦", false);
    let labels = operators(&list);
    assert!(labels.contains(&"-a"), "{labels:?}");
    assert!(labels.contains(&"-o"), "{labels:?}");
    assert!(!labels.contains(&"-f"), "{labels:?}");
    assert!(!labels.contains(&"-nt"), "{labels:?}");
    assert_eq!(operators(&complete("[[ -f x ¦", false)), ["&&", "||"]);
    assert_eq!(operators(&complete("[[ $a == $b ¦", false)), ["&&", "||"]);
    assert_eq!(
        operators(&complete("[[ $a == $b || -¦", false)),
        UNARY_LABELS
    );
}

#[test]
fn typed_letters_narrow_the_operators() {
    let list = complete("[[ -n¦", false);
    assert_eq!(operators(&list), ["-n"]);
    assert!(!list.is_incomplete);
    assert_eq!(operators(&complete("[[ $a -n¦", false)), ["-nt", "-ne"]);
    assert_eq!(operators(&complete("[[ $a -e¦", false)), ["-ef", "-eq"]);
    assert_eq!(operators(&complete("[[ -f¦ x ]]", false)), ["-f"]);
}

#[test]
fn operand_positions_and_other_commands_keep_their_completions() {
    for source in [
        "echo -¦",
        "[[ \"-¦",
        "[[ '-¦",
        "[[ -f ¦",
        "[[ -f -¦",
        "[ $a -eq ¦",
        "[[ $a == ¦",
        "[[ -f x ]] && -¦",
        "[ -f x ]; -¦",
        "echo [ -¦",
        "grep -e [[ -¦",
        "x=$([[ -¦",
        "[[ -f x ¦]]",
        "echo $a # [[ -¦",
        "cat <<EOF\n[[ -¦\nEOF",
    ] {
        let list = complete(source, false);
        assert!(operators(&list).is_empty(), "{source}: {:?}", list.items);
    }
    let list = complete("[[ -f x ]] && ¦", false);
    assert!(
        list.items
            .iter()
            .any(|item| item.kind == Some(types::CompletionItemKind::KEYWORD)),
        "command position after a closed test keeps command completions: {:?}",
        list.items
    );
}

#[test]
fn operator_sites_skip_shell_backed_providers() {
    for source in ["[[ -¦", "test -¦", "[ $a ¦", "[[ ¦"] {
        let list = complete(source, true);
        assert!(!list.is_incomplete, "{source}");
        assert!(
            !list.items.is_empty()
                && list
                    .items
                    .iter()
                    .all(|item| item.kind == Some(types::CompletionItemKind::OPERATOR)),
            "{source}: {:?}",
            list.items
        );
    }
}

#[test]
fn portable_scripts_flag_extensions_inside_brackets() {
    let list = complete("#!/bin/sh\n[ -¦", false);
    let detail = |label: &str| {
        list.items
            .iter()
            .find(|item| item.label == label)
            .and_then(|item| item.detail.clone())
            .unwrap_or_default()
    };
    assert_eq!(detail("-f"), "regular file");
    assert_eq!(detail("-N"), "modified since last read (not POSIX)");
    let list = complete("#!/bin/sh\n[[ -¦", false);
    let modified = list.items.iter().find(|item| item.label == "-N").unwrap();
    assert_eq!(
        modified.detail.as_deref(),
        Some("modified since last read"),
        "[[ is already an extension, so its operators are not flagged"
    );
    let list = complete("#!/bin/bash\n[ -¦", false);
    let modified = list.items.iter().find(|item| item.label == "-N").unwrap();
    assert_eq!(modified.detail.as_deref(), Some("modified since last read"));
}

#[test]
fn unary_operators_keep_presentation_order_ahead_of_alphabetical_sorting() {
    let list = complete("[[ -¦", false);
    let sort_keys: Vec<&str> = list
        .items
        .iter()
        .map(|item| item.sort_text.as_deref().unwrap())
        .collect();
    let mut sorted = sort_keys.clone();
    sorted.sort_unstable();
    assert_eq!(sort_keys, sorted);
    assert_eq!(
        list.items.first().map(|item| item.label.as_str()),
        Some("-a")
    );
    assert_eq!(
        list.items.last().map(|item| item.label.as_str()),
        Some("-R")
    );
}
