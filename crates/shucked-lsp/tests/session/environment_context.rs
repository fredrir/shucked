use super::*;

fn session(roots: &[PathBuf]) -> Session {
    let (events, _) = crossbeam::channel::unbounded();
    let (messages, _) = crossbeam::channel::unbounded();
    let client = Client::new(events, messages);
    let workspaces = Workspaces::new(
        roots
            .iter()
            .map(|root| Workspace::new(Url::from_file_path(root).unwrap()))
            .collect(),
    );
    Session::new(
        &ClientCapabilities::default(),
        PositionEncoding::UTF16,
        GlobalOptions::default().into_settings(client.clone()),
        &workspaces,
        &client,
    )
    .unwrap()
}

#[test]
fn untitled_association_selects_settings_without_applying_a_fictional_file_glob() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(
        first.path().join(".shucked.toml"),
        "[environment.commands.first_tool]\nkind = 'optional'\n",
    )
    .unwrap();
    std::fs::write(
        second.path().join(".shucked.toml"),
        "[environment.commands.second_tool]\nkind = 'generated'\n",
    )
    .unwrap();
    let mut session = session(&[first.path().into(), second.path().into()]);
    let uri = Url::parse("untitled:Untitled-1").unwrap();
    session.open_text_document(uri.clone(), TextDocument::new("second_tool\n".into(), 1));
    let unassigned = session.take_snapshot(uri.clone()).unwrap();
    assert!(unassigned.workspace_cwd.is_none());
    assert!(
        unassigned
            .shuck_settings()
            .command_declarations()
            .is_empty()
    );
    session.select_environment(
        uri.clone(),
        Some(environment_options::EnvironmentOptions {
            workspace_uri: Some(Url::from_file_path(second.path()).unwrap()),
            ..Default::default()
        }),
    );
    let assigned = session.take_snapshot(uri.clone()).unwrap();
    assert_eq!(assigned.workspace_cwd.as_deref(), Some(second.path()));
    assert!(
        assigned
            .shuck_settings()
            .command_declarations()
            .contains_key("second_tool")
    );
    assert!(
        !assigned
            .shuck_settings()
            .command_declarations()
            .contains_key("first_tool")
    );
    assert!(assigned.analysis_settings_epoch() > unassigned.analysis_settings_epoch());
    assert_eq!(assigned.query().file_url(), &uri);
    session.select_environment(uri.clone(), None);
    assert!(session.take_snapshot(uri).unwrap().workspace_cwd.is_none());
}

#[test]
fn only_registered_folders_can_be_associated_and_saved_files_keep_their_folder() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut session = session(&[root.path().into()]);
    let uri = Url::parse("untitled:Untitled-1").unwrap();
    session.open_text_document(uri.clone(), TextDocument::new("printf hi\n".into(), 1));
    assert_eq!(
        session
            .take_snapshot(uri.clone())
            .unwrap()
            .workspace_cwd
            .as_deref(),
        Some(root.path())
    );
    let options = environment_options::EnvironmentOptions {
        workspace_uri: Some(Url::from_file_path(outside.path()).unwrap()),
        ..Default::default()
    };
    session.select_environment(uri.clone(), Some(options.clone()));
    assert!(session.take_snapshot(uri).unwrap().workspace_cwd.is_none());
    let file = Url::from_file_path(root.path().join("script.sh")).unwrap();
    session.open_text_document(file.clone(), TextDocument::new("printf hi\n".into(), 1));
    session.select_environment(file.clone(), Some(options));
    assert_eq!(
        session
            .take_snapshot(file)
            .unwrap()
            .workspace_cwd
            .as_deref(),
        Some(root.path())
    );
}
