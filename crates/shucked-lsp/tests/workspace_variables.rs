use std::fs;
use std::path::Path;

use crossbeam::channel;
use lsp_types::{ClientCapabilities, NumberOrString, Url};
use shucked_lsp::{
    Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
    generate_diagnostics,
};

fn session(root: &Path) -> Session {
    let (events, _) = channel::unbounded();
    let (messages, _) = channel::unbounded();
    let client = Client::new(events, messages);
    let workspaces = Workspaces::new(vec![Workspace::new(Url::from_file_path(root).unwrap())]);
    Session::new(
        &ClientCapabilities::default(),
        PositionEncoding::UTF16,
        GlobalOptions::default().into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap()
}

fn open(session: &mut Session, path: &Path, source: &str, version: i32) -> Url {
    let uri = Url::from_file_path(path).unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.into(), version).with_language_id("shellscript"),
    );
    uri
}

fn unused(session: &Session, uri: &Url) -> Vec<lsp_types::Diagnostic> {
    generate_diagnostics(&session.take_snapshot(uri.clone()).unwrap())
        .into_iter()
        .filter(|d| d.code == Some(NumberOrString::String("C001".into())))
        .collect()
}

#[test]
fn closed_extensionless_consumers_count_as_uses_in_open_helpers() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join(".shucked.toml"),
        "[lint]\nselect = ['C001']\n",
    )
    .unwrap();
    let helper = root.path().join("_common.sh");
    let source = "ADMIN_DIR=/srv/admin\nBACKUP_DIR=/srv/backups\n";
    fs::write(&helper, source).unwrap();
    fs::write(
        root.path().join("backup-sanity"),
        r#"#!/usr/bin/env bash
source "$(dirname -- "${BASH_SOURCE[0]}")/_common.sh"
backup_sanity() { ( cd "$ADMIN_DIR"; echo "$BACKUP_DIR"; ); }
backup_sanity
"#,
    )
    .unwrap();
    let mut session = session(root.path());
    let uri = open(&mut session, &helper, source, 1);
    assert!(unused(&session, &uri).is_empty());
}

#[test]
fn unsaved_consumer_changes_refresh_unchanged_helpers() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join(".shucked.toml"),
        "[lint]\nselect = ['C001']\n",
    )
    .unwrap();
    let helper = root.path().join("_common.sh");
    let consumer = root.path().join("consumer.sh");
    fs::write(&helper, "ADMIN_DIR=/srv/admin\n").unwrap();
    fs::write(&consumer, "source ./_common.sh\n").unwrap();
    let mut session = session(root.path());
    let uri = open(&mut session, &helper, "ADMIN_DIR=/srv/admin\n", 1);
    assert_eq!(unused(&session, &uri).len(), 1);
    open(
        &mut session,
        &consumer,
        "source ./_common.sh\necho \"$ADMIN_DIR\"\n",
        1,
    );
    assert!(unused(&session, &uri).is_empty());
    open(&mut session, &consumer, "source ./_common.sh\n", 2);
    assert_eq!(unused(&session, &uri).len(), 1);
    assert_eq!(
        fs::read_to_string(&consumer).unwrap(),
        "source ./_common.sh\n"
    );
}

#[test]
fn unsaved_helper_definitions_participate_in_workspace_usage() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join(".shucked.toml"),
        "[lint]\nselect = ['C001']\n",
    )
    .unwrap();
    let helper = root.path().join("_common.sh");
    fs::write(&helper, "# empty on disk\n").unwrap();
    fs::write(
        root.path().join("consumer.sh"),
        "source ./_common.sh\necho \"$ADMIN_DIR\"\n",
    )
    .unwrap();
    let mut session = session(root.path());
    let uri = open(&mut session, &helper, "ADMIN_DIR=/srv/admin\n", 1);
    assert!(unused(&session, &uri).is_empty());
}

#[test]
fn derived_source_directories_follow_unsaved_consumer_changes() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join(".shucked.toml"),
        "[lint]\nselect = ['C001']\n",
    )
    .unwrap();
    fs::create_dir_all(root.path().join("scripts")).unwrap();
    fs::create_dir_all(root.path().join("lib one")).unwrap();
    fs::create_dir_all(root.path().join("lib two")).unwrap();
    let first = root.path().join("lib one/common.sh");
    let second = root.path().join("lib two/common.sh");
    let helper_source = "ADMIN_DIR=/srv/admin\n";
    fs::write(&first, helper_source).unwrap();
    fs::write(&second, helper_source).unwrap();
    let consumer = root.path().join("scripts/backup-sanity");
    let source = r#"#!/usr/bin/env bash
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
source "${ROOT_DIR}/lib one/common.sh"
echo "$ADMIN_DIR"
"#;
    fs::write(&consumer, source).unwrap();
    let mut session = session(root.path());
    let first_uri = open(&mut session, &first, helper_source, 1);
    let second_uri = open(&mut session, &second, helper_source, 1);
    assert!(unused(&session, &first_uri).is_empty());
    assert_eq!(unused(&session, &second_uri).len(), 1);
    open(
        &mut session,
        &consumer,
        &source.replace("lib one", "lib two"),
        1,
    );
    assert_eq!(unused(&session, &first_uri).len(), 1);
    assert!(unused(&session, &second_uri).is_empty());
}
