use lsp_types::{self as types, request as req};

use crate::edit::PositionExt;
use crate::editor_features;
use crate::handlers::navigation;
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crate::workspace_functions::{
    WorkspaceFunctionContext, canonical_path, workspace_function_index,
};

/// `textDocument/declaration`.
///
/// Declarations are the `local`, `export`, `typeset`, `declare` and
/// `readonly` sites of a variable and the definitions of functions; a variable
/// that is only assigned falls back to its definitions, and a `source` operand
/// leads to the loaded file.
pub(crate) struct Declaration;

pub(crate) struct DeclarationSnapshot {
    document: Option<DocumentSnapshot>,
    workspace: WorkspaceFunctionContext,
}

impl super::RequestHandler for Declaration {
    type RequestType = req::GotoDeclaration;
}

impl super::super::traits::BackgroundRequestHandler for Declaration {
    type Snapshot = DeclarationSnapshot;

    fn snapshot(
        session: &Session,
        params: &req::GotoDeclarationParams,
        cancellation: RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        Ok(DeclarationSnapshot {
            document: session.take_snapshot(uri),
            workspace: session.workspace_function_context(cancellation),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        client: &Client,
        params: req::GotoDeclarationParams,
    ) -> crate::server::Result<editor_features::DefinitionResponse> {
        let Some(document) = snapshot.document else {
            return Ok(None);
        };
        declaration(document, snapshot.workspace, client, params)
    }
}

fn declaration(
    snapshot: DocumentSnapshot,
    workspace: WorkspaceFunctionContext,
    client: &Client,
    params: req::GotoDeclarationParams,
) -> crate::server::Result<editor_features::DefinitionResponse> {
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    let offset = position.to_offset(
        analysis.source(),
        analysis.line_index(),
        snapshot.encoding(),
    );
    if let Some(path) = snapshot
        .query()
        .file_url()
        .to_file_path()
        .ok()
        .map(|path| canonical_path(&path))
        && let Some(index) = workspace_function_index(&workspace)
        && let Some(locations) = navigation::source_operand_locations(&index, &path, offset)
    {
        return Ok(navigation::response(locations));
    }
    let spans = analysis
        .semantic()
        .editor_query()
        .declaration_spans_at_offset(offset);
    if spans.is_empty() {
        return super::definition::definition(snapshot, workspace, client, params);
    }
    let locations = spans
        .into_iter()
        .map(|span| types::Location {
            uri: snapshot.query().file_url().clone(),
            range: crate::edit::to_lsp_range(
                span.to_range(),
                analysis.source(),
                analysis.line_index(),
                snapshot.encoding(),
            ),
        })
        .collect::<Vec<_>>();
    Ok(navigation::response(locations))
}
