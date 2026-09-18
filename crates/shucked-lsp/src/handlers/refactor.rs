//! Shell refactoring engine implementation for Shucked LSP.

use lsp_types as types;
use shucked_ast::{
    Assignment, AssignmentValue, Command, CompoundCommand, SimpleCommand, Span, Stmt, StmtSeq,
    TextRange, TextSize, Word, WordPart,
};
use shucked_semantic::{AssignmentValueOrigin, BindingOrigin, EditorOccurrenceKind, ScopeKind};

use crate::edit::RangeExt;
use crate::handlers::analysis::DocumentAnalysis;
use crate::handlers::fix::workspace_edit_for_document;
use crate::session::DocumentSnapshot;

/// Generates refactoring code actions for the given document and parameters.
pub(crate) fn refactor_code_actions(
    snapshot: &DocumentSnapshot,
    params: &types::CodeActionParams,
) -> Vec<types::CodeActionOrCommand> {
    let mut actions = Vec::new();
    let Some(analysis) = snapshot.analysis() else {
        return actions;
    };

    if let Some(action) = extract_variable_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = inline_variable_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = quote_variable_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = toggle_quotes_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = modernize_test_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = invert_if_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = extract_function_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(action) = add_strict_mode_action(snapshot, &analysis, params) {
        actions.push(types::CodeActionOrCommand::CodeAction(action));
    }

    if let Some(only) = params.context.only.as_ref() {
        actions.retain(|action| {
            if let types::CodeActionOrCommand::CodeAction(ca) = action {
                if let Some(kind) = &ca.kind {
                    only.iter().any(|req| action_kind_matches(req, kind))
                } else {
                    true
                }
            } else {
                true
            }
        });
    }

    actions
}

fn action_kind_matches(
    requested: &types::CodeActionKind,
    provided: &types::CodeActionKind,
) -> bool {
    let requested = requested.as_str();
    let provided = provided.as_str();
    provided == requested
        || (provided
            .strip_prefix(requested)
            .is_some_and(|suffix| suffix.starts_with('.')))
}

// -----------------------------------------------------------------------------
// 1. refactor.extract.variable
// -----------------------------------------------------------------------------

fn extract_variable_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    if params.range.start == params.range.end {
        return None;
    }

    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let sel_start = usize::from(text_range.start());
    let sel_end = usize::from(text_range.end());
    if sel_start >= sel_end || sel_end > source.len() {
        return None;
    }

    let selected_text = &source[sel_start..sel_end];
    if selected_text.trim().is_empty() {
        return None;
    }

    let innermost_stmt =
        find_innermost_enclosing_stmt(&analysis.parse_result().file.body, sel_start, sel_end)?;

    // If selection covers the entire statement or more, leave that to extract function
    if sel_start <= innermost_stmt.span.start.offset()
        && sel_end >= innermost_stmt.span.end.offset()
    {
        return None;
    }

    let is_in_function = {
        let semantic = analysis.semantic();
        let mut curr = Some(semantic.scope_at(sel_start));
        let mut in_func = false;
        while let Some(sid) = curr {
            let s = semantic.scope(sid);
            if matches!(s.kind, ScopeKind::Function(_)) {
                in_func = true;
                break;
            }
            curr = s.parent;
        }
        in_func
    };

    let stmt_start_offset = innermost_stmt.span.start.offset();
    let stmt_line = line_index.line_number(TextSize::new(stmt_start_offset as u32));
    let line_start = usize::from(line_index.line_start(stmt_line).unwrap_or(TextSize::new(0)));
    let indent = source[line_start..stmt_start_offset]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>();

    let decl_prefix = if is_in_function { "local " } else { "" };
    let var_name = "extracted_var";
    let insert_text = format!("{indent}{decl_prefix}{var_name}={selected_text}\n");

    let insert_pos = types::Position::new((stmt_line.saturating_sub(1)) as u32, 0);
    let insert_edit = types::TextEdit {
        range: types::Range::new(insert_pos, insert_pos),
        new_text: insert_text,
    };
    let replace_edit = types::TextEdit {
        range: params.range,
        new_text: format!("\"${var_name}\""),
    };

    Some(types::CodeAction {
        title: "Extract to variable".to_string(),
        kind: Some(types::CodeActionKind::new("refactor.extract.variable")),
        edit: Some(workspace_edit_for_document(
            snapshot,
            vec![insert_edit, replace_edit],
        )),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// 2. refactor.inline.variable
// -----------------------------------------------------------------------------

fn inline_variable_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let cursor_offset = usize::from(text_range.start());

    let semantic = analysis.semantic();

    // Check if cursor is on a reference or assignment
    let binding_id = if let Some(ref_node) = semantic
        .references()
        .iter()
        .find(|r| span_contains_offset(r.span, cursor_offset))
    {
        semantic.resolved_binding(ref_node.id).map(|b| b.id)
    } else {
        semantic
            .bindings()
            .iter()
            .find(|b| span_contains_offset(b.span, cursor_offset))
            .map(|b| b.id)
    }?;

    let binding = semantic.binding(binding_id);
    let var_name = binding.name.as_str();

    // Must have static literal assignment origin
    if !matches!(
        binding.origin,
        BindingOrigin::Assignment {
            value: AssignmentValueOrigin::StaticLiteral,
            ..
        }
    ) {
        return None;
    }

    // Verify variable is not mutated (only single binding in scope, no write occurrences)
    let bindings_in_scope = semantic
        .bindings()
        .iter()
        .filter(|b| b.name == binding.name && b.scope == binding.scope)
        .count();
    if bindings_in_scope > 1 {
        return None;
    }

    let has_mutations = semantic
        .editor_query()
        .occurrences_for_target(
            &shucked_semantic::EditorSymbolTarget::Binding(binding.id),
            false,
        )
        .iter()
        .any(|occ| occ.kind == EditorOccurrenceKind::Write && occ.span != binding.span);
    if has_mutations {
        return None;
    }

    // Find the assignment in AST to get RHS text and the assignment statement
    let (assignment_ast, stmt_ast) =
        find_assignment_and_stmt(&analysis.parse_result().file.body, binding.span)?;
    let AssignmentValue::Scalar(rhs_word) = &assignment_ast.value else {
        return None;
    };
    let rhs_text = rhs_word.span.slice(source);

    // Build replacement edits for each reference
    let mut edits = Vec::new();
    for ref_id in &binding.references {
        let reference = semantic.reference(*ref_id);
        let ref_span = reference.span;
        let ref_start = ref_span.start.offset();
        let ref_end = ref_span.end.offset();

        // Check if reference is inside quotes e.g. "$var"
        let is_double_quoted = ref_start > 0
            && source.as_bytes().get(ref_start - 1) == Some(&b'"')
            && source.as_bytes().get(ref_end) == Some(&b'"');

        let (target_range, replacement) = if is_double_quoted {
            let quote_range = TextRange::new(
                TextSize::new((ref_start - 1) as u32),
                TextSize::new((ref_end + 1) as u32),
            );
            let lsp_range = crate::edit::to_lsp_range(quote_range, source, line_index, encoding);
            (lsp_range, rhs_text.to_string())
        } else {
            let lsp_range =
                crate::edit::to_lsp_range(ref_span.to_range(), source, line_index, encoding);
            (lsp_range, rhs_text.to_string())
        };

        edits.push(types::TextEdit {
            range: target_range,
            new_text: replacement,
        });
    }

    // Delete the assignment line
    let stmt_line = line_index.line_number(TextSize::new(stmt_ast.span.start.offset() as u32));
    let line_start = usize::from(line_index.line_start(stmt_line).unwrap_or(TextSize::new(0)));
    let next_line_start = line_index
        .line_start(stmt_line + 1)
        .map(usize::from)
        .unwrap_or(source.len());

    let delete_range = crate::edit::to_lsp_range(
        TextRange::new(
            TextSize::new(line_start as u32),
            TextSize::new(next_line_start as u32),
        ),
        source,
        line_index,
        encoding,
    );

    edits.push(types::TextEdit {
        range: delete_range,
        new_text: String::new(),
    });

    Some(types::CodeAction {
        title: format!("Inline variable '{var_name}'"),
        kind: Some(types::CodeActionKind::new("refactor.inline.variable")),
        edit: Some(workspace_edit_for_document(snapshot, edits)),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// 3. refactor.rewrite.quote
// -----------------------------------------------------------------------------

fn quote_variable_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let cursor_offset = usize::from(text_range.start());

    let semantic = analysis.semantic();
    let reference = semantic
        .references()
        .iter()
        .find(|r| span_contains_offset(r.span, cursor_offset))?;

    if !matches!(
        reference.kind,
        shucked_semantic::ReferenceKind::Expansion
            | shucked_semantic::ReferenceKind::ParameterExpansion
    ) {
        return None;
    }

    let start = reference.span.start.offset();
    let end = reference.span.end.offset();

    // Check if already enclosed in double quotes
    if start > 0
        && source.as_bytes().get(start - 1) == Some(&b'"')
        && source.as_bytes().get(end) == Some(&b'"')
    {
        return None;
    }

    // Check if inside double quotes in AST
    if is_offset_inside_double_quotes(&analysis.parse_result().file.body, start) {
        return None;
    }

    let var_ref = reference.span.slice(source);
    let lsp_range =
        crate::edit::to_lsp_range(reference.span.to_range(), source, line_index, encoding);

    let edit = types::TextEdit {
        range: lsp_range,
        new_text: format!("\"{var_ref}\""),
    };

    Some(types::CodeAction {
        title: format!("Quote variable reference \"{var_ref}\""),
        kind: Some(types::CodeActionKind::new("refactor.rewrite.quote")),
        edit: Some(workspace_edit_for_document(snapshot, vec![edit])),
        ..Default::default()
    })
}

fn toggle_quotes_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let cursor_offset = usize::from(text_range.start());

    let (quote_span, is_single) =
        find_quoted_string_at_offset(&analysis.parse_result().file.body, cursor_offset)?;

    let raw_text = quote_span.slice(source);
    if raw_text.len() < 2 {
        return None;
    }

    let inner = &raw_text[1..raw_text.len() - 1];
    let new_text = if is_single {
        // Single to double: escape double quotes
        let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        // Double to single: unescape double quotes
        let unescaped = inner.replace("\\\"", "\"");
        format!("'{unescaped}'")
    };

    let lsp_range = crate::edit::to_lsp_range(quote_span.to_range(), source, line_index, encoding);
    let edit = types::TextEdit {
        range: lsp_range,
        new_text,
    };

    Some(types::CodeAction {
        title: "Toggle quotes".to_string(),
        kind: Some(types::CodeActionKind::new("refactor.rewrite.quote")),
        edit: Some(workspace_edit_for_document(snapshot, vec![edit])),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// 4. refactor.rewrite.test
// -----------------------------------------------------------------------------

fn modernize_test_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    let dialect = analysis.shell_profile().dialect;
    if !matches!(
        dialect,
        shucked_parser::ShellDialect::Bash | shucked_parser::ShellDialect::Zsh
    ) {
        return None;
    }

    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let cursor_offset = usize::from(text_range.start());

    let cmd = find_simple_command_at_offset(&analysis.parse_result().file.body, cursor_offset)?;
    if cmd.name.span.slice(source) != "[" {
        return None;
    }
    if cmd.args.last().map(|w| w.span.slice(source)) != Some("]") {
        return None;
    }

    let mut edits = Vec::new();

    // Replace [ with [[
    let name_range =
        crate::edit::to_lsp_range(cmd.name.span.to_range(), source, line_index, encoding);
    edits.push(types::TextEdit {
        range: name_range,
        new_text: "[[".to_string(),
    });

    // Replace arguments
    let arg_count = cmd.args.len();
    for (i, arg) in cmd.args.iter().enumerate() {
        let arg_text = arg.span.slice(source);
        if arg_text == "-a" {
            let range =
                crate::edit::to_lsp_range(arg.span.to_range(), source, line_index, encoding);
            edits.push(types::TextEdit {
                range,
                new_text: "&&".to_string(),
            });
        } else if arg_text == "-o" {
            let range =
                crate::edit::to_lsp_range(arg.span.to_range(), source, line_index, encoding);
            edits.push(types::TextEdit {
                range,
                new_text: "||".to_string(),
            });
        } else if i == arg_count - 1 && arg_text == "]" {
            let range =
                crate::edit::to_lsp_range(arg.span.to_range(), source, line_index, encoding);
            edits.push(types::TextEdit {
                range,
                new_text: "]]".to_string(),
            });
        }
    }

    Some(types::CodeAction {
        title: "Modernize test to [[ ... ]]".to_string(),
        kind: Some(types::CodeActionKind::new("refactor.rewrite.test")),
        edit: Some(workspace_edit_for_document(snapshot, edits)),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// 5. refactor.rewrite.invertIf
// -----------------------------------------------------------------------------

fn invert_if_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let cursor_offset = usize::from(text_range.start());

    let if_cmd = find_if_command_at_offset(&analysis.parse_result().file.body, cursor_offset)?;
    let else_branch = if_cmd.else_branch.as_ref()?;
    if !if_cmd.elif_branches.is_empty() {
        return None;
    }

    let cond_raw = if_cmd.condition.span.slice(source);
    let (expr_raw, sep) = if let Some(semi_pos) = cond_raw.rfind(';') {
        (&cond_raw[..semi_pos], &cond_raw[semi_pos..])
    } else if let Some(nl_pos) = cond_raw.rfind('\n') {
        (&cond_raw[..nl_pos], &cond_raw[nl_pos..])
    } else {
        (cond_raw, "")
    };

    let leading_ws = expr_raw
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    let trailing_ws = expr_raw
        .chars()
        .rev()
        .take_while(|c| c.is_whitespace())
        .collect::<String>();
    let expr_trimmed = expr_raw.trim();

    let new_expr = if let Some(stripped) = expr_trimmed.strip_prefix("! ") {
        stripped.to_string()
    } else if expr_trimmed == "!" {
        String::new()
    } else if let Some(stripped) = expr_trimmed.strip_prefix('!') {
        stripped.trim_start().to_string()
    } else {
        format!("! {expr_trimmed}")
    };

    let new_cond = format!("{leading_ws}{new_expr}{trailing_ws}{sep}");

    let then_text = if_cmd.then_branch.span.slice(source).to_string();
    let else_text = else_branch.span.slice(source).to_string();

    let cond_range = crate::edit::to_lsp_range(
        if_cmd.condition.span.to_range(),
        source,
        line_index,
        encoding,
    );
    let then_range = crate::edit::to_lsp_range(
        if_cmd.then_branch.span.to_range(),
        source,
        line_index,
        encoding,
    );
    let else_range =
        crate::edit::to_lsp_range(else_branch.span.to_range(), source, line_index, encoding);

    let edits = vec![
        types::TextEdit {
            range: cond_range,
            new_text: new_cond,
        },
        types::TextEdit {
            range: then_range,
            new_text: else_text,
        },
        types::TextEdit {
            range: else_range,
            new_text: then_text,
        },
    ];

    Some(types::CodeAction {
        title: "Invert 'if' condition".to_string(),
        kind: Some(types::CodeActionKind::new("refactor.rewrite.invertIf")),
        edit: Some(workspace_edit_for_document(snapshot, edits)),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// 6. refactor.extract.function
// -----------------------------------------------------------------------------

fn extract_function_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    if params.range.start == params.range.end {
        return None;
    }

    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let text_range = params.range.to_text_range(source, line_index, encoding);
    let sel_start = usize::from(text_range.start());
    let sel_end = usize::from(text_range.end());
    if sel_start >= sel_end || sel_end > source.len() {
        return None;
    }

    let covered_stmts = find_covered_stmts(&analysis.parse_result().file.body, sel_start, sel_end);
    if covered_stmts.is_empty() {
        return None;
    }

    let first_stmt = covered_stmts[0];
    let stmt_line = line_index.line_number(TextSize::new(first_stmt.span.start.offset() as u32));
    let line_start = usize::from(line_index.line_start(stmt_line).unwrap_or(TextSize::new(0)));
    let indent = source[line_start..first_stmt.span.start.offset()]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect::<String>();

    let sel_text = &source[sel_start..sel_end];
    let mut body_lines = Vec::new();
    for line in sel_text.trim_end().lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            body_lines.push(String::new());
        } else {
            body_lines.push(format!("{indent}    {trimmed}"));
        }
    }
    let body = body_lines.join("\n");
    let func_def = format!("{indent}new_function() {{\n{body}\n{indent}}}\n\n");

    let insert_pos = types::Position::new((stmt_line.saturating_sub(1)) as u32, 0);
    let insert_edit = types::TextEdit {
        range: types::Range::new(insert_pos, insert_pos),
        new_text: func_def,
    };
    let replace_edit = types::TextEdit {
        range: params.range,
        new_text: "new_function".to_string(),
    };

    Some(types::CodeAction {
        title: "Extract to function".to_string(),
        kind: Some(types::CodeActionKind::new("refactor.extract.function")),
        edit: Some(workspace_edit_for_document(
            snapshot,
            vec![insert_edit, replace_edit],
        )),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// 7. refactor.rewrite.strictMode
// -----------------------------------------------------------------------------

fn add_strict_mode_action(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    _params: &types::CodeActionParams,
) -> Option<types::CodeAction> {
    let source = analysis.source();

    if source.contains("set -e")
        || source.contains("set -euo pipefail")
        || source.contains("set -o errexit")
        || source.contains("-euo pipefail")
    {
        return None;
    }

    let insert_pos = if source.starts_with("#!") {
        types::Position::new(1, 0)
    } else {
        types::Position::new(0, 0)
    };

    let edit = types::TextEdit {
        range: types::Range::new(insert_pos, insert_pos),
        new_text: "set -euo pipefail\n".to_string(),
    };

    Some(types::CodeAction {
        title: "Add strict mode ('set -euo pipefail')".to_string(),
        kind: Some(types::CodeActionKind::new("refactor.rewrite.strictMode")),
        edit: Some(workspace_edit_for_document(snapshot, vec![edit])),
        ..Default::default()
    })
}

// -----------------------------------------------------------------------------
// AST Traversal Helpers
// -----------------------------------------------------------------------------

fn span_contains_offset(span: Span, offset: usize) -> bool {
    span.start.offset() <= offset && offset <= span.end.offset()
}

fn find_innermost_enclosing_stmt(seq: &StmtSeq, start: usize, end: usize) -> Option<&Stmt> {
    for stmt in &seq.stmts {
        if stmt.span.start.offset() <= start && end <= stmt.span.end.offset() {
            // Check if there is an even more deeply nested statement
            match &stmt.command {
                Command::Compound(compound) => match compound {
                    CompoundCommand::If(if_cmd) => {
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&if_cmd.condition, start, end)
                        {
                            return Some(inner);
                        }
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&if_cmd.then_branch, start, end)
                        {
                            return Some(inner);
                        }
                        for (c, b) in &if_cmd.elif_branches {
                            if let Some(inner) = find_innermost_enclosing_stmt(c, start, end) {
                                return Some(inner);
                            }
                            if let Some(inner) = find_innermost_enclosing_stmt(b, start, end) {
                                return Some(inner);
                            }
                        }
                        if let Some(else_branch) = &if_cmd.else_branch
                            && let Some(inner) =
                                find_innermost_enclosing_stmt(else_branch, start, end)
                        {
                            return Some(inner);
                        }
                    }
                    CompoundCommand::For(for_cmd) => {
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&for_cmd.body, start, end)
                        {
                            return Some(inner);
                        }
                    }
                    CompoundCommand::While(while_cmd) => {
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&while_cmd.condition, start, end)
                        {
                            return Some(inner);
                        }
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&while_cmd.body, start, end)
                        {
                            return Some(inner);
                        }
                    }
                    CompoundCommand::Until(until_cmd) => {
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&until_cmd.condition, start, end)
                        {
                            return Some(inner);
                        }
                        if let Some(inner) =
                            find_innermost_enclosing_stmt(&until_cmd.body, start, end)
                        {
                            return Some(inner);
                        }
                    }
                    CompoundCommand::Subshell(sub) | CompoundCommand::BraceGroup(sub) => {
                        if let Some(inner) = find_innermost_enclosing_stmt(sub, start, end) {
                            return Some(inner);
                        }
                    }
                    _ => {}
                },
                Command::Binary(bin) => {
                    if bin.left.span.start.offset() <= start && end <= bin.left.span.end.offset() {
                        return Some(&bin.left);
                    }
                    if bin.right.span.start.offset() <= start && end <= bin.right.span.end.offset()
                    {
                        return Some(&bin.right);
                    }
                }
                Command::Function(func) => {
                    if func.body.span.start.offset() <= start && end <= func.body.span.end.offset()
                    {
                        return Some(&func.body);
                    }
                }
                _ => {}
            }
            return Some(stmt);
        }
    }
    None
}

fn find_assignment_and_stmt(seq: &StmtSeq, target_span: Span) -> Option<(&Assignment, &Stmt)> {
    for stmt in &seq.stmts {
        match &stmt.command {
            Command::Simple(simple) => {
                for assign in simple.assignments.iter() {
                    if assign.target.name_span == target_span || assign.span == target_span {
                        return Some((assign, stmt));
                    }
                }
            }
            Command::Decl(decl) => {
                for operand in &decl.operands {
                    if let shucked_ast::DeclOperand::Assignment(assign) = operand
                        && (assign.target.name_span == target_span || assign.span == target_span)
                    {
                        return Some((assign, stmt));
                    }
                }
            }
            Command::Compound(compound) => match compound {
                CompoundCommand::If(if_cmd) => {
                    if let Some(res) = find_assignment_and_stmt(&if_cmd.condition, target_span) {
                        return Some(res);
                    }
                    if let Some(res) = find_assignment_and_stmt(&if_cmd.then_branch, target_span) {
                        return Some(res);
                    }
                    if let Some(else_branch) = &if_cmd.else_branch
                        && let Some(res) = find_assignment_and_stmt(else_branch, target_span)
                    {
                        return Some(res);
                    }
                }
                CompoundCommand::Subshell(sub) | CompoundCommand::BraceGroup(sub) => {
                    if let Some(res) = find_assignment_and_stmt(sub, target_span) {
                        return Some(res);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    None
}

fn is_offset_inside_double_quotes(seq: &StmtSeq, offset: usize) -> bool {
    let mut found = false;
    visit_words_in_stmt_seq(seq, &mut |word| {
        if check_word_double_quoted(word, offset) {
            found = true;
        }
    });
    found
}

fn check_word_double_quoted(word: &Word, offset: usize) -> bool {
    for part in &word.parts {
        if let WordPart::DoubleQuoted { parts, .. } = &part.kind
            && part.span.start.offset() <= offset
            && offset < part.span.end.offset()
        {
            for inner in parts {
                if inner.span.start.offset() <= offset && offset <= inner.span.end.offset() {
                    return true;
                }
            }
            return true;
        }
    }
    false
}

fn find_quoted_string_at_offset(seq: &StmtSeq, offset: usize) -> Option<(Span, bool)> {
    let mut result = None;
    visit_words_in_stmt_seq(seq, &mut |word| {
        for part in &word.parts {
            if part.span.start.offset() <= offset && offset <= part.span.end.offset() {
                match &part.kind {
                    WordPart::SingleQuoted { .. } => {
                        result = Some((part.span, true));
                    }
                    WordPart::DoubleQuoted { .. } => {
                        result = Some((part.span, false));
                    }
                    _ => {}
                }
            }
        }
    });
    result
}

fn find_simple_command_at_offset(seq: &StmtSeq, offset: usize) -> Option<&SimpleCommand> {
    for stmt in &seq.stmts {
        match &stmt.command {
            Command::Simple(simple) => {
                if simple.span.start.offset() <= offset && offset <= simple.span.end.offset() {
                    return Some(simple);
                }
            }
            Command::Compound(compound) => match compound {
                CompoundCommand::If(if_cmd) => {
                    if let Some(cmd) = find_simple_command_at_offset(&if_cmd.condition, offset) {
                        return Some(cmd);
                    }
                    if let Some(cmd) = find_simple_command_at_offset(&if_cmd.then_branch, offset) {
                        return Some(cmd);
                    }
                    if let Some(else_branch) = &if_cmd.else_branch
                        && let Some(cmd) = find_simple_command_at_offset(else_branch, offset)
                    {
                        return Some(cmd);
                    }
                }
                CompoundCommand::Subshell(sub) | CompoundCommand::BraceGroup(sub) => {
                    if let Some(cmd) = find_simple_command_at_offset(sub, offset) {
                        return Some(cmd);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    None
}

fn find_if_command_at_offset(seq: &StmtSeq, offset: usize) -> Option<&shucked_ast::IfCommand> {
    for stmt in &seq.stmts {
        if let Command::Compound(compound) = &stmt.command
            && let CompoundCommand::If(if_cmd) = compound
            && if_cmd.span.start.offset() <= offset
            && offset <= if_cmd.span.end.offset()
        {
            return Some(if_cmd);
        }
    }
    None
}

fn find_covered_stmts(seq: &StmtSeq, start: usize, end: usize) -> Vec<&Stmt> {
    let mut covered = Vec::new();
    for stmt in &seq.stmts {
        let stmt_start = stmt.span.start.offset();
        let stmt_end = stmt.span.end.offset();

        if stmt_start >= start && stmt_end <= end {
            covered.push(stmt);
        } else if stmt_start <= start
            && end <= stmt_end
            && let Command::Compound(compound) = &stmt.command
        {
            match compound {
                CompoundCommand::If(if_cmd) => {
                    let sub = find_covered_stmts(&if_cmd.then_branch, start, end);
                    if !sub.is_empty() {
                        return sub;
                    }
                    if let Some(else_branch) = &if_cmd.else_branch {
                        let sub = find_covered_stmts(else_branch, start, end);
                        if !sub.is_empty() {
                            return sub;
                        }
                    }
                }
                CompoundCommand::For(for_cmd) => {
                    let sub = find_covered_stmts(&for_cmd.body, start, end);
                    if !sub.is_empty() {
                        return sub;
                    }
                }
                CompoundCommand::While(while_cmd) => {
                    let sub = find_covered_stmts(&while_cmd.body, start, end);
                    if !sub.is_empty() {
                        return sub;
                    }
                }
                CompoundCommand::Until(until_cmd) => {
                    let sub = find_covered_stmts(&until_cmd.body, start, end);
                    if !sub.is_empty() {
                        return sub;
                    }
                }
                CompoundCommand::Subshell(sub) | CompoundCommand::BraceGroup(sub) => {
                    let res = find_covered_stmts(sub, start, end);
                    if !res.is_empty() {
                        return res;
                    }
                }
                _ => {}
            }
        }
    }
    covered
}

fn visit_words_in_stmt_seq<F: FnMut(&Word)>(seq: &StmtSeq, f: &mut F) {
    for stmt in &seq.stmts {
        visit_words_in_stmt(stmt, f);
    }
}

fn visit_words_in_stmt<F: FnMut(&Word)>(stmt: &Stmt, f: &mut F) {
    match &stmt.command {
        Command::Simple(simple) => {
            f(&simple.name);
            for arg in &simple.args {
                f(arg);
            }
            for assign in simple.assignments.iter() {
                if let AssignmentValue::Scalar(w) = &assign.value {
                    f(w);
                }
            }
        }
        Command::Compound(compound) => match compound {
            CompoundCommand::If(if_cmd) => {
                visit_words_in_stmt_seq(&if_cmd.condition, f);
                visit_words_in_stmt_seq(&if_cmd.then_branch, f);
                for (c, b) in &if_cmd.elif_branches {
                    visit_words_in_stmt_seq(c, f);
                    visit_words_in_stmt_seq(b, f);
                }
                if let Some(else_branch) = &if_cmd.else_branch {
                    visit_words_in_stmt_seq(else_branch, f);
                }
            }
            CompoundCommand::Subshell(sub) | CompoundCommand::BraceGroup(sub) => {
                visit_words_in_stmt_seq(sub, f);
            }
            _ => {}
        },
        _ => {}
    }
}

// -----------------------------------------------------------------------------
// Unit Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam::channel;
    use lsp_types::{
        ClientCapabilities, CodeActionContext, CodeActionParams, Position, Range,
        TextDocumentIdentifier, Url,
    };

    use crate::session::{Client, Session};
    use crate::{
        ClientOptions, GlobalOptions, PositionEncoding, TextDocument, Workspace, Workspaces,
    };

    fn make_test_session(source: &str, language_id: &str) -> (Session, Client, Url) {
        let (main_loop_sender, _main_loop_receiver) = channel::unbounded();
        let (client_sender, _client_receiver) = channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let workspace_root = std::env::temp_dir().join(format!(
            "shuck-refactor-tests-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let workspace_uri = Url::from_file_path(&workspace_root).expect("workspace path to URL");
        let workspaces = Workspaces::new(vec![Workspace::default(workspace_uri)]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &ClientCapabilities::default(),
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");
        session.update_client_options(ClientOptions::default());

        let path = workspace_root.join("script.sh");
        let uri = Url::from_file_path(path).expect("test path to URL");
        session.open_text_document(
            uri.clone(),
            TextDocument::new(source.to_owned(), 1).with_language_id(language_id),
        );

        (session, client, uri)
    }

    fn extract_actions(response: Vec<types::CodeActionOrCommand>) -> Vec<types::CodeAction> {
        response
            .into_iter()
            .filter_map(|entry| match entry {
                types::CodeActionOrCommand::CodeAction(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    fn get_edits(action: &types::CodeAction) -> Vec<types::TextEdit> {
        if let Some(edit) = &action.edit {
            if let Some(changes) = &edit.changes
                && let Some(edits) = changes.values().next()
            {
                return edits.clone();
            }
            if let Some(types::DocumentChanges::Edits(doc_edits)) = &edit.document_changes
                && let Some(doc_edit) = doc_edits.first()
            {
                return doc_edit
                    .edits
                    .iter()
                    .filter_map(|e| match e {
                        types::OneOf::Left(te) => Some(te.clone()),
                        _ => None,
                    })
                    .collect();
            }
        }
        vec![]
    }

    fn apply_edits_to_source(
        source: &str,
        edits: &[types::TextEdit],
        line_index: &shucked_indexer::LineIndex,
        encoding: PositionEncoding,
    ) -> String {
        let mut byte_edits = Vec::new();
        for edit in edits {
            let tr = edit.range.to_text_range(source, line_index, encoding);
            byte_edits.push((
                usize::from(tr.start()),
                usize::from(tr.end()),
                edit.new_text.clone(),
            ));
        }
        byte_edits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        let mut res = source.to_string();
        for (start, end, new_text) in byte_edits {
            res.replace_range(start..end, &new_text);
        }
        res
    }

    #[test]
    fn test_extract_variable_at_root() {
        let source = "echo $(date +%s)\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 5), Position::new(0, 16)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Extract to variable")
            .expect("should offer extract to variable");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.extract.variable"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(
            result,
            "extracted_var=$(date +%s)\necho \"$extracted_var\"\n"
        );
    }

    #[test]
    fn test_extract_variable_in_function() {
        let source = "my_func() {\n    echo $(date +%s)\n}\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(1, 9), Position::new(1, 20)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Extract to variable")
            .expect("should offer extract to variable");

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(
            result,
            "my_func() {\n    local extracted_var=$(date +%s)\n    echo \"$extracted_var\"\n}\n"
        );
    }

    #[test]
    fn test_inline_variable() {
        let source = "var=\"value\"\necho $var\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 2), Position::new(0, 2)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Inline variable 'var'")
            .expect("should offer inline variable");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.inline.variable"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(result, "echo \"value\"\n");
    }

    #[test]
    fn test_inline_variable_from_reference() {
        let source = "var=\"value\"\necho $var\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(1, 6), Position::new(1, 6)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        assert!(actions.iter().any(|a| a.title == "Inline variable 'var'"));
    }

    #[test]
    fn test_inline_variable_not_offered_if_mutated() {
        let source = "var=\"value\"\nvar=\"mutated\"\necho $var\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 2), Position::new(0, 2)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        assert!(
            !actions
                .iter()
                .any(|a| a.title.starts_with("Inline variable"))
        );
    }

    #[test]
    fn test_quote_variable_reference() {
        let source = "echo $foo\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 6), Position::new(0, 6)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Quote variable reference \"$foo\"")
            .expect("should offer quote variable");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.rewrite.quote"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(result, "echo \"$foo\"\n");
    }

    #[test]
    fn test_quote_variable_already_quoted_ignored() {
        let source = "echo \"$foo\"\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 7), Position::new(0, 7)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        assert!(
            !actions
                .iter()
                .any(|a| a.title.starts_with("Quote variable reference"))
        );
    }

    #[test]
    fn test_toggle_quotes_single_to_double() {
        let source = "echo 'hello \"world\"'\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 8), Position::new(0, 8)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Toggle quotes")
            .expect("should offer toggle quotes");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.rewrite.quote"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(result, "echo \"hello \\\"world\\\"\"\n");
    }

    #[test]
    fn test_toggle_quotes_double_to_single() {
        let source = "echo \"hello \\\"world\\\"\"\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 8), Position::new(0, 8)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Toggle quotes")
            .expect("should offer toggle quotes");

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(result, "echo 'hello \"world\"'\n");
    }

    #[test]
    fn test_modernize_test_to_double_brackets() {
        let source = "[ \"$x\" = \"1\" -a \"$y\" = \"2\" ]\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Modernize test to [[ ... ]]")
            .expect("should offer modernize test");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.rewrite.test"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(result, "[[ \"$x\" = \"1\" && \"$y\" = \"2\" ]]\n");
    }

    #[test]
    fn test_invert_if_condition() {
        let source = "if [ \"$x\" = \"1\" ]; then\n    echo \"yes\"\nelse\n    echo \"no\"\nfi\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 1), Position::new(0, 1)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Invert 'if' condition")
            .expect("should offer invert if");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.rewrite.invertIf"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(
            result,
            "if ! [ \"$x\" = \"1\" ]; then\n    echo \"no\"\nelse\n    echo \"yes\"\nfi\n"
        );
    }

    #[test]
    fn test_invert_if_already_negated() {
        let source = "if ! [ \"$x\" = \"1\" ]; then\n    echo \"no\"\nelse\n    echo \"yes\"\nfi\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 1), Position::new(0, 1)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Invert 'if' condition")
            .expect("should offer invert if");

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(
            result,
            "if [ \"$x\" = \"1\" ]; then\n    echo \"yes\"\nelse\n    echo \"no\"\nfi\n"
        );
    }

    #[test]
    fn test_extract_function() {
        let source = "echo \"step 1\"\necho \"step 2\"\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 0), Position::new(1, 13)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Extract to function")
            .expect("should offer extract to function");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.extract.function"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        let expected =
            "new_function() {\n    echo \"step 1\"\n    echo \"step 2\"\n}\n\nnew_function\n";
        assert_eq!(result, expected);
    }

    #[test]
    fn test_add_strict_mode_with_shebang() {
        let source = "#!/usr/bin/env bash\necho \"hello\"\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Add strict mode ('set -euo pipefail')")
            .expect("should offer add strict mode");
        assert_eq!(
            action.kind,
            Some(types::CodeActionKind::new("refactor.rewrite.strictMode"))
        );

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(
            result,
            "#!/usr/bin/env bash\nset -euo pipefail\necho \"hello\"\n"
        );
    }

    #[test]
    fn test_add_strict_mode_without_shebang() {
        let source = "echo \"hello\"\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        let action = actions
            .iter()
            .find(|a| a.title == "Add strict mode ('set -euo pipefail')")
            .expect("should offer add strict mode");

        let edits = get_edits(action);
        let result = apply_edits_to_source(
            source,
            &edits,
            snapshot.analysis().unwrap().line_index(),
            snapshot.encoding(),
        );
        assert_eq!(result, "set -euo pipefail\necho \"hello\"\n");
    }

    #[test]
    fn test_add_strict_mode_already_present() {
        let source = "#!/usr/bin/env bash\nset -e\necho \"hello\"\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        assert!(
            !actions
                .iter()
                .any(|a| a.title.starts_with("Add strict mode"))
        );
    }

    #[test]
    fn test_filter_refactor_actions_by_only() {
        let source = "echo $(date +%s)\n";
        let (session, _client, uri) = make_test_session(source, "bash");
        let snapshot = session.take_snapshot(uri.clone()).unwrap();

        // When requesting only refactor.rewrite, extract.variable should NOT be returned
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 5), Position::new(0, 16)),
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only: Some(vec![types::CodeActionKind::new("refactor.rewrite")]),
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions = extract_actions(refactor_code_actions(&snapshot, &params));
        assert!(!actions.iter().any(|a| a.title == "Extract to variable"));

        // When requesting refactor.extract, extract.variable SHOULD be returned
        let params2 = CodeActionParams {
            text_document: TextDocumentIdentifier { uri },
            range: Range::new(Position::new(0, 5), Position::new(0, 16)),
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only: Some(vec![types::CodeActionKind::new("refactor.extract")]),
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };

        let actions2 = extract_actions(refactor_code_actions(&snapshot, &params2));
        assert!(actions2.iter().any(|a| a.title == "Extract to variable"));
    }
}
