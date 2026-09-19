//! Fish completions use Fish words and its native adapter, without Bourne parsing.
use super::environment::Environment;
use crate::edit::{PositionExt, offset_to_position};
use crate::session::{DocumentSnapshot, RequestCancellationToken};
use lsp_types as types;
use std::collections::BTreeSet;

pub(crate) fn complete(
    snapshot: &DocumentSnapshot,
    params: &types::CompletionParams,
    environment: Option<(&Environment, &RequestCancellationToken)>,
) -> Option<types::CompletionResponse> {
    let document = snapshot.query().document();
    let source = document.contents();
    let position = params.text_document_position.position;
    let offset = position.to_offset(source, document.index(), snapshot.encoding());
    let fish = shucked_semantic::analyze_fish(source);
    if fish
        .comment_spans
        .iter()
        .any(|span| span.start.offset() <= offset && offset <= span.end.offset())
    {
        return None;
    }
    let command = fish
        .commands
        .iter()
        .filter(|site| {
            site.span.start.offset() <= offset
                && (offset <= site.span.end.offset()
                    || source
                        .get(site.span.end.offset()..offset)
                        .is_some_and(|tail| tail.chars().all(|ch| ch == ' ' || ch == '\t')))
        })
        .min_by_key(|site| {
            site.span
                .end
                .offset()
                .saturating_sub(site.span.start.offset())
        });
    let word = command.and_then(|site| {
        site.words
            .iter()
            .find(|word| word.span.start.offset() <= offset && offset <= word.span.end.offset())
    });
    let start = word.map_or(offset, |word| word.span.start.offset());
    let end = word.map_or(offset, |word| word.span.end.offset());
    let raw_prefix = &source[start..offset];
    if raw_prefix.contains(['\n', '\r']) {
        return None;
    }
    let prefix = decode_prefix(raw_prefix);
    let double_quoted = raw_prefix.starts_with('"');
    let variable_prefix = raw_prefix.strip_prefix('"').unwrap_or(raw_prefix);
    let variable = variable_prefix.strip_prefix('$').is_some_and(|name| {
        name.chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    });
    let command_position = command.is_none_or(|site| {
        site.words
            .first()
            .is_some_and(|first| first.span.start.offset() == start)
    });
    let words: Vec<String> = command
        .and_then(|site| {
            site.effective_words
                .iter()
                .take_while(|word| word.span.end.offset() <= start)
                .map(|word| word.text.clone())
                .collect::<Option<Vec<_>>>()
        })
        .unwrap_or_default();
    let range = types::Range::new(
        offset_to_position(
            source,
            document.index(),
            start + usize::from(variable && double_quoted),
            snapshot.encoding(),
        ),
        offset_to_position(
            source,
            document.index(),
            end - usize::from(
                variable && double_quoted && source[start..end].ends_with('"') && end > start + 1,
            ),
            snapshot.encoding(),
        ),
    );
    let mut items = vec![];
    let mut seen = BTreeSet::new();
    let mut add = |name: &str, detail: &str, kind, rank| {
        if super::matches(name, &prefix) && seen.insert(name.to_owned()) {
            let text = if variable {
                name.to_owned()
            } else if raw_prefix.starts_with(['\'', '"']) {
                escape(name)
            } else if let Some(rest) = name.strip_prefix("~/") {
                format!("~/{}", escape(rest))
            } else {
                escape(name)
            };
            items.push(super::item(name, kind, detail, text, range, rank));
        }
    };
    let options = snapshot.client_settings().completion();
    let mut incomplete = false;
    if command_position && !variable {
        for name in shucked_command::builtins(shucked_command::ShellDialect::Fish) {
            add(name, "Fish builtin", types::CompletionItemKind::FUNCTION, 1);
        }
        if options.include_keywords {
            for name in [
                "if", "else", "end", "for", "while", "function", "begin", "switch", "case", "and",
                "or", "not",
            ] {
                add(name, "Fish keyword", types::CompletionItemKind::KEYWORD, 4);
            }
        }
        for function in &fish.functions {
            if function.span.end.offset() <= offset {
                add(
                    &function.name,
                    "Document Fish function",
                    types::CompletionItemKind::FUNCTION,
                    0,
                );
            }
        }
    }
    let command_analysis = snapshot.command_service.analysis(snapshot);
    let local = command_analysis.local_environment;
    let resolved_site = command.and_then(|site| {
        command_analysis
            .sites
            .iter()
            .find(|(facts, _)| facts.span == site.span)
    });
    let grammar_allowed =
        resolved_site.is_none_or(|(facts, resolution)| super::grammar_allowed(facts, resolution));
    if let Some((environment, cancellation)) = environment {
        let scoped = environment.scoped(&command_analysis.context, &command_analysis.environment);
        let environment = &scoped;
        if variable && options.include_environment && local {
            // Keep the dollar sigil in the edit; candidates are never executed.
            for name in &environment.variables {
                add(
                    &format!("${name}"),
                    "Environment variable",
                    types::CompletionItemKind::VARIABLE,
                    2,
                );
            }
        } else {
            if command_position && options.include_environment && !prefix.contains('/') {
                incomplete |= !command_analysis.environment.is_complete();
                for name in
                    super::command_names(&command_analysis.context, &command_analysis.environment)
                {
                    add(
                        &name,
                        &format!("Command · {}", command_analysis.context.target_id),
                        types::CompletionItemKind::FUNCTION,
                        3,
                    );
                }
            }
            if !command_position
                && options.include_native
                && options.include_command_arguments
                && environment.native_allowed
                && local
                && grammar_allowed
            {
                incomplete = true;
                if let Some(candidates) = environment.native.complete(
                    environment,
                    &words,
                    &prefix,
                    &crate::handlers::commands::cwd(snapshot),
                    cancellation,
                    false,
                    "fish",
                ) {
                    for candidate in candidates.iter() {
                        add(
                            &candidate.text,
                            &candidate.description,
                            types::CompletionItemKind::VALUE,
                            1,
                        );
                    }
                }
            }
            if options.include_paths && local {
                let (directory_prefix, basename) = prefix
                    .rsplit_once('/')
                    .map_or(("", prefix.as_str()), |(dir, base)| {
                        (&prefix[..dir.len() + 1], base)
                    });
                let path =
                    if !raw_prefix.starts_with(['\'', '"']) && directory_prefix.starts_with("~/") {
                        environment
                            .home
                            .as_ref()
                            .map(|home| home.join(&directory_prefix[2..]))
                    } else {
                        let path = std::path::PathBuf::from(directory_prefix);
                        Some(if path.is_absolute() {
                            path
                        } else {
                            crate::handlers::commands::cwd(snapshot).join(path)
                        })
                    };
                if let Some(path) = path {
                    let listing = environment.directory(&path, cancellation);
                    incomplete |= listing.incomplete;
                    for entry in &listing.entries {
                        if entry.name.starts_with(basename)
                            && (!entry.name.starts_with('.') || basename.starts_with('.'))
                            && (!command_position || entry.directory || entry.executable)
                        {
                            let name = format!(
                                "{directory_prefix}{}{}",
                                entry.name,
                                if entry.directory { "/" } else { "" }
                            );
                            add(
                                &name,
                                "Workspace path",
                                if entry.directory {
                                    types::CompletionItemKind::FOLDER
                                } else {
                                    types::CompletionItemKind::FILE
                                },
                                3,
                            );
                        }
                    }
                }
            }
        }
    }
    incomplete |= super::finish(&mut items, snapshot, position);
    Some(types::CompletionResponse::List(types::CompletionList {
        is_incomplete: incomplete,
        items,
    }))
}
fn decode_prefix(raw: &str) -> String {
    let mut result = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            result.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }
    result
}
fn escape(word: &str) -> String {
    if word
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '='))
    {
        word.into()
    } else {
        format!("'{}'", word.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

#[cfg(test)]
#[path = "../../../tests/completion/fish.rs"]
mod tests;
