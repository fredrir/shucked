//! Inlay hint handler for shell scripts.
//!
//! Provides inline type and parameter hints:
//! - Parameter fallback behavior (e.g. `${var:-default}`, `${var:=default}`, `${var:+alternate}`)
//! - Variable declaration attributes (e.g. `export FOO=1`, `readonly BAR=2`)
//! - Command substitution output capture (e.g. `$(cmd)`)

use lsp_types as types;
use shucked_ast::{
    ArithmeticExpr, ArithmeticExprNode, ArithmeticLvalue, ArrayElem, AssignmentValue,
    BourneParameterExpansion, BuiltinCommand, Command, CompoundCommand, ConditionalExpr,
    DeclClause, DeclOperand, HeredocBodyPart, ParameterExpansion, ParameterExpansionSyntax,
    ParameterOp, PatternPart, Redirect, RedirectTarget, Span, Stmt, StmtSeq, Subscript, Word,
    WordPart, WordPartNode, ZshDefaultingOp, ZshExpansionOperation, ZshExpansionTarget,
};

use crate::PositionEncoding;
use crate::session::DocumentSnapshot;

/// Computes inlay hints for the requested document snapshot and range.
pub fn inlay_hints(
    snapshot: DocumentSnapshot,
    params: types::InlayHintParams,
) -> crate::server::Result<Option<Vec<types::InlayHint>>> {
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };

    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();

    let mut collector = InlayHintCollector {
        source,
        line_index,
        encoding,
        range: params.range,
        hints: Vec::new(),
    };

    collector.visit_stmt_seq(&analysis.parse_result().file.body);

    collector.hints.sort_by(|a, b| {
        a.position
            .line
            .cmp(&b.position.line)
            .then_with(|| a.position.character.cmp(&b.position.character))
    });

    Ok(Some(collector.hints))
}

struct InlayHintCollector<'a> {
    source: &'a str,
    line_index: &'a shucked_indexer::LineIndex,
    encoding: PositionEncoding,
    range: types::Range,
    hints: Vec<types::InlayHint>,
}

impl<'a> InlayHintCollector<'a> {
    fn to_lsp_position(&self, offset: usize) -> types::Position {
        crate::edit::offset_to_position(self.source, self.line_index, offset, self.encoding)
    }

    fn add_hint_at_offset(
        &mut self,
        offset: usize,
        label: impl Into<String>,
        kind: Option<types::InlayHintKind>,
        tooltip: Option<&str>,
        padding_left: Option<bool>,
        padding_right: Option<bool>,
    ) {
        let position = self.to_lsp_position(offset);
        if position >= self.range.start && position <= self.range.end {
            self.hints.push(types::InlayHint {
                position,
                label: types::InlayHintLabel::String(label.into()),
                kind,
                text_edits: None,
                tooltip: tooltip.map(|t| types::InlayHintTooltip::String(t.to_string())),
                padding_left,
                padding_right,
                data: None,
            });
        }
    }

    fn visit_stmt_seq(&mut self, seq: &StmtSeq) {
        for stmt in seq.iter() {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        self.visit_command(&stmt.command);
        for redirect in &stmt.redirects {
            self.visit_redirect(redirect);
        }
    }

    fn visit_command(&mut self, command: &Command) {
        match command {
            Command::Simple(simple) => {
                for assignment in &simple.assignments {
                    self.visit_assignment_value(&assignment.value);
                }
                self.visit_word(&simple.name);
                for arg in &simple.args {
                    self.visit_word(arg);
                }
            }
            Command::Builtin(builtin) => match builtin {
                BuiltinCommand::Break(b) => {
                    for a in &b.assignments {
                        self.visit_assignment_value(&a.value);
                    }
                    if let Some(d) = &b.depth {
                        self.visit_word(d);
                    }
                    for w in &b.extra_args {
                        self.visit_word(w);
                    }
                }
                BuiltinCommand::Continue(c) => {
                    for a in &c.assignments {
                        self.visit_assignment_value(&a.value);
                    }
                    if let Some(d) = &c.depth {
                        self.visit_word(d);
                    }
                    for w in &c.extra_args {
                        self.visit_word(w);
                    }
                }
                BuiltinCommand::Return(r) => {
                    for a in &r.assignments {
                        self.visit_assignment_value(&a.value);
                    }
                    if let Some(c) = &r.code {
                        self.visit_word(c);
                    }
                    for w in &r.extra_args {
                        self.visit_word(w);
                    }
                }
                BuiltinCommand::Exit(e) => {
                    for a in &e.assignments {
                        self.visit_assignment_value(&a.value);
                    }
                    if let Some(c) = &e.code {
                        self.visit_word(c);
                    }
                    for w in &e.extra_args {
                        self.visit_word(w);
                    }
                }
            },
            Command::Decl(clause) => {
                self.visit_decl_clause(clause);
            }
            Command::Binary(binary) => {
                self.visit_stmt(&binary.left);
                self.visit_stmt(&binary.right);
            }
            Command::Compound(compound) => match compound {
                CompoundCommand::If(if_cmd) => {
                    self.visit_stmt_seq(&if_cmd.condition);
                    self.visit_stmt_seq(&if_cmd.then_branch);
                    for (cond, body) in &if_cmd.elif_branches {
                        self.visit_stmt_seq(cond);
                        self.visit_stmt_seq(body);
                    }
                    if let Some(body) = &if_cmd.else_branch {
                        self.visit_stmt_seq(body);
                    }
                }
                CompoundCommand::For(for_cmd) => {
                    for target in &for_cmd.targets {
                        self.visit_word(&target.word);
                    }
                    if let Some(words) = &for_cmd.words {
                        for word in words {
                            self.visit_word(word);
                        }
                    }
                    self.visit_stmt_seq(&for_cmd.body);
                }
                CompoundCommand::Repeat(repeat) => {
                    self.visit_word(&repeat.count);
                    self.visit_stmt_seq(&repeat.body);
                }
                CompoundCommand::Foreach(foreach) => {
                    for word in &foreach.words {
                        self.visit_word(word);
                    }
                    self.visit_stmt_seq(&foreach.body);
                }
                CompoundCommand::ArithmeticFor(afor) => {
                    if let Some(expression) = &afor.init_ast {
                        self.visit_arithmetic_expr(expression);
                    }
                    if let Some(expression) = &afor.condition_ast {
                        self.visit_arithmetic_expr(expression);
                    }
                    if let Some(expression) = &afor.step_ast {
                        self.visit_arithmetic_expr(expression);
                    }
                    self.visit_stmt_seq(&afor.body);
                }
                CompoundCommand::While(while_cmd) => {
                    self.visit_stmt_seq(&while_cmd.condition);
                    self.visit_stmt_seq(&while_cmd.body);
                }
                CompoundCommand::Until(until_cmd) => {
                    self.visit_stmt_seq(&until_cmd.condition);
                    self.visit_stmt_seq(&until_cmd.body);
                }
                CompoundCommand::Case(case_cmd) => {
                    self.visit_word(&case_cmd.word);
                    for item in &case_cmd.cases {
                        for pat in &item.patterns {
                            for part in &pat.parts {
                                if let PatternPart::Word(w) = &part.kind {
                                    self.visit_word(w);
                                }
                            }
                        }
                        self.visit_stmt_seq(&item.body);
                    }
                }
                CompoundCommand::Select(select) => {
                    for word in &select.words {
                        self.visit_word(word);
                    }
                    self.visit_stmt_seq(&select.body);
                }
                CompoundCommand::Subshell(seq) | CompoundCommand::BraceGroup(seq) => {
                    self.visit_stmt_seq(seq);
                }
                CompoundCommand::Arithmetic(arith) => {
                    if let Some(expression) = &arith.expr_ast {
                        self.visit_arithmetic_expr(expression);
                    }
                }
                CompoundCommand::Time(time) => {
                    if let Some(cmd) = &time.command {
                        self.visit_stmt(cmd);
                    }
                }
                CompoundCommand::Conditional(cond) => {
                    self.visit_conditional_expr(&cond.expression);
                }
                CompoundCommand::Coproc(coproc) => {
                    self.visit_stmt(&coproc.body);
                }
                CompoundCommand::Always(always) => {
                    self.visit_stmt_seq(&always.body);
                    self.visit_stmt_seq(&always.always_body);
                }
            },
            Command::Function(func) => {
                self.visit_stmt(&func.body);
            }
            Command::AnonymousFunction(anon) => {
                self.visit_stmt(&anon.body);
                for arg in &anon.args {
                    self.visit_word(arg);
                }
            }
        }
    }

    fn visit_decl_clause(&mut self, clause: &DeclClause) {
        let variant = clause.variant.as_str();
        let is_export_cmd = variant == "export";
        let is_readonly_cmd = variant == "readonly";
        let mut has_export_flag = false;
        let mut has_readonly_flag = false;
        let mut removes_export_flag = false;
        let mut removes_readonly_flag = false;

        for operand in &clause.operands {
            if let DeclOperand::Flag(word) = operand
                && let Some(text) = shucked_ast::static_word_text(word, self.source)
            {
                if text.starts_with('-') {
                    for ch in text.chars().skip(1) {
                        if ch == 'x' {
                            has_export_flag = true;
                        }
                        if ch == 'r' {
                            has_readonly_flag = true;
                        }
                    }
                } else if text.starts_with('+') {
                    for ch in text.chars().skip(1) {
                        if ch == 'x' {
                            removes_export_flag = true;
                        }
                        if ch == 'r' {
                            removes_readonly_flag = true;
                        }
                    }
                }
            }
        }

        let is_exported = (has_export_flag || is_export_cmd) && !removes_export_flag;
        let is_readonly = (has_readonly_flag || is_readonly_cmd) && !removes_readonly_flag;

        let attribute_hint = match (is_exported, is_readonly) {
            (true, true) => Some((": readonly, export", "Readonly exported variable")),
            (true, false) => Some((": export", "Exported environment variable")),
            (false, true) => Some((": readonly", "Readonly variable")),
            (false, false) => None,
        };

        for operand in &clause.operands {
            match operand {
                DeclOperand::Name(var_ref) => {
                    if let Some((label, tooltip)) = attribute_hint {
                        let offset = var_ref
                            .subscript
                            .as_deref()
                            .map_or(var_ref.name_span.end.offset(), |s| s.span().end.offset());
                        self.add_hint_at_offset(
                            offset,
                            label,
                            Some(types::InlayHintKind::TYPE),
                            Some(tooltip),
                            Some(false),
                            Some(true),
                        );
                    }
                    if let Some(subscript) = &var_ref.subscript {
                        self.visit_subscript(subscript);
                    }
                }
                DeclOperand::Assignment(assignment) => {
                    if let Some((label, tooltip)) = attribute_hint {
                        let offset = assignment
                            .target
                            .subscript
                            .as_deref()
                            .map_or(assignment.target.name_span.end.offset(), |s| {
                                s.span().end.offset()
                            });
                        self.add_hint_at_offset(
                            offset,
                            label,
                            Some(types::InlayHintKind::TYPE),
                            Some(tooltip),
                            Some(false),
                            Some(true),
                        );
                    }
                    if let Some(subscript) = &assignment.target.subscript {
                        self.visit_subscript(subscript);
                    }
                    self.visit_assignment_value(&assignment.value);
                }
                DeclOperand::Flag(word) | DeclOperand::Dynamic(word) => {
                    self.visit_word(word);
                }
            }
        }

        for assignment in &clause.assignments {
            self.visit_assignment_value(&assignment.value);
        }
    }

    fn visit_parameter_op_hint(
        &mut self,
        operator: &ParameterOp,
        colon_variant: bool,
        operand_word_ast: Option<&Word>,
        full_span: Span,
    ) {
        let (label, tooltip) = match operator {
            ParameterOp::UseDefault => (
                ": default",
                if colon_variant {
                    "Use default value if unset or empty"
                } else {
                    "Use default value if unset"
                },
            ),
            ParameterOp::AssignDefault => (
                "= default",
                if colon_variant {
                    "Assign default value if unset or empty"
                } else {
                    "Assign default value if unset"
                },
            ),
            ParameterOp::UseReplacement => (
                "+ alternate",
                if colon_variant {
                    "Use alternate value if set and not empty"
                } else {
                    "Use alternate value if set"
                },
            ),
            ParameterOp::Error => (
                "? error",
                if colon_variant {
                    "Error if unset or empty"
                } else {
                    "Error if unset"
                },
            ),
            _ => return,
        };

        let offset = if let Some(operand_word) = operand_word_ast {
            operand_word.span.start.offset()
        } else {
            let end = full_span.end.offset();
            if end > 0 && self.source.as_bytes().get(end - 1) == Some(&b'}') {
                end - 1
            } else {
                end
            }
        };

        self.add_hint_at_offset(
            offset,
            label,
            Some(types::InlayHintKind::PARAMETER),
            Some(tooltip),
            Some(false),
            Some(true),
        );
    }

    fn visit_parameter_expansion(&mut self, parameter: &ParameterExpansion, full_span: Span) {
        match &parameter.syntax {
            ParameterExpansionSyntax::Bourne(bourne) => match bourne {
                BourneParameterExpansion::Operation {
                    operator,
                    operand_word_ast,
                    colon_variant,
                    ..
                } => {
                    self.visit_parameter_op_hint(
                        operator,
                        *colon_variant,
                        operand_word_ast.as_deref(),
                        full_span,
                    );
                    if let Some(operand) = operand_word_ast {
                        self.visit_word(operand);
                    }
                    match &**operator {
                        ParameterOp::ReplaceFirst {
                            replacement_word_ast,
                            ..
                        }
                        | ParameterOp::ReplaceAll {
                            replacement_word_ast,
                            ..
                        } => {
                            self.visit_word(replacement_word_ast);
                        }
                        _ => {}
                    }
                }
                BourneParameterExpansion::Indirect {
                    operator,
                    operand_word_ast,
                    colon_variant,
                    ..
                } => {
                    if let Some(operator) = operator {
                        self.visit_parameter_op_hint(
                            operator,
                            *colon_variant,
                            operand_word_ast.as_deref(),
                            full_span,
                        );
                    }
                    if let Some(operand) = operand_word_ast {
                        self.visit_word(operand);
                    }
                }
                BourneParameterExpansion::Slice {
                    offset_word_ast,
                    length_word_ast,
                    ..
                } => {
                    self.visit_word(offset_word_ast);
                    if let Some(length) = length_word_ast {
                        self.visit_word(length);
                    }
                }
                BourneParameterExpansion::Access { reference }
                | BourneParameterExpansion::Length { reference }
                | BourneParameterExpansion::Indices { reference }
                | BourneParameterExpansion::Transformation { reference, .. } => {
                    if let Some(subscript) = &reference.subscript {
                        self.visit_subscript(subscript);
                    }
                }
                BourneParameterExpansion::PrefixMatch { .. } => {}
            },
            ParameterExpansionSyntax::Zsh(zsh) => {
                match &zsh.target {
                    ZshExpansionTarget::Word(word) => self.visit_word(word),
                    ZshExpansionTarget::Nested(nested) => {
                        self.visit_parameter_expansion(nested, full_span);
                    }
                    ZshExpansionTarget::Reference(reference) => {
                        if let Some(subscript) = &reference.subscript {
                            self.visit_subscript(subscript);
                        }
                    }
                    ZshExpansionTarget::Empty => {}
                }
                for modifier in &zsh.modifiers {
                    if let Some(word) = modifier.argument_word_ast() {
                        self.visit_word(word);
                    }
                }
                if let Some(operation) = &zsh.operation {
                    match operation {
                        ZshExpansionOperation::Defaulting {
                            kind,
                            operand_word_ast,
                            colon_variant,
                            ..
                        } => {
                            let mapped_op = match kind {
                                ZshDefaultingOp::UseDefault => ParameterOp::UseDefault,
                                ZshDefaultingOp::AssignDefault => ParameterOp::AssignDefault,
                                ZshDefaultingOp::UseReplacement => ParameterOp::UseReplacement,
                                ZshDefaultingOp::Error => ParameterOp::Error,
                            };
                            self.visit_parameter_op_hint(
                                &mapped_op,
                                *colon_variant,
                                Some(operand_word_ast),
                                full_span,
                            );
                            self.visit_word(operand_word_ast);
                        }
                        ZshExpansionOperation::PatternOperation {
                            operand_word_ast, ..
                        }
                        | ZshExpansionOperation::TrimOperation {
                            operand_word_ast, ..
                        } => {
                            self.visit_word(operand_word_ast);
                        }
                        ZshExpansionOperation::ReplacementOperation {
                            pattern_word_ast,
                            replacement_word_ast,
                            ..
                        } => {
                            self.visit_word(pattern_word_ast);
                            if let Some(replacement) = replacement_word_ast {
                                self.visit_word(replacement);
                            }
                        }
                        ZshExpansionOperation::Slice {
                            offset_word_ast,
                            length_word_ast,
                            ..
                        } => {
                            self.visit_word(offset_word_ast);
                            if let Some(length) = length_word_ast {
                                self.visit_word(length);
                            }
                        }
                        ZshExpansionOperation::Unknown { .. } => {}
                    }
                }
            }
        }
    }

    fn visit_word(&mut self, word: &Word) {
        for part in &word.parts {
            self.visit_word_part(part);
        }
    }

    fn visit_word_part(&mut self, part_node: &WordPartNode) {
        match &part_node.kind {
            WordPart::Literal(_) | WordPart::Variable(_) | WordPart::PrefixMatch { .. } => {}
            WordPart::SingleQuoted { .. } => {}
            WordPart::DoubleQuoted { parts, .. } => {
                for part in parts {
                    self.visit_word_part(part);
                }
            }
            WordPart::CommandSubstitution { body, .. } => {
                self.add_hint_at_offset(
                    part_node.span.end.offset(),
                    ": stdout",
                    Some(types::InlayHintKind::TYPE),
                    Some("Command substitution captures standard output"),
                    Some(false),
                    Some(false),
                );
                self.visit_stmt_seq(body);
            }
            WordPart::ProcessSubstitution { body, .. } => {
                self.visit_stmt_seq(body);
            }
            WordPart::ArithmeticExpansion {
                expression_ast,
                expression_word_ast,
                ..
            } => {
                if let Some(expr) = expression_ast {
                    self.visit_arithmetic_expr(expr);
                } else {
                    self.visit_word(expression_word_ast);
                }
            }
            WordPart::Parameter(param) => {
                self.visit_parameter_expansion(param, part_node.span);
            }
            WordPart::ParameterExpansion {
                operator,
                operand_word_ast,
                colon_variant,
                ..
            } => {
                self.visit_parameter_op_hint(
                    operator,
                    *colon_variant,
                    operand_word_ast.as_deref(),
                    part_node.span,
                );
                if let Some(operand) = operand_word_ast {
                    self.visit_word(operand);
                }
            }
            WordPart::IndirectExpansion {
                operator,
                operand_word_ast,
                colon_variant,
                ..
            } => {
                if let Some(operator) = operator {
                    self.visit_parameter_op_hint(
                        operator,
                        *colon_variant,
                        operand_word_ast.as_deref(),
                        part_node.span,
                    );
                }
                if let Some(operand) = operand_word_ast {
                    self.visit_word(operand);
                }
            }
            WordPart::Substring {
                offset_ast,
                offset_word_ast,
                length_ast,
                length_word_ast,
                ..
            }
            | WordPart::ArraySlice {
                offset_ast,
                offset_word_ast,
                length_ast,
                length_word_ast,
                ..
            } => {
                if let Some(expr) = offset_ast {
                    self.visit_arithmetic_expr(expr);
                } else {
                    self.visit_word(offset_word_ast);
                }
                if let Some(expr) = length_ast {
                    self.visit_arithmetic_expr(expr);
                } else if let Some(word) = length_word_ast {
                    self.visit_word(word);
                }
            }
            WordPart::Length(_)
            | WordPart::ArrayAccess(_)
            | WordPart::ArrayLength(_)
            | WordPart::ArrayIndices(_)
            | WordPart::Transformation { .. } => {}
            WordPart::ZshQualifiedGlob(_) => {}
        }
    }

    fn visit_redirect(&mut self, redirect: &Redirect) {
        match &redirect.target {
            RedirectTarget::Word(word) => self.visit_word(word),
            RedirectTarget::Heredoc(heredoc) => {
                for part in &heredoc.body.parts {
                    match &part.kind {
                        HeredocBodyPart::Literal(_) | HeredocBodyPart::Variable(_) => {}
                        HeredocBodyPart::CommandSubstitution { body, .. } => {
                            self.add_hint_at_offset(
                                part.span.end.offset(),
                                ": stdout",
                                Some(types::InlayHintKind::TYPE),
                                Some("Command substitution captures standard output"),
                                Some(false),
                                Some(false),
                            );
                            self.visit_stmt_seq(body);
                        }
                        HeredocBodyPart::ArithmeticExpansion {
                            expression_ast,
                            expression_word_ast,
                            ..
                        } => {
                            if let Some(expr) = expression_ast {
                                self.visit_arithmetic_expr(expr);
                            } else {
                                self.visit_word(expression_word_ast);
                            }
                        }
                        HeredocBodyPart::Parameter(param) => {
                            self.visit_parameter_expansion(param, part.span);
                        }
                    }
                }
            }
        }
    }

    fn visit_assignment_value(&mut self, value: &AssignmentValue) {
        match value {
            AssignmentValue::Scalar(word) => self.visit_word(word),
            AssignmentValue::Compound(array_expr) => {
                for elem in &array_expr.elements {
                    self.visit_word(&elem.value().word);
                    match elem {
                        ArrayElem::Keyed { key, .. } | ArrayElem::KeyedAppend { key, .. } => {
                            self.visit_subscript(key);
                        }
                        ArrayElem::Sequential(_) => {}
                    }
                }
            }
        }
    }

    fn visit_subscript(&mut self, subscript: &Subscript) {
        if let Some(word) = &subscript.word_ast {
            self.visit_word(word);
        }
        if let Some(arithmetic) = &subscript.arithmetic_ast {
            self.visit_arithmetic_expr(arithmetic);
        }
    }

    fn visit_conditional_expr(&mut self, expr: &ConditionalExpr) {
        match expr {
            ConditionalExpr::Binary(binary) => {
                self.visit_conditional_expr(&binary.left);
                self.visit_conditional_expr(&binary.right);
            }
            ConditionalExpr::Unary(unary) => {
                self.visit_conditional_expr(&unary.expr);
            }
            ConditionalExpr::Parenthesized(paren) => {
                self.visit_conditional_expr(&paren.expr);
            }
            ConditionalExpr::Word(word) | ConditionalExpr::Regex(word) => {
                self.visit_word(word);
            }
            ConditionalExpr::Pattern(pattern) => {
                for part in &pattern.parts {
                    if let PatternPart::Word(word) = &part.kind {
                        self.visit_word(word);
                    }
                }
            }
            ConditionalExpr::VarRef(var_ref) => {
                if let Some(subscript) = &var_ref.subscript {
                    self.visit_subscript(subscript);
                }
            }
        }
    }

    fn visit_arithmetic_expr(&mut self, expr: &ArithmeticExprNode) {
        match &expr.kind {
            ArithmeticExpr::Number(_) | ArithmeticExpr::Variable(_) => {}
            ArithmeticExpr::Indexed { index, .. } => self.visit_arithmetic_expr(index),
            ArithmeticExpr::ShellWord(word) => self.visit_word(word),
            ArithmeticExpr::Parenthesized { expression } => self.visit_arithmetic_expr(expression),
            ArithmeticExpr::Unary { expr, .. } | ArithmeticExpr::Postfix { expr, .. } => {
                self.visit_arithmetic_expr(expr);
            }
            ArithmeticExpr::Binary { left, right, .. } => {
                self.visit_arithmetic_expr(left);
                self.visit_arithmetic_expr(right);
            }
            ArithmeticExpr::Conditional {
                condition,
                then_expr,
                else_expr,
            } => {
                self.visit_arithmetic_expr(condition);
                self.visit_arithmetic_expr(then_expr);
                self.visit_arithmetic_expr(else_expr);
            }
            ArithmeticExpr::Assignment { target, value, .. } => {
                self.visit_arithmetic_lvalue(target);
                self.visit_arithmetic_expr(value);
            }
        }
    }

    fn visit_arithmetic_lvalue(&mut self, target: &ArithmeticLvalue) {
        match target {
            ArithmeticLvalue::Variable(_) => {}
            ArithmeticLvalue::Indexed { index, .. } => self.visit_arithmetic_expr(index),
        }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam::channel;
    use lsp_types::{self as types, ClientCapabilities, Url};

    use super::*;
    use crate::session::{Client, GlobalOptions, Session, Workspace, Workspaces};
    use crate::{PositionEncoding, TextDocument};

    fn test_snapshot(content: &str) -> DocumentSnapshot {
        let (main_loop_sender, _main_loop_receiver) = channel::unbounded();
        let (client_sender, _client_receiver) = channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let workspaces = Workspaces::new(vec![Workspace::default(
            Url::from_file_path(std::env::temp_dir())
                .expect("temporary directory should convert to a file URL"),
        )]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &ClientCapabilities::default(),
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");

        let uri = Url::from_file_path(std::env::temp_dir().join("test_inlay.sh")).unwrap();
        session.open_text_document(
            uri.clone(),
            TextDocument::new(content.to_owned(), 1).with_language_id("shellscript"),
        );

        session
            .take_snapshot(uri)
            .expect("test document should produce a snapshot")
    }

    fn full_range() -> types::Range {
        types::Range {
            start: types::Position {
                line: 0,
                character: 0,
            },
            end: types::Position {
                line: u32::MAX,
                character: u32::MAX,
            },
        }
    }

    #[test]
    fn test_parameter_expansion_defaults() {
        let script = r#"
echo "${var:-default}"
echo "${var:=assign}"
echo "${var:+alternate}"
echo "${var:?error_msg}"
"#;
        let snapshot = test_snapshot(script);
        let params = types::InlayHintParams {
            text_document: types::TextDocumentIdentifier {
                uri: types::Url::parse("file:///test.sh").unwrap(),
            },
            range: full_range(),
            work_done_progress_params: types::WorkDoneProgressParams::default(),
        };

        let result = inlay_hints(snapshot, params).unwrap().unwrap();
        let labels: Vec<_> = result
            .iter()
            .map(|h| match &h.label {
                types::InlayHintLabel::String(s) => s.as_str(),
                _ => panic!("expected string label"),
            })
            .collect();

        assert_eq!(
            labels,
            vec![": default", "= default", "+ alternate", "? error"]
        );
        for hint in &result {
            assert_eq!(hint.kind, Some(types::InlayHintKind::PARAMETER));
        }
    }

    #[test]
    fn test_declarations_attributes() {
        let script = r#"
export FOO=1
readonly BAR=2
declare -x EXPORTED=3
declare -r READONLY=4
declare -xr BOTH=5
export BARE_EXPORT
readonly BARE_READONLY
"#;
        let snapshot = test_snapshot(script);
        let params = types::InlayHintParams {
            text_document: types::TextDocumentIdentifier {
                uri: types::Url::parse("file:///test.sh").unwrap(),
            },
            range: full_range(),
            work_done_progress_params: types::WorkDoneProgressParams::default(),
        };

        let result = inlay_hints(snapshot, params).unwrap().unwrap();
        let labels: Vec<_> = result
            .iter()
            .map(|h| match &h.label {
                types::InlayHintLabel::String(s) => s.as_str(),
                _ => panic!("expected string label"),
            })
            .collect();

        assert_eq!(
            labels,
            vec![
                ": export",
                ": readonly",
                ": export",
                ": readonly",
                ": readonly, export",
                ": export",
                ": readonly",
            ]
        );
        for hint in &result {
            assert_eq!(hint.kind, Some(types::InlayHintKind::TYPE));
        }
    }

    #[test]
    fn test_command_substitution() {
        let script = r#"
result=$(echo hello)
nested=$(echo $(whoami))
"#;
        let snapshot = test_snapshot(script);
        let params = types::InlayHintParams {
            text_document: types::TextDocumentIdentifier {
                uri: types::Url::parse("file:///test.sh").unwrap(),
            },
            range: full_range(),
            work_done_progress_params: types::WorkDoneProgressParams::default(),
        };

        let result = inlay_hints(snapshot, params).unwrap().unwrap();
        let labels: Vec<_> = result
            .iter()
            .map(|h| match &h.label {
                types::InlayHintLabel::String(s) => s.as_str(),
                _ => panic!("expected string label"),
            })
            .collect();

        assert_eq!(labels, vec![": stdout", ": stdout", ": stdout"]);
    }

    #[test]
    fn test_combined_and_range_filtering() {
        let script = "export URL=${DEFAULT_URL:-http://localhost}\nmsg=$(hostname)\n";
        let snapshot = test_snapshot(script);

        // Request only the first line
        let params = types::InlayHintParams {
            text_document: types::TextDocumentIdentifier {
                uri: types::Url::parse("file:///test.sh").unwrap(),
            },
            range: types::Range {
                start: types::Position {
                    line: 0,
                    character: 0,
                },
                end: types::Position {
                    line: 0,
                    character: 50,
                },
            },
            work_done_progress_params: types::WorkDoneProgressParams::default(),
        };

        let result = inlay_hints(snapshot, params).unwrap().unwrap();
        let labels: Vec<_> = result
            .iter()
            .map(|h| match &h.label {
                types::InlayHintLabel::String(s) => s.as_str(),
                _ => panic!("expected string label"),
            })
            .collect();

        // First line has export URL (char 10) and ${DEFAULT_URL:-http://localhost} (char 26)
        assert_eq!(labels, vec![": export", ": default"]);
    }
}
