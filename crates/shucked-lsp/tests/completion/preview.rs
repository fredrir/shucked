use super::*;
use crate::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
};

fn fixture(
    root: &Path,
    marked: &str,
    options: serde_json::Value,
) -> (
    DocumentSnapshot,
    Client,
    types::Position,
    crossbeam::channel::Receiver<lsp_server::Message>,
) {
    let cursor = marked.find('¦').unwrap();
    let source = marked.replacen('¦', "", 1);
    let (main, _) = crossbeam::channel::unbounded();
    let (out, messages) = crossbeam::channel::unbounded();
    let client = Client::new(main, out);
    let workspaces = Workspaces::new(vec![Workspace::default(
        types::Url::from_file_path(root).unwrap(),
    )]);
    let global: GlobalOptions = serde_json::from_value(options).unwrap();
    let mut session = Session::new(
        &Default::default(),
        PositionEncoding::UTF16,
        global.into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap();
    let uri = types::Url::from_file_path(root.join("preview.sh")).unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.clone(), 1).with_language_id("shellscript"),
    );
    let snapshot = session.take_snapshot(uri).unwrap();
    let position = crate::edit::offset_to_position(
        &source,
        snapshot.query().document().index(),
        cursor,
        snapshot.encoding(),
    );
    (snapshot, client, position, messages)
}

#[test]
fn directory_preview_uses_its_own_background_lane_without_workspace_analysis() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder space")).unwrap();
    std::fs::write(root.path().join("ordinary-file"), "").unwrap();
    let (snapshot, client, position, messages) =
        fixture(root.path(), "cd ¦", serde_json::json!({}));
    let mut environment = Environment::fixture(root.path());
    environment.synchronous = false;
    assert!(
        directory_preview(&snapshot, &environment, &client, position)
            .unwrap()
            .is_empty()
    );
    assert!(
        snapshot
            .command_service
            .cached_analysis(&snapshot)
            .is_none()
    );
    assert!(
        crate::workspace_functions::cached_workspace_function_index(
            snapshot.workspace_functions.as_ref().unwrap()
        )
        .is_none()
    );
    messages
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let items = directory_preview(&snapshot, &environment, &client, position).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "folder space/");
    assert_eq!(items[0].kind, Some(types::CompletionItemKind::FOLDER));
}

#[test]
fn preview_preserves_home_expansion_and_existing_quote_ranges() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder space")).unwrap();
    let environment = Environment::fixture(root.path());
    for (marked, start, end, expected) in [
        ("cd ~/fo¦", 5, 7, "folder\\ space/"),
        ("cd 'fo¦wrong'", 4, 11, "folder space/"),
    ] {
        let (snapshot, client, position, _) = fixture(root.path(), marked, serde_json::json!({}));
        let items = directory_preview(&snapshot, &environment, &client, position)
            .unwrap_or_else(|| panic!("literal directory preview: {marked}"));
        let Some(types::CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("replacement edit")
        };
        assert_eq!(edit.range.start.character, start);
        assert_eq!(edit.range.end.character, end);
        assert_eq!(edit.new_text, expected);
    }
}

#[test]
fn preview_does_not_guess_for_shadowed_dynamic_or_nonlocal_commands() {
    let root = tempfile::tempdir().unwrap();
    let environment = Environment::fixture(root.path());
    for marked in [
        "cd() { :; }\ncd ¦",
        "alias cd=echo\ncd ¦",
        "source helpers.sh\ncd ¦",
        "PATH=$OTHER\ncd ¦",
        "cd $HOME/¦",
        "cd -¦",
        "docker ¦",
        "# cd ¦",
        "#!/usr/bin/env fish\ncd ¦",
    ] {
        let (snapshot, client, position, _) = fixture(root.path(), marked, serde_json::json!({}));
        assert!(
            directory_preview(&snapshot, &environment, &client, position).is_none(),
            "{marked}"
        );
    }
    for options in [
        serde_json::json!({"environment": {"policy": "portable"}}),
        serde_json::json!({"environment": {"sessionId": "attached"}}),
        serde_json::json!({"environment": {"targetInventory": "/missing/inventory.json"}}),
    ] {
        let (snapshot, client, position, _) = fixture(root.path(), "cd ¦", options);
        assert!(directory_preview(&snapshot, &environment, &client, position).is_none());
    }
}
