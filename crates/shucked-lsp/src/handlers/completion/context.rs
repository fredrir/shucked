use std::ops::Range;

use shucked_ast::TextSize;
use shucked_indexer::{Indexer, RegionKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::handlers) enum Quote {
    #[default]
    None,
    Single,
    Double,
}

#[derive(Clone, Debug)]
pub(in crate::handlers) struct Site {
    pub range: Range<usize>,
    pub prefix: String,
    pub suffix: String,
    pub option: Option<String>,
    pub words: Vec<String>,
    pub quote: Quote,
    pub closed_quote: bool,
    pub redirect: bool,
    pub command: bool,
}

#[derive(Default)]
struct Frame {
    words: Vec<String>,
    start: Option<usize>,
    word: String,
    quote: Quote,
    redirect: bool,
}

impl Frame {
    fn finish(&mut self) {
        if self.start.take().is_some() {
            if self.redirect {
                self.redirect = false;
            } else {
                self.words.push(std::mem::take(&mut self.word));
            }
            self.word.clear();
        }
    }
}

pub(in crate::handlers) fn at(source: &str, indexer: &Indexer, offset: usize) -> Option<Site> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    for probe in [offset, offset.saturating_sub(1)] {
        if matches!(
            indexer
                .region_index()
                .region_at(TextSize::new(probe as u32)),
            Some(RegionKind::Heredoc | RegionKind::Arithmetic)
        ) {
            return None;
        }
    }
    if indexer
        .comment_index()
        .is_comment(TextSize::new(offset as u32))
    {
        return None;
    }

    let mut frame = Frame::default();
    let mut parents = Vec::new();
    let mut chars = source[..offset].char_indices().peekable();
    while let Some((pos, ch)) = chars.next() {
        match (frame.quote, ch) {
            (Quote::Single, '\'') => frame.quote = Quote::None,
            (Quote::Single, _) => frame.word.push(ch),
            (_, '\\') => {
                let (_, next) = chars.next()?;
                if next == '\n' {
                    continue;
                }
                frame.start.get_or_insert(pos);
                if frame.quote == Quote::Double && !matches!(next, '$' | '`' | '"' | '\\') {
                    frame.word.push('\\');
                }
                frame.word.push(next);
            }
            (_, '$') if chars.peek().is_some_and(|(_, ch)| *ch == '(') => {
                chars.next();
                frame.start.get_or_insert(pos);
                frame.word.push_str("$()");
                parents.push(std::mem::take(&mut frame));
            }
            (Quote::Double, '"') => frame.quote = Quote::None,
            (Quote::Double, _) => frame.word.push(ch),
            (Quote::None, '\'' | '"') => {
                frame.start.get_or_insert(pos);
                frame.quote = if ch == '\'' {
                    Quote::Single
                } else {
                    Quote::Double
                };
            }
            (Quote::None, '#') if frame.start.is_none() => {
                let mut ended = false;
                for (_, ch) in chars.by_ref() {
                    if ch == '\n' {
                        ended = true;
                        break;
                    }
                }
                if !ended {
                    return None;
                }
                frame = Frame::default();
            }
            (Quote::None, '(') => {
                parents.push(std::mem::take(&mut frame));
            }
            (Quote::None, ')') => {
                frame = parents.pop().unwrap_or_default();
            }
            (Quote::None, ';' | '|' | '&' | '\n') => frame = Frame::default(),
            (Quote::None, '{' | '}') if frame.start.is_none() => frame = Frame::default(),
            (Quote::None, '<' | '>') => {
                // Descriptor prefixes belong to the redirect, not the command.
                if frame.word.bytes().all(|byte| byte.is_ascii_digit()) {
                    frame.word.clear();
                    frame.start = None;
                } else {
                    frame.finish();
                }
                frame.redirect = true;
                while chars
                    .peek()
                    .is_some_and(|(_, ch)| matches!(ch, '<' | '>' | '|' | '&'))
                {
                    chars.next();
                }
            }
            (Quote::None, ch) if ch.is_whitespace() => frame.finish(),
            (_, ch) => {
                frame.start.get_or_insert(pos);
                frame.word.push(ch);
            }
        }
    }

    let words = command_words(frame.words);
    let mut start = frame.start.unwrap_or(offset);
    let mut raw = &source[start..offset];
    if let Some((name, _)) = raw.split_once('=')
        && identifier(name)
        && words.is_empty()
    {
        start += name.len() + 1;
        frame.word = frame
            .word
            .split_once('=')
            .map(|(_, value)| value.to_owned())
            .unwrap_or_default();
        frame.redirect = true;
        raw = &source[start..offset];
    }
    let option = raw
        .split_once('=')
        .filter(|(name, _)| {
            (name.starts_with("--") || identifier(name)) && !name.contains(['\\', '\'', '"'])
        })
        .map(|(name, _)| format!("{name}="));
    let value_raw = option.as_ref().map_or(raw, |option| &raw[option.len()..]);
    let quote = match value_raw.chars().next() {
        Some('\'') => Quote::Single,
        Some('"') => Quote::Double,
        _ => Quote::None,
    };
    // Mixed quoted fragments and completed substitutions cannot be resolved as paths.
    if (quote != Quote::None && frame.quote != quote)
        || (quote == Quote::None && value_raw.contains(['\'', '"', '`']))
        || raw.contains("$(")
    {
        return None;
    }
    let mut end = offset;
    let mut closed_quote = false;
    let mut escaped = false;
    for ch in source[offset..].chars() {
        if escaped {
            escaped = false;
        } else if quote != Quote::Single && ch == '\\' {
            escaped = true;
        } else if (quote == Quote::Single && ch == '\'') || (quote == Quote::Double && ch == '"') {
            end += ch.len_utf8();
            closed_quote = true;
            break;
        } else if quote == Quote::None && (ch.is_whitespace() || ";|&()<>{}\"'".contains(ch)) {
            break;
        }
        end += ch.len_utf8();
    }

    let command = words.is_empty() && !frame.redirect;
    Some(Site {
        range: start..end,
        prefix: frame.word,
        suffix: decode_suffix(&source[offset..end - usize::from(closed_quote)], quote),
        option,
        words,
        quote,
        closed_quote,
        redirect: frame.redirect,
        command,
    })
}

fn decode_suffix(source: &str, quote: Quote) -> String {
    let mut result = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && quote != Quote::Single
            && let Some(next) = chars.peek().copied()
            && (quote == Quote::None || matches!(next, '$' | '`' | '"' | '\\' | '\n'))
        {
            chars.next();
            if next != '\n' {
                result.push(next);
            }
            continue;
        }
        result.push(ch);
    }
    result
}

fn command_words(words: Vec<String>) -> Vec<String> {
    let mut start = 0;
    while let Some(word) = words.get(start) {
        if matches!(
            word.as_str(),
            "then" | "do" | "else" | "elif" | "if" | "while" | "until" | "!" | "time"
        ) || assignment(word)
        {
            start += 1;
        } else {
            break;
        }
    }
    while let Some(wrapper) = words.get(start).map(String::as_str) {
        if !matches!(wrapper, "command" | "builtin" | "exec" | "env" | "sudo") {
            break;
        }
        start += 1;
        while let Some(word) = words.get(start) {
            if word == "--" {
                start += 1;
                break;
            }
            if !word.starts_with('-') || word == "-" {
                break;
            }
            let takes_value = match wrapper {
                "env" => matches!(word.as_str(), "-u" | "--unset" | "-C" | "--chdir"),
                "exec" => word == "-a",
                "sudo" => matches!(
                    word.as_str(),
                    "-u" | "--user"
                        | "-g"
                        | "--group"
                        | "-h"
                        | "--host"
                        | "-p"
                        | "--prompt"
                        | "-C"
                        | "--close-from"
                        | "-T"
                        | "--command-timeout"
                ),
                _ => false,
            };
            if takes_value && words.get(start + 1).is_none() {
                return words;
            }
            start += if takes_value { 2 } else { 1 };
        }
        if wrapper == "env" {
            while words.get(start).is_some_and(|word| assignment(word)) {
                start += 1;
            }
        }
    }
    words.into_iter().skip(start).collect()
}

fn assignment(word: &str) -> bool {
    word.split_once('=')
        .is_some_and(|(name, _)| identifier(name))
}

pub(in crate::handlers) fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

impl Site {
    pub(in crate::handlers) fn insert(&self, text: &str) -> String {
        let mut result = String::new();
        let text = if let Some(option) = &self.option
            && let Some(value) = text.strip_prefix(option)
        {
            result.push_str(option);
            value
        } else {
            text
        };
        let delimiter = match self.quote {
            Quote::None => None,
            Quote::Single => Some('\''),
            Quote::Double => Some('"'),
        };
        if let Some(delimiter) = delimiter {
            result.push(delimiter);
        }
        for ch in text.chars() {
            match self.quote {
                Quote::Single if ch == '\'' => result.push_str("'\\''"),
                Quote::Double if matches!(ch, '$' | '`' | '"' | '\\') => {
                    result.push('\\');
                    result.push(ch);
                }
                Quote::None if !(ch.is_alphanumeric() || "_./-:@%+=,".contains(ch)) => {
                    if ch == '\n' {
                        result.push_str("'\n'");
                    } else {
                        result.push('\\');
                        result.push(ch);
                    }
                }
                _ => result.push(ch),
            }
        }
        if self.closed_quote
            && let Some(delimiter) = delimiter
        {
            result.push(delimiter);
        }
        result
    }
}
