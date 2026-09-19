use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
};
use shucked_ast::{
    ArithmeticCommand, ArithmeticExpr, ArithmeticExprNode, ArithmeticForCommand, ArrayElem,
    AssignmentValue, BuiltinCommand, CaseCommand, Command, CompoundCommand, File, ForCommand,
    ForSyntax, IfCommand, IfSyntax, Name, Span, Stmt, StmtSeq, TextRange, UntilCommand,
    WhileCommand, Word, WordPart, WordPartNode,
};
use shucked_indexer::LineIndex;

use crate::edit::{PositionEncoding, offset_to_position};
use crate::session::DocumentSnapshot;

pub const SUPPORTED_TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::COMMENT,
    SemanticTokenType::TYPE,
    SemanticTokenType::new("shellCommand"),
    SemanticTokenType::new("shellAlias"),
];

pub const SUPPORTED_TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::DEFINITION,
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DEFAULT_LIBRARY,
    SemanticTokenModifier::new("invalid"),
];

pub(crate) const TOKEN_TYPE_KEYWORD: u32 = 0;
pub(crate) const TOKEN_TYPE_FUNCTION: u32 = 1;
pub(crate) const TOKEN_TYPE_VARIABLE: u32 = 2;
pub(crate) const TOKEN_TYPE_PARAMETER: u32 = 3;
pub(crate) const TOKEN_TYPE_STRING: u32 = 4;
pub(crate) const TOKEN_TYPE_NUMBER: u32 = 5;
pub(crate) const TOKEN_TYPE_COMMENT: u32 = 7;

pub(crate) const MODIFIER_DECLARATION: u32 = 1 << 0;
pub(crate) const MODIFIER_DEFINITION: u32 = 1 << 1;
pub(crate) const MODIFIER_READONLY: u32 = 1 << 2;

/// Returns the server's semantic tokens legend.
pub fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: SUPPORTED_TOKEN_TYPES.to_vec(),
        token_modifiers: SUPPORTED_TOKEN_MODIFIERS.to_vec(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenSpan {
    start: usize,
    end: usize,
    token_type: u32,
    modifiers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawSemanticToken {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

/// Computes full semantic tokens for the given document snapshot.
pub fn semantic_tokens_full(
    snapshot: DocumentSnapshot,
) -> crate::server::Result<Option<SemanticTokens>> {
    if crate::handlers::commands::dialect(&snapshot) == "fish" {
        return Ok(Some(super::semantic_tokens_fish::full(&snapshot)));
    }
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };

    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();
    let file = &analysis.parse_result().file;

    // 1. Collect specific non-string tokens
    let mut specific_tokens = Vec::new();

    // Comments
    for comment in analysis.indexer().comment_index().comments() {
        let start = usize::from(comment.range.start());
        let end = usize::from(comment.range.end());
        if start < end && end <= source.len() {
            specific_tokens.push(TokenSpan {
                start,
                end,
                token_type: TOKEN_TYPE_COMMENT,
                modifiers: 0,
            });
        }
    }

    // Function definitions
    for binding in analysis.semantic().function_definition_bindings() {
        let start = binding.span.start.offset();
        let end = binding.span.end.offset();
        if start < end && end <= source.len() {
            specific_tokens.push(TokenSpan {
                start,
                end,
                token_type: TOKEN_TYPE_FUNCTION,
                modifiers: MODIFIER_DEFINITION | MODIFIER_DECLARATION,
            });
        }
    }

    // The same resolution snapshot supplies diagnostics, hover and highlighting.
    let commands = snapshot.command_service.analysis(&snapshot);
    for (site, resolution) in &commands.sites {
        let (token_type, modifiers) = match resolution {
            shucked_command::CommandResolution::Missing(_) => (9, 1 << 4),
            shucked_command::CommandResolution::Resolved(command) => (
                if !site.aliases.is_empty() || !command.alias_chain.is_empty() {
                    10
                } else if command.kind == shucked_command::CommandKind::Function {
                    TOKEN_TYPE_FUNCTION
                } else {
                    9
                },
                if command.kind == shucked_command::CommandKind::Builtin {
                    1 << 3
                } else {
                    0
                },
            ),
            shucked_command::CommandResolution::Unknown(_) => (9, 0),
        };
        let span = site.name_span();
        if span.start.offset() < span.end.offset() && span.end.offset() <= source.len() {
            specific_tokens.push(TokenSpan {
                start: span.start.offset(),
                end: span.end.offset(),
                token_type,
                modifiers,
            });
        }
    }

    // Variable bindings
    for binding in analysis.semantic().bindings() {
        if matches!(
            binding.kind,
            shucked_semantic::BindingKind::FunctionDefinition
        ) {
            continue;
        }
        let start = binding.span.start.offset();
        let end = binding.span.end.offset();
        if start < end && end <= source.len() {
            let mut modifiers = MODIFIER_DECLARATION;
            if binding
                .attributes
                .contains(shucked_semantic::BindingAttributes::READONLY)
            {
                modifiers |= MODIFIER_READONLY;
            }
            specific_tokens.push(TokenSpan {
                start,
                end,
                token_type: TOKEN_TYPE_VARIABLE,
                modifiers,
            });
        }
    }

    // Variable references & positional parameters
    for reference in analysis.semantic().references() {
        let is_param = is_positional_param(&reference.name);
        let span_to_use = if reference.span.start.offset() < reference.span.end.offset()
            && reference.span.end.offset() <= source.len()
        {
            let text = reference.span.slice(source);
            if text.starts_with('$') && !text.starts_with("${") {
                reference.span
            } else {
                reference.name_span
            }
        } else {
            reference.name_span
        };

        let start = span_to_use.start.offset();
        let end = span_to_use.end.offset();
        if start < end && end <= source.len() {
            specific_tokens.push(TokenSpan {
                start,
                end,
                token_type: if is_param {
                    TOKEN_TYPE_PARAMETER
                } else {
                    TOKEN_TYPE_VARIABLE
                },
                modifiers: 0,
            });
        }
    }

    // AST visitor for keywords, numbers, and string regions
    let mut ast_collector = AstCollector::new(source);
    ast_collector.visit_file(file);

    // Merge AST-discovered specific tokens (keywords, numbers, single quotes)
    specific_tokens.extend(ast_collector.specific_tokens);

    // Include heredoc bodies from region index as string candidates
    let mut string_candidates = ast_collector.string_candidates;
    for heredoc_range in analysis.indexer().region_index().heredoc_ranges() {
        let start = usize::from(heredoc_range.start());
        let end = usize::from(heredoc_range.end());
        if start < end && end <= source.len() {
            string_candidates.push((start, end));
        }
    }

    // 2. Build non-string ranges to subtract from string candidates
    let non_string_ranges = specific_tokens
        .iter()
        .map(|t| (t.start, t.end))
        .collect::<Vec<_>>();

    // Subtract non-string tokens from string candidates to produce disjoint STRING tokens
    let mut all_tokens = specific_tokens;
    for (cand_start, cand_end) in string_candidates {
        let fragments = subtract_intervals(cand_start, cand_end, &non_string_ranges);
        for (f_start, f_end) in fragments {
            all_tokens.push(TokenSpan {
                start: f_start,
                end: f_end,
                token_type: TOKEN_TYPE_STRING,
                modifiers: 0,
            });
        }
    }

    // 3. Convert all TokenSpans to line-bounded RawSemanticTokens
    let mut raw_tokens = Vec::new();
    for token in all_tokens {
        split_span_to_raw_tokens(token, source, line_index, encoding, &mut raw_tokens);
    }

    // 4. Sort raw tokens by line, then start_char
    raw_tokens.sort_unstable_by(|a, b| {
        a.line
            .cmp(&b.line)
            .then_with(|| a.start_char.cmp(&b.start_char))
            .then_with(|| a.length.cmp(&b.length))
    });

    // 5. Deduplicate and ensure non-overlapping
    let mut sanitized: Vec<RawSemanticToken> = Vec::with_capacity(raw_tokens.len());
    for token in raw_tokens {
        if token.length == 0 {
            continue;
        }
        if let Some(last) = sanitized.last_mut()
            && last.line == token.line
        {
            let last_end = last.start_char + last.length;
            if token.start_char < last_end {
                // Skip or clamp if overlapping
                if token.start_char + token.length <= last_end {
                    continue;
                }
                let new_start = last_end;
                let new_len = (token.start_char + token.length).saturating_sub(new_start);
                if new_len == 0 {
                    continue;
                }
                sanitized.push(RawSemanticToken {
                    line: token.line,
                    start_char: new_start,
                    length: new_len,
                    token_type: token.token_type,
                    modifiers: token.modifiers,
                });
                continue;
            }
        }
        sanitized.push(token);
    }

    // 6. Delta encode according to LSP 3.16 specification
    let mut encoded = Vec::with_capacity(sanitized.len());
    let mut prev_line = 0;
    let mut prev_start = 0;

    for token in sanitized {
        let delta_line = token.line - prev_line;
        let delta_start = if delta_line == 0 {
            token.start_char - prev_start
        } else {
            token.start_char
        };

        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.modifiers,
        });

        prev_line = token.line;
        prev_start = token.start_char;
    }

    Ok(Some(SemanticTokens {
        result_id: None,
        data: encoded,
    }))
}

fn is_positional_param(name: &Name) -> bool {
    let s = name.as_str();
    matches!(
        s,
        "@" | "#" | "*" | "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    ) || (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

fn subtract_intervals(
    base_start: usize,
    base_end: usize,
    subtractions: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    if base_start >= base_end {
        return Vec::new();
    }

    let mut relevant = subtractions
        .iter()
        .copied()
        .filter_map(|(s, e)| {
            let s_clamped = s.max(base_start);
            let e_clamped = e.min(base_end);
            if s_clamped < e_clamped {
                Some((s_clamped, e_clamped))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if relevant.is_empty() {
        return vec![(base_start, base_end)];
    }

    relevant.sort_unstable_by_key(|&(s, _)| s);

    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(relevant.len());
    let mut curr_sub = relevant[0];
    for &(s, e) in &relevant[1..] {
        if s <= curr_sub.1 {
            curr_sub.1 = curr_sub.1.max(e);
        } else {
            merged.push(curr_sub);
            curr_sub = (s, e);
        }
    }
    merged.push(curr_sub);

    let mut result = Vec::new();
    let mut current = base_start;
    for (s, e) in merged {
        if current < s {
            result.push((current, s));
        }
        current = current.max(e);
    }
    if current < base_end {
        result.push((current, base_end));
    }
    result
}

fn split_span_to_raw_tokens(
    token: TokenSpan,
    text: &str,
    index: &LineIndex,
    encoding: PositionEncoding,
    output: &mut Vec<RawSemanticToken>,
) {
    if token.start >= token.end || token.end > text.len() {
        return;
    }

    let start_pos = offset_to_position(text, index, token.start, encoding);
    let end_pos = offset_to_position(text, index, token.end, encoding);

    if start_pos.line == end_pos.line {
        let length = end_pos.character.saturating_sub(start_pos.character);
        if length > 0 {
            output.push(RawSemanticToken {
                line: start_pos.line,
                start_char: start_pos.character,
                length,
                token_type: token.token_type,
                modifiers: token.modifiers,
            });
        }
        return;
    }

    // Multi-line span: split across lines
    for line_num in start_pos.line..=end_pos.line {
        let line_1based = (line_num + 1) as usize;
        let line_start_offset = index.line_start(line_1based).map(usize::from).unwrap_or(0);
        let line_range = index.line_range(line_1based, text);
        let line_end_offset = line_range
            .map(|r| usize::from(r.end()))
            .unwrap_or(text.len());

        let seg_start = if line_num == start_pos.line {
            token.start
        } else {
            line_start_offset
        };

        let seg_end = if line_num == end_pos.line {
            token.end
        } else {
            // Trim trailing \r or \n from line
            let mut end = line_end_offset;
            while end > seg_start
                && (text.as_bytes().get(end - 1) == Some(&b'\n')
                    || text.as_bytes().get(end - 1) == Some(&b'\r'))
            {
                end -= 1;
            }
            end
        };

        if seg_start < seg_end && seg_end <= text.len() {
            let p_start = offset_to_position(text, index, seg_start, encoding);
            let p_end = offset_to_position(text, index, seg_end, encoding);
            let length = p_end.character.saturating_sub(p_start.character);
            if length > 0 {
                output.push(RawSemanticToken {
                    line: p_start.line,
                    start_char: p_start.character,
                    length,
                    token_type: token.token_type,
                    modifiers: token.modifiers,
                });
            }
        }
    }
}

struct AstCollector<'a> {
    source: &'a str,
    specific_tokens: Vec<TokenSpan>,
    string_candidates: Vec<(usize, usize)>,
}

impl<'a> AstCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            specific_tokens: Vec::new(),
            string_candidates: Vec::new(),
        }
    }

    fn add_keyword_span(&mut self, span: Span) {
        let start = span.start.offset();
        let end = span.end.offset();
        if start < end && end <= self.source.len() {
            self.specific_tokens.push(TokenSpan {
                start,
                end,
                token_type: TOKEN_TYPE_KEYWORD,
                modifiers: 0,
            });
        }
    }

    fn add_keyword_at(&mut self, start_offset: usize, len: usize) {
        let end_offset = start_offset + len;
        if end_offset <= self.source.len() {
            self.specific_tokens.push(TokenSpan {
                start: start_offset,
                end: end_offset,
                token_type: TOKEN_TYPE_KEYWORD,
                modifiers: 0,
            });
        }
    }

    fn find_and_add_keyword(&mut self, start: usize, end: usize, kw: &str) {
        if let Some(range) = find_keyword_in_range(self.source, start..end, kw) {
            let s = usize::from(range.start());
            let e = usize::from(range.end());
            if s < e && e <= self.source.len() {
                self.specific_tokens.push(TokenSpan {
                    start: s,
                    end: e,
                    token_type: TOKEN_TYPE_KEYWORD,
                    modifiers: 0,
                });
            }
        }
    }

    fn visit_file(&mut self, file: &File) {
        self.visit_stmt_seq(&file.body);
    }

    fn visit_stmt_seq(&mut self, seq: &StmtSeq) {
        for stmt in seq.iter() {
            self.visit_stmt(stmt);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        self.visit_command(&stmt.command);
    }

    fn visit_command(&mut self, command: &Command) {
        match command {
            Command::Simple(cmd) => {
                let name_str = cmd.name.to_string();
                if matches!(
                    name_str.as_str(),
                    "return" | "exit" | "local" | "export" | "declare"
                ) {
                    self.add_keyword_span(cmd.name.span);
                } else {
                    self.visit_word(&cmd.name);
                }
                for arg in &cmd.args {
                    self.visit_word(arg);
                }
                for assign in &cmd.assignments {
                    self.visit_assignment_value(&assign.value);
                }
            }
            Command::Builtin(cmd) => match cmd {
                BuiltinCommand::Return(ret) => {
                    self.find_and_add_keyword(
                        ret.span.start.offset(),
                        ret.span.end.offset(),
                        "return",
                    );
                    if let Some(code) = &ret.code {
                        self.visit_word(code);
                    }
                    for extra in &ret.extra_args {
                        self.visit_word(extra);
                    }
                }
                BuiltinCommand::Exit(exit) => {
                    self.find_and_add_keyword(
                        exit.span.start.offset(),
                        exit.span.end.offset(),
                        "exit",
                    );
                    if let Some(code) = &exit.code {
                        self.visit_word(code);
                    }
                    for extra in &exit.extra_args {
                        self.visit_word(extra);
                    }
                }
                _ => {}
            },
            Command::Decl(cmd) => {
                self.add_keyword_span(cmd.variant_span);
                for operand in &cmd.operands {
                    match operand {
                        shucked_ast::DeclOperand::Flag(w)
                        | shucked_ast::DeclOperand::Dynamic(w) => {
                            self.visit_word(w);
                        }
                        shucked_ast::DeclOperand::Assignment(assign) => {
                            self.visit_assignment_value(&assign.value);
                        }
                        _ => {}
                    }
                }
            }
            Command::Binary(cmd) => {
                self.visit_stmt(&cmd.left);
                self.visit_stmt(&cmd.right);
            }
            Command::Compound(cmd) => self.visit_compound(cmd),
            Command::Function(func) => {
                if let Some(span) = func.header.function_keyword_span {
                    self.add_keyword_span(span);
                }
                self.visit_stmt(&func.body);
            }
            Command::AnonymousFunction(func) => {
                self.visit_stmt(&func.body);
            }
        }
    }

    fn visit_assignment_value(&mut self, value: &AssignmentValue) {
        match value {
            AssignmentValue::Scalar(w) => self.visit_word(w),
            AssignmentValue::Compound(arr) => {
                for elem in &arr.elements {
                    match elem {
                        ArrayElem::Sequential(w) => self.visit_word(w),
                        ArrayElem::Keyed { value, .. } | ArrayElem::KeyedAppend { value, .. } => {
                            self.visit_word(value);
                        }
                    }
                }
            }
        }
    }

    fn visit_compound(&mut self, compound: &CompoundCommand) {
        match compound {
            CompoundCommand::If(if_cmd) => self.visit_if(if_cmd),
            CompoundCommand::For(for_cmd) => self.visit_for(for_cmd),
            CompoundCommand::ArithmeticFor(afor_cmd) => self.visit_arithmetic_for(afor_cmd),
            CompoundCommand::While(while_cmd) => self.visit_while(while_cmd),
            CompoundCommand::Until(until_cmd) => self.visit_until(until_cmd),
            CompoundCommand::Case(case_cmd) => self.visit_case(case_cmd),
            CompoundCommand::Subshell(seq) | CompoundCommand::BraceGroup(seq) => {
                self.visit_stmt_seq(seq);
            }
            CompoundCommand::Arithmetic(arith) => self.visit_arithmetic_cmd(arith),
            _ => {}
        }
    }

    fn visit_if(&mut self, if_cmd: &IfCommand) {
        let cmd_start = if_cmd.span.start.offset();
        self.add_keyword_at(cmd_start, 2); // "if"

        self.visit_stmt_seq(&if_cmd.condition);

        match if_cmd.syntax {
            IfSyntax::ThenFi { then_span, fi_span } => {
                self.add_keyword_span(then_span);
                self.add_keyword_span(fi_span);
            }
            IfSyntax::Brace { .. } => {}
        }

        self.visit_stmt_seq(&if_cmd.then_branch);

        let mut prev_branch_end = if_cmd.then_branch.span.end.offset();
        for (elif_cond, elif_body) in &if_cmd.elif_branches {
            let cond_start = elif_cond.span.start.offset();
            self.find_and_add_keyword(prev_branch_end, cond_start, "elif");
            self.visit_stmt_seq(elif_cond);

            let cond_end = elif_cond.span.end.offset();
            let body_start = elif_body.span.start.offset();
            self.find_and_add_keyword(cond_end, body_start, "then");
            self.visit_stmt_seq(elif_body);

            prev_branch_end = elif_body.span.end.offset();
        }

        if let Some(else_branch) = &if_cmd.else_branch {
            let else_start = else_branch.span.start.offset();
            self.find_and_add_keyword(prev_branch_end, else_start, "else");
            self.visit_stmt_seq(else_branch);
        }
    }

    fn visit_for(&mut self, for_cmd: &ForCommand) {
        let cmd_start = for_cmd.span.start.offset();
        self.add_keyword_at(cmd_start, 3); // "for"

        match for_cmd.syntax {
            ForSyntax::InDoDone {
                in_span,
                do_span,
                done_span,
            } => {
                if let Some(span) = in_span {
                    self.add_keyword_span(span);
                }
                self.add_keyword_span(do_span);
                self.add_keyword_span(done_span);
            }
            ForSyntax::ParenDoDone {
                do_span, done_span, ..
            } => {
                self.add_keyword_span(do_span);
                self.add_keyword_span(done_span);
            }
            ForSyntax::InDirect { in_span } | ForSyntax::InBrace { in_span, .. } => {
                if let Some(span) = in_span {
                    self.add_keyword_span(span);
                }
            }
            _ => {}
        }

        if let Some(words) = &for_cmd.words {
            for word in words {
                self.visit_word(word);
            }
        }
        self.visit_stmt_seq(&for_cmd.body);
    }

    fn visit_arithmetic_for(&mut self, afor_cmd: &ArithmeticForCommand) {
        let cmd_start = afor_cmd.span.start.offset();
        self.add_keyword_at(cmd_start, 3); // "for"

        if let Some(init) = &afor_cmd.init_ast {
            self.visit_arithmetic_node(init);
        }
        if let Some(cond) = &afor_cmd.condition_ast {
            self.visit_arithmetic_node(cond);
        }
        if let Some(step) = &afor_cmd.step_ast {
            self.visit_arithmetic_node(step);
        }

        let rparen_end = afor_cmd.right_paren_span.end.offset();
        let body_start = afor_cmd.body.span.start.offset();
        self.find_and_add_keyword(rparen_end, body_start, "do");

        if let Some(done_span) = afor_cmd.done_span {
            self.add_keyword_span(done_span);
        } else {
            let body_end = afor_cmd.body.span.end.offset();
            let cmd_end = afor_cmd.span.end.offset();
            self.find_and_add_keyword(body_end, cmd_end, "done");
        }

        self.visit_stmt_seq(&afor_cmd.body);
    }

    fn visit_while(&mut self, while_cmd: &WhileCommand) {
        let cmd_start = while_cmd.span.start.offset();
        self.add_keyword_at(cmd_start, 5); // "while"

        self.visit_stmt_seq(&while_cmd.condition);

        let cond_end = while_cmd.condition.span.end.offset();
        let body_start = while_cmd.body.span.start.offset();
        self.find_and_add_keyword(cond_end, body_start, "do");

        if let Some(done_span) = while_cmd.done_span {
            self.add_keyword_span(done_span);
        } else {
            let body_end = while_cmd.body.span.end.offset();
            let cmd_end = while_cmd.span.end.offset();
            self.find_and_add_keyword(body_end, cmd_end, "done");
        }

        self.visit_stmt_seq(&while_cmd.body);
    }

    fn visit_until(&mut self, until_cmd: &UntilCommand) {
        let cmd_start = until_cmd.span.start.offset();
        self.add_keyword_at(cmd_start, 5); // "until"

        self.visit_stmt_seq(&until_cmd.condition);

        let cond_end = until_cmd.condition.span.end.offset();
        let body_start = until_cmd.body.span.start.offset();
        self.find_and_add_keyword(cond_end, body_start, "do");

        if let Some(done_span) = until_cmd.done_span {
            self.add_keyword_span(done_span);
        } else {
            let body_end = until_cmd.body.span.end.offset();
            let cmd_end = until_cmd.span.end.offset();
            self.find_and_add_keyword(body_end, cmd_end, "done");
        }

        self.visit_stmt_seq(&until_cmd.body);
    }

    fn visit_case(&mut self, case_cmd: &CaseCommand) {
        let cmd_start = case_cmd.span.start.offset();
        self.add_keyword_at(cmd_start, 4); // "case"

        self.visit_word(&case_cmd.word);

        let word_end = case_cmd.word.span.end.offset();
        let first_pattern_start = case_cmd
            .cases
            .first()
            .and_then(|c| c.patterns.first())
            .map(|p| p.span.start.offset())
            .unwrap_or(case_cmd.esac_span.start.offset());
        self.find_and_add_keyword(word_end, first_pattern_start, "in");

        for item in &case_cmd.cases {
            for pattern in &item.patterns {
                for part in &pattern.parts {
                    if let shucked_ast::PatternPart::Word(w) = &part.kind {
                        self.visit_word(w);
                    }
                }
            }
            self.visit_stmt_seq(&item.body);
        }

        self.add_keyword_span(case_cmd.esac_span);
    }

    fn visit_arithmetic_cmd(&mut self, arith: &ArithmeticCommand) {
        if let Some(expr) = &arith.expr_ast {
            self.visit_arithmetic_node(expr);
        }
    }

    fn visit_arithmetic_node(&mut self, node: &ArithmeticExprNode) {
        match &node.kind {
            ArithmeticExpr::Number(_) => {
                let start = node.span.start.offset();
                let end = node.span.end.offset();
                if start < end && end <= self.source.len() {
                    self.specific_tokens.push(TokenSpan {
                        start,
                        end,
                        token_type: TOKEN_TYPE_NUMBER,
                        modifiers: 0,
                    });
                }
            }
            ArithmeticExpr::Unary { expr, .. }
            | ArithmeticExpr::Postfix { expr, .. }
            | ArithmeticExpr::Parenthesized { expression: expr } => {
                self.visit_arithmetic_node(expr);
            }
            ArithmeticExpr::Binary { left, right, .. } => {
                self.visit_arithmetic_node(left);
                self.visit_arithmetic_node(right);
            }
            ArithmeticExpr::Conditional {
                condition,
                then_expr,
                else_expr,
            } => {
                self.visit_arithmetic_node(condition);
                self.visit_arithmetic_node(then_expr);
                self.visit_arithmetic_node(else_expr);
            }
            ArithmeticExpr::Assignment { value, .. } => {
                self.visit_arithmetic_node(value);
            }
            ArithmeticExpr::ShellWord(word) => {
                self.visit_word(word);
            }
            _ => {}
        }
    }

    fn visit_word(&mut self, word: &Word) {
        for part in &word.parts {
            self.visit_word_part(part);
        }
    }

    fn visit_word_part(&mut self, part: &WordPartNode) {
        match &part.kind {
            WordPart::SingleQuoted { .. } => {
                let start = part.span.start.offset();
                let end = part.span.end.offset();
                if start < end && end <= self.source.len() {
                    self.specific_tokens.push(TokenSpan {
                        start,
                        end,
                        token_type: TOKEN_TYPE_STRING,
                        modifiers: 0,
                    });
                }
            }
            WordPart::DoubleQuoted { parts, .. } => {
                let start = part.span.start.offset();
                let end = part.span.end.offset();
                if start < end && end <= self.source.len() {
                    self.string_candidates.push((start, end));
                }
                for inner_part in parts {
                    self.visit_word_part(inner_part);
                }
            }
            WordPart::CommandSubstitution { body, .. } => {
                self.visit_stmt_seq(body);
            }
            WordPart::ArithmeticExpansion {
                expression_ast,
                expression_word_ast,
                ..
            } => {
                if let Some(ast) = expression_ast {
                    self.visit_arithmetic_node(ast);
                } else {
                    self.visit_word(expression_word_ast);
                }
            }
            _ => {}
        }
    }
}

fn find_keyword_in_range(
    source: &str,
    range: std::ops::Range<usize>,
    kw: &str,
) -> Option<TextRange> {
    if range.start >= range.end || range.end > source.len() {
        return None;
    }
    let slice = &source[range.clone()];
    let mut search_idx = 0;
    while let Some(pos) = slice[search_idx..].find(kw) {
        let actual_pos = search_idx + pos;
        let before_ok = actual_pos == 0
            || (!slice.as_bytes()[actual_pos - 1].is_ascii_alphanumeric()
                && slice.as_bytes()[actual_pos - 1] != b'_');
        let after_idx = actual_pos + kw.len();
        let after_ok = after_idx == slice.len()
            || (!slice.as_bytes()[after_idx].is_ascii_alphanumeric()
                && slice.as_bytes()[after_idx] != b'_');
        if before_ok && after_ok {
            let start = shucked_ast::TextSize::from((range.start + actual_pos) as u32);
            let end = shucked_ast::TextSize::from((range.start + after_idx) as u32);
            return Some(TextRange::new(start, end));
        }
        search_idx = actual_pos + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::TextDocument;
    use crate::session::{Client, GlobalOptions, Session, Workspace, Workspaces};
    use lsp_types::Url;

    fn fixture_snapshot(source: &str) -> DocumentSnapshot {
        let (main_loop_sender, _main_loop_receiver) = crossbeam::channel::unbounded();
        let (client_sender, _client_receiver) = crossbeam::channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let temp_dir = std::env::temp_dir();
        let workspaces = Workspaces::new(vec![Workspace::default(
            Url::from_file_path(&temp_dir)
                .expect("temporary directory should convert to a file URL"),
        )]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &lsp_types::ClientCapabilities::default(),
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");

        let uri = Url::from_file_path(temp_dir.join("test_semantic_tokens.sh")).unwrap();
        session.open_text_document(
            uri.clone(),
            TextDocument::new(source.to_owned(), 1).with_language_id("shellscript"),
        );

        session
            .take_snapshot(uri)
            .expect("test document should produce a snapshot")
    }

    #[test]
    fn test_legend_structure() {
        let legend = semantic_tokens_legend();
        assert_eq!(legend.token_types.len(), 11);
        assert_eq!(legend.token_types[0], SemanticTokenType::KEYWORD);
        assert_eq!(legend.token_types[1], SemanticTokenType::FUNCTION);
        assert_eq!(legend.token_types[2], SemanticTokenType::VARIABLE);
        assert_eq!(legend.token_types[3], SemanticTokenType::PARAMETER);
        assert_eq!(legend.token_types[4], SemanticTokenType::STRING);
        assert_eq!(legend.token_types[5], SemanticTokenType::NUMBER);
        assert_eq!(legend.token_types[6], SemanticTokenType::OPERATOR);
        assert_eq!(legend.token_types[7], SemanticTokenType::COMMENT);
        assert_eq!(legend.token_types[8], SemanticTokenType::TYPE);

        assert_eq!(legend.token_modifiers.len(), 5);
        assert_eq!(
            legend.token_modifiers[0],
            SemanticTokenModifier::DECLARATION
        );
        assert_eq!(legend.token_modifiers[1], SemanticTokenModifier::DEFINITION);
        assert_eq!(legend.token_modifiers[2], SemanticTokenModifier::READONLY);
        assert_eq!(
            legend.token_modifiers[3],
            SemanticTokenModifier::DEFAULT_LIBRARY
        );
    }

    #[test]
    fn test_subtract_intervals() {
        // [5, 20] with [8, 12] subtracted should yield [5, 8] and [12, 20]
        let res = subtract_intervals(5, 20, &[(8, 12)]);
        assert_eq!(res, vec![(5, 8), (12, 20)]);

        // Overlapping subtractions [8, 12] and [10, 15]
        let res = subtract_intervals(5, 20, &[(8, 12), (10, 15)]);
        assert_eq!(res, vec![(5, 8), (15, 20)]);

        // No subtractions
        let res = subtract_intervals(5, 20, &[]);
        assert_eq!(res, vec![(5, 20)]);

        // Complete coverage
        let res = subtract_intervals(5, 20, &[(0, 25)]);
        assert_eq!(res, Vec::<(usize, usize)>::new());
    }

    #[test]
    fn test_semantic_tokens_keywords_and_comments() {
        let script = "#!/bin/bash\n# A comment\nif true; then\n  echo \"hi\"\nfi\n";
        let snapshot = fixture_snapshot(script);
        let tokens = semantic_tokens_full(snapshot).unwrap().unwrap();
        assert!(!tokens.data.is_empty());

        // Decode tokens into absolute positions and verify types
        let mut absolute_tokens = Vec::new();
        let mut current_line = 0;
        let mut current_char = 0;
        for t in &tokens.data {
            current_line += t.delta_line;
            if t.delta_line == 0 {
                current_char += t.delta_start;
            } else {
                current_char = t.delta_start;
            }
            absolute_tokens.push((current_line, current_char, t.length, t.token_type));
        }

        // Line 0: #!/bin/bash (comment)
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, char_idx, len, ty)| line == 0
                    && char_idx == 0
                    && len == 11
                    && ty == TOKEN_TYPE_COMMENT)
        );

        // Line 1: # A comment
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, char_idx, len, ty)| line == 1
                    && char_idx == 0
                    && len == 11
                    && ty == TOKEN_TYPE_COMMENT)
        );

        // Line 2: if and then
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, char_idx, len, ty)| line == 2
                    && char_idx == 0
                    && len == 2
                    && ty == TOKEN_TYPE_KEYWORD)
        );
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, len, ty)| line == 2 && len == 4 && ty == TOKEN_TYPE_KEYWORD)
        );

        // Line 4: fi
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, char_idx, len, ty)| line == 4
                    && char_idx == 0
                    && len == 2
                    && ty == TOKEN_TYPE_KEYWORD)
        );
    }

    #[test]
    fn test_semantic_tokens_functions_and_variables() {
        let script = "greet() {\n  local name=$1\n  echo \"hello $name\"\n}\ngreet \"world\"\n";
        let snapshot = fixture_snapshot(script);
        let tokens = semantic_tokens_full(snapshot).unwrap().unwrap();
        assert!(!tokens.data.is_empty());

        let mut absolute_tokens = Vec::new();
        let mut current_line = 0;
        let mut current_char = 0;
        for t in &tokens.data {
            current_line += t.delta_line;
            if t.delta_line == 0 {
                current_char += t.delta_start;
            } else {
                current_char = t.delta_start;
            }
            absolute_tokens.push((
                current_line,
                current_char,
                t.length,
                t.token_type,
                t.token_modifiers_bitset,
            ));
        }

        // Line 0: greet() function definition
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, char_idx, len, ty, m)| line == 0
                    && char_idx == 0
                    && len == 5
                    && ty == TOKEN_TYPE_FUNCTION
                    && (m & MODIFIER_DEFINITION != 0))
        );

        // Line 1: local (keyword), name (variable), $1 (parameter)
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, len, ty, _)| line == 1 && len == 5 && ty == TOKEN_TYPE_KEYWORD)
        ); // local
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, len, ty, m)| line == 1
                    && len == 4
                    && ty == TOKEN_TYPE_VARIABLE
                    && (m & MODIFIER_DECLARATION != 0))
        ); // name
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, len, ty, _)| line == 1 && len == 2 && ty == TOKEN_TYPE_PARAMETER)
        ); // $1

        // Line 2: "hello " (string), $name (variable), """ (string)
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, _, ty, _)| line == 2 && ty == TOKEN_TYPE_VARIABLE)
        );

        // Line 4: greet (function call)
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, char_idx, len, ty, m)| line == 4
                    && char_idx == 0
                    && len == 5
                    && ty == TOKEN_TYPE_FUNCTION
                    && m == 0)
        );
    }

    #[test]
    fn test_semantic_tokens_arithmetic() {
        let script = "val=$(( 40 + 2 ))\n";
        let snapshot = fixture_snapshot(script);
        let tokens = semantic_tokens_full(snapshot).unwrap().unwrap();
        assert!(!tokens.data.is_empty());

        let mut absolute_tokens = Vec::new();
        let mut current_line = 0;
        let mut current_char = 0;
        for t in &tokens.data {
            current_line += t.delta_line;
            if t.delta_line == 0 {
                current_char += t.delta_start;
            } else {
                current_char = t.delta_start;
            }
            absolute_tokens.push((current_line, current_char, t.length, t.token_type));
        }

        // 40 and 2 should be numbers
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, len, ty)| line == 0 && len == 2 && ty == TOKEN_TYPE_NUMBER)
        );
        assert!(
            absolute_tokens
                .iter()
                .any(|&(line, _, len, ty)| line == 0 && len == 1 && ty == TOKEN_TYPE_NUMBER)
        );
    }
}
