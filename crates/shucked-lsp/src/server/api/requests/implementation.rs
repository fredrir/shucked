use lsp_types::{self as types, request as req};
use shucked_semantic::{BindingKind, EditorSymbolTarget};

use crate::edit::PositionExt;
use crate::editor_features;
use crate::handlers::navigation;
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crate::workspace_functions::{
    WorkspaceFunctionContext, canonical_path, workspace_function_index,
};
use crate::workspace_variables::variable_target;

/// `textDocument/implementation`.
///
/// Shell has no interfaces, so "implementation" answers the question "which
/// code runs for this name?": every candidate function body (including
/// conditional redefinitions), every file loaded by a `source` operand, the
/// script behind an external command, or every assignment that may produce a
/// variable's value.
pub(crate) struct Implementation;

pub(crate) struct ImplementationSnapshot {
    document: Option<DocumentSnapshot>,
    workspace: WorkspaceFunctionContext,
}

impl super::RequestHandler for Implementation {
    type RequestType = req::GotoImplementation;
}

impl super::super::traits::BackgroundRequestHandler for Implementation {
    type Snapshot = ImplementationSnapshot;

    fn snapshot(
        session: &Session,
        params: &req::GotoImplementationParams,
        cancellation: RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        Ok(ImplementationSnapshot {
            document: session.take_snapshot(uri),
            workspace: session.workspace_function_context(cancellation),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        client: &Client,
        params: req::GotoImplementationParams,
    ) -> crate::server::Result<editor_features::DefinitionResponse> {
        let Some(document) = snapshot.document else {
            return Ok(None);
        };
        implementation(document, snapshot.workspace, client, params)
    }
}

fn implementation(
    snapshot: DocumentSnapshot,
    workspace: WorkspaceFunctionContext,
    client: &Client,
    params: req::GotoImplementationParams,
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
    let path = snapshot
        .query()
        .file_url()
        .to_file_path()
        .ok()
        .map(|path| canonical_path(&path));
    let index = path
        .as_ref()
        .and_then(|_| workspace_function_index(&workspace));
    if let (Some(index), Some(path)) = (&index, &path) {
        if let Some(locations) = navigation::source_operand_locations(index, path, offset) {
            return Ok(navigation::response(locations));
        }
        if let Some(locations) = navigation::widget_locations(index, path, offset) {
            return Ok(navigation::response(locations));
        }
    }
    let target = analysis.semantic().editor_query().target_at_offset(offset);
    let Some(target) = target else {
        return Ok(navigation::command_script_location(&snapshot, offset)
            .map(types::GotoDefinitionResponse::Scalar));
    };
    let local = || editor_features::definition(snapshot.clone(), client, params.clone());
    let (Some(index), Some(path)) = (index, path) else {
        return local();
    };
    match &target {
        EditorSymbolTarget::FunctionCall(call) => {
            // Every body that may run: the bindings proven at this call site
            // plus any redefinition elsewhere in the workspace, since a shell
            // function can be replaced by whichever file is sourced last.
            let mut definitions = index.function_resolution(&path, call.name_span).definitions;
            for definition in index.function_definitions_named(call.name.as_str()) {
                if !definitions.contains(&definition) {
                    definitions.push(definition);
                }
            }
            definitions.sort_by(|a, b| {
                (&a.path, a.definition.def_span.start.offset())
                    .cmp(&(&b.path, b.definition.def_span.start.offset()))
            });
            let locations = navigation::function_definition_locations(&index, &definitions, true);
            if locations.is_empty() {
                if let Some(location) = navigation::command_script_location(&snapshot, offset) {
                    return Ok(Some(types::GotoDefinitionResponse::Scalar(location)));
                }
                return local();
            }
            Ok(navigation::response(locations))
        }
        EditorSymbolTarget::Binding(binding_id)
            if matches!(
                analysis.semantic().binding(*binding_id).kind,
                BindingKind::FunctionDefinition
            ) =>
        {
            let name = analysis.semantic().binding(*binding_id).name.clone();
            let locations = navigation::function_definition_locations(
                &index,
                &index.function_definitions_named(name.as_str()),
                true,
            );
            if locations.is_empty() {
                return local();
            }
            Ok(navigation::response(locations))
        }
        EditorSymbolTarget::Binding(_)
        | EditorSymbolTarget::Reference(_)
        | EditorSymbolTarget::RuntimeName(_) => {
            let Some(variable) = variable_target(analysis.semantic(), &target) else {
                return local();
            };
            match index.variable_definition_locations(&path, &variable, &workspace.cancellation) {
                Some(locations) if !locations.is_empty() => Ok(navigation::response(locations)),
                _ => local(),
            }
        }
    }
}
