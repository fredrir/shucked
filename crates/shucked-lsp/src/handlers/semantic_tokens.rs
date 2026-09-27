//! Semantic tokens for Bourne-family shell documents.
//!
//! Token candidates come from four sources: the comment index, the semantic
//! model (bindings and references), command-site resolution, and a syntax walk
//! over the AST. Every candidate carries a priority, and overlapping candidates
//! are resolved deterministically so that the more specific one wins and the
//! less specific one keeps only the uncovered remainder.

use std::collections::BTreeMap;

use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
};
use shucked_ast::{
    AnonymousFunctionSurface, ArithmeticCommand, ArithmeticExpansionSyntax, ArithmeticExpr,
    ArithmeticExprNode, ArithmeticForCommand, ArithmeticLvalue, ArrayElem, Assignment,
    AssignmentValue, BourneParameterExpansion, BuiltinCommand, CaseCommand, Command,
    CommandSubstitutionSyntax, CompoundCommand, ConditionalExpr, DeclClause, DeclOperand, File,
    ForCommand, ForSyntax, ForeachSyntax, Heredoc, HeredocBodyPart, IfCommand, IfSyntax, Name,
    ParameterExpansion, ParameterExpansionSyntax, ParameterOp, Pattern, PatternPart, Redirect,
    RedirectTarget, RepeatSyntax, Span, Stmt, StmtSeq, Subscript, VarRef, Word, WordPart,
    WordPartNode, ZshExpansionOperation, ZshExpansionTarget, ZshGlobSegment,
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
    SemanticTokenType::new("shellOption"),
];

pub const SUPPORTED_TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::DEFINITION,
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DEFAULT_LIBRARY,
    SemanticTokenModifier::new("invalid"),
    SemanticTokenModifier::STATIC,
];

// Indexes into `SUPPORTED_TOKEN_TYPES`.
pub(crate) const TOKEN_TYPE_KEYWORD: u32 = 0;
pub(crate) const TOKEN_TYPE_FUNCTION: u32 = 1;
pub(crate) const TOKEN_TYPE_VARIABLE: u32 = 2;
pub(crate) const TOKEN_TYPE_PARAMETER: u32 = 3;
pub(crate) const TOKEN_TYPE_STRING: u32 = 4;
pub(crate) const TOKEN_TYPE_NUMBER: u32 = 5;
pub(crate) const TOKEN_TYPE_OPERATOR: u32 = 6;
pub(crate) const TOKEN_TYPE_COMMENT: u32 = 7;
#[allow(dead_code)]
pub(crate) const TOKEN_TYPE_TYPE: u32 = 8;
pub(crate) const TOKEN_TYPE_SHELL_COMMAND: u32 = 9;
pub(crate) const TOKEN_TYPE_SHELL_ALIAS: u32 = 10;
pub(crate) const TOKEN_TYPE_SHELL_OPTION: u32 = 11;

// Bits into `SUPPORTED_TOKEN_MODIFIERS`.
pub(crate) const MODIFIER_DECLARATION: u32 = 1 << 0;
pub(crate) const MODIFIER_DEFINITION: u32 = 1 << 1;
pub(crate) const MODIFIER_READONLY: u32 = 1 << 2;
pub(crate) const MODIFIER_DEFAULT_LIBRARY: u32 = 1 << 3;
pub(crate) const MODIFIER_INVALID: u32 = 1 << 4;
pub(crate) const MODIFIER_STATIC: u32 = 1 << 5;

// Overlap priorities: a higher value claims its range first, and lower-priority
// candidates keep only whatever remains uncovered.
/// Literal text of double-quoted strings and heredoc bodies.
pub(crate) const PRIORITY_STRING: u8 = 0;
/// Keywords, operators, numbers, options, and fallback names from the syntax walk.
pub(crate) const PRIORITY_SYNTAX: u8 = 1;
/// Variable bindings and references from the semantic model.
pub(crate) const PRIORITY_NAME: u8 = 2;
/// Special parameters and names whose role is fixed by their command (aliases, autoload).
pub(crate) const PRIORITY_SPECIAL_NAME: u8 = 3;
/// Resolved command names.
pub(crate) const PRIORITY_COMMAND: u8 = 4;
/// Function definitions and reserved words that must beat command resolution.
pub(crate) const PRIORITY_KEYWORD: u8 = 5;
/// Punctuation that outranks command resolution, such as the `[` test bracket.
pub(crate) const PRIORITY_PUNCTUATION: u8 = 6;
/// Comments are never subdivided by anything else.
pub(crate) const PRIORITY_COMMENT: u8 = 7;

/// Returns the server's semantic tokens legend.
pub fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: SUPPORTED_TOKEN_TYPES.to_vec(),
        token_modifiers: SUPPORTED_TOKEN_MODIFIERS.to_vec(),
    }
}

/// A byte-offset token candidate before overlap resolution and line splitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) token_type: u32,
    pub(crate) modifiers: u32,
    pub(crate) priority: u8,
}

impl TokenSpan {
    pub(crate) const fn new(
        start: usize,
        end: usize,
        token_type: u32,
        modifiers: u32,
        priority: u8,
    ) -> Self {
        Self {
            start,
            end,
            token_type,
            modifiers,
            priority,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RawSemanticToken {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

/// Tokenises the whole document. Requests go through
/// [`super::semantic_tokens_cache::SemanticTokensCache`], which memoises this per document
/// state and assigns the result id; the tokens returned here carry none.
pub fn semantic_tokens_full(
    snapshot: &DocumentSnapshot,
) -> crate::server::Result<Option<SemanticTokens>> {
    if crate::handlers::commands::dialect(snapshot) == "fish" {
        return Ok(Some(super::semantic_tokens_fish::full(snapshot)));
    }
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };

    let source = analysis.source();
    let line_index = analysis.line_index();
    let encoding = snapshot.encoding();
    let file = &analysis.parse_result().file;

    let mut tokens = Vec::new();

    for comment in analysis.indexer().comment_index().comments() {
        push_token(
            &mut tokens,
            source,
            usize::from(comment.range.start()),
            usize::from(comment.range.end()),
            TOKEN_TYPE_COMMENT,
            0,
            PRIORITY_COMMENT,
        );
    }

    for binding in analysis.semantic().function_definition_bindings() {
        push_token(
            &mut tokens,
            source,
            binding.span.start.offset(),
            binding.span.end.offset(),
            TOKEN_TYPE_FUNCTION,
            MODIFIER_DEFINITION | MODIFIER_DECLARATION,
            PRIORITY_KEYWORD,
        );
    }

    // The same resolution snapshot supplies diagnostics, hover and highlighting.
    let commands = snapshot.command_service.analysis(snapshot);
    for (site, resolution) in &commands.sites {
        let span = site.name_span();
        for word in &site.words {
            if word.injected || word.span.start.offset() >= span.start.offset() {
                continue;
            }
            if word
                .text
                .as_deref()
                .is_some_and(|text| matches!(text, "command" | "builtin" | "exec" | "env"))
            {
                push_token(
                    &mut tokens,
                    source,
                    word.span.start.offset(),
                    word.span.end.offset(),
                    TOKEN_TYPE_KEYWORD,
                    MODIFIER_DEFAULT_LIBRARY,
                    PRIORITY_KEYWORD,
                );
            }
        }
        // A wrapper such as `env -i ls` can leave an option or assignment as the
        // effective name; those words are never commands.
        if site
            .name()
            .is_some_and(|name| name.starts_with(['-', '+']) || name.contains('='))
        {
            continue;
        }
        let (token_type, modifiers) = command_site_token(site, resolution);
        push_token(
            &mut tokens,
            source,
            span.start.offset(),
            span.end.offset(),
            token_type,
            modifiers,
            PRIORITY_COMMAND,
        );
    }

    for binding in analysis.semantic().bindings() {
        if matches!(
            binding.kind,
            shucked_semantic::BindingKind::FunctionDefinition
        ) {
            continue;
        }
        let mut modifiers = MODIFIER_DECLARATION;
        if binding
            .attributes
            .contains(shucked_semantic::BindingAttributes::READONLY)
        {
            modifiers |= MODIFIER_READONLY;
        }
        if binding
            .attributes
            .contains(shucked_semantic::BindingAttributes::EXPORTED)
        {
            modifiers |= MODIFIER_STATIC;
        }
        push_token(
            &mut tokens,
            source,
            binding.span.start.offset(),
            binding.span.end.offset(),
            TOKEN_TYPE_VARIABLE,
            modifiers,
            PRIORITY_NAME,
        );
    }

    for reference in analysis.semantic().references() {
        let (token_type, modifiers, priority) = name_token(&reference.name);
        let use_full_span = slice(
            source,
            reference.span.start.offset(),
            reference.span.end.offset(),
        )
        .is_some_and(|text| text.starts_with('$') && !text.starts_with("${"));
        let span = if use_full_span {
            reference.span
        } else {
            reference.name_span
        };
        push_token(
            &mut tokens,
            source,
            span.start.offset(),
            span.end.offset(),
            token_type,
            modifiers,
            priority.max(PRIORITY_NAME),
        );
    }

    let mut collector = AstCollector::new(source);
    collector.visit_file(file);

    // Heredoc bodies whose AST text is not source-backed (for example after
    // `<<-` tab stripping) still get their literal text from the region index.
    for range in analysis.indexer().region_index().heredoc_ranges() {
        let (start, end) = (usize::from(range.start()), usize::from(range.end()));
        if collector
            .painted_heredocs
            .iter()
            .any(|&(body_start, body_end)| body_start < end && start < body_end)
        {
            continue;
        }
        push_token(
            &mut tokens,
            source,
            start,
            end,
            TOKEN_TYPE_STRING,
            0,
            PRIORITY_STRING,
        );
    }
    tokens.extend(collector.tokens);

    Ok(Some(encode_tokens(tokens, source, line_index, encoding)))
}

fn command_site_token(
    site: &shucked_semantic::CommandSiteFacts,
    resolution: &shucked_command::CommandResolution,
) -> (u32, u32) {
    match resolution {
        shucked_command::CommandResolution::Missing(_) => {
            let token_type = if site.aliases.is_empty() {
                TOKEN_TYPE_SHELL_COMMAND
            } else {
                TOKEN_TYPE_SHELL_ALIAS
            };
            (token_type, MODIFIER_INVALID)
        }
        shucked_command::CommandResolution::Resolved(command) => {
            let token_type = if !site.aliases.is_empty() || !command.alias_chain.is_empty() {
                TOKEN_TYPE_SHELL_ALIAS
            } else if command.kind == shucked_command::CommandKind::Function {
                TOKEN_TYPE_FUNCTION
            } else {
                TOKEN_TYPE_SHELL_COMMAND
            };
            let modifiers = if command.kind == shucked_command::CommandKind::Builtin {
                MODIFIER_DEFAULT_LIBRARY
            } else {
                0
            };
            (token_type, modifiers)
        }
        shucked_command::CommandResolution::Unknown(_) => {
            let token_type = if site.aliases.is_empty() {
                TOKEN_TYPE_SHELL_COMMAND
            } else {
                TOKEN_TYPE_SHELL_ALIAS
            };
            (token_type, 0)
        }
    }
}

/// Classifies a shell name used in an expansion: special parameters, positional
/// parameters, or ordinary variables.
fn name_token(name: &Name) -> (u32, u32, u8) {
    let text = name.as_str();
    if is_special_parameter(text) {
        (
            TOKEN_TYPE_PARAMETER,
            MODIFIER_DEFAULT_LIBRARY,
            PRIORITY_SPECIAL_NAME,
        )
    } else if is_positional_parameter(text) {
        (TOKEN_TYPE_PARAMETER, 0, PRIORITY_SYNTAX)
    } else {
        (TOKEN_TYPE_VARIABLE, 0, PRIORITY_SYNTAX)
    }
}

fn is_special_parameter(name: &str) -> bool {
    matches!(name, "?" | "$" | "!" | "-" | "_" | "0" | "#" | "@" | "*")
}

fn is_positional_parameter(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn slice(source: &str, start: usize, end: usize) -> Option<&str> {
    (start < end && end <= source.len())
        .then(|| source.get(start..end))
        .flatten()
}

fn push_token(
    tokens: &mut Vec<TokenSpan>,
    source: &str,
    start: usize,
    end: usize,
    token_type: u32,
    modifiers: u32,
    priority: u8,
) {
    if start < end
        && end <= source.len()
        && source.is_char_boundary(start)
        && source.is_char_boundary(end)
    {
        tokens.push(TokenSpan::new(start, end, token_type, modifiers, priority));
    }
}

/// Resolves overlaps, splits multi-line spans, and delta-encodes the result.
pub(crate) fn encode_tokens(
    tokens: Vec<TokenSpan>,
    source: &str,
    line_index: &LineIndex,
    encoding: PositionEncoding,
) -> SemanticTokens {
    let resolved = resolve_overlaps(tokens);

    let mut raw_tokens = Vec::with_capacity(resolved.len());
    for token in resolved {
        split_span_to_raw_tokens(token, source, line_index, encoding, &mut raw_tokens);
    }
    raw_tokens.sort_unstable_by_key(|token| (token.line, token.start_char, token.length));

    let mut encoded = Vec::with_capacity(raw_tokens.len());
    let mut prev_line = 0;
    let mut prev_start = 0;
    let mut prev_end = 0;
    for token in raw_tokens {
        if token.length == 0 || (token.line == prev_line && token.start_char < prev_end) {
            continue;
        }
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
        prev_end = token.start_char + token.length;
    }

    SemanticTokens {
        result_id: None,
        data: encoded,
    }
}

/// Claims ranges in priority order; each candidate keeps only the fragments no
/// higher-priority candidate already covers. Ties are broken by position, type
/// and modifiers so the output never depends on collection order.
pub(crate) fn resolve_overlaps(mut tokens: Vec<TokenSpan>) -> Vec<TokenSpan> {
    tokens.retain(|token| token.start < token.end);
    tokens.sort_unstable_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.start.cmp(&b.start))
            .then_with(|| b.end.cmp(&a.end))
            .then_with(|| a.token_type.cmp(&b.token_type))
            .then_with(|| a.modifiers.cmp(&b.modifiers))
    });
    tokens.dedup();

    let mut claimed: BTreeMap<usize, usize> = BTreeMap::new();
    let mut resolved = Vec::with_capacity(tokens.len());
    let mut fragments = Vec::new();
    for token in tokens {
        fragments.clear();
        let mut cursor = token.start;
        let overlapping = claimed
            .range(..token.start)
            .next_back()
            .filter(|(_, end)| **end > token.start)
            .map(|(&start, &end)| (start, end))
            .into_iter()
            .chain(
                claimed
                    .range(token.start..token.end)
                    .map(|(&start, &end)| (start, end)),
            );
        for (start, end) in overlapping {
            if start > cursor {
                fragments.push((cursor, start));
            }
            cursor = cursor.max(end);
        }
        if cursor < token.end {
            fragments.push((cursor, token.end));
        }
        for &(start, end) in &fragments {
            claimed.insert(start, end);
            resolved.push(TokenSpan {
                start,
                end,
                ..token
            });
        }
    }
    resolved.sort_unstable_by_key(|token| (token.start, token.end));
    resolved
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

    for line_num in start_pos.line..=end_pos.line {
        let line_1based = (line_num + 1) as usize;
        let line_start_offset = index.line_start(line_1based).map(usize::from).unwrap_or(0);
        let line_end_offset = index
            .line_range(line_1based, text)
            .map(|range| usize::from(range.end()))
            .unwrap_or(text.len());

        let seg_start = if line_num == start_pos.line {
            token.start
        } else {
            line_start_offset
        };
        let seg_end = if line_num == end_pos.line {
            token.end
        } else {
            let mut end = line_end_offset;
            while end > seg_start && matches!(text.as_bytes().get(end - 1), Some(b'\n' | b'\r')) {
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

/// Where a word appears, which decides whether options and numbers apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WordRole {
    /// A command argument: `-flags` are options and bare numbers are numbers.
    Argument,
    /// An operand such as an assignment value or loop word: bare numbers only.
    Value,
    /// Anything else: only nested expansions and quotes are painted.
    Plain,
}

/// Names that make a simple command's arguments name aliases, functions, or options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgumentNaming {
    None,
    AliasDefinition,
    AliasLookup,
    FunctionDeclaration,
    FunctionLookup,
    ShellOption,
    TestExpression,
}

struct AstCollector<'a> {
    source: &'a str,
    tokens: Vec<TokenSpan>,
    /// Heredoc bodies whose literal text the walk painted from the AST.
    painted_heredocs: Vec<(usize, usize)>,
}

impl<'a> AstCollector<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            tokens: Vec::new(),
            painted_heredocs: Vec::new(),
        }
    }

    fn push(&mut self, start: usize, end: usize, token_type: u32, modifiers: u32, priority: u8) {
        push_token(
            &mut self.tokens,
            self.source,
            start,
            end,
            token_type,
            modifiers,
            priority,
        );
    }

    fn push_span(&mut self, span: Span, token_type: u32, modifiers: u32, priority: u8) {
        self.push(
            span.start.offset(),
            span.end.offset(),
            token_type,
            modifiers,
            priority,
        );
    }

    fn keyword(&mut self, span: Span) {
        self.push_span(span, TOKEN_TYPE_KEYWORD, 0, PRIORITY_KEYWORD);
    }

    fn operator(&mut self, span: Span) {
        self.push_span(span, TOKEN_TYPE_OPERATOR, 0, PRIORITY_SYNTAX);
    }

    fn operator_at(&mut self, start: usize, end: usize) {
        self.push(start, end, TOKEN_TYPE_OPERATOR, 0, PRIORITY_SYNTAX);
    }

    fn text(&self, start: usize, end: usize) -> Option<&'a str> {
        slice(self.source, start, end)
    }

    fn span_text(&self, span: Span) -> Option<&'a str> {
        self.text(span.start.offset(), span.end.offset())
    }

    fn byte_at(&self, offset: usize) -> Option<u8> {
        self.source.as_bytes().get(offset).copied()
    }

    fn valid_span(&self, span: Span) -> bool {
        span.start.offset() < span.end.offset() && span.end.offset() <= self.source.len()
    }

    /// Paints `keyword` when it is the first token at or after `from`.
    fn keyword_at_start(&mut self, from: usize, to: usize, keyword: &str) -> Option<usize> {
        let to = to.min(self.source.len());
        let mut position = from;
        while position < to
            && self
                .byte_at(position)
                .is_some_and(|b| b.is_ascii_whitespace())
        {
            position += 1;
        }
        let end = position + keyword.len();
        if end > to || self.text(position, end) != Some(keyword) {
            return None;
        }
        if self.byte_at(end).is_some_and(is_identifier_byte) {
            return None;
        }
        self.push(position, end, TOKEN_TYPE_KEYWORD, 0, PRIORITY_KEYWORD);
        Some(end)
    }

    /// Finds `keyword` as a whole word between two AST spans. The gap can only
    /// hold whitespace, terminators, braces and comments, and comments are
    /// skipped, so the match can never come from a string or another word.
    fn find_in_gap(&self, from: usize, to: usize, keyword: &str) -> Option<(usize, usize)> {
        let to = to.min(self.source.len());
        if from >= to {
            return None;
        }
        let bytes = self.source.as_bytes();
        let mut position = from;
        while position < to {
            let byte = bytes[position];
            if byte == b'#' && (position == from || !is_identifier_byte(bytes[position - 1])) {
                while position < to && bytes[position] != b'\n' {
                    position += 1;
                }
                continue;
            }
            if !is_identifier_byte(byte) {
                position += 1;
                continue;
            }
            let run_start = position;
            while position < to && is_identifier_byte(bytes[position]) {
                position += 1;
            }
            let preceded_by_identifier = run_start > 0 && is_identifier_byte(bytes[run_start - 1]);
            if !preceded_by_identifier && &self.source[run_start..position] == keyword {
                return Some((run_start, position));
            }
        }
        None
    }

    fn keyword_in_gap(&mut self, from: usize, to: usize, keyword: &str) {
        if let Some((start, end)) = self.find_in_gap(from, to, keyword) {
            self.push(start, end, TOKEN_TYPE_KEYWORD, 0, PRIORITY_KEYWORD);
        }
    }

    /// Paints every non-blank run between the child spans of a syntactic
    /// container as `token_type`. Used for expansion operators, arithmetic
    /// operators and case-pattern punctuation, whose spans the AST omits.
    fn paint_gaps(
        &mut self,
        start: usize,
        end: usize,
        children: &mut [(usize, usize)],
        token_type: u32,
    ) {
        if start >= end || end > self.source.len() {
            return;
        }
        if children.iter().any(|&(s, e)| s > e || s < start || e > end) {
            return;
        }
        children.sort_unstable();
        let mut cursor = start;
        for &(child_start, child_end) in children.iter() {
            if child_start > cursor {
                self.paint_runs(cursor, child_start, token_type);
            }
            cursor = cursor.max(child_end);
        }
        if cursor < end {
            self.paint_runs(cursor, end, token_type);
        }
    }

    fn paint_runs(&mut self, start: usize, end: usize, token_type: u32) {
        let bytes = self.source.as_bytes();
        let mut position = start;
        while position < end {
            while position < end && bytes[position].is_ascii_whitespace() {
                position += 1;
            }
            let run_start = position;
            while position < end && !bytes[position].is_ascii_whitespace() {
                position += 1;
            }
            if run_start < position {
                self.push(run_start, position, token_type, 0, PRIORITY_SYNTAX);
            }
        }
    }

    /// Paints `[start, end)` minus `holes` as literal string text.
    fn paint_string_fragments(&mut self, start: usize, end: usize, holes: &mut [(usize, usize)]) {
        holes.sort_unstable();
        let mut cursor = start;
        for &(hole_start, hole_end) in holes.iter() {
            if hole_start > cursor {
                self.push(cursor, hole_start, TOKEN_TYPE_STRING, 0, PRIORITY_STRING);
            }
            cursor = cursor.max(hole_end);
        }
        if cursor < end {
            self.push(cursor, end, TOKEN_TYPE_STRING, 0, PRIORITY_STRING);
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
        let start = stmt.span.start.offset();
        if stmt.negated && self.byte_at(start) == Some(b'!') {
            self.operator_at(start, start + 1);
        }
        self.visit_command(&stmt.command);
        for redirect in &stmt.redirects {
            self.visit_redirect(redirect);
        }
        if let Some(span) = stmt.terminator_span {
            self.operator(span);
        }
    }

    fn visit_command(&mut self, command: &Command) {
        match command {
            Command::Simple(cmd) => {
                for assignment in &cmd.assignments {
                    self.visit_assignment(assignment);
                }
                let name = shucked_ast::static_word_text(&cmd.name, self.source);
                let naming = match name.as_deref() {
                    Some(
                        "return" | "exit" | "local" | "export" | "declare" | "typeset" | "readonly",
                    ) => {
                        self.keyword(cmd.name.span);
                        ArgumentNaming::None
                    }
                    Some("[") => {
                        self.push_span(cmd.name.span, TOKEN_TYPE_OPERATOR, 0, PRIORITY_PUNCTUATION);
                        ArgumentNaming::TestExpression
                    }
                    Some("test") => ArgumentNaming::TestExpression,
                    Some("alias") => ArgumentNaming::AliasDefinition,
                    Some("unalias") => ArgumentNaming::AliasLookup,
                    Some("autoload") => ArgumentNaming::FunctionDeclaration,
                    Some("unfunction") => ArgumentNaming::FunctionLookup,
                    Some("setopt" | "unsetopt" | "shopt") => ArgumentNaming::ShellOption,
                    _ => {
                        self.visit_word(&cmd.name, WordRole::Plain);
                        ArgumentNaming::None
                    }
                };
                self.visit_arguments(&cmd.args, naming);
            }
            Command::Builtin(builtin) => self.visit_builtin(builtin),
            Command::Decl(clause) => self.visit_decl(clause),
            Command::Binary(binary) => {
                self.visit_stmt(&binary.left);
                self.operator(binary.op_span);
                self.visit_stmt(&binary.right);
            }
            Command::Compound(compound) => self.visit_compound(compound),
            Command::Function(function) => {
                if let Some(span) = function.header.function_keyword_span {
                    self.keyword(span);
                }
                for entry in &function.header.entries {
                    if entry.static_name.is_some() {
                        self.push_span(
                            entry.word.span,
                            TOKEN_TYPE_FUNCTION,
                            MODIFIER_DEFINITION | MODIFIER_DECLARATION,
                            PRIORITY_SYNTAX,
                        );
                    } else {
                        self.visit_word(&entry.word, WordRole::Plain);
                    }
                }
                if let Some(span) = function.header.trailing_parens_span {
                    self.operator(span);
                }
                self.visit_stmt(&function.body);
            }
            Command::AnonymousFunction(function) => {
                match function.surface {
                    AnonymousFunctionSurface::FunctionKeyword {
                        function_keyword_span,
                    } => self.keyword(function_keyword_span),
                    AnonymousFunctionSurface::Parens { parens_span } => self.operator(parens_span),
                }
                self.visit_stmt(&function.body);
                self.visit_arguments(&function.args, ArgumentNaming::None);
            }
        }
    }

    fn visit_arguments(&mut self, args: &[Word], naming: ArgumentNaming) {
        let mut options_allowed = true;
        let last = args.len().saturating_sub(1);
        for (index, arg) in args.iter().enumerate() {
            let literal = self.literal_prefix(arg);
            let bare = literal
                .filter(|&(start, end)| arg.parts.len() == 1 && (start, end) == word_bounds(arg))
                .and_then(|(start, end)| self.text(start, end));

            if options_allowed && bare == Some("--") {
                self.push_span(arg.span, TOKEN_TYPE_SHELL_OPTION, 0, PRIORITY_SYNTAX);
                options_allowed = false;
                continue;
            }

            match naming {
                ArgumentNaming::TestExpression => {
                    if let Some(text) = bare
                        && (is_test_operator(text) || (index == last && text == "]"))
                    {
                        let priority = if text == "]" {
                            PRIORITY_PUNCTUATION
                        } else {
                            PRIORITY_SYNTAX
                        };
                        self.push_span(arg.span, TOKEN_TYPE_OPERATOR, 0, priority);
                        continue;
                    }
                    self.visit_word(arg, WordRole::Value);
                    continue;
                }
                ArgumentNaming::AliasDefinition | ArgumentNaming::AliasLookup
                    if options_allowed && self.is_option_word(literal) => {}
                ArgumentNaming::AliasDefinition => {
                    if let Some((start, end)) = literal
                        && let Some(text) = self.text(start, end)
                    {
                        let name_end = text.find('=').map_or(end, |offset| start + offset);
                        let modifiers = if name_end < end {
                            MODIFIER_DECLARATION | MODIFIER_DEFINITION
                        } else {
                            0
                        };
                        self.push(
                            start,
                            name_end,
                            TOKEN_TYPE_SHELL_ALIAS,
                            modifiers,
                            PRIORITY_SPECIAL_NAME,
                        );
                    }
                    self.visit_word(arg, WordRole::Plain);
                    continue;
                }
                ArgumentNaming::AliasLookup => {
                    if let Some((start, end)) = literal {
                        self.push(start, end, TOKEN_TYPE_SHELL_ALIAS, 0, PRIORITY_SPECIAL_NAME);
                    }
                    self.visit_word(arg, WordRole::Plain);
                    continue;
                }
                ArgumentNaming::FunctionDeclaration | ArgumentNaming::FunctionLookup
                    if options_allowed && self.is_option_word(literal) => {}
                ArgumentNaming::FunctionDeclaration | ArgumentNaming::FunctionLookup => {
                    if let Some((start, end)) = literal {
                        let modifiers = if naming == ArgumentNaming::FunctionDeclaration {
                            MODIFIER_DECLARATION
                        } else {
                            0
                        };
                        self.push(
                            start,
                            end,
                            TOKEN_TYPE_FUNCTION,
                            modifiers,
                            PRIORITY_SPECIAL_NAME,
                        );
                    }
                    self.visit_word(arg, WordRole::Plain);
                    continue;
                }
                ArgumentNaming::ShellOption if options_allowed && self.is_option_word(literal) => {}
                ArgumentNaming::ShellOption => {
                    if let Some((start, end)) = literal {
                        self.push(start, end, TOKEN_TYPE_SHELL_OPTION, 0, PRIORITY_SYNTAX);
                    }
                    self.visit_word(arg, WordRole::Plain);
                    continue;
                }
                ArgumentNaming::None => {}
            }

            let role = if options_allowed {
                WordRole::Argument
            } else {
                WordRole::Value
            };
            self.visit_word(arg, role);
        }
    }

    fn visit_builtin(&mut self, builtin: &BuiltinCommand) {
        let (keyword, span, assignments, operand, extra) = match builtin {
            BuiltinCommand::Break(cmd) => (
                "break",
                cmd.span,
                &cmd.assignments,
                cmd.depth.as_ref(),
                &cmd.extra_args,
            ),
            BuiltinCommand::Continue(cmd) => (
                "continue",
                cmd.span,
                &cmd.assignments,
                cmd.depth.as_ref(),
                &cmd.extra_args,
            ),
            BuiltinCommand::Return(cmd) => (
                "return",
                cmd.span,
                &cmd.assignments,
                cmd.code.as_ref(),
                &cmd.extra_args,
            ),
            BuiltinCommand::Exit(cmd) => (
                "exit",
                cmd.span,
                &cmd.assignments,
                cmd.code.as_ref(),
                &cmd.extra_args,
            ),
        };
        let mut from = span.start.offset();
        for assignment in assignments.iter() {
            self.visit_assignment(assignment);
            from = from.max(assignment.span.end.offset());
        }
        let to = operand.map_or(span.end.offset(), |word| word.span.start.offset());
        self.keyword_at_start(from, to, keyword);
        if let Some(word) = operand {
            self.visit_word(word, WordRole::Value);
        }
        self.visit_arguments(extra, ArgumentNaming::None);
    }

    fn visit_decl(&mut self, clause: &DeclClause) {
        for assignment in &clause.assignments {
            self.visit_assignment(assignment);
        }
        self.keyword(clause.variant_span);
        let names_functions = clause.operands.iter().any(|operand| {
            matches!(operand, DeclOperand::Flag(word)
            if self.span_text(word.span).is_some_and(|text| {
                text.starts_with('-') && text[1..].contains(['f', 'F'])
            }))
        });
        for operand in &clause.operands {
            match operand {
                DeclOperand::Flag(word) => {
                    self.push_span(word.span, TOKEN_TYPE_SHELL_OPTION, 0, PRIORITY_SYNTAX);
                }
                DeclOperand::Name(var_ref) => {
                    if names_functions {
                        self.push_span(
                            var_ref.name_span,
                            TOKEN_TYPE_FUNCTION,
                            0,
                            PRIORITY_SPECIAL_NAME,
                        );
                    } else {
                        self.push_span(
                            var_ref.name_span,
                            TOKEN_TYPE_VARIABLE,
                            MODIFIER_DECLARATION,
                            PRIORITY_SYNTAX,
                        );
                    }
                    if let Some(subscript) = &var_ref.subscript {
                        self.visit_subscript(subscript);
                    }
                }
                DeclOperand::Assignment(assignment) => self.visit_assignment(assignment),
                DeclOperand::Dynamic(word) => self.visit_word(word, WordRole::Argument),
            }
        }
    }

    fn visit_assignment(&mut self, assignment: &Assignment) {
        self.push_span(
            assignment.target.name_span,
            TOKEN_TYPE_VARIABLE,
            MODIFIER_DECLARATION,
            PRIORITY_SYNTAX,
        );
        if let Some(subscript) = &assignment.target.subscript {
            self.visit_subscript(subscript);
        }
        match &assignment.value {
            AssignmentValue::Scalar(word) => self.visit_word(word, WordRole::Value),
            AssignmentValue::Compound(array) => {
                // The array span can start at the first element, so locate the
                // parentheses from the `=` that follows the target instead.
                let mut open = assignment
                    .target
                    .subscript
                    .as_deref()
                    .map_or(assignment.target.name_span.end.offset(), |subscript| {
                        subscript.span().end.offset() + 1
                    });
                while self.byte_at(open).is_some_and(|b| matches!(b, b'+' | b'=')) {
                    open += 1;
                }
                if self.byte_at(open) == Some(b'(') {
                    self.operator_at(open, open + 1);
                }
                let close = assignment.span.end.offset();
                if close > open && self.byte_at(close - 1) == Some(b')') {
                    self.operator_at(close - 1, close);
                } else if self.byte_at(array.span.end.offset()) == Some(b')') {
                    self.operator_at(array.span.end.offset(), array.span.end.offset() + 1);
                }
                for element in &array.elements {
                    match element {
                        ArrayElem::Sequential(value) => {
                            self.visit_word(&value.word, WordRole::Value);
                        }
                        ArrayElem::Keyed { key, value } | ArrayElem::KeyedAppend { key, value } => {
                            self.visit_subscript(key);
                            self.visit_word(&value.word, WordRole::Value);
                        }
                    }
                }
            }
        }
    }

    fn visit_subscript(&mut self, subscript: &Subscript) {
        let span = subscript.span();
        if !self.valid_span(span) {
            return;
        }
        let start = span.start.offset();
        let end = span.end.offset();
        if start > 0 && self.byte_at(start - 1) == Some(b'[') {
            self.operator_at(start - 1, start);
        }
        if self.byte_at(end) == Some(b']') {
            self.operator_at(end, end + 1);
        }
        if subscript.is_array_selector() {
            self.operator_at(start, end);
            return;
        }
        if let Some(arithmetic) = &subscript.arithmetic_ast {
            self.visit_arithmetic(arithmetic);
        } else if let Some(word) = &subscript.word_ast {
            self.visit_word(word, WordRole::Value);
        }
    }

    fn visit_compound(&mut self, compound: &CompoundCommand) {
        match compound {
            CompoundCommand::If(if_cmd) => self.visit_if(if_cmd),
            CompoundCommand::For(for_cmd) => self.visit_for(for_cmd),
            CompoundCommand::Repeat(repeat) => {
                let start = repeat.span.start.offset();
                self.keyword_at_start(start, repeat.count.span.start.offset(), "repeat");
                self.visit_word(&repeat.count, WordRole::Value);
                match repeat.syntax {
                    RepeatSyntax::DoDone { do_span, done_span } => {
                        self.keyword(do_span);
                        self.keyword(done_span);
                    }
                    RepeatSyntax::Direct | RepeatSyntax::Brace { .. } => {}
                }
                self.visit_stmt_seq(&repeat.body);
            }
            CompoundCommand::Foreach(foreach) => {
                let start = foreach.span.start.offset();
                self.keyword_at_start(start, foreach.variable_span.start.offset(), "foreach");
                self.push_span(
                    foreach.variable_span,
                    TOKEN_TYPE_VARIABLE,
                    MODIFIER_DECLARATION,
                    PRIORITY_SYNTAX,
                );
                match foreach.syntax {
                    ForeachSyntax::ParenBrace {
                        left_paren_span,
                        right_paren_span,
                        ..
                    } => {
                        self.operator(left_paren_span);
                        self.operator(right_paren_span);
                        self.keyword_in_gap(
                            foreach.body.span.end.offset(),
                            foreach.span.end.offset(),
                            "end",
                        );
                    }
                    ForeachSyntax::InDoDone {
                        in_span,
                        do_span,
                        done_span,
                    } => {
                        self.keyword(in_span);
                        self.keyword(do_span);
                        self.keyword(done_span);
                    }
                }
                for word in &foreach.words {
                    self.visit_word(word, WordRole::Value);
                }
                self.visit_stmt_seq(&foreach.body);
            }
            CompoundCommand::ArithmeticFor(afor) => self.visit_arithmetic_for(afor),
            CompoundCommand::While(while_cmd) => {
                self.visit_loop(
                    "while",
                    while_cmd.span,
                    &while_cmd.condition,
                    &while_cmd.body,
                    while_cmd.done_span,
                );
            }
            CompoundCommand::Until(until_cmd) => {
                self.visit_loop(
                    "until",
                    until_cmd.span,
                    &until_cmd.condition,
                    &until_cmd.body,
                    until_cmd.done_span,
                );
            }
            CompoundCommand::Case(case_cmd) => self.visit_case(case_cmd),
            CompoundCommand::Select(select) => {
                let start = select.span.start.offset();
                self.keyword_at_start(start, select.variable_span.start.offset(), "select");
                self.push_span(
                    select.variable_span,
                    TOKEN_TYPE_VARIABLE,
                    MODIFIER_DECLARATION,
                    PRIORITY_SYNTAX,
                );
                let mut header_end = select.variable_span.end.offset();
                if let Some(first) = select.words.first() {
                    self.keyword_in_gap(header_end, first.span.start.offset(), "in");
                    for word in &select.words {
                        self.visit_word(word, WordRole::Value);
                        header_end = header_end.max(word.span.end.offset());
                    }
                }
                self.keyword_in_gap(header_end, select.body.span.start.offset(), "do");
                self.visit_stmt_seq(&select.body);
                self.keyword(select.done_span);
            }
            CompoundCommand::Subshell(seq) | CompoundCommand::BraceGroup(seq) => {
                self.visit_stmt_seq(seq);
            }
            CompoundCommand::Arithmetic(arith) => self.visit_arithmetic_command(arith),
            CompoundCommand::Time(time) => {
                let start = time.span.start.offset();
                let end = time
                    .command
                    .as_ref()
                    .map_or(time.span.end.offset(), |cmd| cmd.span.start.offset());
                let after_keyword = self.keyword_at_start(start, end, "time");
                if time.posix_format
                    && let Some(from) = after_keyword
                    && let Some((flag_start, flag_end)) = self.find_in_gap(from, end, "p")
                    && flag_start > 0
                    && self.byte_at(flag_start - 1) == Some(b'-')
                {
                    self.push(
                        flag_start - 1,
                        flag_end,
                        TOKEN_TYPE_SHELL_OPTION,
                        0,
                        PRIORITY_SYNTAX,
                    );
                }
                if let Some(cmd) = &time.command {
                    self.visit_stmt(cmd);
                }
            }
            CompoundCommand::Conditional(cond) => {
                self.operator(cond.left_bracket_span);
                self.visit_conditional_expr(&cond.expression);
                self.operator(cond.right_bracket_span);
            }
            CompoundCommand::Coproc(coproc) => {
                let start = coproc.span.start.offset();
                let end = coproc
                    .name_span
                    .map_or(coproc.body.span.start.offset(), |span| span.start.offset());
                self.keyword_at_start(start, end, "coproc");
                if let Some(span) = coproc.name_span {
                    self.push_span(
                        span,
                        TOKEN_TYPE_VARIABLE,
                        MODIFIER_DECLARATION,
                        PRIORITY_SYNTAX,
                    );
                }
                self.visit_stmt(&coproc.body);
            }
            CompoundCommand::Always(always) => {
                self.visit_stmt_seq(&always.body);
                self.keyword_in_gap(
                    always.body.span.end.offset(),
                    always.always_body.span.start.offset(),
                    "always",
                );
                self.visit_stmt_seq(&always.always_body);
            }
        }
    }

    fn visit_loop(
        &mut self,
        keyword: &str,
        span: Span,
        condition: &StmtSeq,
        body: &StmtSeq,
        done_span: Option<Span>,
    ) {
        self.keyword_at_start(span.start.offset(), condition.span.start.offset(), keyword);
        self.visit_stmt_seq(condition);
        self.keyword_in_gap(condition.span.end.offset(), body.span.start.offset(), "do");
        self.visit_stmt_seq(body);
        if let Some(done_span) = done_span {
            self.keyword(done_span);
        } else {
            self.keyword_in_gap(body.span.end.offset(), span.end.offset(), "done");
        }
    }

    fn visit_if(&mut self, if_cmd: &IfCommand) {
        self.keyword_at_start(
            if_cmd.span.start.offset(),
            if_cmd.condition.span.start.offset(),
            "if",
        );
        self.visit_stmt_seq(&if_cmd.condition);
        match if_cmd.syntax {
            IfSyntax::ThenFi { then_span, fi_span } => {
                self.keyword(then_span);
                self.keyword(fi_span);
            }
            IfSyntax::Brace { .. } => {}
        }
        self.visit_stmt_seq(&if_cmd.then_branch);

        let mut previous_end = if_cmd.then_branch.span.end.offset();
        for (condition, body) in &if_cmd.elif_branches {
            self.keyword_in_gap(previous_end, condition.span.start.offset(), "elif");
            self.visit_stmt_seq(condition);
            self.keyword_in_gap(
                condition.span.end.offset(),
                body.span.start.offset(),
                "then",
            );
            self.visit_stmt_seq(body);
            previous_end = body.span.end.offset();
        }
        if let Some(else_branch) = &if_cmd.else_branch {
            self.keyword_in_gap(previous_end, else_branch.span.start.offset(), "else");
            self.visit_stmt_seq(else_branch);
        }
    }

    fn visit_for(&mut self, for_cmd: &ForCommand) {
        let header_end = for_cmd
            .targets
            .first()
            .map_or(for_cmd.body.span.start.offset(), |target| {
                target.span.start.offset()
            });
        self.keyword_at_start(for_cmd.span.start.offset(), header_end, "for");
        for target in &for_cmd.targets {
            if target.name.is_some() {
                self.push_span(
                    target.word.span,
                    TOKEN_TYPE_VARIABLE,
                    MODIFIER_DECLARATION,
                    PRIORITY_SYNTAX,
                );
            } else {
                self.visit_word(&target.word, WordRole::Plain);
            }
        }
        match for_cmd.syntax {
            ForSyntax::InDoDone {
                in_span,
                do_span,
                done_span,
            } => {
                if let Some(span) = in_span {
                    self.keyword(span);
                }
                self.keyword(do_span);
                self.keyword(done_span);
            }
            ForSyntax::InDirect { in_span } | ForSyntax::InBrace { in_span, .. } => {
                if let Some(span) = in_span {
                    self.keyword(span);
                }
            }
            ForSyntax::ParenDoDone {
                left_paren_span,
                right_paren_span,
                do_span,
                done_span,
            } => {
                self.operator(left_paren_span);
                self.operator(right_paren_span);
                self.keyword(do_span);
                self.keyword(done_span);
            }
            ForSyntax::ParenDirect {
                left_paren_span,
                right_paren_span,
            }
            | ForSyntax::ParenBrace {
                left_paren_span,
                right_paren_span,
                ..
            } => {
                self.operator(left_paren_span);
                self.operator(right_paren_span);
            }
        }
        if let Some(words) = &for_cmd.words {
            for word in words {
                self.visit_word(word, WordRole::Value);
            }
        }
        self.visit_stmt_seq(&for_cmd.body);
    }

    fn visit_arithmetic_for(&mut self, afor: &ArithmeticForCommand) {
        self.keyword_at_start(
            afor.span.start.offset(),
            afor.left_paren_span.start.offset(),
            "for",
        );
        self.operator(afor.left_paren_span);
        for expression in [&afor.init_ast, &afor.condition_ast, &afor.step_ast]
            .into_iter()
            .flatten()
        {
            self.visit_arithmetic(expression);
        }
        self.operator(afor.first_semicolon_span);
        self.operator(afor.second_semicolon_span);
        self.operator(afor.right_paren_span);
        self.keyword_in_gap(
            afor.right_paren_span.end.offset(),
            afor.body.span.start.offset(),
            "do",
        );
        self.visit_stmt_seq(&afor.body);
        if let Some(done_span) = afor.done_span {
            self.keyword(done_span);
        } else {
            self.keyword_in_gap(afor.body.span.end.offset(), afor.span.end.offset(), "done");
        }
    }

    fn visit_case(&mut self, case_cmd: &CaseCommand) {
        self.keyword_at_start(
            case_cmd.span.start.offset(),
            case_cmd.word.span.start.offset(),
            "case",
        );
        self.visit_word(&case_cmd.word, WordRole::Plain);

        let first_pattern_start = case_cmd
            .cases
            .first()
            .and_then(|item| item.patterns.first())
            .map_or(case_cmd.esac_span.start.offset(), |pattern| {
                pattern.span.start.offset()
            });
        self.keyword_in_gap(case_cmd.word.span.end.offset(), first_pattern_start, "in");

        for item in &case_cmd.cases {
            if let (Some(first), Some(last)) = (item.patterns.first(), item.patterns.last()) {
                let first_start = first.span.start.offset();
                let mut before = first_start;
                while before > 0
                    && self
                        .byte_at(before - 1)
                        .is_some_and(|b| b == b' ' || b == b'\t')
                {
                    before -= 1;
                }
                if before > 0 && self.byte_at(before - 1) == Some(b'(') {
                    self.operator_at(before - 1, before);
                }
                let mut children = item
                    .patterns
                    .iter()
                    .map(|pattern| (pattern.span.start.offset(), pattern.span.end.offset()))
                    .collect::<Vec<_>>();
                let list_end = item.body.span.start.offset().max(last.span.end.offset());
                self.paint_gaps(first_start, list_end, &mut children, TOKEN_TYPE_OPERATOR);
                for pattern in &item.patterns {
                    self.visit_pattern(pattern);
                }
            }
            self.visit_stmt_seq(&item.body);
            if let Some(span) = item.terminator_span {
                self.operator(span);
            }
        }
        self.keyword(case_cmd.esac_span);
    }

    fn visit_pattern(&mut self, pattern: &Pattern) {
        for part in &pattern.parts {
            match &part.kind {
                PatternPart::Literal(_) => {}
                PatternPart::AnyString | PatternPart::AnyChar | PatternPart::CharClass(_) => {
                    self.operator(part.span);
                }
                PatternPart::Group { patterns, .. } => {
                    let start = part.span.start.offset();
                    let end = part.span.end.offset();
                    if self.valid_span(part.span) {
                        self.operator_at(start, (start + 2).min(end));
                        if end > start + 2 && self.byte_at(end - 1) == Some(b')') {
                            self.operator_at(end - 1, end);
                        }
                    }
                    for nested in patterns {
                        self.visit_pattern(nested);
                    }
                }
                PatternPart::Word(word) => self.visit_word(word, WordRole::Plain),
            }
        }
    }

    fn visit_conditional_expr(&mut self, expr: &ConditionalExpr) {
        match expr {
            ConditionalExpr::Binary(binary) => {
                self.visit_conditional_expr(&binary.left);
                self.operator(binary.op_span);
                self.visit_conditional_expr(&binary.right);
            }
            ConditionalExpr::Unary(unary) => {
                self.operator(unary.op_span);
                self.visit_conditional_expr(&unary.expr);
            }
            ConditionalExpr::Parenthesized(paren) => {
                self.operator(paren.left_paren_span);
                self.visit_conditional_expr(&paren.expr);
                self.operator(paren.right_paren_span);
            }
            ConditionalExpr::Word(word) => self.visit_word(word, WordRole::Value),
            ConditionalExpr::Regex(word) => self.visit_word(word, WordRole::Plain),
            ConditionalExpr::Pattern(pattern) => self.visit_pattern(pattern),
            ConditionalExpr::VarRef(var_ref) => {
                let mut children = Vec::new();
                self.visit_var_ref(var_ref, &mut children);
            }
        }
    }

    fn visit_arithmetic_command(&mut self, arith: &ArithmeticCommand) {
        self.operator(arith.left_paren_span);
        if let Some(expression) = &arith.expr_ast {
            self.visit_arithmetic(expression);
        }
        self.operator(arith.right_paren_span);
    }

    /// Paints numbers and variables inside arithmetic and fills every gap
    /// between operand spans with operator tokens.
    fn visit_arithmetic(&mut self, node: &ArithmeticExprNode) {
        let start = node.span.start.offset();
        let end = node.span.end.offset();
        let mut children: Vec<(usize, usize)> = Vec::new();
        match &node.kind {
            ArithmeticExpr::Number(_) => {
                self.push(start, end, TOKEN_TYPE_NUMBER, 0, PRIORITY_SYNTAX);
                return;
            }
            ArithmeticExpr::Variable(name) => {
                let (token_type, modifiers, priority) = name_token(name);
                self.push(start, end, token_type, modifiers, priority);
                return;
            }
            ArithmeticExpr::Indexed { name, index } => {
                if let Some(name_span) = self.name_at(start, name) {
                    self.push(
                        name_span.0,
                        name_span.1,
                        TOKEN_TYPE_VARIABLE,
                        0,
                        PRIORITY_SYNTAX,
                    );
                    children.push(name_span);
                } else {
                    self.visit_arithmetic(index);
                    return;
                }
                children.push(node_bounds(index));
                self.visit_arithmetic(index);
            }
            ArithmeticExpr::ShellWord(word) => {
                self.visit_word(word, WordRole::Plain);
                return;
            }
            ArithmeticExpr::Parenthesized { expression } => {
                children.push(node_bounds(expression));
                self.visit_arithmetic(expression);
            }
            ArithmeticExpr::Unary { expr, .. } | ArithmeticExpr::Postfix { expr, .. } => {
                children.push(node_bounds(expr));
                self.visit_arithmetic(expr);
            }
            ArithmeticExpr::Binary { left, right, .. } => {
                children.push(node_bounds(left));
                children.push(node_bounds(right));
                self.visit_arithmetic(left);
                self.visit_arithmetic(right);
            }
            ArithmeticExpr::Conditional {
                condition,
                then_expr,
                else_expr,
            } => {
                for expr in [condition, then_expr, else_expr] {
                    children.push(node_bounds(expr));
                    self.visit_arithmetic(expr);
                }
            }
            ArithmeticExpr::Assignment { target, value, .. } => {
                let (name, index) = match target {
                    ArithmeticLvalue::Variable(name) => (name, None),
                    ArithmeticLvalue::Indexed { name, index } => (name, Some(index)),
                };
                let Some(name_span) = self.name_at(start, name) else {
                    if let Some(index) = index {
                        self.visit_arithmetic(index);
                    }
                    self.visit_arithmetic(value);
                    return;
                };
                self.push(
                    name_span.0,
                    name_span.1,
                    TOKEN_TYPE_VARIABLE,
                    MODIFIER_DECLARATION,
                    PRIORITY_SYNTAX,
                );
                children.push(name_span);
                if let Some(index) = index {
                    children.push(node_bounds(index));
                    self.visit_arithmetic(index);
                }
                children.push(node_bounds(value));
                self.visit_arithmetic(value);
            }
        }
        self.paint_gaps(start, end, &mut children, TOKEN_TYPE_OPERATOR);
    }

    /// The span of `name` when the source spells it at `start`.
    fn name_at(&self, start: usize, name: &Name) -> Option<(usize, usize)> {
        let end = start + name.as_str().len();
        (self.text(start, end) == Some(name.as_str())).then_some((start, end))
    }

    fn visit_redirect(&mut self, redirect: &Redirect) {
        if let Some(span) = redirect.fd_var_span {
            self.push_span(
                span,
                TOKEN_TYPE_VARIABLE,
                MODIFIER_DECLARATION,
                PRIORITY_SYNTAX,
            );
        }
        let start = redirect.span.start.offset();
        let end = redirect.span.end.offset();
        if !self.valid_span(redirect.span) {
            return;
        }

        // Skip a leading descriptor (`2>`) or descriptor variable (`{fd}>`).
        let mut op_start = start;
        while op_start < end && self.byte_at(op_start).is_some_and(|b| b.is_ascii_digit()) {
            op_start += 1;
        }
        if op_start > start {
            self.push(start, op_start, TOKEN_TYPE_NUMBER, 0, PRIORITY_SYNTAX);
        }
        if self.byte_at(op_start) == Some(b'{')
            && let Some(close) = self.source[op_start..end].find('}')
        {
            op_start += close + 1;
        }

        match &redirect.target {
            RedirectTarget::Word(word) => {
                let target_start = word.span.start.offset();
                let target_valid = self.valid_span(word.span)
                    && target_start >= op_start
                    && word.span.end.offset() <= end;
                let op_end = if target_valid {
                    target_start
                } else {
                    let mut position = op_start;
                    while position < end
                        && self
                            .byte_at(position)
                            .is_some_and(|b| matches!(b, b'<' | b'>' | b'&' | b'|'))
                    {
                        position += 1;
                    }
                    if self.text(op_start, position) == Some("<<")
                        && self.byte_at(position) == Some(b'-')
                    {
                        position += 1;
                    }
                    position
                };
                self.paint_runs(op_start, op_end, TOKEN_TYPE_OPERATOR);
                if target_valid {
                    self.visit_word(word, WordRole::Value);
                } else if let Some(rest) = self.text(op_end, end).map(str::trim) {
                    let rest_start = op_end + self.source[op_end..end].find(rest).unwrap_or(0);
                    if is_numeric_literal(rest) {
                        self.push(
                            rest_start,
                            rest_start + rest.len(),
                            TOKEN_TYPE_NUMBER,
                            0,
                            PRIORITY_SYNTAX,
                        );
                    } else if rest == "-" {
                        self.operator_at(rest_start, rest_start + 1);
                    }
                }
            }
            RedirectTarget::Heredoc(heredoc) => {
                let delimiter = heredoc.delimiter.span;
                let op_end = if self.valid_span(delimiter) && delimiter.start.offset() >= op_start {
                    delimiter.start.offset()
                } else {
                    end
                };
                self.paint_runs(op_start, op_end, TOKEN_TYPE_OPERATOR);
                self.push_span(delimiter, TOKEN_TYPE_STRING, 0, PRIORITY_SYNTAX);
                self.visit_heredoc(heredoc);
            }
        }
    }

    fn visit_heredoc(&mut self, heredoc: &Heredoc) {
        let body = &heredoc.body;
        if !self.valid_span(body.span) {
            return;
        }
        let start = body.span.start.offset();
        let end = body.span.end.offset();
        if body.source_backed {
            self.painted_heredocs.push((start, end));
            self.visit_heredoc_body(heredoc, start, end);
        }
        self.visit_heredoc_marker(heredoc, end);
    }

    fn visit_heredoc_body(&mut self, heredoc: &Heredoc, start: usize, end: usize) {
        let body = &heredoc.body;
        let mut holes = Vec::new();
        for part in &body.parts {
            if !self.valid_span(part.span) {
                continue;
            }
            let part_start = part.span.start.offset();
            let part_end = part.span.end.offset();
            match &part.kind {
                HeredocBodyPart::Literal(_) => {}
                HeredocBodyPart::Variable(name) => {
                    holes.push((part_start, part_end));
                    let (token_type, modifiers, priority) = name_token(name);
                    self.push(part_start, part_end, token_type, modifiers, priority);
                }
                HeredocBodyPart::CommandSubstitution { body, syntax } => {
                    holes.push((part_start, part_end));
                    self.visit_command_substitution(part_start, part_end, body, *syntax);
                }
                HeredocBodyPart::ArithmeticExpansion {
                    expression_ast,
                    expression_word_ast,
                    syntax,
                    ..
                } => {
                    holes.push((part_start, part_end));
                    self.visit_arithmetic_expansion(
                        part_start,
                        part_end,
                        expression_ast.as_deref(),
                        expression_word_ast,
                        *syntax,
                    );
                }
                HeredocBodyPart::Parameter(parameter) => {
                    holes.push((part_start, part_end));
                    self.visit_parameter_expansion(parameter, part.span);
                }
            }
        }
        self.paint_string_fragments(start, end, &mut holes);
    }

    /// The closing marker follows the body on its own line.
    fn visit_heredoc_marker(&mut self, heredoc: &Heredoc, end: usize) {
        let mut marker = end;
        if heredoc.delimiter.strip_tabs {
            while self.byte_at(marker) == Some(b'\t') {
                marker += 1;
            }
        }
        let cooked = heredoc.delimiter.cooked.as_str();
        let marker_end = marker + cooked.len();
        if !cooked.is_empty()
            && self.text(marker, marker_end) == Some(cooked)
            && self
                .byte_at(marker_end)
                .is_none_or(|b| matches!(b, b'\n' | b'\r'))
        {
            self.push(marker, marker_end, TOKEN_TYPE_STRING, 0, PRIORITY_SYNTAX);
        }
    }

    fn visit_command_substitution(
        &mut self,
        start: usize,
        end: usize,
        body: &StmtSeq,
        syntax: CommandSubstitutionSyntax,
    ) {
        match syntax {
            CommandSubstitutionSyntax::DollarParen => {
                if self.text(start, start + 2) == Some("$(") {
                    self.operator_at(start, start + 2);
                }
                if end > start && self.byte_at(end - 1) == Some(b')') {
                    self.operator_at(end - 1, end);
                }
            }
            CommandSubstitutionSyntax::Backtick => {
                if self.byte_at(start) == Some(b'`') {
                    self.operator_at(start, start + 1);
                }
                if end > start + 1 && self.byte_at(end - 1) == Some(b'`') {
                    self.operator_at(end - 1, end);
                }
            }
        }
        self.visit_stmt_seq(body);
    }

    fn visit_arithmetic_expansion(
        &mut self,
        start: usize,
        end: usize,
        expression_ast: Option<&ArithmeticExprNode>,
        expression_word: &Word,
        syntax: ArithmeticExpansionSyntax,
    ) {
        let (open, close) = match syntax {
            ArithmeticExpansionSyntax::DollarParenParen => ("$((", "))"),
            ArithmeticExpansionSyntax::LegacyBracket => ("$[", "]"),
        };
        if self.text(start, start + open.len()) == Some(open) {
            self.operator_at(start, start + open.len());
        }
        if end >= start + open.len() + close.len()
            && self.text(end - close.len(), end) == Some(close)
        {
            self.operator_at(end - close.len(), end);
        }
        if let Some(expression) = expression_ast {
            self.visit_arithmetic(expression);
        } else {
            self.visit_word(expression_word, WordRole::Plain);
        }
    }

    fn visit_word(&mut self, word: &Word, role: WordRole) {
        let literal = self.literal_prefix(word);
        let bare = literal
            .filter(|&(start, end)| word.parts.len() == 1 && (start, end) == word_bounds(word))
            .and_then(|(start, end)| self.text(start, end));
        if role != WordRole::Plain
            && let Some((start, end)) = literal
            && bare.is_some_and(is_numeric_literal)
        {
            self.push(start, end, TOKEN_TYPE_NUMBER, 0, PRIORITY_SYNTAX);
            return;
        }
        if role == WordRole::Argument
            && let Some((start, end)) = literal
            && self.is_option_word(literal)
            && let Some(text) = self.text(start, end)
        {
            let name_end = text
                .find('=')
                .filter(|&offset| offset > 1)
                .map_or(end, |offset| start + offset);
            self.push(start, name_end, TOKEN_TYPE_SHELL_OPTION, 0, PRIORITY_SYNTAX);
        }
        for part in &word.parts {
            self.visit_word_part(part);
        }
    }

    /// The span of a word's leading literal part.
    fn literal_prefix(&self, word: &Word) -> Option<(usize, usize)> {
        let first = word.parts.first()?;
        if !matches!(first.kind, WordPart::Literal(_)) || !self.valid_span(first.span) {
            return None;
        }
        Some((first.span.start.offset(), first.span.end.offset()))
    }

    fn is_option_word(&self, literal: Option<(usize, usize)>) -> bool {
        literal
            .and_then(|(start, end)| self.text(start, end))
            .is_some_and(|text| {
                text.len() > 1
                    && (text.starts_with('-') || text.starts_with('+'))
                    && text
                        .as_bytes()
                        .get(1)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
    }

    fn visit_word_part(&mut self, part: &WordPartNode) {
        let start = part.span.start.offset();
        let end = part.span.end.offset();
        match &part.kind {
            WordPart::Literal(_) => {}
            WordPart::ZshQualifiedGlob(glob) => {
                for segment in &glob.segments {
                    match segment {
                        ZshGlobSegment::Pattern(pattern) => self.visit_pattern(pattern),
                        ZshGlobSegment::InlineControl(control) => {
                            let span = match control {
                                shucked_ast::ZshInlineGlobControl::CaseInsensitive { span }
                                | shucked_ast::ZshInlineGlobControl::Backreferences { span }
                                | shucked_ast::ZshInlineGlobControl::StartAnchor { span }
                                | shucked_ast::ZshInlineGlobControl::EndAnchor { span } => *span,
                            };
                            self.operator(span);
                        }
                    }
                }
                if let Some(qualifiers) = &glob.qualifiers {
                    self.operator(qualifiers.span);
                }
            }
            WordPart::SingleQuoted { .. } => {
                self.push(start, end, TOKEN_TYPE_STRING, 0, PRIORITY_SYNTAX);
            }
            WordPart::DoubleQuoted { parts, .. } => {
                let mut holes = Vec::new();
                for inner in parts {
                    if !matches!(inner.kind, WordPart::Literal(_)) && self.valid_span(inner.span) {
                        holes.push((inner.span.start.offset(), inner.span.end.offset()));
                    }
                    self.visit_word_part(inner);
                }
                if self.valid_span(part.span) {
                    self.paint_string_fragments(start, end, &mut holes);
                }
            }
            WordPart::Variable(name) => {
                let (token_type, modifiers, priority) = name_token(name);
                self.push(start, end, token_type, modifiers, priority);
            }
            WordPart::CommandSubstitution { body, syntax } => {
                self.visit_command_substitution(start, end, body, *syntax);
            }
            WordPart::ArithmeticExpansion {
                expression_ast,
                expression_word_ast,
                syntax,
                ..
            } => {
                self.visit_arithmetic_expansion(
                    start,
                    end,
                    expression_ast.as_deref(),
                    expression_word_ast,
                    *syntax,
                );
            }
            WordPart::Parameter(parameter) => self.visit_parameter_expansion(parameter, part.span),
            WordPart::ParameterExpansion {
                reference,
                operator,
                operand_word_ast,
                ..
            }
            | WordPart::IndirectExpansion {
                reference,
                operator: Some(operator),
                operand_word_ast,
                ..
            } => {
                let mut children = Vec::new();
                self.visit_braced_start(part.span, &mut children, |this, children| {
                    this.visit_var_ref(reference, children);
                    this.visit_parameter_op(operator, operand_word_ast.as_deref(), children);
                });
            }
            WordPart::IndirectExpansion {
                reference,
                operator: None,
                operand_word_ast,
                ..
            } => {
                let mut children = Vec::new();
                self.visit_braced_start(part.span, &mut children, |this, children| {
                    this.visit_var_ref(reference, children);
                    if let Some(operand) = operand_word_ast {
                        children.push(word_bounds(operand));
                        this.visit_word(operand, WordRole::Value);
                    }
                });
            }
            WordPart::Length(reference)
            | WordPart::ArrayAccess(reference)
            | WordPart::ArrayLength(reference)
            | WordPart::ArrayIndices(reference)
            | WordPart::Transformation { reference, .. } => {
                let mut children = Vec::new();
                self.visit_braced_start(part.span, &mut children, |this, children| {
                    this.visit_var_ref(reference, children);
                });
            }
            WordPart::Substring {
                reference,
                offset_ast,
                offset_word_ast,
                length_ast,
                length_word_ast,
                ..
            }
            | WordPart::ArraySlice {
                reference,
                offset_ast,
                offset_word_ast,
                length_ast,
                length_word_ast,
                ..
            } => {
                let mut children = Vec::new();
                self.visit_braced_start(part.span, &mut children, |this, children| {
                    this.visit_var_ref(reference, children);
                    this.visit_slice_operand(
                        offset_ast.as_deref(),
                        Some(offset_word_ast),
                        children,
                    );
                    this.visit_slice_operand(
                        length_ast.as_deref(),
                        length_word_ast.as_deref(),
                        children,
                    );
                });
            }
            WordPart::PrefixMatch { prefix, .. } => {
                self.visit_prefix_match(part.span, prefix);
            }
            WordPart::ProcessSubstitution { body, .. } => {
                if self.valid_span(part.span) {
                    self.operator_at(start, (start + 2).min(end));
                    if end > start + 2 && self.byte_at(end - 1) == Some(b')') {
                        self.operator_at(end - 1, end);
                    }
                }
                self.visit_stmt_seq(body);
            }
        }
    }

    /// Paints `${` and `}` around a braced expansion, runs `inner` to collect
    /// child spans, then paints every remaining gap as an operator.
    fn visit_braced_start(
        &mut self,
        span: Span,
        children: &mut Vec<(usize, usize)>,
        inner: impl FnOnce(&mut Self, &mut Vec<(usize, usize)>),
    ) {
        let start = span.start.offset();
        let end = span.end.offset();
        let braced = self.text(start, start + 2) == Some("${")
            && end > start + 2
            && self.byte_at(end - 1) == Some(b'}');
        inner(self, children);
        if braced {
            self.operator_at(start, start + 2);
            self.operator_at(end - 1, end);
            self.paint_gaps(start + 2, end - 1, children, TOKEN_TYPE_OPERATOR);
        }
    }

    fn visit_prefix_match(&mut self, span: Span, prefix: &Name) {
        let start = span.start.offset();
        let end = span.end.offset();
        if self.text(start, start + 3) != Some("${!") || end < start + 5 {
            return;
        }
        self.operator_at(start, start + 3);
        let name_start = start + 3;
        let name_end = (name_start + prefix.as_str().len()).min(end - 2);
        if self.text(name_start, name_end) == Some(prefix.as_str()) {
            self.push(
                name_start,
                name_end,
                TOKEN_TYPE_VARIABLE,
                0,
                PRIORITY_SYNTAX,
            );
        }
        self.operator_at(end - 2, end);
    }

    fn visit_var_ref(&mut self, reference: &VarRef, children: &mut Vec<(usize, usize)>) {
        let name_span = reference.name_span;
        if self.valid_span(name_span) {
            let (token_type, modifiers, priority) = name_token(&reference.name);
            self.push_span(name_span, token_type, modifiers, priority);
            children.push((name_span.start.offset(), name_span.end.offset()));
        }
        if let Some(subscript) = &reference.subscript {
            let span = subscript.span();
            if self.valid_span(span) {
                let start = span.start.offset();
                let end = span.end.offset();
                let with_brackets = (
                    if start > 0 && self.byte_at(start - 1) == Some(b'[') {
                        start - 1
                    } else {
                        start
                    },
                    if self.byte_at(end) == Some(b']') {
                        end + 1
                    } else {
                        end
                    },
                );
                children.push(with_brackets);
            }
            self.visit_subscript(subscript);
        }
    }

    fn visit_parameter_op(
        &mut self,
        operator: &ParameterOp,
        operand: Option<&Word>,
        children: &mut Vec<(usize, usize)>,
    ) {
        if let Some(operand) = operand
            && self.valid_span(operand.span)
        {
            children.push(word_bounds(operand));
            self.visit_word(operand, WordRole::Value);
        }
        match operator {
            ParameterOp::RemovePrefixShort { pattern }
            | ParameterOp::RemovePrefixLong { pattern }
            | ParameterOp::RemoveSuffixShort { pattern }
            | ParameterOp::RemoveSuffixLong { pattern } => {
                if self.valid_span(pattern.span) {
                    children.push((pattern.span.start.offset(), pattern.span.end.offset()));
                }
                self.visit_pattern(pattern);
            }
            ParameterOp::ReplaceFirst {
                pattern,
                replacement_word_ast,
                ..
            }
            | ParameterOp::ReplaceAll {
                pattern,
                replacement_word_ast,
                ..
            } => {
                if self.valid_span(pattern.span) {
                    children.push((pattern.span.start.offset(), pattern.span.end.offset()));
                }
                self.visit_pattern(pattern);
                if self.valid_span(replacement_word_ast.span) {
                    children.push(word_bounds(replacement_word_ast));
                    self.visit_word(replacement_word_ast, WordRole::Plain);
                }
            }
            ParameterOp::UseDefault
            | ParameterOp::AssignDefault
            | ParameterOp::UseReplacement
            | ParameterOp::Error
            | ParameterOp::UpperFirst
            | ParameterOp::UpperAll
            | ParameterOp::LowerFirst
            | ParameterOp::LowerAll => {}
        }
    }

    fn visit_slice_operand(
        &mut self,
        arithmetic: Option<&ArithmeticExprNode>,
        word: Option<&Word>,
        children: &mut Vec<(usize, usize)>,
    ) {
        if let Some(word) = word
            && self.valid_span(word.span)
        {
            children.push(word_bounds(word));
        }
        if let Some(expression) = arithmetic {
            self.visit_arithmetic(expression);
        } else if let Some(word) = word {
            self.visit_word(word, WordRole::Value);
        }
    }

    fn visit_parameter_expansion(&mut self, parameter: &ParameterExpansion, span: Span) {
        if let ParameterExpansionSyntax::Bourne(BourneParameterExpansion::PrefixMatch {
            prefix,
            ..
        }) = &parameter.syntax
        {
            self.visit_prefix_match(span, prefix);
            return;
        }
        let mut children = Vec::new();
        self.visit_braced_start(span, &mut children, |this, children| {
            match &parameter.syntax {
                ParameterExpansionSyntax::Bourne(bourne) => match bourne {
                    BourneParameterExpansion::Access { reference }
                    | BourneParameterExpansion::Length { reference }
                    | BourneParameterExpansion::Indices { reference }
                    | BourneParameterExpansion::Transformation { reference, .. } => {
                        this.visit_var_ref(reference, children);
                    }
                    BourneParameterExpansion::Indirect {
                        reference,
                        operator,
                        operand_word_ast,
                        ..
                    } => {
                        this.visit_var_ref(reference, children);
                        if let Some(operator) = operator {
                            this.visit_parameter_op(
                                operator,
                                operand_word_ast.as_deref(),
                                children,
                            );
                        } else if let Some(operand) = operand_word_ast {
                            children.push(word_bounds(operand));
                            this.visit_word(operand, WordRole::Value);
                        }
                    }
                    BourneParameterExpansion::PrefixMatch { .. } => {}
                    BourneParameterExpansion::Slice {
                        reference,
                        offset_ast,
                        offset_word_ast,
                        length_ast,
                        length_word_ast,
                        ..
                    } => {
                        this.visit_var_ref(reference, children);
                        this.visit_slice_operand(
                            offset_ast.as_deref(),
                            Some(offset_word_ast),
                            children,
                        );
                        this.visit_slice_operand(
                            length_ast.as_deref(),
                            length_word_ast.as_deref(),
                            children,
                        );
                    }
                    BourneParameterExpansion::Operation {
                        reference,
                        operator,
                        operand_word_ast,
                        ..
                    } => {
                        this.visit_var_ref(reference, children);
                        this.visit_parameter_op(operator, operand_word_ast.as_deref(), children);
                    }
                },
                ParameterExpansionSyntax::Zsh(zsh) => {
                    match &zsh.target {
                        ZshExpansionTarget::Reference(reference) => {
                            this.visit_var_ref(reference, children);
                        }
                        ZshExpansionTarget::Nested(nested) => {
                            if this.valid_span(nested.span) {
                                children
                                    .push((nested.span.start.offset(), nested.span.end.offset()));
                            }
                            this.visit_parameter_expansion(nested, nested.span);
                        }
                        ZshExpansionTarget::Word(word) => {
                            if this.valid_span(word.span) {
                                children.push(word_bounds(word));
                            }
                            this.visit_word(word, WordRole::Plain);
                        }
                        ZshExpansionTarget::Empty => {}
                    }
                    for modifier in &zsh.modifiers {
                        if this.valid_span(modifier.span) {
                            children
                                .push((modifier.span.start.offset(), modifier.span.end.offset()));
                            this.operator(modifier.span);
                        }
                        if let Some(argument) = modifier.argument_word_ast() {
                            this.visit_word(argument, WordRole::Plain);
                        }
                    }
                    if let Some(span) = zsh.length_prefix
                        && this.valid_span(span)
                    {
                        children.push((span.start.offset(), span.end.offset()));
                        this.operator(span);
                    }
                    match &zsh.operation {
                        Some(
                            ZshExpansionOperation::PatternOperation {
                                operand_word_ast, ..
                            }
                            | ZshExpansionOperation::Defaulting {
                                operand_word_ast, ..
                            }
                            | ZshExpansionOperation::TrimOperation {
                                operand_word_ast, ..
                            },
                        ) => {
                            if this.valid_span(operand_word_ast.span) {
                                children.push(word_bounds(operand_word_ast));
                            }
                            this.visit_word(operand_word_ast, WordRole::Value);
                        }
                        Some(ZshExpansionOperation::ReplacementOperation {
                            pattern_word_ast,
                            replacement_word_ast,
                            ..
                        }) => {
                            if this.valid_span(pattern_word_ast.span) {
                                children.push(word_bounds(pattern_word_ast));
                            }
                            this.visit_word(pattern_word_ast, WordRole::Plain);
                            if let Some(replacement) = replacement_word_ast {
                                if this.valid_span(replacement.span) {
                                    children.push(word_bounds(replacement));
                                }
                                this.visit_word(replacement, WordRole::Value);
                            }
                        }
                        Some(ZshExpansionOperation::Slice {
                            offset_word_ast,
                            length_word_ast,
                            ..
                        }) => {
                            this.visit_slice_operand(None, Some(offset_word_ast), children);
                            this.visit_slice_operand(None, length_word_ast.as_deref(), children);
                        }
                        Some(ZshExpansionOperation::Unknown { word_ast, .. }) => {
                            if this.valid_span(word_ast.span) {
                                children.push(word_bounds(word_ast));
                                this.push_span(
                                    word_ast.span,
                                    TOKEN_TYPE_OPERATOR,
                                    0,
                                    PRIORITY_SYNTAX,
                                );
                            }
                        }
                        None => {}
                    }
                }
            }
        });
    }
}

fn word_bounds(word: &Word) -> (usize, usize) {
    (word.span.start.offset(), word.span.end.offset())
}

fn node_bounds(node: &ArithmeticExprNode) -> (usize, usize) {
    (node.span.start.offset(), node.span.end.offset())
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Decimal, floating, or hexadecimal literal spellings that shells accept as numbers.
pub(crate) fn is_numeric_literal(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned.is_empty() {
        return false;
    }
    if let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    !integer.is_empty()
        && integer.bytes().all(|b| b.is_ascii_digit())
        && (!unsigned.contains('.')
            || (!fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit())))
}

/// Operators accepted by the `test` and `[` commands.
pub(crate) fn is_test_operator(text: &str) -> bool {
    matches!(
        text,
        "=" | "=="
            | "!="
            | "!"
            | "<"
            | ">"
            | "-a"
            | "-o"
            | "("
            | ")"
            | "=~"
            | "-eq"
            | "-ne"
            | "-lt"
            | "-le"
            | "-gt"
            | "-ge"
            | "-nt"
            | "-ot"
            | "-ef"
    ) || (text.len() == 2 && text.starts_with('-') && text.as_bytes()[1].is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legend_structure() {
        let legend = semantic_tokens_legend();
        assert_eq!(legend.token_types.len(), 12);
        assert_eq!(
            legend.token_types[TOKEN_TYPE_KEYWORD as usize],
            SemanticTokenType::KEYWORD
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_FUNCTION as usize],
            SemanticTokenType::FUNCTION
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_VARIABLE as usize],
            SemanticTokenType::VARIABLE
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_PARAMETER as usize],
            SemanticTokenType::PARAMETER
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_STRING as usize],
            SemanticTokenType::STRING
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_NUMBER as usize],
            SemanticTokenType::NUMBER
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_OPERATOR as usize],
            SemanticTokenType::OPERATOR
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_COMMENT as usize],
            SemanticTokenType::COMMENT
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_TYPE as usize],
            SemanticTokenType::TYPE
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_SHELL_COMMAND as usize].as_str(),
            "shellCommand"
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_SHELL_ALIAS as usize].as_str(),
            "shellAlias"
        );
        assert_eq!(
            legend.token_types[TOKEN_TYPE_SHELL_OPTION as usize].as_str(),
            "shellOption"
        );

        assert_eq!(legend.token_modifiers.len(), 6);
        assert_eq!(
            legend.token_modifiers[MODIFIER_DECLARATION.trailing_zeros() as usize],
            SemanticTokenModifier::DECLARATION
        );
        assert_eq!(
            legend.token_modifiers[MODIFIER_DEFINITION.trailing_zeros() as usize],
            SemanticTokenModifier::DEFINITION
        );
        assert_eq!(
            legend.token_modifiers[MODIFIER_READONLY.trailing_zeros() as usize],
            SemanticTokenModifier::READONLY
        );
        assert_eq!(
            legend.token_modifiers[MODIFIER_DEFAULT_LIBRARY.trailing_zeros() as usize],
            SemanticTokenModifier::DEFAULT_LIBRARY
        );
        assert_eq!(
            legend.token_modifiers[MODIFIER_INVALID.trailing_zeros() as usize].as_str(),
            "invalid"
        );
        assert_eq!(
            legend.token_modifiers[MODIFIER_STATIC.trailing_zeros() as usize],
            SemanticTokenModifier::STATIC
        );
    }

    #[test]
    fn higher_priority_claims_first_and_lower_keeps_the_remainder() {
        let string = TokenSpan::new(5, 20, TOKEN_TYPE_STRING, 0, PRIORITY_STRING);
        let variable = TokenSpan::new(8, 12, TOKEN_TYPE_VARIABLE, 0, PRIORITY_NAME);
        let resolved = resolve_overlaps(vec![string, variable]);
        assert_eq!(
            resolved,
            vec![
                TokenSpan::new(5, 8, TOKEN_TYPE_STRING, 0, PRIORITY_STRING),
                variable,
                TokenSpan::new(12, 20, TOKEN_TYPE_STRING, 0, PRIORITY_STRING),
            ]
        );
    }

    #[test]
    fn identical_spans_resolve_by_priority_regardless_of_order() {
        let command = TokenSpan::new(0, 4, TOKEN_TYPE_SHELL_COMMAND, 0, PRIORITY_COMMAND);
        let keyword = TokenSpan::new(0, 4, TOKEN_TYPE_KEYWORD, 0, PRIORITY_KEYWORD);
        assert_eq!(resolve_overlaps(vec![command, keyword]), vec![keyword]);
        assert_eq!(resolve_overlaps(vec![keyword, command]), vec![keyword]);
    }

    #[test]
    fn equal_priority_overlaps_are_ordered_by_position_then_type() {
        let a = TokenSpan::new(0, 6, TOKEN_TYPE_OPERATOR, 0, PRIORITY_SYNTAX);
        let b = TokenSpan::new(3, 9, TOKEN_TYPE_NUMBER, 0, PRIORITY_SYNTAX);
        let expected = vec![
            a,
            TokenSpan::new(6, 9, TOKEN_TYPE_NUMBER, 0, PRIORITY_SYNTAX),
        ];
        assert_eq!(resolve_overlaps(vec![a, b]), expected);
        assert_eq!(resolve_overlaps(vec![b, a]), expected);
    }

    #[test]
    fn numeric_and_test_operator_classification() {
        assert!(is_numeric_literal("1"));
        assert!(is_numeric_literal("-5"));
        assert!(is_numeric_literal("0.25"));
        assert!(is_numeric_literal("0xff"));
        assert!(!is_numeric_literal("1."));
        assert!(!is_numeric_literal("-"));
        assert!(!is_numeric_literal("v1"));
        assert!(is_test_operator("-f"));
        assert!(is_test_operator("-nt"));
        assert!(is_test_operator("=="));
        assert!(!is_test_operator("--"));
        assert!(!is_test_operator("-vv"));
    }
}

#[cfg(test)]
#[path = "../../tests/semantic_tokens/mod.rs"]
mod fixture_tests;
