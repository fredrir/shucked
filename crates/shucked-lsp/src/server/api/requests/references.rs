use lsp_types::{self as types, request as req};
use shucked_semantic::{EditorCallHierarchyTarget, EditorSymbolTarget};

use crate::edit::PositionExt;
use crate::editor_features;
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crate::workspace_functions::{
    WorkspaceFunctionContext, canonical_path, workspace_function_index,
};
use crate::workspace_variables::variable_target;

pub(crate) struct References;

pub(crate) struct ReferencesSnapshot {
    document: Option<DocumentSnapshot>,
    workspace: WorkspaceFunctionContext,
}

impl super::RequestHandler for References {
    type RequestType = req::References;
}

impl super::super::traits::BackgroundRequestHandler for References {
    type Snapshot = ReferencesSnapshot;

    fn snapshot(
        session: &Session,
        params: &types::ReferenceParams,
        cancellation: RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        let uri = params.text_document_position.text_document.uri.clone();
        Ok(ReferencesSnapshot {
            document: session
                .take_snapshot(uri)
                .map(|snapshot| snapshot.with_analysis_cancellation(cancellation.clone())),
            workspace: session.workspace_function_context(cancellation),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        client: &Client,
        params: types::ReferenceParams,
    ) -> crate::server::Result<editor_features::ReferencesResponse> {
        let Some(document) = snapshot.document else {
            return Ok(None);
        };
        references(document, snapshot.workspace, client, params)
    }
}

fn references(
    snapshot: DocumentSnapshot,
    workspace: WorkspaceFunctionContext,
    client: &Client,
    params: types::ReferenceParams,
) -> crate::server::Result<editor_features::ReferencesResponse> {
    if workspace.cancellation.is_cancelled() {
        return Ok(None);
    }
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };
    let position = params.text_document_position.position;
    let offset = position.to_offset(
        analysis.source(),
        analysis.line_index(),
        snapshot.encoding(),
    );
    let Some(target) = analysis.semantic().editor_query().target_at_offset(offset) else {
        return Ok(None);
    };
    let Some(path) = snapshot
        .query()
        .file_url()
        .to_file_path()
        .ok()
        .map(|path| canonical_path(&path))
    else {
        return editor_features::references(snapshot, client, params);
    };
    let function_item = match &target {
        EditorSymbolTarget::Binding(_) | EditorSymbolTarget::Reference(_) => analysis
            .semantic()
            .editor_query()
            .prepare_call_hierarchy(offset)
            .filter(|item| matches!(item.target, EditorCallHierarchyTarget::Function(_))),
        EditorSymbolTarget::FunctionCall(_) | EditorSymbolTarget::RuntimeName(_) => None,
    };
    if function_item.is_none()
        && let Some(variable) = variable_target(analysis.semantic(), &target)
    {
        let Some(index) = workspace_function_index(&workspace) else {
            return editor_features::references(snapshot, client, params);
        };
        let Some(details) = index.variable_details(&path, &variable, &workspace.cancellation)
        else {
            return Ok(None);
        };
        if workspace.cancellation.is_cancelled() {
            return Ok(None);
        }
        if details.incomplete {
            client.show_message(
                format!(
                    "Workspace references are incomplete: {}.",
                    index
                        .incomplete_reason()
                        .unwrap_or_else(|| "some source effects could not be followed".into())
                ),
                types::MessageType::WARNING,
            )?;
        }
        let mut locations = details.references;
        if params.context.include_declaration {
            for declaration in details.definitions {
                if !locations.contains(&declaration) {
                    locations.push(declaration);
                }
            }
        }
        return Ok((!locations.is_empty()).then_some(locations));
    }
    let Some(index) = workspace_function_index(&workspace) else {
        return Ok(None);
    };
    let resolution = match target {
        EditorSymbolTarget::FunctionCall(call) => index.function_resolution(&path, call.name_span),
        _ => shucked_semantic::WorkspaceFunctionResolution {
            definitions: index.function_definitions(&path, offset),
            ..Default::default()
        },
    };
    if resolution.definitions.is_empty() {
        return editor_features::references(snapshot, client, params);
    }
    let (mut locations, incomplete) = index.function_references(&resolution.definitions);
    if incomplete || resolution.incomplete || resolution.may_be_absent {
        client.show_message(
            "Workspace function references include possible calls; resolution is incomplete.",
            types::MessageType::WARNING,
        )?;
    }
    if params.context.include_declaration {
        for declaration in index.function_locations(&resolution.definitions) {
            if !locations.contains(&declaration) {
                locations.push(declaration);
            }
        }
    }
    Ok((!locations.is_empty()).then_some(locations))
}
