use super::*;
use crate::{GlobalOptions, PositionEncoding, TextDocument, Workspaces};
use lsp_server::Message;
use std::sync::Arc;

type Fixture = (
    tempfile::TempDir,
    Session,
    Client,
    crossbeam::channel::Receiver<Message>,
    types::Url,
);

fn fixture(deferred: bool) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let capabilities: types::ClientCapabilities = serde_json::from_value(serde_json::json!({
        "workspace": { "applyEdit": true, "workspaceEdit": { "documentChanges": true } },
        "textDocument": { "codeAction": { "dataSupport": deferred, "resolveSupport": { "properties": ["edit"] } } }
    })).unwrap();
    let (events, _) = crossbeam::channel::unbounded();
    let (messages, receiver) = crossbeam::channel::unbounded();
    let client = Client::new(events, messages);
    let global = GlobalOptions::default().into_settings(client.clone());
    let mut session = Session::new(
        &capabilities,
        PositionEncoding::UTF16,
        global,
        &Workspaces::new(vec![]),
        &client,
    )
    .unwrap();
    let uri = types::Url::from_file_path(root.path().join("fixture.sh")).unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new("pritnf hello\n".into(), 1).with_language_id("shellscript"),
    );
    (root, session, client, receiver, uri)
}

fn action(session: &Session, uri: &types::Url) -> types::CodeAction {
    let mut snapshot = session.take_snapshot(uri.clone()).unwrap();
    snapshot.command_service =
        Arc::new(super::super::commands::CommandService::fixture(Vec::new()));
    code_actions(
        &snapshot,
        &types::Range::new(types::Position::new(0, 0), types::Position::new(0, 6)),
    )
    .into_iter()
    .find_map(|action| match action {
        types::CodeActionOrCommand::CodeAction(action)
            if action.title == "Replace with `printf`" =>
        {
            Some(action)
        }
        _ => None,
    })
    .expect("manual typo correction")
}

fn payload(action: &types::CodeAction) -> Vec<serde_json::Value> {
    action.command.as_ref().unwrap().arguments.clone().unwrap()
}

#[test]
fn manual_correction_applies_only_the_issued_versioned_edit_and_cannot_replay() {
    let (_root, session, client, receiver, uri) = fixture(false);
    let action = action(&session, &uri);
    assert!(action.edit.is_none());
    assert!(action.data.is_none());
    let arguments = payload(&action);
    assert!(
        arguments[0]["sourceFingerprint"].is_string(),
        "fingerprints must survive JavaScript JSON round trips"
    );
    execute(&session, &client, &arguments).unwrap();
    let request = receiver
        .try_iter()
        .find_map(|message| {
            if let Message::Request(request) = message {
                Some(request)
            } else {
                None
            }
        })
        .expect("workspace edit request");
    assert_eq!(request.method, "workspace/applyEdit");
    assert_eq!(
        request.params["edit"]["documentChanges"][0]["textDocument"]["version"],
        1
    );
    assert_eq!(
        request.params["edit"]["documentChanges"][0]["edits"][0]["newText"],
        "printf"
    );
    assert!(execute(&session, &client, &arguments).is_err());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn switching_target_with_unchanged_document_invalidates_resolve_and_execute() {
    let (_root, mut session, client, receiver, uri) = fixture(true);
    let action = action(&session, &uri);
    let arguments = payload(&action);
    session.select_environment(
        uri,
        Some(crate::session::environment_options::EnvironmentOptions {
            policy: Some("portable".into()),
            ..Default::default()
        }),
    );
    let resolved = resolve(&session, action);
    assert!(resolved.disabled.is_some());
    assert!(resolved.edit.is_none());
    assert!(resolved.command.is_none());
    assert!(execute(&session, &client, &arguments).is_err());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn document_edit_invalidates_a_previous_correction() {
    let (_root, mut session, client, receiver, uri) = fixture(false);
    let arguments = payload(&action(&session, &uri));
    let key = session.key_from_url(uri);
    session
        .update_text_document(
            &key,
            vec![types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "echo changed\n".into(),
            }],
            2,
        )
        .unwrap();
    assert!(execute(&session, &client, &arguments).is_err());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn forged_payload_cannot_supply_a_replacement_or_borrow_a_correction() {
    let (_root, session, client, receiver, uri) = fixture(false);
    let original = payload(&action(&session, &uri));
    let mut forged = original.clone();
    forged[0]["replacement"] = "$(touch marker)".into();
    assert!(execute(&session, &client, &forged).is_err());
    forged = original;
    forged[0]["key"]["targetId"] = "different-host".into();
    assert!(execute(&session, &client, &forged).is_err());
    assert!(receiver.try_recv().is_err());
}

#[test]
fn resolving_keeps_execution_guard_so_a_later_target_change_still_blocks_application() {
    let (_root, mut session, client, receiver, uri) = fixture(true);
    let resolved = resolve(&session, action(&session, &uri));
    assert!(resolved.disabled.is_none());
    assert!(resolved.edit.is_none());
    let arguments = payload(&resolved);
    session.select_environment(uri, None);
    assert!(execute(&session, &client, &arguments).is_err());
    assert!(receiver.try_recv().is_err());
}
