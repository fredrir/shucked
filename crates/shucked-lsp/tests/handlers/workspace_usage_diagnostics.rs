use super::*;
use crate::{GlobalOptions, PositionEncoding, Session, Workspace, Workspaces};

#[test]
fn closed_helper_reports_refresh_after_consumer_edits_and_close() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".shucked.toml"),
        "[lint]\nselect = ['C001']\n",
    )
    .unwrap();
    let helper = root.path().join("_common.sh");
    let consumer = root.path().join("consumer.sh");
    std::fs::write(&helper, "ADMIN_DIR=/srv/admin\n").unwrap();
    let used = "source ./_common.sh\necho \"$ADMIN_DIR\"\n";
    std::fs::write(&consumer, used).unwrap();
    let (events, _event_receiver) = crossbeam::channel::unbounded();
    let (messages, _message_receiver) = crossbeam::channel::unbounded();
    let client = Client::new(events, messages);
    let mut session = Session::new(
        &Default::default(),
        PositionEncoding::UTF16,
        GlobalOptions::default().into_settings(client.clone()),
        &Workspaces::new(vec![Workspace::new(
            types::Url::from_file_path(root.path()).unwrap(),
        )]),
        &client,
    )
    .unwrap();
    let reports = |session: &Session| {
        let context = session.workspace_diagnostic_context(RequestCancellationToken::default());
        let result = workspace_diagnostics(
            context,
            &client,
            &types::WorkspaceDiagnosticParams {
                identifier: None,
                previous_result_ids: Vec::new(),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            },
        )
        .unwrap();
        let types::WorkspaceDiagnosticReportResult::Report(report) = result else {
            panic!("full report expected");
        };
        report
            .items
            .into_iter()
            .filter_map(|item| match item {
                types::WorkspaceDocumentDiagnosticReport::Full(report)
                    if report.uri.to_file_path().unwrap().file_name() == helper.file_name() =>
                {
                    Some(report.full_document_diagnostic_report.items)
                }
                _ => None,
            })
            .next()
            .unwrap()
    };
    assert!(reports(&session).is_empty());
    // Reuse a cached closed-file report before changing its consumer.
    assert!(reports(&session).is_empty());
    let uri = types::Url::from_file_path(&consumer).unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new("source ./_common.sh\n".into(), 1),
    );
    assert_eq!(reports(&session).len(), 1);
    let key = session.key_from_url(uri);
    session.close_document(&key).unwrap();
    assert!(reports(&session).is_empty());
}
