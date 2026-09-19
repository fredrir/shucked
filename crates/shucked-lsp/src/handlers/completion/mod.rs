pub(super) mod context;
pub(crate) mod environment;
mod native;
mod native_process;
mod native_zsh;
mod specs;

use std::collections::BTreeSet;
use std::path::Path;

use lsp_types as types;
use shucked_ast::{TextRange, TextSize};

use crate::analysis::DocumentAnalysis;
use crate::edit::PositionExt;
use crate::session::{DocumentSnapshot, RequestCancellationToken};
use context::{Quote, Site};
use environment::Environment;

pub(super) fn extend(
    items: &mut Vec<types::CompletionItem>,
    site: &Site,
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    offset: usize,
    native: (&Environment, &RequestCancellationToken),
    parameter_start: Option<usize>,
) -> bool {
    let (environment, cancellation) = native;
    let options = snapshot.client_settings().completion();
    let mut incomplete = false;
    let mut seen: BTreeSet<_> = items.iter().map(|item| item.label.clone()).collect();
    let range = range(snapshot, analysis, site.range.clone());
    if let Some(start) = parameter_start {
        if options.include_environment {
            let prefix = &analysis.source()[start..offset];
            let mut end = offset;
            for ch in analysis.source()[offset..].chars() {
                if ch != '_' && !ch.is_ascii_alphanumeric() {
                    break;
                }
                end += ch.len_utf8();
            }
            let range = self::range(snapshot, analysis, start..end);
            for name in &environment.variables {
                if matches(name, prefix) && seen.insert(name.clone()) {
                    items.push(item(
                        name,
                        types::CompletionItemKind::VARIABLE,
                        "Environment variable",
                        name.clone(),
                        range,
                        3,
                    ));
                }
            }
        }
        return incomplete;
    }
    if site.command && !site.prefix.contains('/') && options.include_environment {
        let (commands, partial) = environment.commands(&site.prefix, cancellation);
        incomplete |= partial;
        for (name, path) in commands {
            if seen.insert(name.clone()) {
                items.push(item(
                    &name,
                    types::CompletionItemKind::FUNCTION,
                    &format!("Executable · {}", path.display()),
                    site.insert(&name),
                    range,
                    3,
                ));
            }
        }
    }
    let arguments = specs::arguments(&site.words);
    let mut native_arguments = false;
    let native_enabled = !site.command
        && !site.redirect
        && !arguments.expecting_value
        && options.include_command_arguments
        && options.include_native
        && environment.native_allowed;
    // Live providers can be busy or change their results between keystrokes.
    // Do not let the editor permanently filter a temporary fallback response.
    incomplete |= native_enabled;
    if native_enabled
        && let Some(candidates) = environment.native.complete(
            environment,
            &site.words,
            &site.prefix,
            analysis
                .path()
                .and_then(Path::parent)
                .unwrap_or(&environment.cwd),
            cancellation,
            options.use_shell_config,
        )
    {
        native_arguments = !candidates.is_empty();
        incomplete |= candidates.len() >= 2000;
        for candidate in candidates.iter() {
            if candidate.text.starts_with(&site.prefix)
                && !site
                    .words
                    .iter()
                    .skip(1)
                    .any(|word| word == &candidate.text)
                && seen.insert(candidate.text.clone())
            {
                items.push(item(
                    &candidate.text,
                    if candidate.text.starts_with('-') {
                        types::CompletionItemKind::FIELD
                    } else {
                        types::CompletionItemKind::VALUE
                    },
                    if candidate.description.is_empty() {
                        "Native completion"
                    } else {
                        &candidate.description
                    },
                    site.insert(&candidate.text),
                    range,
                    1,
                ));
            }
        }
    }

    if !site.command
        && !site.redirect
        && options.include_command_arguments
        && !native_arguments
        && !arguments.after_separator
        && !arguments.expecting_value
    {
        if site.prefix.starts_with('-') {
            for flag in arguments.flags {
                if flag.starts_with(&site.prefix) && seen.insert(flag.to_owned()) {
                    items.push(item(
                        flag,
                        types::CompletionItemKind::FIELD,
                        specs::description(flag),
                        site.insert(flag),
                        range,
                        2,
                    ));
                }
            }
        } else {
            for command in arguments.subcommands {
                if command.starts_with(&site.prefix) && seen.insert(command.to_owned()) {
                    items.push(item(
                        command,
                        types::CompletionItemKind::ENUM_MEMBER,
                        "Subcommand",
                        site.insert(command),
                        range,
                        2,
                    ));
                }
            }
        }
    }
    if options.include_paths
        && !native_arguments
        && (!site.command || site.prefix.contains('/') || site.prefix.starts_with('~'))
        && (!site.prefix.starts_with('-')
            || arguments.after_separator
            || arguments.expecting_value
            || site.redirect)
    {
        incomplete |= paths(
            items,
            site,
            snapshot,
            analysis,
            offset,
            environment,
            cancellation,
        );
    }
    incomplete
}

fn paths(
    items: &mut Vec<types::CompletionItem>,
    site: &Site,
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    offset: usize,
    environment: &Environment,
    cancellation: &RequestCancellationToken,
) -> bool {
    let (parent, prefix) = site
        .prefix
        .rsplit_once('/')
        .map_or(("", site.prefix.as_str()), |(parent, prefix)| {
            (parent, prefix)
        });
    let base = analysis
        .path()
        .and_then(Path::parent)
        .unwrap_or(&environment.cwd);
    let raw = &analysis.source()[site.range.start..offset];
    let expands_variables = site.quote != Quote::Single && !raw.starts_with("\\$");
    let directory = if parent.is_empty() && site.prefix.starts_with('/') {
        std::path::PathBuf::from("/")
    } else if site.quote == Quote::None && parent == "~" {
        let Some(home) = &environment.home else {
            return false;
        };
        home.clone()
    } else if site.quote == Quote::None && parent.starts_with("~/") {
        let Some(home) = &environment.home else {
            return false;
        };
        home.join(&parent[2..])
    } else if expands_variables && parent.starts_with('$') {
        let (name, rest) = if let Some(braced) = parent.strip_prefix("${") {
            let Some((name, rest)) = braced.split_once('}') else {
                return false;
            };
            (name, rest.trim_start_matches('/'))
        } else {
            parent[1..].split_once('/').unwrap_or((&parent[1..], ""))
        };
        let Some(path) = environment.path_variables.get(name) else {
            return false;
        };
        path.join(rest)
    } else {
        if parent.contains(['$', '`', '*', '?', '[']) && expands_variables {
            return false;
        }
        base.join(parent)
    };
    if prefix.contains(['$', '`']) && expands_variables {
        return false;
    }
    let listing = environment.directory(&directory, cancellation);
    let directories_only = !site.redirect
        && site
            .words
            .first()
            .is_some_and(|word| matches!(word.as_str(), "cd" | "pushd" | "rmdir"));
    let raw = &analysis.source()[site.range.start..offset];
    let start = raw.rfind('/').map_or(
        site.range.start + usize::from(site.quote != Quote::None),
        |slash| site.range.start + slash + 1,
    );
    let end = site.range.end - usize::from(site.closed_quote);
    let range = self::range(snapshot, analysis, start..end);
    for entry in &listing.entries {
        if cancellation.is_cancelled() {
            return true;
        }
        if !entry.name.starts_with(prefix)
            || (entry.name.starts_with('.') && !prefix.starts_with('.'))
            || (directories_only && !entry.directory)
            || (site.command && !entry.directory && !entry.executable)
        {
            continue;
        }
        let name = format!("{}{}", entry.name, if entry.directory { "/" } else { "" });
        let mut inserted = site.insert(&name);
        if site.quote != Quote::None {
            inserted.remove(0);
            if site.closed_quote {
                inserted.pop();
            }
        }
        let mut candidate = item(
            &name,
            if entry.directory {
                types::CompletionItemKind::FOLDER
            } else {
                types::CompletionItemKind::FILE
            },
            if entry.directory { "Directory" } else { "File" },
            inserted,
            range,
            4,
        );
        candidate.filter_text = candidate.text_edit.as_ref().and_then(|edit| match edit {
            types::CompletionTextEdit::Edit(edit) => Some(edit.new_text.clone()),
            _ => None,
        });
        items.push(candidate);
    }
    listing.incomplete
}

pub(super) fn item(
    label: &str,
    kind: types::CompletionItemKind,
    detail: &str,
    text: String,
    range: types::Range,
    rank: u8,
) -> types::CompletionItem {
    types::CompletionItem {
        label: label.to_owned(),
        kind: Some(kind),
        detail: Some(detail.to_owned()),
        sort_text: Some(format!("{rank}:{label}")),
        filter_text: Some(text.clone()),
        insert_text_format: Some(types::InsertTextFormat::PLAIN_TEXT),
        text_edit: Some(types::CompletionTextEdit::Edit(types::TextEdit::new(
            range, text,
        ))),
        ..Default::default()
    }
}

pub(super) fn range(
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    range: std::ops::Range<usize>,
) -> types::Range {
    crate::edit::to_lsp_range(
        TextRange::new(
            TextSize::new(range.start as u32),
            TextSize::new(range.end as u32),
        ),
        analysis.source(),
        analysis.line_index(),
        snapshot.encoding(),
    )
}

pub(super) fn matches(candidate: &str, prefix: &str) -> bool {
    match_score(candidate, prefix).is_some()
}

fn match_score(candidate: &str, prefix: &str) -> Option<u8> {
    if candidate.starts_with(prefix) {
        return Some(0);
    }
    let candidate = candidate.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    if candidate.starts_with(&prefix) {
        return Some(1);
    }
    let mut chars = candidate.chars();
    prefix
        .chars()
        .all(|expected| chars.by_ref().any(|ch| ch == expected))
        .then_some(2)
}

pub(super) fn finish(
    items: &mut Vec<types::CompletionItem>,
    snapshot: &DocumentSnapshot,
    position: types::Position,
) -> bool {
    items.retain(|item| match &item.text_edit {
        Some(types::CompletionTextEdit::Edit(edit)) => {
            edit.range.start.line == position.line
                && edit.range.end.line == position.line
                && edit.range.start <= position
                && edit.range.end >= position
        }
        _ => true,
    });
    if let Some(analysis) = snapshot.analysis() {
        let source = analysis.source();
        let offset = position.to_offset(source, analysis.line_index(), snapshot.encoding());
        for item in items.iter_mut() {
            if let Some(types::CompletionTextEdit::Edit(edit)) = &item.text_edit {
                let start =
                    edit.range
                        .start
                        .to_offset(source, analysis.line_index(), snapshot.encoding());
                let score = match_score(&edit.new_text, &source[start..offset]).unwrap_or(3);
                item.sort_text = Some(format!(
                    "{score}:{}",
                    item.sort_text.as_deref().unwrap_or(&item.label)
                ));
            }
        }
    }
    items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text).then(a.label.cmp(&b.label)));
    let limit = snapshot
        .client_settings()
        .completion()
        .max_items
        .clamp(1, 2000);
    let incomplete = items.len() > limit;
    items.truncate(limit);
    if snapshot
        .resolved_client_capabilities()
        .completion_insert_replace
    {
        for item in items {
            if let Some(types::CompletionTextEdit::Edit(edit)) = item.text_edit.take() {
                let insert = types::Range::new(edit.range.start, position);
                item.text_edit = Some(types::CompletionTextEdit::InsertAndReplace(
                    types::InsertReplaceEdit {
                        new_text: edit.new_text,
                        insert,
                        replace: edit.range,
                    },
                ));
            }
        }
    }
    incomplete
}

#[cfg(test)]
#[path = "../../../tests/completion/context.rs"]
mod tests;
