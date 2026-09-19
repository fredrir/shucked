use lsp_types::{self as types, request as req};
use shucked_semantic::EditorSymbolTarget;

use crate::edit::PositionExt;
use crate::resolve;
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crate::workspace_functions::{
    WorkspaceFunctionContext, canonical_path, workspace_function_index,
};

pub(crate) struct Hover;

pub(crate) struct HoverSnapshot {
    document: Option<DocumentSnapshot>,
    workspace: WorkspaceFunctionContext,
}

impl super::RequestHandler for Hover {
    type RequestType = req::HoverRequest;
}

impl super::super::traits::BackgroundRequestHandler for Hover {
    type Snapshot = HoverSnapshot;

    fn snapshot(
        session: &Session,
        params: &types::HoverParams,
        cancellation: RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        Ok(HoverSnapshot {
            document: session
                .take_snapshot(uri)
                .map(|snapshot| snapshot.with_analysis_cancellation(cancellation.clone())),
            workspace: session.workspace_function_context(cancellation),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        client: &Client,
        params: types::HoverParams,
    ) -> crate::server::Result<Option<types::Hover>> {
        let Some(document) = snapshot.document else {
            return Ok(None);
        };
        let offset = params.text_document_position_params.position.to_offset(
            document.query().document().contents(),
            document.query().document().index(),
            document.encoding(),
        );
        let environment_hover = crate::handlers::commands::hover(&document, offset);
        let existing = hover(document, snapshot.workspace, client, params)?;
        Ok(match (existing, environment_hover) {
            (Some(mut existing), Some(environment)) => {
                if let types::HoverContents::Markup(details) = environment.contents
                    && let types::HoverContents::Markup(content) = &mut existing.contents
                {
                    if content.kind == types::MarkupKind::Markdown {
                        content.value.push_str(&format!(
                            "\n\n```text\n{}\n```",
                            details.value.replace("```", "` ` `")
                        ));
                    } else {
                        content.value.push_str(&format!("\n\n{}", details.value));
                    }
                }
                Some(existing)
            }
            (Some(existing), None) => Some(existing),
            (None, environment) => environment,
        })
    }
}

fn hover(
    snapshot: DocumentSnapshot,
    workspace: WorkspaceFunctionContext,
    client: &Client,
    params: types::HoverParams,
) -> crate::server::Result<Option<types::Hover>> {
    if crate::handlers::commands::dialect(&snapshot) == "fish" {
        return Ok(None);
    }
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };
    let position = params.text_document_position_params.position;
    let offset = position.to_offset(
        analysis.source(),
        analysis.line_index(),
        snapshot.encoding(),
    );
    let Some(EditorSymbolTarget::FunctionCall(call)) =
        analysis.semantic().editor_query().target_at_offset(offset)
    else {
        return resolve::hover(snapshot, client, params);
    };

    // Without a source operation, the document-local semantic answer is both
    // exact and cheaper than materializing the workspace function index.
    if analysis.semantic().source_refs().is_empty() {
        return resolve::hover(snapshot, client, params);
    }
    let Some(path) = snapshot
        .query()
        .file_url()
        .to_file_path()
        .ok()
        .map(|path| canonical_path(&path))
    else {
        return resolve::hover(snapshot, client, params);
    };
    let Some(index) = workspace_function_index(&workspace) else {
        return Ok(None);
    };
    let Some(target) =
        index.resolve_call_site_exact(&path, call.name_span, &workspace.cancellation)
    else {
        return Ok(None);
    };
    if target.path == path {
        return resolve::hover(snapshot, client, params);
    }
    let Some(definition_span) = target.selection_span.or(target.def_span) else {
        return Ok(None);
    };
    let Some(file) = index.file(&target.path) else {
        return Ok(None);
    };
    Ok(Some(resolve::render_sourced_function_hover(
        &snapshot,
        analysis.source(),
        analysis.line_index(),
        resolve::SourcedFunctionHover {
            name: call.name.as_str(),
            target_span: call.name_span,
            definition_uri: file.editor_uri(),
            definition_span,
        },
    )))
}
