#[cfg(test)]
use std::path::Path;
pub(super) mod context;
pub(crate) mod environment;
pub(crate) mod fish;
mod native;
pub(crate) mod native_process;
mod native_zsh;
pub(crate) use native_zsh::decode_bash_candidate;
pub(crate) mod background;
mod service;

use std::collections::BTreeSet;

use lsp_types as types;
use shucked_ast::{TextRange, TextSize};

use crate::analysis::DocumentAnalysis;
use crate::edit::PositionExt;
use crate::session::{DocumentSnapshot, RequestCancellationToken};
use context::{Quote, Site};
use environment::Environment;

/// Show literal local directory operands while cross-file preparation continues.
/// Authoritative completion still replaces this provisional, incomplete list.
pub(crate) fn directory_preview(
    snapshot: &DocumentSnapshot,
    environment: &Environment,
    client: &crate::session::Client,
    position: types::Position,
) -> Option<Vec<types::CompletionItem>> {
    let options = snapshot.client_settings().environment();
    if !snapshot.client_settings().completion().include_paths
        || options.policy.as_deref() == Some("portable")
        || options.target_inventory.is_some()
        || options.session_id.is_some()
        || crate::handlers::commands::dialect(snapshot) == "fish"
        || snapshot.query().document().contents().len() > 8192
    {
        return None;
    }
    let analysis = snapshot.analysis()?;
    let offset = position.to_offset(
        analysis.source(),
        analysis.line_index(),
        snapshot.encoding(),
    );
    let site = context::at(analysis.source(), analysis.indexer(), offset)?;
    if site.command
        || site.redirect
        || site.words.len() != 1
        || !matches!(site.words[0].as_str(), "cd" | "pushd" | "rmdir")
        || site.prefix.starts_with('-')
        || analysis.source()[site.range.clone()].contains(['$', '`', '*', '?', '['])
    {
        return None;
    }
    let semantic = analysis.semantic();
    if !semantic.source_refs().is_empty() {
        return None;
    }
    let facts = semantic.command_site_facts();
    let facts = facts
        .iter()
        .rfind(|facts| facts.span.start.offset() <= offset)?;
    if facts.name() != Some(site.words[0].as_str())
        || facts.visible_function.is_some()
        || !facts.aliases.is_empty()
        || facts.environment_uncertain.is_some()
        || facts
            .effective_words
            .iter()
            .any(|word| word.text.is_none() && word.span.start.offset() < site.range.start)
    {
        return None;
    }
    if snapshot
        .workspace_functions
        .as_ref()
        .and_then(crate::workspace_functions::cached_workspace_function_index)
        .is_some()
    {
        let command = snapshot.command_service.cached_analysis(snapshot)?;
        let (facts, resolution) = command
            .sites
            .iter()
            .rfind(|(facts, _)| facts.span.start.offset() <= offset)?;
        if !grammar_allowed(facts, resolution) {
            return None;
        }
    }
    let mut items = Vec::new();
    paths(
        &mut items,
        &site,
        snapshot,
        &analysis,
        offset,
        environment,
        Some(background::notice(snapshot, client, position)),
    );
    finish(&mut items, snapshot, position);
    Some(items)
}

pub(super) fn extend(
    items: &mut Vec<types::CompletionItem>,
    site: &Site,
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    offset: usize,
    native: (
        &Environment,
        &RequestCancellationToken,
        &crate::session::Client,
    ),
    parameter_start: Option<usize>,
) -> bool {
    let (environment, _cancellation, client) = native;
    let command_analysis = snapshot.command_service.analysis(snapshot);
    let local = command_analysis.local_environment;
    let scoped_environment =
        environment.scoped(&command_analysis.context, &command_analysis.environment);
    let environment = &scoped_environment;
    let options = snapshot.client_settings().completion();
    let mut incomplete = false;
    let mut seen: BTreeSet<_> = items.iter().map(|item| item.label.clone()).collect();
    let range = range(snapshot, analysis, site.range.clone());
    if let Some(start) = parameter_start {
        if options.include_environment && local {
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
        incomplete |= !command_analysis.environment.is_complete();
        for name in command_names(&command_analysis.context, &command_analysis.environment) {
            if matches(&name, &site.prefix) && seen.insert(name.clone()) {
                items.push(item(
                    &name,
                    types::CompletionItemKind::FUNCTION,
                    &format!("Command · {}", command_analysis.context.target_id),
                    site.insert(&name),
                    range,
                    3,
                ));
            }
        }
    }
    let command_site = command_analysis
        .sites
        .iter()
        .filter(|(facts, _)| {
            facts.span.start.offset() <= offset
                && (offset <= facts.span.end.offset()
                    || analysis
                        .source()
                        .get(facts.span.end.offset()..offset)
                        .is_some_and(|tail| tail.chars().all(|ch| matches!(ch, ' ' | '\t'))))
        })
        .min_by_key(|(facts, _)| {
            facts
                .span
                .end
                .offset()
                .saturating_sub(facts.span.start.offset())
        });
    let grammar_allowed =
        command_site.is_none_or(|(facts, resolution)| grammar_allowed(facts, resolution));
    let effective_words = command_site.and_then(|(facts, resolution)| {
        let shucked_command::CommandResolution::Resolved(resolved) = resolution else {
            return None;
        };
        if facts.aliases.is_empty() && resolved.alias_chain.is_empty() {
            return None;
        }
        let completed = facts
            .effective_words
            .iter()
            .filter(|word| word.injected || word.span.end.offset() <= site.range.start)
            .count();
        let injected = resolved
            .effective_words
            .len()
            .saturating_sub(facts.effective_words.len());
        Some(
            resolved
                .effective_words
                .iter()
                .take(completed + injected)
                .cloned()
                .collect::<Vec<_>>(),
        )
    });
    let words = effective_words.as_ref().unwrap_or(&site.words);
    let directory_context = grammar_allowed
        && words
            .first()
            .is_some_and(|word| matches!(word.as_str(), "cd" | "pushd" | "rmdir"));
    let local_directories = directory_context && !site.prefix.starts_with('-');
    let position = crate::edit::offset_to_position(
        analysis.source(),
        analysis.line_index(),
        offset,
        snapshot.encoding(),
    );
    environment.service.cancel_document(
        snapshot.query().file_url(),
        Some((snapshot.query().document().version(), position)),
    );
    environment
        .service
        .remember(background::notice(snapshot, client, position));
    let native_enabled = !site.command
        && !site.redirect
        && options.include_command_arguments
        && options.include_native
        && environment.native_allowed
        && local
        && grammar_allowed
        && !local_directories;
    let live_enabled = !site.command
        && !site.redirect
        && options.include_command_arguments
        && options.include_native
        && environment.native_allowed
        && local
        && command_analysis.context.mode == shucked_command::ExecutionMode::InteractiveSession
        && command_site.is_some_and(|(facts, _)| {
            facts.environment_uncertain.is_none()
                && facts.effective_words.iter().all(|word| word.text.is_some())
        });
    let mut native_arguments = false;
    let mut provider_active = false;
    tracing::debug!(
        native_enabled,
        live_enabled,
        local,
        grammar_allowed,
        allowed = environment.native_allowed,
        command_position = site.command,
        redirect = site.redirect,
        "completion provider eligibility"
    );
    for live in [true, false] {
        if !(if live { live_enabled } else { native_enabled }) {
            continue;
        }
        let (candidates, pending) = background::candidates(
            environment,
            snapshot,
            client,
            words,
            &site.prefix,
            &site.suffix,
            position,
            live,
            false,
        );
        incomplete |= pending;
        provider_active |= pending || candidates.is_some();
        if let Some(candidates) = candidates {
            incomplete |= candidates.len() >= 2000;
            for candidate in candidates.iter() {
                if candidate.text.starts_with(&site.prefix) && seen.insert(candidate.text.clone()) {
                    native_arguments = true;
                    let detail = if candidate.description.is_empty() {
                        candidate.provider.clone()
                    } else if candidate.provider.is_empty() {
                        candidate.description.clone()
                    } else {
                        format!("{} · {}", candidate.description, candidate.provider)
                    };
                    let kind = candidate.kind.unwrap_or_else(|| {
                        if candidate.text.starts_with('-') {
                            types::CompletionItemKind::FIELD
                        } else if candidate.text.ends_with('/') {
                            types::CompletionItemKind::FOLDER
                        } else {
                            types::CompletionItemKind::VALUE
                        }
                    });
                    let mut completed = item(
                        &candidate.text,
                        kind,
                        &detail,
                        site.insert(&candidate.text),
                        range,
                        if live { 0 } else { 1 },
                    );
                    if !candidate.no_space
                        && !candidate.text.ends_with(['/', '='])
                        && site.quote == Quote::None
                    {
                        completed.commit_characters = Some(vec![" ".into()]);
                    }
                    items.push(completed);
                    if candidate.text == site.prefix && native_enabled {
                        background::prewarm_next(
                            environment,
                            snapshot,
                            client,
                            words,
                            &site.prefix,
                            position,
                        );
                    }
                }
            }
            if native_arguments {
                break;
            }
        }
    }
    if site.command
        && environment.native_allowed
        && local
        && options.include_command_arguments
        && command_names(&command_analysis.context, &command_analysis.environment)
            .contains(&site.prefix)
    {
        background::prewarm_next(environment, snapshot, client, words, &site.prefix, position);
    }
    if options.include_paths
        && local
        && !native_arguments
        && path_fallback(
            &site.prefix,
            site.command,
            site.redirect,
            local_directories,
            provider_active,
        )
        && (!site.prefix.starts_with('-')
            || words.iter().any(|word| word == "--")
            || site.prefix.contains('=')
            || site.redirect)
    {
        // Complete just the value locally while preserving the complete option for providers.
        let mut path_site = site.clone();
        if !site.redirect
            && site.option.is_some()
            && let Some((name, value)) = site.prefix.split_once('=')
        {
            path_site.prefix = value.to_owned();
            path_site.range.start += name.len() + 1;
            path_site.option = None;
        }
        incomplete |= paths(
            items,
            &path_site,
            snapshot,
            analysis,
            offset,
            environment,
            Some(background::notice(snapshot, client, position)),
        );
    }

    incomplete
}

/// A provider owns argument meaning, including a valid empty result. Only
/// explicit paths or shell directory operands can bypass pending provider work.
pub(super) fn path_fallback(
    prefix: &str,
    command: bool,
    redirect: bool,
    directory: bool,
    provider_active: bool,
) -> bool {
    redirect
        || directory
        || prefix.contains('/')
        || prefix.starts_with('~')
        || (!command && !provider_active && !prefix.is_empty())
}

fn grammar_allowed(
    facts: &shucked_semantic::CommandSiteFacts,
    resolution: &shucked_command::CommandResolution,
) -> bool {
    facts.environment_uncertain.is_none()
        && match resolution {
            shucked_command::CommandResolution::Unknown(_) => false,
            shucked_command::CommandResolution::Resolved(command) => {
                command.kind != shucked_command::CommandKind::Function
            }
            shucked_command::CommandResolution::Missing(_) => facts.aliases.is_empty(),
        }
}

fn command_names(
    context: &shucked_command::ExecutionContext,
    environment: &shucked_command::EnvironmentSnapshot,
) -> BTreeSet<String> {
    let mut names = environment.command_names();
    names.extend(
        shucked_command::builtins(context.dialect)
            .into_iter()
            .map(str::to_owned),
    );
    if context.mode == shucked_command::ExecutionMode::InteractiveSession && environment.fresh {
        names.extend(environment.functions.iter().cloned());
        names.extend(environment.aliases.keys().cloned());
    }
    names
}

fn paths(
    items: &mut Vec<types::CompletionItem>,
    site: &Site,
    snapshot: &DocumentSnapshot,
    analysis: &DocumentAnalysis,
    offset: usize,
    environment: &Environment,
    notice: Option<service::Notice>,
) -> bool {
    let (parent, prefix) = site
        .prefix
        .rsplit_once('/')
        .map_or(("", site.prefix.as_str()), |(parent, prefix)| {
            (parent, prefix)
        });
    let base = crate::handlers::commands::cwd(snapshot);
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
    let listing = environment.directory_cached(&directory, notice);
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
        if snapshot.analysis_cancellation().is_cancelled() {
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
    if candidate == prefix {
        return Some(0);
    }
    if candidate.starts_with(prefix) {
        return Some(1);
    }
    let candidate = candidate.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    if candidate.starts_with(&prefix) {
        return Some(2);
    }
    let mut chars = candidate.chars();
    prefix
        .chars()
        .all(|expected| chars.by_ref().any(|ch| ch == expected))
        .then_some(3)
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
                let score = match_score(
                    item.filter_text.as_deref().unwrap_or(&edit.new_text),
                    &source[start..offset],
                )
                .unwrap_or(4);
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

#[cfg(test)]
#[path = "../../../tests/completion/preview.rs"]
mod preview_tests;
