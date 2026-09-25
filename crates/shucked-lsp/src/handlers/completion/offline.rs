//! Instant argument candidates that need no completion engine: the bundled
//! option grammars.
//!
//! These sources answer on the request thread, before any native provider is
//! consulted. Native results then enrich them: an item that both sources know
//! keeps one entry, and the native description wins when both exist.
use std::collections::{BTreeSet, HashMap};

use lsp_types as types;

use super::context::{Quote, Site};
use super::native_zsh::Candidate;
use crate::handlers::commands::CommandAnalysis;
use crate::session::RequestCancellationToken;

/// Labels the offline sources contributed, mapped to their item index so a
/// later native candidate can enrich rather than duplicate them.
#[derive(Default)]
pub(super) struct Offline {
    labels: HashMap<String, usize>,
}

impl Offline {
    #[cfg(test)]
    pub(super) fn from_items(items: &[types::CompletionItem]) -> Self {
        Self {
            labels: items
                .iter()
                .enumerate()
                .map(|(index, item)| (item.label.clone(), index))
                .collect(),
        }
    }

    /// Merge a native candidate into an offline item of the same label.
    /// Returns false when no such item exists.
    pub(super) fn enrich(
        &self,
        items: &mut [types::CompletionItem],
        candidate: &Candidate,
    ) -> bool {
        let Some(&index) = self.labels.get(&candidate.text) else {
            return false;
        };
        let Some(item) = items.get_mut(index) else {
            return false;
        };
        if !candidate.description.is_empty() {
            item.detail = Some(if candidate.provider.is_empty() {
                candidate.description.clone()
            } else {
                format!("{} · {}", candidate.description, candidate.provider)
            });
        }
        if let Some(kind) = candidate.kind {
            item.kind = Some(kind);
        }
        if candidate.no_space {
            item.commit_characters = None;
        }
        true
    }

    fn push(
        &mut self,
        items: &mut Vec<types::CompletionItem>,
        seen: &mut BTreeSet<String>,
        site: &Site,
        range: types::Range,
        candidate: Candidate,
    ) {
        if !seen.insert(candidate.text.clone()) {
            return;
        }
        let detail = if candidate.description.is_empty() {
            candidate.provider.clone()
        } else if candidate.provider.is_empty() {
            candidate.description.clone()
        } else {
            format!("{} · {}", candidate.description, candidate.provider)
        };
        let kind = candidate
            .kind
            .unwrap_or(if candidate.text.starts_with('-') {
                types::CompletionItemKind::FIELD
            } else {
                types::CompletionItemKind::VALUE
            });
        let mut item = super::item(
            &candidate.text,
            kind,
            &detail,
            site.insert(&candidate.text),
            range,
            1,
        );
        if !candidate.no_space && !candidate.text.ends_with(['/', '=']) && site.quote == Quote::None
        {
            item.commit_characters = Some(vec![" ".into()]);
        }
        self.labels.insert(candidate.text.clone(), items.len());
        items.push(item);
    }
}

pub(super) struct Request<'a> {
    pub site: &'a Site,
    /// Alias-expanded words before the cursor, the command first.
    pub words: &'a [String],
    pub resolved: &'a shucked_command::ResolvedCommand,
    pub analysis: &'a std::sync::Arc<CommandAnalysis>,
    pub range: types::Range,
    pub cancellation: &'a RequestCancellationToken,
    /// Native tool execution is permitted for this request.
    pub execution: bool,
}

/// Add grammar candidates for the site. Returns whether a background query is
/// still pending (the response stays incomplete and a readiness notice
/// follows) and whether anything was contributed.
pub(super) fn extend(
    items: &mut Vec<types::CompletionItem>,
    seen: &mut BTreeSet<String>,
    offline: &mut Offline,
    request: &Request<'_>,
) -> (bool, bool) {
    let before = items.len();
    let pending = false;
    if let Some(binding) = super::grammar::bind(request) {
        for candidate in super::grammar::candidates(&binding, request.words, &request.site.prefix) {
            offline.push(items, seen, request.site, request.range, candidate);
        }
    }
    (pending, items.len() > before)
}

/// Forget bindings held in memory; the next request re-reads the executable
/// identity.
pub(super) fn invalidate() {
    super::grammar::invalidate();
}

#[cfg(test)]
#[path = "../../../tests/completion/offline.rs"]
mod tests;
