//! Definition, declaration and implementation navigation across `source`
//! operands, function candidates, declaration sites and command scripts.

use std::path::Path;

use crossbeam::channel::{self, Receiver};
use lsp_types as types;

use super::super::traits::BackgroundRequestHandler;
use super::{Declaration, Implementation};
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

fn params(uri: &types::Url, line: u32, character: u32) -> types::GotoDefinitionParams {
    types::GotoDefinitionParams {
        text_document_position_params: types::TextDocumentPositionParams {
            text_document: types::TextDocumentIdentifier { uri: uri.clone() },
            position: types::Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    }
}

fn locations(response: Option<types::GotoDefinitionResponse>) -> Vec<types::Location> {
    match response {
        None => Vec::new(),
        Some(types::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(types::GotoDefinitionResponse::Array(locations)) => locations,
        Some(types::GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| types::Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            })
            .collect(),
    }
}

fn definition(
    session: &Session,
    client: &Client,
    params: types::GotoDefinitionParams,
) -> Vec<types::Location> {
    let snapshot = session
        .take_snapshot(
            params
                .text_document_position_params
                .text_document
                .uri
                .clone(),
        )
        .unwrap();
    let workspace = session.workspace_function_context(RequestCancellationToken::default());
    locations(super::definition::definition(snapshot, workspace, client, params).unwrap())
}

fn implementation(
    session: &Session,
    client: &Client,
    params: types::GotoDefinitionParams,
) -> Vec<types::Location> {
    let snapshot =
        Implementation::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    locations(Implementation::run_with_snapshot(snapshot, client, params).unwrap())
}

fn declaration(
    session: &Session,
    client: &Client,
    params: types::GotoDefinitionParams,
) -> Vec<types::Location> {
    let snapshot =
        Declaration::snapshot(session, &params, RequestCancellationToken::default()).unwrap();
    locations(Declaration::run_with_snapshot(snapshot, client, params).unwrap())
}

fn file_name(location: &types::Location) -> String {
    location
        .uri
        .to_file_path()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn definition_and_implementation_on_a_source_operand_open_the_loaded_file() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("helper.sh"), "VALUE=1\n").unwrap();
    let (mut session, client, _) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "source helper.sh\necho \"$VALUE\"\n",
    );
    for locations in [
        definition(&session, &client, params(&uri, 0, 10)),
        implementation(&session, &client, params(&uri, 0, 10)),
        declaration(&session, &client, params(&uri, 0, 10)),
    ] {
        assert_eq!(locations.len(), 1, "{locations:?}");
        assert_eq!(file_name(&locations[0]), "helper.sh");
        assert_eq!(locations[0].range.start, types::Position::new(0, 0));
    }
    // The `source` word itself is a command, not the operand.
    assert!(definition(&session, &client, params(&uri, 0, 2)).is_empty());
}

#[test]
fn implementation_on_a_call_lists_every_candidate_body() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("linux.sh"),
        "open_url() { xdg-open \"$1\"; }\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("darwin.sh"),
        "open_url() { open \"$1\"; }\n",
    )
    .unwrap();
    let (mut session, client, _) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "if [[ $OSTYPE == darwin* ]]; then\n  source darwin.sh\nelse\n  source linux.sh\nfi\nopen_url https://example.invalid\n",
    );
    let mut bodies = implementation(&session, &client, params(&uri, 5, 3))
        .iter()
        .map(file_name)
        .collect::<Vec<_>>();
    bodies.sort();
    assert_eq!(bodies, ["darwin.sh", "linux.sh"]);
}

#[test]
fn implementation_on_a_definition_lists_redefinitions_across_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("override.sh"),
        "greet() { echo override; }\n",
    )
    .unwrap();
    let (mut session, client, _) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "greet() { echo hi; }\nsource override.sh\ngreet\n",
    );
    let mut bodies = implementation(&session, &client, params(&uri, 0, 2))
        .iter()
        .map(file_name)
        .collect::<Vec<_>>();
    bodies.sort();
    assert_eq!(bodies, ["main.sh", "override.sh"]);
}

#[test]
fn declaration_prefers_declaration_builtins_and_falls_back_to_assignments() {
    let root = tempfile::tempdir().unwrap();
    let (mut session, client, _) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "export VALUE=1\nVALUE=2\necho \"$VALUE\"\nOTHER=1\necho \"$OTHER\"\n",
    );
    let declared = declaration(&session, &client, params(&uri, 2, 8));
    assert_eq!(declared.len(), 1, "{declared:?}");
    assert_eq!(declared[0].range.start.line, 0);
    let assigned = declaration(&session, &client, params(&uri, 4, 8));
    assert_eq!(assigned.len(), 1, "{assigned:?}");
    assert_eq!(assigned[0].range.start.line, 3);
}

#[test]
fn definition_on_an_external_command_opens_its_script() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = bin.join("deploy-tool");
    std::fs::write(&script, "#!/bin/sh\necho deploying\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let binary = bin.join("compiled-tool");
    std::fs::write(&binary, [0x7f, b'E', b'L', b'F', 0, 0]).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (mut session, client, _) = session(root.path());
    let uri = open(
        &mut session,
        &root.path().join("main.sh"),
        "deploy-tool --dry-run\ncompiled-tool\n",
    );
    let workspace = session.workspace_function_context(RequestCancellationToken::default());
    let mut snapshot = session.take_snapshot(uri.clone()).unwrap();
    snapshot.command_service =
        std::sync::Arc::new(crate::handlers::commands::CommandService::fixture(vec![
            bin.clone(),
        ]));
    let found = locations(
        super::definition::definition(
            snapshot.clone(),
            workspace.clone(),
            &client,
            params(&uri, 0, 3),
        )
        .unwrap(),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].uri.to_file_path().unwrap(), script);
    let compiled = locations(
        super::definition::definition(snapshot, workspace, &client, params(&uri, 1, 3)).unwrap(),
    );
    assert!(compiled.is_empty(), "{compiled:?}");
}
