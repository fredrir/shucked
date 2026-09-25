//! Operator completions inside `[[ ... ]]`, `[ ... ]` and `test` commands.
//!
//! The site is located by re-scanning the raw text before the cursor rather than
//! from the parsed word list: the generic context resets its words at `&&`/`||`,
//! which are connectors inside `[[ ]]`, and it cannot say whether the token before
//! the cursor was an operand (expecting a binary operator) or an operator
//! (expecting its operand).
use lsp_types as types;

use crate::analysis::DocumentAnalysis;
use crate::handlers::completion::context::{Quote, Site};
use crate::session::DocumentSnapshot;

/// The test command enclosing the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opener {
    /// `[[ ... ]]` as in bash, ksh and zsh.
    Conditional,
    /// `[ ... ]`.
    Bracket,
    /// `test ...`.
    Test,
}

/// What the test grammar accepts at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    /// The start of a test: a unary operator, `!`, a group or an operand.
    Test,
    /// The operand that follows a unary or binary operator.
    Operand,
    /// A binary operator or a connector after a bare operand.
    Operator,
    /// A connector or the closer after a complete test.
    Connector,
}

struct Operator {
    label: &'static str,
    detail: &'static str,
    documentation: &'static str,
    /// Specified for the POSIX `test` utility.
    portable: bool,
}

const fn op(
    label: &'static str,
    detail: &'static str,
    documentation: &'static str,
    portable: bool,
) -> Operator {
    Operator {
        label,
        detail,
        documentation,
        portable,
    }
}

/// Operators that take a single operand, in presentation order.
const UNARY: &[Operator] = &[
    op(
        "-a",
        "existing file",
        "True when the file exists; the same test as -e.",
        false,
    ),
    op("-b", "block special file", "", true),
    op("-c", "character special file", "", true),
    op("-d", "directory", "", true),
    op("-e", "existing file", "", true),
    op("-f", "regular file", "", true),
    op(
        "-G",
        "group-owned file",
        "True when the file exists and its group matches the effective group id.",
        false,
    ),
    op(
        "-g",
        "setgid bit",
        "True when the file exists and its set-group-id bit is set.",
        true,
    ),
    op("-h", "symbolic link", "", true),
    op(
        "-k",
        "sticky bit",
        "True when the file exists and its sticky bit is set.",
        false,
    ),
    op("-L", "symbolic link", "", true),
    op(
        "-n",
        "non-empty string",
        "True when the string has a length greater than zero.",
        true,
    ),
    op(
        "-N",
        "modified since last read",
        "True when the file exists and was modified after it was last read.",
        false,
    ),
    op(
        "-o",
        "shell option set",
        "True when the named shell option is enabled, as with set -o.",
        false,
    ),
    op(
        "-O",
        "owned by current user",
        "True when the file exists and is owned by the effective user id.",
        false,
    ),
    op("-p", "named pipe", "", true),
    op("-r", "readable file", "", true),
    op(
        "-s",
        "non-empty file",
        "True when the file exists and its size is greater than zero.",
        true,
    ),
    op("-S", "socket", "", true),
    op(
        "-t",
        "open terminal file descriptor",
        "True when the file descriptor number is open and refers to a terminal.",
        true,
    ),
    op(
        "-u",
        "setuid bit",
        "True when the file exists and its set-user-id bit is set.",
        true,
    ),
    op(
        "-v",
        "set variable",
        "True when the named shell variable has been assigned a value (bash, ksh, zsh).",
        false,
    ),
    op("-w", "writable file", "", true),
    op(
        "-x",
        "executable file",
        "True when the file exists and can be executed, or is a directory that can be searched.",
        true,
    ),
    op(
        "-z",
        "empty string",
        "True when the string has zero length.",
        true,
    ),
    op(
        "-R",
        "name reference variable (bash)",
        "True when the named variable is set and was declared as a name reference.",
        false,
    ),
    op(
        "!",
        "negation",
        "Inverts the result of the test that follows.",
        true,
    ),
];

/// Operators that compare two operands, in presentation order.
const BINARY: &[Operator] = &[
    op(
        "-nt",
        "newer than",
        "True when the left file has a more recent modification time, or the right file does not exist.",
        false,
    ),
    op(
        "-ot",
        "older than",
        "True when the left file has an older modification time, or the left file does not exist.",
        false,
    ),
    op(
        "-ef",
        "same file",
        "True when both names refer to the same device and inode.",
        false,
    ),
    op("-eq", "equal integers", "", true),
    op("-ne", "unequal integers", "", true),
    op("-lt", "less than (integers)", "", true),
    op("-le", "less than or equal (integers)", "", true),
    op("-gt", "greater than (integers)", "", true),
    op("-ge", "greater than or equal (integers)", "", true),
    op(
        "=",
        "equal strings",
        "Inside [[ ]] an unquoted right side is matched as a pattern.",
        true,
    ),
    op(
        "==",
        "equal strings",
        "Inside [[ ]] an unquoted right side is matched as a pattern.",
        false,
    ),
    op(
        "!=",
        "unequal strings",
        "Inside [[ ]] an unquoted right side is matched as a pattern.",
        true,
    ),
    op(
        "=~",
        "regex match",
        "True when the left string matches the extended regular expression on the right; captured groups are stored in BASH_REMATCH.",
        false,
    ),
    op(
        "<",
        "sorts before (strings)",
        "Compares the two strings in collation order.",
        false,
    ),
    op(
        ">",
        "sorts after (strings)",
        "Compares the two strings in collation order.",
        false,
    ),
];

/// Connectors between tests inside `[[ ... ]]`.
const CONDITIONAL_CONNECTORS: &[Operator] = &[
    op(
        "&&",
        "and (both tests)",
        "True when both tests succeed; the right side is skipped when the left one fails.",
        false,
    ),
    op(
        "||",
        "or (either test)",
        "True when either test succeeds; the right side is skipped when the left one passes.",
        false,
    ),
];

/// Connectors between tests inside `[ ... ]` and `test`.
const BRACKET_CONNECTORS: &[Operator] = &[
    op(
        "-a",
        "and (both tests)",
        "Combines two tests; POSIX marks it obsolescent, so separate [ ] commands joined with && are preferred.",
        true,
    ),
    op(
        "-o",
        "or (either test)",
        "Combines two tests; POSIX marks it obsolescent, so separate [ ] commands joined with || are preferred.",
        true,
    ),
];

/// Replace `items` with test operators when the cursor sits where the grammar of
/// an enclosing `[[`, `[` or `test` command accepts one. Returns whether the site
/// was claimed, in which case the caller skips the shell-backed providers.
pub(in crate::handlers) fn apply(
    site: &Site,
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    range: types::Range,
    items: &mut Vec<types::CompletionItem>,
) -> bool {
    let Some((opener, expect)) = site_expectation(site, analysis.source()) else {
        return false;
    };
    let conditional = opener == Opener::Conditional;
    let offered: Vec<&Operator> = match expect {
        Expect::Test => UNARY.iter().collect(),
        Expect::Operator => BINARY
            .iter()
            .filter(|operator| conditional || operator.label != "=~")
            .chain(connectors(opener))
            .collect(),
        Expect::Connector => connectors(opener).collect(),
        Expect::Operand => return false,
    };
    let strict = !conditional && crate::handlers::commands::dialect(snapshot) == "sh";
    items.clear();
    for (index, operator) in offered.into_iter().enumerate() {
        if !operator.label.starts_with(site.prefix.as_str()) {
            continue;
        }
        let text = if !conditional && matches!(operator.label, "<" | ">") {
            format!("\\{}", operator.label)
        } else {
            operator.label.to_owned()
        };
        let detail = if strict && !operator.portable {
            format!("{} (not POSIX)", operator.detail)
        } else {
            operator.detail.to_owned()
        };
        items.push(types::CompletionItem {
            label: operator.label.to_owned(),
            kind: Some(types::CompletionItemKind::OPERATOR),
            detail: Some(detail),
            documentation: (!operator.documentation.is_empty())
                .then(|| types::Documentation::String(operator.documentation.to_owned())),
            sort_text: Some(format!("0:{index:02}")),
            filter_text: Some(text.clone()),
            insert_text_format: Some(types::InsertTextFormat::PLAIN_TEXT),
            text_edit: Some(types::CompletionTextEdit::Edit(types::TextEdit::new(
                range, text,
            ))),
            ..Default::default()
        });
    }
    true
}

fn connectors(opener: Opener) -> std::slice::Iter<'static, Operator> {
    match opener {
        Opener::Conditional => CONDITIONAL_CONNECTORS.iter(),
        Opener::Bracket | Opener::Test => BRACKET_CONNECTORS.iter(),
    }
}

fn site_expectation(site: &Site, source: &str) -> Option<(Opener, Expect)> {
    if site.quote != Quote::None
        || site.redirect
        || site.option.is_some()
        || site.suffix.starts_with(']')
        || !(site.prefix.is_empty() || site.prefix.starts_with(['-', '=', '!', '<', '>']))
    {
        return None;
    }
    let before = source.get(..site.range.start)?;
    let segment = &before[segment_start(before)..];
    let (opener, expect) = enclosing_test(segment)?;
    (expect != Expect::Operand).then_some((opener, expect))
}

/// Start of the command line the cursor is on, walking back over `\`-newline
/// continuations and lines ended by a pipe or connector.
fn segment_start(text: &str) -> usize {
    let mut end = text.len();
    loop {
        let Some(newline) = text[..end].rfind('\n') else {
            return 0;
        };
        let line = text[..newline].trim_end_matches([' ', '\t', '\r']);
        let escaped = line.chars().rev().take_while(|ch| *ch == '\\').count() % 2 == 1;
        if !(escaped || line.ends_with("&&") || line.ends_with('|')) {
            return newline + 1;
        }
        end = newline;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token<'a> {
    Word(&'a str),
    Open,
    Close,
    And,
    Or,
    Separator,
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    fn flush<'a>(text: &'a str, word: &mut Option<usize>, end: usize, out: &mut Vec<Token<'a>>) {
        if let Some(start) = word.take() {
            out.push(Token::Word(&text[start..end]));
        }
    }
    let mut out = Vec::new();
    let mut word = None;
    let mut chars = text.char_indices().peekable();
    while let Some((pos, ch)) = chars.next() {
        match ch {
            '\\' => {
                word.get_or_insert(pos);
                chars.next();
            }
            '\'' | '`' => {
                word.get_or_insert(pos);
                for (_, next) in chars.by_ref() {
                    if next == ch {
                        break;
                    }
                }
            }
            '"' => {
                word.get_or_insert(pos);
                let mut escaped = false;
                for (_, next) in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if next == '\\' {
                        escaped = true;
                    } else if next == '"' {
                        break;
                    }
                }
            }
            '$' if chars
                .peek()
                .is_some_and(|(_, next)| matches!(next, '(' | '{')) =>
            {
                word.get_or_insert(pos);
                let open = chars.next().map_or('(', |(_, open)| open);
                let close = if open == '(' { ')' } else { '}' };
                let mut depth = 1usize;
                while let Some((_, next)) = chars.next() {
                    if next == '\\' {
                        chars.next();
                    } else if next == open {
                        depth += 1;
                    } else if next == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            '(' => {
                flush(text, &mut word, pos, &mut out);
                out.push(Token::Open);
            }
            ')' => {
                flush(text, &mut word, pos, &mut out);
                out.push(Token::Close);
            }
            '&' | '|' => {
                flush(text, &mut word, pos, &mut out);
                if chars.peek().is_some_and(|(_, next)| *next == ch) {
                    chars.next();
                    out.push(if ch == '&' { Token::And } else { Token::Or });
                } else {
                    out.push(Token::Separator);
                }
            }
            ';' => {
                flush(text, &mut word, pos, &mut out);
                out.push(Token::Separator);
            }
            '{' | '}'
                if word.is_none()
                    && chars
                        .peek()
                        .is_none_or(|(_, next)| next.is_whitespace() || *next == ';') =>
            {
                out.push(Token::Separator);
            }
            '#' if word.is_none() => {
                for (_, next) in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
            }
            '<' | '>' => {
                flush(text, &mut word, pos, &mut out);
                out.push(Token::Word(&text[pos..pos + 1]));
            }
            _ if ch.is_whitespace() => flush(text, &mut word, pos, &mut out),
            _ => {
                word.get_or_insert(pos);
            }
        }
    }
    flush(text, &mut word, text.len(), &mut out);
    out
}

/// Walk the command line up to the cursor and report the test command still open
/// there, together with what its grammar expects next.
fn enclosing_test(segment: &str) -> Option<(Opener, Expect)> {
    let mut active: Option<(Opener, Expect)> = None;
    let mut command_position = true;
    for token in tokens(segment) {
        match active {
            Some((opener, expect)) => match token {
                Token::Word(word) if opener != Opener::Test && word == closer(opener) => {
                    active = None;
                    command_position = false;
                }
                Token::And | Token::Or | Token::Open if opener == Opener::Conditional => {
                    active = Some((opener, Expect::Test));
                }
                Token::Close if opener == Opener::Conditional => {
                    active = Some((opener, Expect::Connector));
                }
                Token::Word(word) => active = Some((opener, step(opener, expect, word))),
                _ => {
                    active = None;
                    command_position = true;
                }
            },
            None => match token {
                Token::Word("[[") if command_position => {
                    active = Some((Opener::Conditional, Expect::Test));
                }
                Token::Word("[") if command_position => {
                    active = Some((Opener::Bracket, Expect::Test));
                }
                Token::Word("test") if command_position => {
                    active = Some((Opener::Test, Expect::Test));
                }
                Token::Word(word) => {
                    if matches!(
                        word,
                        "if" | "then"
                            | "else"
                            | "elif"
                            | "while"
                            | "until"
                            | "do"
                            | "!"
                            | "time"
                            | "command"
                            | "builtin"
                            | "exec"
                            | "env"
                            | "sudo"
                    ) {
                        command_position = true;
                    } else if !assignment(word) {
                        command_position = false;
                    }
                }
                _ => command_position = true,
            },
        }
    }
    active
}

fn closer(opener: Opener) -> &'static str {
    match opener {
        Opener::Conditional => "]]",
        Opener::Bracket => "]",
        Opener::Test => "",
    }
}

fn step(opener: Opener, expect: Expect, word: &str) -> Expect {
    let grouped = opener != Opener::Conditional;
    let bare = bare(word);
    match expect {
        Expect::Test if word == "!" || (grouped && bare == "(") => Expect::Test,
        Expect::Test if unary(word) => Expect::Operand,
        Expect::Test if grouped && bare == ")" => Expect::Connector,
        Expect::Test => Expect::Operator,
        Expect::Operand => Expect::Connector,
        Expect::Operator | Expect::Connector => {
            if connector(opener, word) {
                Expect::Test
            } else if binary(opener, bare) {
                Expect::Operand
            } else if grouped && bare == ")" {
                Expect::Connector
            } else {
                Expect::Operator
            }
        }
    }
}

/// Strip one layer of quoting so `\<`, `'<'` and `"<"` read as the operator.
fn bare(word: &str) -> &str {
    if let Some(rest) = word.strip_prefix('\\') {
        rest
    } else if let Some(rest) = word
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        rest
    } else if let Some(rest) = word
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        rest
    } else {
        word
    }
}

fn unary(word: &str) -> bool {
    word.len() == 2 && word.starts_with('-') && UNARY.iter().any(|op| op.label == word)
}

fn binary(opener: Opener, word: &str) -> bool {
    (opener == Opener::Conditional || word != "=~") && BINARY.iter().any(|op| op.label == word)
}

fn connector(opener: Opener, word: &str) -> bool {
    opener != Opener::Conditional && matches!(word, "-a" | "-o")
}

fn assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| crate::handlers::completion::context::identifier(name))
}

#[cfg(test)]
#[path = "../../../tests/completion/test_operators.rs"]
mod tests;
