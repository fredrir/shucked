use super::*;

#[test]
fn standalone_clients_do_not_inherit_the_server_working_directory_as_a_workspace() {
    for folders in [None, Some(Vec::new())] {
        let workspaces =
            Workspaces::from_workspace_folders(folders, None, None, WorkspaceOptionsMap::default())
                .unwrap();
        assert!(
            workspaces.is_empty(),
            "an unrelated server launch directory must not be scanned for a standalone document"
        );
    }
}
