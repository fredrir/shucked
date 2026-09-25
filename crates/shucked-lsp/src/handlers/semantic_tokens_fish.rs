//! Fish semantic tokens consume the same command identities as diagnostics and hover.
//!
//! Command names come from the Fish analysis; comments, keywords, strings,
//! variables, options and numbers come from a small quote-aware scan of the
//! source that only classifies bare words by their position in a command.
use lsp_types::SemanticTokens;

use super::semantic_tokens::{
    MODIFIER_DECLARATION, MODIFIER_DEFAULT_LIBRARY, MODIFIER_DEFINITION, MODIFIER_INVALID,
    PRIORITY_COMMAND, PRIORITY_COMMENT, PRIORITY_KEYWORD, PRIORITY_NAME, PRIORITY_PUNCTUATION,
    PRIORITY_SPECIAL_NAME, PRIORITY_STRING, PRIORITY_SYNTAX, TOKEN_TYPE_COMMENT,
    TOKEN_TYPE_FUNCTION, TOKEN_TYPE_KEYWORD, TOKEN_TYPE_NUMBER, TOKEN_TYPE_OPERATOR,
    TOKEN_TYPE_PARAMETER, TOKEN_TYPE_SHELL_COMMAND, TOKEN_TYPE_SHELL_OPTION, TOKEN_TYPE_STRING,
    TOKEN_TYPE_VARIABLE, TokenSpan, encode_tokens, is_numeric_literal, is_test_operator,
};
use crate::session::DocumentSnapshot;

/// Reserved words that open, continue or close Fish blocks and pipelines.
const KEYWORDS: &[&str] = &[
    "if", "else", "end", "for", "in", "while", "function", "switch", "case", "begin", "and", "or",
    "not", "return", "break", "continue",
];

/// Words after which the next word is again in command position.
const CHAINING: &[&str] = &[
    "if", "else", "while", "and", "or", "not", "begin", "time", "command", "builtin", "exec",
];

/// Variables Fish itself maintains.
const SPECIAL_VARIABLES: &[&str] = &[
    "status",
    "argv",
    "pipestatus",
    "fish_pid",
    "history",
    "version",
    "hostname",
    "CMD_DURATION",
    "PWD",
    "HOME",
    "USER",
    "PATH",
    "SHLVL",
    "COLUMNS",
    "LINES",
];

pub(crate) fn full(snapshot: &DocumentSnapshot) -> SemanticTokens {
    let document = snapshot.query().document();
    let source = document.contents();
    let fish = shucked_semantic::analyze_fish(source);
    let analysis = snapshot.command_service.analysis(snapshot);
    let mut tokens = Vec::new();

    for span in &fish.comment_spans {
        push(
            &mut tokens,
            source,
            span.start.offset(),
            span.end.offset(),
            TOKEN_TYPE_COMMENT,
            0,
            PRIORITY_COMMENT,
        );
    }
    for function in &fish.functions {
        push(
            &mut tokens,
            source,
            function.name_span.start.offset(),
            function.name_span.end.offset(),
            TOKEN_TYPE_FUNCTION,
            MODIFIER_DECLARATION | MODIFIER_DEFINITION,
            PRIORITY_KEYWORD,
        );
    }
    for (site, resolution) in &analysis.sites {
        let (token_type, modifiers) = match resolution {
            shucked_command::CommandResolution::Resolved(resolved)
                if resolved.kind == shucked_command::CommandKind::Function =>
            {
                (TOKEN_TYPE_FUNCTION, 0)
            }
            shucked_command::CommandResolution::Resolved(resolved) => (
                TOKEN_TYPE_SHELL_COMMAND,
                if resolved.kind == shucked_command::CommandKind::Builtin {
                    MODIFIER_DEFAULT_LIBRARY
                } else {
                    0
                },
            ),
            shucked_command::CommandResolution::Missing(_) => {
                (TOKEN_TYPE_SHELL_COMMAND, MODIFIER_INVALID)
            }
            shucked_command::CommandResolution::Unknown(_) => (TOKEN_TYPE_SHELL_COMMAND, 0),
        };
        let name_span = site.name_span();
        push(
            &mut tokens,
            source,
            name_span.start.offset(),
            name_span.end.offset(),
            token_type,
            modifiers,
            PRIORITY_COMMAND,
        );
        for word in &site.words {
            if word.span.start.offset() < name_span.start.offset()
                && word
                    .text
                    .as_deref()
                    .is_some_and(|text| matches!(text, "command" | "builtin" | "exec"))
            {
                push(
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
    }

    let mut scanner = FishScanner {
        source,
        tokens: &mut tokens,
    };
    scanner.scan();

    encode_tokens(tokens, source, document.index(), snapshot.encoding())
}

fn push(
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

/// Per-command state of the scan.
#[derive(Debug, Clone, Copy)]
struct CommandState<'a> {
    /// The command word, once seen; `None` while in command position.
    name: Option<&'a str>,
    /// Arguments seen so far after the command word.
    arg_index: usize,
    /// Whether `-flags` still count as options (`--` ends them).
    options_allowed: bool,
    /// For `set`: whether the target name was already painted.
    target_seen: bool,
    /// For `set`: whether a query/erase flag makes the target a reference.
    query: bool,
}

impl CommandState<'_> {
    const START: Self = Self {
        name: None,
        arg_index: 0,
        options_allowed: true,
        target_seen: false,
        query: false,
    };

    fn in_command_position(&self) -> bool {
        self.name.is_none()
    }
}

struct FishScanner<'a> {
    source: &'a str,
    tokens: &'a mut Vec<TokenSpan>,
}

impl<'a> FishScanner<'a> {
    fn push(&mut self, start: usize, end: usize, token_type: u32, modifiers: u32, priority: u8) {
        push(
            self.tokens,
            self.source,
            start,
            end,
            token_type,
            modifiers,
            priority,
        );
    }

    fn scan(&mut self) {
        let bytes = self.source.as_bytes();
        let len = bytes.len();
        let mut i = 0;
        let mut state = CommandState::START;
        let mut enclosing: Vec<CommandState<'a>> = Vec::new();

        while i < len {
            let byte = bytes[i];
            if byte == b'#' {
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            if byte == b'\n' {
                state = CommandState::START;
                i += 1;
                continue;
            }
            if matches!(byte, b';' | b'|' | b'&') {
                let start = i;
                while i < len && matches!(bytes[i], b';' | b'|' | b'&') {
                    i += 1;
                }
                self.push(start, i, TOKEN_TYPE_OPERATOR, 0, PRIORITY_SYNTAX);
                state = CommandState::START;
                continue;
            }
            if matches!(byte, b'<' | b'>') {
                let start = i;
                while i < len && matches!(bytes[i], b'<' | b'>' | b'&' | b'?' | b'|') {
                    i += 1;
                }
                self.push(start, i, TOKEN_TYPE_OPERATOR, 0, PRIORITY_SYNTAX);
                continue;
            }
            if byte == b'(' {
                enclosing.push(state);
                state = CommandState::START;
                i += 1;
                continue;
            }
            if byte == b')' {
                if let Some(saved) = enclosing.pop() {
                    state = saved;
                    if state.in_command_position() {
                        state.name = Some("");
                    }
                }
                i += 1;
                continue;
            }
            if byte.is_ascii_whitespace() {
                i += 1;
                continue;
            }

            let start = i;
            let mut quote = None;
            let mut quoted = false;
            while i < len {
                let c = bytes[i];
                if let Some(open) = quote {
                    if c == b'\\' && open == b'"' {
                        i = (i + 2).min(len);
                        continue;
                    }
                    if c == open {
                        quote = None;
                    }
                    i += 1;
                    continue;
                }
                if c == b'\\' {
                    i = (i + 2).min(len);
                    continue;
                }
                if matches!(c, b'\'' | b'"') {
                    quote = Some(c);
                    quoted = true;
                    i += 1;
                    continue;
                }
                if c.is_ascii_whitespace()
                    || matches!(c, b'(' | b')' | b';' | b'|' | b'&' | b'<' | b'>')
                {
                    break;
                }
                i += 1;
            }
            let end = i.min(len);
            if end <= start {
                i = start + 1;
                continue;
            }
            self.paint_word_contents(start, end);
            let text = &self.source[start..end];

            if state.in_command_position() {
                if quoted {
                    state.name = Some("");
                } else if KEYWORDS.contains(&text) {
                    self.push(start, end, TOKEN_TYPE_KEYWORD, 0, PRIORITY_KEYWORD);
                    if !CHAINING.contains(&text) {
                        state.name = Some(text);
                    }
                } else if text == "[" {
                    self.push(start, end, TOKEN_TYPE_OPERATOR, 0, PRIORITY_PUNCTUATION);
                    state.name = Some(text);
                } else {
                    state.name = Some(text);
                }
                continue;
            }

            state.arg_index += 1;
            if quoted {
                continue;
            }
            match state.name {
                Some("for") => {
                    if state.arg_index == 1 {
                        self.push(
                            start,
                            end,
                            TOKEN_TYPE_VARIABLE,
                            MODIFIER_DECLARATION,
                            PRIORITY_SYNTAX,
                        );
                    } else if state.arg_index == 2 && text == "in" {
                        self.push(start, end, TOKEN_TYPE_KEYWORD, 0, PRIORITY_KEYWORD);
                    } else {
                        self.paint_value(start, end, text);
                    }
                }
                Some("function") => {
                    if state.arg_index > 1 {
                        self.paint_argument(start, end, text, &mut state);
                    }
                }
                Some("set") => {
                    if state.options_allowed && is_option(text) {
                        if matches!(text, "-q" | "--query" | "-e" | "--erase" | "-S" | "--show") {
                            state.query = true;
                        }
                        self.paint_argument(start, end, text, &mut state);
                    } else if !state.target_seen {
                        state.target_seen = true;
                        let name_end = start + text.find('[').unwrap_or(text.len());
                        let modifiers = if state.query { 0 } else { MODIFIER_DECLARATION };
                        self.push(
                            start,
                            name_end,
                            TOKEN_TYPE_VARIABLE,
                            modifiers,
                            PRIORITY_SYNTAX,
                        );
                    } else {
                        self.paint_value(start, end, text);
                    }
                }
                Some("test" | "[") => {
                    if text == "]" {
                        self.push(start, end, TOKEN_TYPE_OPERATOR, 0, PRIORITY_PUNCTUATION);
                    } else if is_test_operator(text) {
                        self.push(start, end, TOKEN_TYPE_OPERATOR, 0, PRIORITY_SYNTAX);
                    } else {
                        self.paint_value(start, end, text);
                    }
                }
                _ => self.paint_argument(start, end, text, &mut state),
            }
        }
    }

    fn paint_argument(&mut self, start: usize, end: usize, text: &str, state: &mut CommandState) {
        if state.options_allowed && text == "--" {
            self.push(start, end, TOKEN_TYPE_SHELL_OPTION, 0, PRIORITY_SYNTAX);
            state.options_allowed = false;
        } else if state.options_allowed && is_option(text) {
            let name_end = text
                .find('=')
                .filter(|&offset| offset > 1)
                .map_or(end, |offset| start + offset);
            self.push(start, name_end, TOKEN_TYPE_SHELL_OPTION, 0, PRIORITY_SYNTAX);
        } else {
            self.paint_value(start, end, text);
        }
    }

    fn paint_value(&mut self, start: usize, end: usize, text: &str) {
        if is_numeric_literal(text) {
            self.push(start, end, TOKEN_TYPE_NUMBER, 0, PRIORITY_SYNTAX);
        }
    }

    /// Paints quoted segments as strings and `$name` expansions as variables.
    fn paint_word_contents(&mut self, start: usize, end: usize) {
        let bytes = self.source.as_bytes();
        let mut i = start;
        while i < end {
            match bytes[i] {
                b'\\' => i = (i + 2).min(end),
                b'\'' => {
                    let quote_start = i;
                    i += 1;
                    while i < end && bytes[i] != b'\'' {
                        i += 1;
                    }
                    i = (i + 1).min(end);
                    self.push(quote_start, i, TOKEN_TYPE_STRING, 0, PRIORITY_SYNTAX);
                }
                b'"' => {
                    let quote_start = i;
                    i += 1;
                    let mut holes = Vec::new();
                    while i < end && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i = (i + 2).min(end);
                        } else if bytes[i] == b'$'
                            && let Some(name_end) = self.paint_variable(i, end)
                        {
                            holes.push((i, name_end));
                            i = name_end;
                        } else {
                            i += 1;
                        }
                    }
                    i = (i + 1).min(end);
                    let mut cursor = quote_start;
                    for (hole_start, hole_end) in holes {
                        if hole_start > cursor {
                            self.push(cursor, hole_start, TOKEN_TYPE_STRING, 0, PRIORITY_STRING);
                        }
                        cursor = hole_end;
                    }
                    if cursor < i {
                        self.push(cursor, i, TOKEN_TYPE_STRING, 0, PRIORITY_STRING);
                    }
                }
                b'$' => {
                    if let Some(name_end) = self.paint_variable(i, end) {
                        i = name_end;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
    }

    /// Paints `$name` at `start` and returns the end of the name.
    fn paint_variable(&mut self, start: usize, limit: usize) -> Option<usize> {
        let bytes = self.source.as_bytes();
        let mut i = start + 1;
        while i < limit && bytes[i] == b'$' {
            i += 1;
        }
        let name_start = i;
        while i < limit && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == name_start {
            return None;
        }
        let name = &self.source[name_start..i];
        if SPECIAL_VARIABLES.contains(&name) {
            self.push(
                start,
                i,
                TOKEN_TYPE_PARAMETER,
                MODIFIER_DEFAULT_LIBRARY,
                PRIORITY_SPECIAL_NAME,
            );
        } else {
            self.push(start, i, TOKEN_TYPE_VARIABLE, 0, PRIORITY_NAME);
        }
        Some(i)
    }
}

fn is_option(text: &str) -> bool {
    text.len() > 1
        && text.starts_with('-')
        && text
            .as_bytes()
            .get(1)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}
