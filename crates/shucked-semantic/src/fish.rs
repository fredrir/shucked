//! Fish document frontend. Its lexer and block recovery are independent of Bourne syntax.
use crate::{CommandNamespace, CommandSiteFacts, CommandWord};
use shucked_ast::{Position, Span};
use std::collections::BTreeSet;

/// A recoverable Fish syntax error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FishDiagnostic {
    /// Source range to mark.
    pub span: Span,
    /// Independently authored explanation.
    pub message: String,
}
/// A source-defined Fish function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FishFunction {
    /// Literal function name.
    pub name: String,
    /// Name range.
    pub name_span: Span,
    /// Definition range, including the closing end when available.
    pub span: Span,
}
/// Explicit Fish syntax/command analysis, never routed through the Bash parser.
#[derive(Debug, Clone, Default)]
pub struct FishDocument {
    /// Source-backed command sites, including command substitutions.
    pub commands: Vec<CommandSiteFacts>,
    /// Recoverable syntax errors.
    pub diagnostics: Vec<FishDiagnostic>,
    /// Source-defined functions.
    pub functions: Vec<FishFunction>,
    /// Per-command visible function names; ranges identify their call sites.
    pub function_calls: Vec<(Span, String)>,
    /// Comment regions, excluding the terminating newline.
    pub comment_spans: Vec<Span>,
}
#[derive(Clone)]
struct Block {
    kind: String,
    span: Span,
    function: Option<usize>,
    guard: Option<String>,
    positive: bool,
    branch_terminates: bool,
    missing_branch_terminates: bool,
    inherited_available: BTreeSet<String>,
}
#[derive(Clone)]
enum Token {
    Word(CommandWord),
    Separator(char, Span),
    Redirect(Span),
}

/// Analyze Fish syntax without running Fish or evaluating expansions.
/// Unknown runtime effects remain explicit on command sites.
pub fn analyze_fish(text: &str) -> FishDocument {
    let mut document = FishDocument::default();
    analyze_region(text, Position::new(), &mut document, 0);
    document
        .commands
        .sort_by_key(|site| site.span.start.offset());
    document
}
fn analyze_region(text: &str, start: Position, document: &mut FishDocument, depth: usize) {
    if depth > 128 {
        document.diagnostics.push(FishDiagnostic {
            span: Span { start, end: start },
            message: "Fish nesting exceeds the analysis limit".into(),
        });
        return;
    }
    let tokens = tokenize(text, start, document, depth);
    let mut blocks: Vec<Block> = vec![];
    let mut words = vec![];
    let mut redirect_pending = None;
    let mut functions = BTreeSet::new();
    let mut uncertain = None;
    let mut available = BTreeSet::new();
    let mut last_operator = None;
    for token in tokens.into_iter().chain(std::iter::once(Token::Separator(
        '\n',
        Span { start, end: start },
    ))) {
        match token {
            Token::Word(word) => {
                if redirect_pending.take().is_none() {
                    words.push(word);
                }
                last_operator = None;
            }
            Token::Redirect(span) => {
                if let Some(previous) = redirect_pending.replace(span) {
                    document.diagnostics.push(FishDiagnostic {
                        span: previous,
                        message: "A redirection needs a destination".into(),
                    });
                }
            }
            Token::Separator(operator, span) => {
                if let Some(redirection) = redirect_pending.take() {
                    document.diagnostics.push(FishDiagnostic {
                        span: redirection,
                        message: "A redirection needs a destination".into(),
                    });
                }
                if words.is_empty() {
                    if operator == '|' {
                        document.diagnostics.push(FishDiagnostic {
                            span,
                            message: "A pipeline needs a command on each side".into(),
                        });
                    }
                } else {
                    process_command(
                        std::mem::take(&mut words),
                        &mut blocks,
                        &mut functions,
                        &mut uncertain,
                        &mut available,
                        operator == '&',
                        document,
                    );
                }
                if operator == '|' {
                    last_operator = Some(span);
                }
            }
        }
    }
    if let Some(span) = last_operator {
        document.diagnostics.push(FishDiagnostic {
            span,
            message: "A pipeline needs a following command".into(),
        });
    }
    for block in blocks {
        document.diagnostics.push(FishDiagnostic {
            span: block.span,
            message: format!("The {} block needs a closing end", block.kind),
        });
    }
}
fn process_command(
    mut words: Vec<CommandWord>,
    blocks: &mut Vec<Block>,
    functions: &mut BTreeSet<String>,
    uncertain: &mut Option<String>,
    available: &mut BTreeSet<String>,
    background: bool,
    doc: &mut FishDocument,
) {
    let span = Span {
        start: words[0].span.start,
        end: words.last().expect("nonempty words").span.end,
    };
    let name = words[0].text.clone().unwrap_or_default();
    match name.as_str() {
        "end" => {
            if let Some(block) = blocks.pop() {
                *available = block.inherited_available;
                if block.kind == "if"
                    && (block.missing_branch_terminates
                        || !block.positive && block.branch_terminates)
                    && let Some(name) = block.guard
                {
                    available.insert(name);
                }
                if let Some(index) = block.function {
                    doc.functions[index].span.end = span.end;
                    if blocks.is_empty() {
                        functions.insert(doc.functions[index].name.clone());
                    }
                }
            } else {
                doc.diagnostics.push(FishDiagnostic {
                    span,
                    message: "No open block matches this end".into(),
                });
            }
            return;
        }
        "else" => {
            if let Some(block) = blocks.last_mut().filter(|b| b.kind == "if") {
                block.missing_branch_terminates |= !block.positive && block.branch_terminates;
                block.branch_terminates = false;
                block.positive = !block.positive;
                *available = block.inherited_available.clone();
                if words.get(1).and_then(|word| word.text.as_deref()) == Some("if") {
                    block.guard = None;
                }
            } else {
                doc.diagnostics.push(FishDiagnostic {
                    span,
                    message: "else requires an open if block".into(),
                });
            }
            if words.get(1).and_then(|w| w.text.as_deref()) == Some("if") {
                words.drain(..2);
            } else {
                return;
            }
        }
        "case" => {
            if blocks.last().is_none_or(|b| b.kind != "switch") {
                doc.diagnostics.push(FishDiagnostic {
                    span,
                    message: "case requires an open switch block".into(),
                });
            }
            return;
        }
        "function" | "for" | "switch" | "begin" | "if" | "while" => {
            let function = if name == "function" {
                if let Some(word) = words.get(1).filter(|w| w.text.is_some()) {
                    let index = doc.functions.len();
                    doc.functions.push(FishFunction {
                        name: word.text.clone().expect("static word"),
                        name_span: word.span,
                        span,
                    });
                    Some(index)
                } else {
                    doc.diagnostics.push(FishDiagnostic {
                        span,
                        message: "A function definition needs a literal name".into(),
                    });
                    None
                }
            } else {
                None
            };
            let negative = words.get(1).and_then(|word| word.text.as_deref()) == Some("not");
            let base = if negative { 2 } else { 1 };
            let guard = if name == "if"
                && words.len() == base + 3
                && words[base].text.as_deref() == Some("command")
                && words[base + 1]
                    .text
                    .as_deref()
                    .is_some_and(|s| matches!(s, "-q" | "--query" | "-s" | "--search"))
            {
                words[base + 2].text.clone()
            } else {
                None
            };
            blocks.push(Block {
                kind: name.clone(),
                span,
                function,
                guard,
                positive: !negative,
                branch_terminates: false,
                missing_branch_terminates: false,
                inherited_available: available.clone(),
            });
            if function.is_some() {
                available.clear();
            }
            if matches!(name.as_str(), "if" | "while") {
                words.remove(0);
                if words.is_empty() {
                    doc.diagnostics.push(FishDiagnostic {
                        span,
                        message: format!("{name} needs a condition command"),
                    });
                    return;
                }
            } else {
                return;
            }
        }
        _ => {}
    }
    if words.is_empty() {
        return;
    }
    let original_words = words.clone();
    while words
        .first()
        .and_then(|w| w.text.as_deref())
        .is_some_and(|s| matches!(s, "and" | "or" | "not" | "time"))
    {
        words.remove(0);
        if words.is_empty() {
            doc.diagnostics.push(FishDiagnostic {
                span,
                message: "A command modifier needs a following command".into(),
            });
            return;
        }
    }
    let mut namespace = CommandNamespace::Shell;
    if let Some(name) = words.first().and_then(|w| w.text.as_deref())
        && matches!(name, "command" | "builtin" | "exec")
        && !words
            .get(1)
            .and_then(|w| w.text.as_deref())
            .is_some_and(|s| s.starts_with('-'))
    {
        namespace = if name == "builtin" {
            CommandNamespace::Builtin
        } else {
            CommandNamespace::External
        };
        words.remove(0);
    }
    let name = words.first().and_then(|w| w.text.as_deref());
    let guarded_available = name.is_some_and(|name| {
        available.contains(name)
            || blocks
                .iter()
                .any(|b| b.positive && b.guard.as_deref() == Some(name))
    });
    let in_function = blocks.iter().any(|b| b.function.is_some());
    if !background
        && matches!(name, Some("exit"))
        && original_words.first().and_then(|word| word.text.as_deref()) == Some("exit")
        && !functions.contains("exit")
        && let Some(block) = blocks.last_mut()
    {
        block.branch_terminates = true;
    }
    let mut site_uncertain = uncertain.clone();
    if in_function {
        site_uncertain = Some("Fish function execution context depends on its callers".into());
    }
    if name.is_none() {
        site_uncertain = Some("Fish command name requires runtime expansion".into());
    }
    if namespace == CommandNamespace::Shell && name.is_some_and(|n| functions.contains(n)) {
        doc.function_calls
            .push((words[0].span, name.expect("known function").into()));
    }
    let mutates = matches!(
        name,
        Some("source" | "eval" | "cd" | "pushd" | "popd" | "fish_add_path")
    ) || name == Some("set")
        && words
            .iter()
            .any(|w| matches!(w.text.as_deref(), Some("PATH" | "fish_user_paths")));
    doc.commands.push(CommandSiteFacts {
        span,
        words: original_words,
        effective_words: words,
        aliases: vec![],
        visible_function: None,
        namespace,
        environment_uncertain: site_uncertain,
        guarded_available,
    });
    if mutates {
        *uncertain =
            Some("Earlier Fish source, directory, or PATH changes may alter command lookup".into());
    }
}
fn tokenize(text: &str, start: Position, doc: &mut FishDocument, depth: usize) -> Vec<Token> {
    let mut tokens = vec![];
    let mut chars = text.char_indices().peekable();
    let mut pos = start;
    while let Some((_, ch)) = chars.peek().copied() {
        if ch == '#' {
            let comment_start = pos;
            let mut comment_end = pos;
            for (_, ch) in chars.by_ref() {
                pos.advance(ch);
                if ch == '\n' {
                    tokens.push(Token::Separator(
                        '\n',
                        Span {
                            start: pos,
                            end: pos,
                        },
                    ));
                    break;
                }
                comment_end = pos;
            }
            doc.comment_spans.push(Span {
                start: comment_start,
                end: comment_end,
            });
            continue;
        }
        if ch.is_whitespace() || matches!(ch, ';' | '|' | '&') {
            chars.next();
            let previous = pos;
            pos.advance(ch);
            if matches!(ch, '\n' | ';' | '|' | '&') {
                tokens.push(Token::Separator(
                    if ch == '|' && chars.peek().is_some_and(|(_, c)| *c == '|') {
                        chars.next();
                        pos.advance('|');
                        ';'
                    } else if ch == '&' && chars.peek().is_some_and(|(_, c)| *c == '&') {
                        chars.next();
                        pos.advance('&');
                        ';'
                    } else {
                        ch
                    },
                    Span {
                        start: previous,
                        end: pos,
                    },
                ));
            }
            continue;
        }
        if matches!(ch, '<' | '>') {
            let previous = pos;
            let mut descriptor_redirect = false;
            while let Some((_, ch)) = chars
                .peek()
                .copied()
                .filter(|(_, c)| matches!(c, '<' | '>' | '?' | '&') || c.is_ascii_digit())
            {
                chars.next();
                pos.advance(ch);
                descriptor_redirect |= ch == '&';
            }
            if !descriptor_redirect {
                tokens.push(Token::Redirect(Span {
                    start: previous,
                    end: pos,
                }));
            }
            continue;
        }
        let word_start = pos;
        let mut literal = String::new();
        let mut dynamic = false;
        let mut quote = None;
        while let Some((offset, ch)) = chars.peek().copied() {
            if quote.is_none() && (ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '<' | '>'))
            {
                break;
            }
            chars.next();
            pos.advance(ch);
            if ch == '\\' {
                if let Some((_, next)) = chars.next() {
                    pos.advance(next);
                    if next == '\n' {
                        continue;
                    }
                    if quote.is_some() && !matches!(next, '\\' | '\'' | '"' | '$') {
                        literal.push('\\');
                    }
                    if quote.is_none() && next.is_ascii_alphanumeric() {
                        dynamic = true;
                    }
                    literal.push(next);
                } else {
                    doc.diagnostics.push(FishDiagnostic {
                        span: Span {
                            start: word_start,
                            end: pos,
                        },
                        message: "The escape needs a following character".into(),
                    });
                }
                continue;
            }
            if matches!(ch, '\'' | '"') {
                if quote == Some(ch) {
                    quote = None
                } else if quote.is_none() {
                    quote = Some(ch)
                } else {
                    literal.push(ch)
                }
                continue;
            }
            if ch == '(' && (quote.is_none() || quote == Some('"') && literal.ends_with('$')) {
                dynamic = true;
                let nested_start = pos;
                let body_start = offset + 1;
                let mut nesting = 1;
                let mut nested_quote = None;
                let mut escaped = false;
                let mut body_end = text.len();
                for (index, next) in chars.by_ref() {
                    pos.advance(next);
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if next == '\\' {
                        escaped = true;
                        continue;
                    }
                    if matches!(next, '\'' | '"') {
                        if nested_quote == Some(next) {
                            nested_quote = None
                        } else if nested_quote.is_none() {
                            nested_quote = Some(next)
                        }
                        continue;
                    }
                    if nested_quote.is_none() {
                        if next == '(' {
                            nesting += 1
                        } else if next == ')' {
                            nesting -= 1;
                            if nesting == 0 {
                                body_end = index;
                                break;
                            }
                        }
                    }
                }
                if nesting != 0 {
                    doc.diagnostics.push(FishDiagnostic {
                        span: Span {
                            start: nested_start,
                            end: pos,
                        },
                        message: "The command substitution needs a closing parenthesis".into(),
                    });
                }
                analyze_region(&text[body_start..body_end], nested_start, doc, depth + 1);
                continue;
            }
            if ch == ')' && quote.is_none() {
                doc.diagnostics.push(FishDiagnostic {
                    span: Span {
                        start: word_start,
                        end: pos,
                    },
                    message: "No command substitution matches this parenthesis".into(),
                });
                dynamic = true;
            }
            if ch == '$' && quote != Some('\'')
                || quote.is_none() && matches!(ch, '*' | '?' | '{' | '}' | '~')
            {
                dynamic = true;
            }
            literal.push(ch);
        }
        if quote.is_some() {
            doc.diagnostics.push(FishDiagnostic {
                span: Span {
                    start: word_start,
                    end: pos,
                },
                message: "The quoted word needs a closing quote".into(),
            });
        }
        // Numeric file descriptor prefixes belong to a following redirection.
        if literal.bytes().all(|c| c.is_ascii_digit())
            && !literal.is_empty()
            && chars.peek().is_some_and(|(_, c)| matches!(c, '<' | '>'))
        {
            continue;
        }
        tokens.push(Token::Word(CommandWord {
            text: (!dynamic).then_some(literal),
            span: Span {
                start: word_start,
                end: pos,
            },
            alias_eligible: false,
            injected: false,
        }));
    }
    tokens
}
