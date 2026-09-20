use lsp_types::{self as types, request as req};

use crate::editor_features;
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crate::workspace_functions::{
    WorkspaceFunctionContext, canonical_path, workspace_function_index,
};

pub(crate) struct Completion;

pub(crate) struct CompletionSnapshot {
    document: Option<DocumentSnapshot>,
    workspace: WorkspaceFunctionContext,
    environment: std::sync::Arc<crate::handlers::completion::environment::Environment>,
    cancellation: RequestCancellationToken,
}

impl super::RequestHandler for Completion {
    type RequestType = req::Completion;
}

impl super::super::traits::BackgroundRequestHandler for Completion {
    type Snapshot = CompletionSnapshot;

    fn snapshot(
        session: &Session,
        params: &types::CompletionParams,
        cancellation: RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        let uri = params.text_document_position.text_document.uri.clone();
        Ok(CompletionSnapshot {
            document: session
                .take_snapshot(uri)
                .map(|snapshot| snapshot.with_analysis_cancellation(cancellation.clone())),
            environment: session.completion_environment.clone(),
            cancellation: cancellation.clone(),
            workspace: session.workspace_function_context(cancellation),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        client: &Client,
        params: types::CompletionParams,
    ) -> crate::server::Result<editor_features::CompletionResponse> {
        let Some(document) = snapshot.document else {
            return Ok(None);
        };
        let path = document
            .query()
            .file_url()
            .to_file_path()
            .ok()
            .map(|path| canonical_path(&path));
        editor_features::completion_with_environment(
            document,
            client,
            params,
            Some((&snapshot.environment, &snapshot.cancellation)),
            move |_analysis, offset| {
                let Some(path) = path.as_deref() else {
                    return Vec::new();
                };
                let Some(index) = workspace_function_index(&snapshot.workspace) else {
                    return Vec::new();
                };
                index.function_completions(path, offset)
            },
        )
    }
}
