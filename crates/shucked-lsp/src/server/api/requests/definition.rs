use lsp_types::{self as types, request as req};
use shucked_semantic::EditorSymbolTarget;

use crate::edit::PositionExt;
use crate::editor_features;
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crate::workspace_functions::{
    WorkspaceFunctionContext, canonical_path, workspace_function_index,
};
use crate::workspace_variables::variable_target;

pub(crate) struct Definition;

pub(crate) struct DefinitionSnapshot {
    document: Option<DocumentSnapshot>,
    workspace: WorkspaceFunctionContext,
}

impl super::RequestHandler for Definition {
    type RequestType = req::GotoDefinition;
}

impl super::super::traits::BackgroundRequestHandler for Definition {
    type Snapshot = DefinitionSnapshot;

    fn snapshot(
        session: &Session,
        params: &types::GotoDefinitionParams,
        cancellation: RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        Ok(DefinitionSnapshot {
            document: session.take_snapshot(uri),
            workspace: session.workspace_function_context(cancellation),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        client: &Client,
        params: types::GotoDefinitionParams,
    ) -> crate::server::Result<editor_features::DefinitionResponse> {
        let Some(document) = snapshot.document else {
            return Ok(None);
        };
        definition(document, snapshot.workspace, client, params)
    }
}

fn definition(
    snapshot: DocumentSnapshot,
    workspace: WorkspaceFunctionContext,
    client: &Client,
    params: types::GotoDefinitionParams,
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
    let Some(target) = analysis.semantic().editor_query().target_at_offset(offset) else {
        return editor_features::definition(snapshot, client, params);
    };
    let Some(path) = snapshot
        .query()
        .file_url()
        .to_file_path()
        .ok()
        .map(|path| canonical_path(&path))
    else {
        return editor_features::definition(snapshot, client, params);
    };
    let workspace_variable = variable_target(analysis.semantic(), &target);
    match target {
        EditorSymbolTarget::FunctionCall(call) => {
            let Some(index) = workspace_function_index(&workspace) else {
                return editor_features::definition(snapshot, client, params);
            };
            let resolution = index.function_resolution(&path, call.name_span);
            let mut locations = index.function_locations(&resolution.definitions);
            for (location, target) in locations.iter_mut().zip(&resolution.definitions) {
                if let Some(range) = index.range_of(&target.path, target.definition.def_span) {
                    location.range = range;
                }
            }
            if !locations.is_empty() {
                return Ok(Some(if locations.len() == 1 {
                    types::GotoDefinitionResponse::Scalar(locations[0].clone())
                } else {
                    types::GotoDefinitionResponse::Array(locations)
                }));
            }

            // A partial workspace index may omit an otherwise proven local binding.
            // Preserve the document-local answer only when no source operation could
            // have replaced it; without the active file's projected facts, any source
            // effect is conservatively treated as relevant.
            if call.binding.is_some()
                && !index.contains(&path)
                && analysis.semantic().source_refs().is_empty()
            {
                return editor_features::definition(snapshot, client, params);
            }
            Ok(None)
        }
        EditorSymbolTarget::Binding(_)
        | EditorSymbolTarget::Reference(_)
        | EditorSymbolTarget::RuntimeName(_) => {
            let Some(target) = workspace_variable else {
                return editor_features::definition(snapshot, client, params);
            };
            let Some(index) = workspace_function_index(&workspace) else {
                return editor_features::definition(snapshot, client, params);
            };
            let Some(locations) =
                index.variable_definition_locations(&path, &target, &workspace.cancellation)
            else {
                return Ok(None);
            };
            Ok(match locations.as_slice() {
                [] => return editor_features::definition(snapshot, client, params),
                [location] => Some(types::GotoDefinitionResponse::Scalar(location.clone())),
                _ => Some(types::GotoDefinitionResponse::Array(locations)),
            })
        }
    }
}
