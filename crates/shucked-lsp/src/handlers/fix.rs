use anyhow::{Context, anyhow};
use lsp_server::ErrorCode;
use lsp_types as types;
use serde::{Deserialize, Serialize};

use crate::lint::{
    AssociatedDiagnosticData, associated_diagnostic_data, collect_raw_diagnostics_for_snapshot,
    directive_edit_for_line, fix_all_document_edits, generate_diagnostics, to_lsp_text_edit,
};
use crate::session::{Client, DocumentSnapshot, Session};

pub(crate) fn code_actions(
    snapshot: DocumentSnapshot,
    _client: &Client,
    params: types::CodeActionParams,
) -> crate::server::Result<Option<types::CodeActionResponse>> {
    let mut actions = Vec::new();
    let only = params.context.only.as_ref();
    let include_quickfix = wants_kind(only, &types::CodeActionKind::QUICKFIX);
    let include_fix_all = wants_kind(only, &crate::SOURCE_FIX_ALL_SHUCKED)
        || wants_kind(only, &crate::SOURCE_FIX_ALL_SHUCKED);
    let include_refactor = only.is_none_or(|kinds| {
        kinds.iter().any(|kind| {
            let s = kind.as_str();
            s == "refactor"
                || s.starts_with("refactor.")
                || action_kind_matches(kind, &types::CodeActionKind::REFACTOR)
        })
    });

    let mut semantic_fixes = Vec::new();
    let mut fixable_rule_codes = Vec::new();
    let mut suppression_actions = Vec::new();

    if include_quickfix {
        for diagnostic in diagnostics_for_range(&snapshot, &params.range) {
            let Some(data) = associated_diagnostic_data(&snapshot, &diagnostic) else {
                continue;
            };

            if should_offer_fix(&snapshot, &data) {
                semantic_fixes.push(types::CodeActionOrCommand::CodeAction(
                    diagnostic_fix_action(&snapshot, &diagnostic, &data),
                ));
                if !fixable_rule_codes.contains(&data.code) {
                    fixable_rule_codes.push(data.code.clone());
                }
            }

            if let Some(edit) = data.directive_edit.clone() {
                suppression_actions.push(types::CodeActionOrCommand::CodeAction(
                    diagnostic_directive_action(&snapshot, &diagnostic, &data, edit),
                ));
            }

            let file_edit = file_suppression_edit(&snapshot, &data.code);
            suppression_actions.push(types::CodeActionOrCommand::CodeAction(
                diagnostic_file_directive_action(&snapshot, &diagnostic, &data.code, file_edit),
            ));
        }
    }

    let mut batch_fixes = Vec::new();
    if include_quickfix {
        for rule_code in fixable_rule_codes {
            let edits = batch_rule_edits(&snapshot, &rule_code);
            if !edits.is_empty() {
                batch_fixes.push(types::CodeActionOrCommand::CodeAction(batch_fix_action(
                    &snapshot, &rule_code, edits,
                )));
            }
        }
    }

    actions.extend(semantic_fixes);
    actions.extend(batch_fixes);

    if include_fix_all && snapshot.client_settings().fix_all() {
        let edits = fix_all_document_edits(
            &snapshot,
            if snapshot.client_settings().unsafe_fixes() {
                shucked_linter::Applicability::Unsafe
            } else {
                shucked_linter::Applicability::Safe
            },
        );
        if !edits.is_empty() {
            actions.push(types::CodeActionOrCommand::CodeAction(fix_all_action(
                &snapshot, edits,
            )?));
        }
    }

    actions.extend(suppression_actions);

    if include_refactor {
        let refactor_actions = crate::handlers::refactor::refactor_code_actions(&snapshot, &params);
        actions.extend(refactor_actions);
    }

    Ok((!actions.is_empty()).then_some(actions))
}

pub(crate) fn resolve_code_action(
    session: &Session,
    _client: &Client,
    mut action: types::CodeAction,
) -> crate::server::Result<types::CodeAction> {
    if action.edit.is_some() {
        return Ok(action);
    }

    let Some(data) = action.data.clone() else {
        return Ok(action);
    };
    let resolved: ResolveCodeActionData =
        serde_json::from_value(data).context("deserialize code action resolve payload")?;
    let Some(snapshot) = session.take_snapshot(resolved.uri.clone()) else {
        return Ok(action);
    };

    let edits = match resolved.kind {
        ResolveCodeActionKind::FixAll => fix_all_document_edits(
            &snapshot,
            if resolved.include_unsafe {
                shucked_linter::Applicability::Unsafe
            } else {
                shucked_linter::Applicability::Safe
            },
        ),
    };
    if !edits.is_empty() {
        action.edit = Some(workspace_edit_for_document(&snapshot, edits));
    }
    Ok(action)
}

pub(crate) fn execute_command(
    session: &mut Session,
    client: &Client,
    params: types::ExecuteCommandParams,
) -> crate::server::Result<Option<serde_json::Value>> {
    match params.command.as_str() {
        "shucked.applyAutofix" | "shuck.applyAutofix" => {
            let uri = command_uri(&params.arguments)?;
            let Some(snapshot) = session.take_snapshot(uri) else {
                return Ok(None);
            };
            let edits = fix_all_document_edits(
                &snapshot,
                if snapshot.client_settings().unsafe_fixes() {
                    shucked_linter::Applicability::Unsafe
                } else {
                    shucked_linter::Applicability::Safe
                },
            );
            apply_workspace_edit(session, client, "Shucked: apply autofix", &snapshot, edits)?;
            Ok(None)
        }
        "shucked.applyDirective" | "shuck.applyDirective" => {
            let args: ApplyDirectiveCommand = command_args(&params.arguments)?;
            let Some(snapshot) = session.take_snapshot(args.uri.clone()) else {
                return Ok(None);
            };
            let Some(edit) = directive_edit_for_line(&snapshot, args.line) else {
                return Ok(None);
            };
            apply_workspace_edit(
                session,
                client,
                "Shucked: disable for this line",
                &snapshot,
                vec![edit],
            )?;
            Ok(None)
        }
        "shucked.applyFormat" | "shuck.applyFormat" => {
            let uri = command_uri(&params.arguments)?;
            let Some(snapshot) = session.take_snapshot(uri) else {
                return Ok(None);
            };
            let edits = crate::format::format_document(
                snapshot.clone(),
                client,
                types::DocumentFormattingParams {
                    text_document: types::TextDocumentIdentifier {
                        uri: snapshot.query().file_url().clone(),
                    },
                    options: types::FormattingOptions {
                        tab_size: 8,
                        insert_spaces: false,
                        ..types::FormattingOptions::default()
                    },
                    work_done_progress_params: types::WorkDoneProgressParams::default(),
                },
            )?
            .unwrap_or_default();
            apply_workspace_edit(session, client, "Shucked: apply format", &snapshot, edits)?;
            Ok(None)
        }
        "shucked.printDebugInformation" | "shuck.printDebugInformation" => {
            tracing::info!(
                "shucked server state: open_documents={} workspace_roots={:?}",
                session.open_document_count(),
                session.workspace_roots()
            );
            Ok(None)
        }
        other => Err(crate::server::Error::new(
            anyhow!("unsupported executeCommand request: {other}"),
            ErrorCode::MethodNotFound,
        )),
    }
}

fn should_offer_fix(snapshot: &DocumentSnapshot, data: &AssociatedDiagnosticData) -> bool {
    !data.edits.is_empty()
        && shucked_linter::code_to_rule(&data.code)
            .is_some_and(|rule| snapshot.shuck_settings().fixable_rules().contains(rule))
        && (snapshot.client_settings().unsafe_fixes()
            || data.applicability == crate::lint::DiagnosticApplicability::Safe)
}

fn diagnostic_fix_action(
    snapshot: &DocumentSnapshot,
    diagnostic: &types::Diagnostic,
    data: &AssociatedDiagnosticData,
) -> types::CodeAction {
    types::CodeAction {
        title: format!("Shucked ({}): {}", data.code, data.title),
        kind: Some(types::CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(workspace_edit_for_document(snapshot, data.edits.clone())),
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    }
}

fn diagnostic_directive_action(
    snapshot: &DocumentSnapshot,
    diagnostic: &types::Diagnostic,
    data: &AssociatedDiagnosticData,
    edit: types::TextEdit,
) -> types::CodeAction {
    types::CodeAction {
        title: format!("Shucked ({}): Disable for this line", data.code),
        kind: Some(types::CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(workspace_edit_for_document(snapshot, vec![edit])),
        command: None,
        is_preferred: Some(false),
        disabled: None,
        data: None,
    }
}

fn diagnostic_file_directive_action(
    snapshot: &DocumentSnapshot,
    diagnostic: &types::Diagnostic,
    code: &str,
    edit: types::TextEdit,
) -> types::CodeAction {
    types::CodeAction {
        title: format!("Shucked ({code}): Disable for entire file"),
        kind: Some(types::CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(workspace_edit_for_document(snapshot, vec![edit])),
        command: None,
        is_preferred: Some(false),
        disabled: None,
        data: None,
    }
}

fn file_suppression_edit(snapshot: &DocumentSnapshot, code: &str) -> types::TextEdit {
    let source = snapshot.query().document().contents();
    let line_index = snapshot.query().document().index();
    let encoding = snapshot.encoding();

    let mut offset = 0;
    let mut lines = source.split_inclusive('\n');

    if let Some(first_line) = lines.next() {
        let trimmed = first_line.trim_start();
        if trimmed.starts_with("#!") || trimmed.starts_with('#') {
            offset += first_line.len();
            for line in lines {
                let trimmed = line.trim_start();
                if trimmed.starts_with('#') {
                    offset += line.len();
                } else {
                    break;
                }
            }
        }
    }

    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let prefix_nl = if offset > 0 && !source[..offset].ends_with('\n') {
        newline
    } else {
        ""
    };

    let range = crate::edit::to_lsp_range(
        shucked_ast::TextRange::new(
            shucked_ast::TextSize::new(offset as u32),
            shucked_ast::TextSize::new(offset as u32),
        ),
        source,
        line_index,
        encoding,
    );

    types::TextEdit {
        range,
        new_text: format!("{prefix_nl}# shuck: disable-file={code}{newline}"),
    }
}

fn batch_fix_action(
    snapshot: &DocumentSnapshot,
    rule_code: &str,
    edits: Vec<types::TextEdit>,
) -> types::CodeAction {
    types::CodeAction {
        title: format!("Shucked ({rule_code}): Fix all in this file"),
        kind: Some(types::CodeActionKind::QUICKFIX),
        diagnostics: None,
        edit: Some(workspace_edit_for_document(snapshot, edits)),
        command: None,
        is_preferred: Some(false),
        disabled: None,
        data: None,
    }
}

pub(crate) fn batch_rule_edits(
    snapshot: &DocumentSnapshot,
    rule_code: &str,
) -> Vec<types::TextEdit> {
    let Some(raw) = collect_raw_diagnostics_for_snapshot(snapshot) else {
        return Vec::new();
    };

    let source = snapshot.query().document().contents();
    let line_index = snapshot.query().document().index();
    let encoding = snapshot.encoding();
    let allow_unsafe = snapshot.client_settings().unsafe_fixes();

    let matching_diagnostics = raw.shell_diagnostics.iter().filter(|diagnostic| {
        diagnostic.code() == rule_code
            && snapshot
                .shuck_settings()
                .fixable_rules()
                .contains(diagnostic.rule)
    });

    let mut candidate_fixes: Vec<Vec<shucked_linter::Edit>> = Vec::new();
    for diagnostic in matching_diagnostics {
        let Some(fix) = diagnostic.fix.as_ref() else {
            continue;
        };
        if !allow_unsafe && fix.applicability() != shucked_linter::Applicability::Safe {
            continue;
        }
        let mut edits = fix.edits().to_vec();
        if edits.is_empty() {
            continue;
        }
        edits.sort_by(compare_shuck_edits);
        candidate_fixes.push(edits);
    }

    candidate_fixes.sort_by(|a, b| compare_candidate_fixes(a, b));

    let mut applied_edits: Vec<shucked_linter::Edit> = Vec::new();
    for candidate_edits in candidate_fixes {
        if has_internal_edit_conflicts(&candidate_edits) {
            continue;
        }
        if candidate_edits.iter().any(|edit| {
            applied_edits
                .iter()
                .any(|applied| shuck_edits_conflict(edit, applied))
        }) {
            continue;
        }
        applied_edits.extend(candidate_edits);
    }

    if applied_edits.is_empty() {
        return Vec::new();
    }

    applied_edits.sort_by(compare_shuck_edits);

    applied_edits
        .iter()
        .map(|edit| to_lsp_text_edit(edit, source, line_index, encoding))
        .collect()
}

fn compare_candidate_fixes(
    left: &[shucked_linter::Edit],
    right: &[shucked_linter::Edit],
) -> std::cmp::Ordering {
    for (left_edit, right_edit) in left.iter().zip(right) {
        let ordering = compare_shuck_edits(left_edit, right_edit);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn compare_shuck_edits(
    left: &shucked_linter::Edit,
    right: &shucked_linter::Edit,
) -> std::cmp::Ordering {
    let left_start = usize::from(left.range().start());
    let right_start = usize::from(right.range().start());
    let left_end = usize::from(left.range().end());
    let right_end = usize::from(right.range().end());

    left_start
        .cmp(&right_start)
        .then(left_end.cmp(&right_end))
        .then(left.content().cmp(right.content()))
}

fn has_internal_edit_conflicts(edits: &[shucked_linter::Edit]) -> bool {
    edits
        .windows(2)
        .any(|window| shuck_edits_conflict(&window[0], &window[1]))
}

fn shuck_edits_conflict(left: &shucked_linter::Edit, right: &shucked_linter::Edit) -> bool {
    let left_start = usize::from(left.range().start());
    let left_end = usize::from(left.range().end());
    let right_start = usize::from(right.range().start());
    let right_end = usize::from(right.range().end());

    let left_is_insertion = left.range().is_empty();
    let right_is_insertion = right.range().is_empty();

    if left_is_insertion && right_is_insertion {
        return left_start == right_start;
    }

    if left_is_insertion {
        return right_start <= left_start && left_start <= right_end;
    }

    if right_is_insertion {
        return left_start <= right_start && right_start <= left_end;
    }

    left_start < right_end && right_start < left_end
}

fn fix_all_action(
    snapshot: &DocumentSnapshot,
    edits: Vec<types::TextEdit>,
) -> crate::server::Result<types::CodeAction> {
    let mut action = types::CodeAction {
        title: "Shucked: Fix all auto-fixable issues".to_owned(),
        kind: Some(crate::SOURCE_FIX_ALL_SHUCKED),
        diagnostics: None,
        edit: None,
        command: None,
        is_preferred: Some(true),
        disabled: None,
        data: None,
    };
    if snapshot
        .resolved_client_capabilities()
        .code_action_deferred_edit_resolution
    {
        action.data = Some(
            serde_json::to_value(ResolveCodeActionData {
                kind: ResolveCodeActionKind::FixAll,
                uri: snapshot.query().file_url().clone(),
                include_unsafe: snapshot.client_settings().unsafe_fixes(),
            })
            .map_err(anyhow::Error::new)?,
        );
    } else {
        action.edit = Some(workspace_edit_for_document(snapshot, edits));
    }
    Ok(action)
}

pub(crate) fn workspace_edit_for_document(
    snapshot: &DocumentSnapshot,
    edits: Vec<types::TextEdit>,
) -> types::WorkspaceEdit {
    if snapshot.resolved_client_capabilities().document_changes {
        return types::WorkspaceEdit {
            changes: None,
            document_changes: Some(types::DocumentChanges::Edits(vec![
                types::TextDocumentEdit {
                    text_document: types::OptionalVersionedTextDocumentIdentifier {
                        uri: snapshot.query().file_url().clone(),
                        version: Some(snapshot.query().document().version()),
                    },
                    edits: edits.into_iter().map(types::OneOf::Left).collect(),
                },
            ])),
            change_annotations: None,
        };
    }

    let mut changes = std::collections::HashMap::new();
    changes.insert(snapshot.query().file_url().clone(), edits);
    types::WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

fn apply_workspace_edit(
    session: &Session,
    client: &Client,
    label: &str,
    snapshot: &DocumentSnapshot,
    edits: Vec<types::TextEdit>,
) -> crate::server::Result<()> {
    if edits.is_empty() {
        return Ok(());
    }
    if !snapshot.resolved_client_capabilities().apply_edit {
        return Err(crate::server::Error::new(
            anyhow!("LSP client does not advertise workspace/applyEdit support"),
            ErrorCode::InvalidRequest,
        ));
    }

    client.send_request::<types::request::ApplyWorkspaceEdit>(
        session,
        types::ApplyWorkspaceEditParams {
            label: Some(label.to_owned()),
            edit: workspace_edit_for_document(snapshot, edits),
        },
        |_, _, response| {
            if !response.applied {
                tracing::warn!(
                    "Client rejected workspace edit: {}",
                    response
                        .failure_reason
                        .unwrap_or_else(|| "unknown reason".to_owned())
                );
            }
        },
    )?;
    Ok(())
}

fn wants_kind(only: Option<&Vec<types::CodeActionKind>>, expected: &types::CodeActionKind) -> bool {
    only.is_none_or(|kinds| kinds.iter().any(|kind| action_kind_matches(kind, expected)))
}

fn diagnostics_for_range(
    snapshot: &DocumentSnapshot,
    requested_range: &types::Range,
) -> Vec<types::Diagnostic> {
    generate_diagnostics(snapshot)
        .into_iter()
        .filter(|diagnostic| ranges_overlap(&diagnostic.range, requested_range))
        .collect()
}

fn action_kind_matches(
    requested: &types::CodeActionKind,
    provided: &types::CodeActionKind,
) -> bool {
    provided.as_str() == requested.as_str()
        || provided
            .as_str()
            .strip_prefix(requested.as_str())
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn ranges_overlap(left: &types::Range, right: &types::Range) -> bool {
    if range_is_empty(left) {
        return range_contains(right, left.start);
    }
    if range_is_empty(right) {
        return range_contains(left, right.start);
    }

    position_lt(left.start, right.end) && position_lt(right.start, left.end)
}

fn range_contains(range: &types::Range, position: types::Position) -> bool {
    if range_is_empty(range) {
        return range.start == position;
    }

    position_leq(range.start, position) && position_lt(position, range.end)
}

fn range_is_empty(range: &types::Range) -> bool {
    range.start == range.end
}

fn position_leq(left: types::Position, right: types::Position) -> bool {
    (left.line, left.character) <= (right.line, right.character)
}

fn position_lt(left: types::Position, right: types::Position) -> bool {
    (left.line, left.character) < (right.line, right.character)
}

fn command_uri(arguments: &[serde_json::Value]) -> crate::server::Result<lsp_types::Url> {
    if let Some(uri) = arguments
        .first()
        .and_then(|value| value.as_str())
        .and_then(|value| lsp_types::Url::parse(value).ok())
    {
        return Ok(uri);
    }

    #[derive(Deserialize)]
    struct UriArg {
        uri: lsp_types::Url,
    }

    let arg = arguments
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("missing executeCommand argument"))?;
    Ok(serde_json::from_value::<UriArg>(arg)
        .map_err(anyhow::Error::new)?
        .uri)
}

fn command_args<T: for<'de> Deserialize<'de>>(
    arguments: &[serde_json::Value],
) -> crate::server::Result<T> {
    let value = arguments
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("missing executeCommand argument"))?;
    Ok(serde_json::from_value(value).map_err(anyhow::Error::new)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum ResolveCodeActionKind {
    FixAll,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolveCodeActionData {
    kind: ResolveCodeActionKind,
    uri: lsp_types::Url,
    include_unsafe: bool,
}

#[derive(Debug, Deserialize)]
struct ApplyDirectiveCommand {
    uri: lsp_types::Url,
    line: usize,
}

#[cfg(test)]
mod tests {
    use crossbeam::channel;
    use lsp_server::Message;
    use lsp_types::{
        ClientCapabilities, CodeActionContext, CodeActionParams, PartialResultParams, Position,
        Range, TextDocumentContentChangeEvent, TextDocumentIdentifier, Url, WorkDoneProgressParams,
    };

    use super::*;
    use crate::{
        ClientOptions, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace,
        Workspaces, lint::generate_diagnostics,
    };

    fn make_session(
        client_capabilities: ClientCapabilities,
        source: &str,
        language_id: &str,
        file_name: &str,
    ) -> (Session, Client, channel::Receiver<Message>, lsp_types::Url) {
        let (main_loop_sender, _main_loop_receiver) = channel::unbounded();
        let (client_sender, client_receiver) = channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let workspace_root = std::env::temp_dir().join("shuck-server-fix-tests");
        let workspace_uri =
            Url::from_file_path(&workspace_root).expect("workspace path should convert to a URL");
        let workspaces = Workspaces::new(vec![Workspace::default(workspace_uri)]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &client_capabilities,
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");
        session.update_client_options(ClientOptions {
            unsafe_fixes: Some(true),
            ..ClientOptions::default()
        });

        let path = workspace_root.join(file_name);
        let uri = Url::from_file_path(path).expect("test path should convert to a URL");
        session.open_text_document(
            uri.clone(),
            TextDocument::new(source.to_owned(), 1).with_language_id(language_id),
        );

        (session, client, client_receiver, uri)
    }

    fn extract_actions(response: types::CodeActionResponse) -> Vec<types::CodeAction> {
        response
            .into_iter()
            .map(|entry| match entry {
                types::CodeActionOrCommand::CodeAction(action) => action,
                types::CodeActionOrCommand::Command(command) => {
                    panic!("unexpected command response: {}", command.title)
                }
            })
            .collect()
    }

    fn first_edit_range(action: &types::CodeAction) -> Range {
        let edit = action
            .edit
            .as_ref()
            .expect("code action should include an edit");
        let document_changes = edit
            .document_changes
            .as_ref()
            .expect("workspace edit should use document changes");
        let types::DocumentChanges::Edits(edits) = document_changes else {
            panic!("workspace edit should contain document edits");
        };
        let text_edit = edits[0]
            .edits
            .first()
            .expect("workspace edit should contain at least one text edit");
        let types::OneOf::Left(text_edit) = text_edit else {
            panic!("workspace edit should contain plain text edits");
        };
        text_edit.range
    }

    fn deferred_capabilities() -> ClientCapabilities {
        serde_json::from_value(serde_json::json!({
            "textDocument": {
                "codeAction": {
                    "dataSupport": true,
                    "resolveSupport": { "properties": ["edit"] }
                }
            },
            "workspace": {
                "applyEdit": true,
                "workspaceEdit": {
                    "documentChanges": true
                }
            }
        }))
        .expect("test client capabilities should deserialize")
    }

    #[test]
    fn code_actions_include_quickfix_disable_and_fix_all() {
        let capabilities = deferred_capabilities();
        let (session, client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "script.sh");
        let snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");
        let diagnostics = generate_diagnostics(&snapshot);

        let response = code_actions(
            snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                context: CodeActionContext {
                    diagnostics,
                    only: None,
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("code action request should succeed")
        .expect("violating document should produce actions");

        let actions = extract_actions(response);

        assert!(
            actions
                .iter()
                .any(|action| action.title.contains("rename the unused assignment target"))
        );
        assert!(
            actions
                .iter()
                .any(|action| action.title.contains("Disable for this line"))
        );
        let fix_all = actions
            .iter()
            .find(|action| action.kind == Some(crate::SOURCE_FIX_ALL_SHUCKED))
            .expect("fix-all action should be present");
        assert!(fix_all.edit.is_none());
        assert!(fix_all.data.is_some());
    }

    #[test]
    fn adjacent_ranges_do_not_overlap() {
        let left = Range::new(Position::new(0, 0), Position::new(0, 3));
        let right = Range::new(Position::new(0, 3), Position::new(0, 5));

        assert!(!ranges_overlap(&left, &right));
    }

    #[test]
    fn empty_requested_range_overlaps_when_inside_diagnostic() {
        let diagnostic = Range::new(Position::new(0, 0), Position::new(0, 3));
        let cursor = Range::new(Position::new(0, 2), Position::new(0, 2));

        assert!(ranges_overlap(&diagnostic, &cursor));
    }

    #[test]
    fn empty_requested_range_at_diagnostic_end_does_not_overlap() {
        let diagnostic = Range::new(Position::new(0, 0), Position::new(0, 3));
        let cursor = Range::new(Position::new(0, 3), Position::new(0, 3));

        assert!(!ranges_overlap(&diagnostic, &cursor));
    }

    #[test]
    fn code_actions_return_none_for_non_shell_documents() {
        let capabilities = deferred_capabilities();
        let (session, client, _client_receiver, uri) =
            make_session(capabilities, "# heading\n", "markdown", "README.md");
        let snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");

        let response = code_actions(
            snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri },
                range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                context: CodeActionContext {
                    diagnostics: Vec::new(),
                    only: None,
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("code action request should succeed");

        assert!(response.is_none());
    }

    #[test]
    fn code_action_resolve_materializes_deferred_fix_all_edit() {
        let capabilities = deferred_capabilities();
        let (session, client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "script.sh");
        let snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");
        let diagnostics = generate_diagnostics(&snapshot);
        let response = code_actions(
            snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                context: CodeActionContext {
                    diagnostics,
                    only: Some(vec![crate::SOURCE_FIX_ALL_SHUCKED]),
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("fix-all action request should succeed")
        .expect("fix-all action should be present");
        let action = match response
            .into_iter()
            .next()
            .expect("fix-all action expected")
        {
            types::CodeActionOrCommand::CodeAction(action) => action,
            types::CodeActionOrCommand::Command(_) => panic!("expected a code action"),
        };
        let expected_snapshot = session
            .take_snapshot(uri)
            .expect("test document should produce a snapshot");
        let expected_edit = workspace_edit_for_document(
            &expected_snapshot,
            fix_all_document_edits(&expected_snapshot, shucked_linter::Applicability::Unsafe),
        );

        let resolved =
            resolve_code_action(&session, &client, action).expect("resolve request should succeed");
        let edit = resolved
            .edit
            .expect("resolved action should include an edit");
        assert_eq!(edit, expected_edit);
    }

    #[test]
    fn apply_autofix_command_dispatches_workspace_edit_request() {
        let capabilities = deferred_capabilities();
        let (mut session, client, client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "script.sh");

        execute_command(
            &mut session,
            &client,
            types::ExecuteCommandParams {
                command: "shucked.applyAutofix".to_owned(),
                arguments: vec![serde_json::Value::String(uri.to_string())],
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
        .expect("applyAutofix should succeed");

        let message = client_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("applyAutofix should send a workspace/applyEdit request");
        let Message::Request(request) = message else {
            panic!("expected a client request");
        };
        assert_eq!(request.method, "workspace/applyEdit");
    }

    #[test]
    fn code_actions_skip_rules_marked_unfixable_in_project_config() {
        let capabilities = deferred_capabilities();
        let workspace_root = tempfile::tempdir().expect("tempdir should be created");
        std::fs::write(
            workspace_root.path().join(".shuck.toml"),
            "[lint]\nunfixable = ['C001']\n",
        )
        .expect("config should be written");
        let script_path = workspace_root.path().join("script.sh");
        std::fs::write(&script_path, "foo=1\n").expect("source should be written");

        let (main_loop_sender, _main_loop_receiver) = channel::unbounded();
        let (client_sender, _client_receiver) = channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let workspace_uri = Url::from_file_path(workspace_root.path())
            .expect("workspace path should convert to a URL");
        let workspaces = Workspaces::new(vec![Workspace::default(workspace_uri)]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &capabilities,
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");
        session.update_client_options(ClientOptions {
            unsafe_fixes: Some(true),
            ..ClientOptions::default()
        });

        let uri = Url::from_file_path(&script_path).expect("test path should convert to a URL");
        session.open_text_document(
            uri.clone(),
            TextDocument::new("foo=1\n".to_owned(), 1).with_language_id("shellscript"),
        );
        let snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");
        let diagnostics = generate_diagnostics(&snapshot);

        let response = code_actions(
            snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri },
                range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                context: CodeActionContext {
                    diagnostics,
                    only: None,
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("code action request should succeed")
        .expect("violating document should still produce a disable action");

        let actions = extract_actions(response);

        assert!(
            actions
                .iter()
                .any(|action| action.title.contains("Disable for this line"))
        );
        assert!(
            !actions
                .iter()
                .any(|action| action.title.contains("rename the unused assignment target"))
        );
        assert!(
            !actions
                .iter()
                .any(|action| action.kind == Some(crate::SOURCE_FIX_ALL_SHUCKED))
        );
    }

    #[test]
    fn code_actions_recompute_live_quickfix_and_disable_edits() {
        let capabilities = deferred_capabilities();
        let (mut session, client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "script.sh");
        let stale_snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");
        let stale_diagnostics = generate_diagnostics(&stale_snapshot);

        let key = session.key_from_url(uri.clone());
        session
            .update_text_document(
                &key,
                vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: "\nfoo=1\n".to_owned(),
                }],
                2,
            )
            .expect("text document update should succeed");

        let live_snapshot = session
            .take_snapshot(uri.clone())
            .expect("updated document should produce a snapshot");
        let response = code_actions(
            live_snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri },
                range: Range::new(Position::new(1, 0), Position::new(1, 3)),
                context: CodeActionContext {
                    diagnostics: stale_diagnostics,
                    only: Some(vec![types::CodeActionKind::QUICKFIX]),
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("code action request should succeed")
        .expect("live diagnostic should still produce actions");

        let actions = extract_actions(response);
        let quickfix = actions
            .iter()
            .find(|action| action.title.contains("rename the unused assignment target"))
            .expect("quickfix action should be present");
        let disable = actions
            .iter()
            .find(|action| action.title.contains("Disable for this line"))
            .expect("disable action should be present");

        assert_eq!(first_edit_range(quickfix).start.line, 1);
        assert_eq!(first_edit_range(disable).start.line, 1);
        assert_eq!(
            quickfix
                .diagnostics
                .as_ref()
                .expect("quickfix should preserve associated diagnostic")[0]
                .range
                .start
                .line,
            1
        );
    }

    #[test]
    fn code_actions_match_parent_fix_all_kinds() {
        let capabilities = deferred_capabilities();
        let (session, client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "script.sh");

        for only in [
            types::CodeActionKind::SOURCE_FIX_ALL,
            types::CodeActionKind::SOURCE,
        ] {
            let snapshot = session
                .take_snapshot(uri.clone())
                .expect("test document should produce a snapshot");
            let response = code_actions(
                snapshot,
                &client,
                CodeActionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                    context: CodeActionContext {
                        diagnostics: Vec::new(),
                        only: Some(vec![only]),
                        trigger_kind: None,
                    },
                    work_done_progress_params: WorkDoneProgressParams::default(),
                    partial_result_params: PartialResultParams::default(),
                },
            )
            .expect("code action request should succeed")
            .expect("fix-all action should be returned for parent kind filters");

            let actions = extract_actions(response);
            assert!(
                actions
                    .iter()
                    .any(|action| action.kind == Some(crate::SOURCE_FIX_ALL_SHUCKED))
            );
        }
    }

    #[test]
    fn batch_rule_fix_offers_fix_for_all_occurrences_in_file() {
        let capabilities = ClientCapabilities::default();
        let (session, client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\nbar=2\n", "shellscript", "script.sh");
        let snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");

        let response = code_actions(
            snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                context: CodeActionContext {
                    diagnostics: Vec::new(),
                    only: Some(vec![types::CodeActionKind::QUICKFIX]),
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("code action request should succeed")
        .expect("code actions should be present");

        let actions = extract_actions(response);

        let semantic_fix = actions
            .iter()
            .find(|a| a.title.contains("rename the unused assignment target"))
            .expect("semantic fix should be offered");
        assert_eq!(semantic_fix.kind, Some(types::CodeActionKind::QUICKFIX));
        assert_eq!(semantic_fix.is_preferred, Some(true));

        let batch_fix = actions
            .iter()
            .find(|a| a.title == "Shucked (C001): Fix all in this file")
            .expect("batch fix should be offered");
        assert_eq!(batch_fix.kind, Some(types::CodeActionKind::QUICKFIX));
        assert_eq!(batch_fix.is_preferred, Some(false));

        let edit = batch_fix
            .edit
            .as_ref()
            .expect("batch fix must have an edit");
        let edit_count = if let Some(types::DocumentChanges::Edits(edits)) = &edit.document_changes
        {
            edits.first().map(|e| e.edits.len()).unwrap_or(0)
        } else if let Some(changes) = &edit.changes {
            changes.values().map(|v| v.len()).sum()
        } else {
            0
        };
        assert_eq!(edit_count, 2);
    }

    #[test]
    fn file_suppression_edit_inserts_after_shebang_and_comments() {
        let capabilities = ClientCapabilities::default();

        // 1. Shebang only
        let (session, _client, _client_receiver, uri) = make_session(
            capabilities.clone(),
            "#!/bin/bash\nfoo=1\n",
            "shellscript",
            "s1.sh",
        );
        let snapshot = session.take_snapshot(uri).unwrap();
        let edit = file_suppression_edit(&snapshot, "C001");
        assert_eq!(edit.range.start.line, 1);
        assert_eq!(edit.range.start.character, 0);
        assert_eq!(edit.new_text, "# shuck: disable-file=C001\n");

        // 2. Shebang and initial comments
        let (session, _client, _client_receiver, uri) = make_session(
            capabilities.clone(),
            "#!/bin/bash\n# comment 1\n# comment 2\nfoo=1\n",
            "shellscript",
            "s2.sh",
        );
        let snapshot = session.take_snapshot(uri).unwrap();
        let edit = file_suppression_edit(&snapshot, "C001");
        assert_eq!(edit.range.start.line, 3);
        assert_eq!(edit.range.start.character, 0);
        assert_eq!(edit.new_text, "# shuck: disable-file=C001\n");

        // 3. Comments without shebang
        let (session, _client, _client_receiver, uri) = make_session(
            capabilities.clone(),
            "# header\nfoo=1\n",
            "shellscript",
            "s3.sh",
        );
        let snapshot = session.take_snapshot(uri).unwrap();
        let edit = file_suppression_edit(&snapshot, "C001");
        assert_eq!(edit.range.start.line, 1);
        assert_eq!(edit.range.start.character, 0);
        assert_eq!(edit.new_text, "# shuck: disable-file=C001\n");

        // 4. No shebang, no comments
        let (session, _client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "s4.sh");
        let snapshot = session.take_snapshot(uri).unwrap();
        let edit = file_suppression_edit(&snapshot, "C001");
        assert_eq!(edit.range.start.line, 0);
        assert_eq!(edit.range.start.character, 0);
        assert_eq!(edit.new_text, "# shuck: disable-file=C001\n");
    }

    #[test]
    fn code_actions_order_semantic_batch_then_suppressions_at_bottom() {
        let capabilities = ClientCapabilities::default();
        let (session, client, _client_receiver, uri) =
            make_session(capabilities, "foo=1\n", "shellscript", "script.sh");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let response = code_actions(
            snapshot,
            &client,
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range::new(Position::new(0, 0), Position::new(0, 3)),
                context: CodeActionContext {
                    diagnostics: Vec::new(),
                    only: None,
                    trigger_kind: None,
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
        )
        .expect("code action request should succeed")
        .expect("code actions should be present");

        let actions = extract_actions(response);
        let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();

        // Semantic fix is first
        assert!(titles[0].starts_with("Shucked (C001): rename"));
        // Batch fix is next
        assert_eq!(titles[1], "Shucked (C001): Fix all in this file");
        // Fix all is next
        assert_eq!(titles[2], "Shucked: Fix all auto-fixable issues");
        // Suppressions are at the bottom
        assert_eq!(titles[3], "Shucked (C001): Disable for this line");
        assert_eq!(titles[4], "Shucked (C001): Disable for entire file");
    }
}
