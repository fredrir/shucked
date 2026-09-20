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
    let target = analysis.semantic().editor_query().target_at_offset(offset);
    let variable = target.as_ref().and_then(|target| {
        crate::workspace_variables::variable_target(analysis.semantic(), target)
    });
    let source_hover = analysis.semantic().source_refs().iter().any(|source| {
        let contains =
            |span: shucked_ast::Span| span.start.offset() <= offset && offset < span.end.offset();
        contains(source.span) || source.directive_path_span.is_some_and(contains)
    });
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
    if source_hover && let Some(details) = index.source_details(&path, offset) {
        let span = details
            .directive_span
            .filter(|span| span.start.offset() <= offset && offset < span.end.offset())
            .unwrap_or_else(|| {
                if details.path_span.start.offset() <= offset
                    && offset < details.path_span.end.offset()
                {
                    details.path_span
                } else {
                    details.span
                }
            });
        return Ok(Some(types::Hover {
            contents: types::HoverContents::Markup(types::MarkupContent {
                kind: types::MarkupKind::Markdown,
                value: crate::handlers::workspace_explain::source(details, &index),
            }),
            range: Some(crate::edit::to_lsp_range(
                span.to_range(),
                analysis.source(),
                analysis.line_index(),
                snapshot.encoding(),
            )),
        }));
    }
    if source_hover && let Some(reason) = index.incomplete_reason() {
        return Ok(Some(types::Hover {
            contents: types::HoverContents::Markup(types::MarkupContent {
                kind: types::MarkupKind::Markdown,
                value: format!("**Incomplete workspace discovery:** {reason}."),
            }),
            range: None,
        }));
    }
    if let Some(variable) = variable {
        let details = index.variable_details(&path, &variable, &workspace.cancellation);
        let mut existing = resolve::hover(snapshot, client, params)?;
        if let Some(details) = details {
            let value = crate::handlers::workspace_explain::variable(&details, &index);
            if let Some(types::Hover {
                contents: types::HoverContents::Markup(content),
                ..
            }) = &mut existing
            {
                content.value.push_str(&format!("\n\n---\n\n{value}"));
            } else {
                existing = Some(types::Hover {
                    contents: types::HoverContents::Markup(types::MarkupContent {
                        kind: types::MarkupKind::Markdown,
                        value,
                    }),
                    range: None,
                });
            }
        }
        return Ok(existing);
    }
    let (resolution, span) = match target {
        Some(EditorSymbolTarget::FunctionCall(call)) => (
            index.function_resolution(&path, call.name_span),
            call.name_span,
        ),
        _ => {
            let definitions = index.function_definitions(&path, offset);
            let Some(definition) = definitions.first() else {
                return resolve::hover(snapshot, client, params);
            };
            let span = definition.definition.selection_span;
            (
                shucked_semantic::WorkspaceFunctionResolution {
                    definitions,
                    ..Default::default()
                },
                span,
            )
        }
    };
    if resolution.definitions.is_empty() {
        return resolve::hover(snapshot, client, params);
    }
    let text = crate::handlers::workspace_explain::function(&resolution, &index);
    Ok(Some(types::Hover {
        contents: types::HoverContents::Markup(types::MarkupContent {
            kind: types::MarkupKind::Markdown,
            value: text,
        }),
        range: index.range_of(&path, span),
    }))
}
