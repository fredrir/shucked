//! Per-document semantic token results shared by the full, range and delta requests.
//!
//! Each open document owns one slot with two parts. The memo holds the tokens of the document
//! state last tokenised, and every request reads it, so a document is tokenised at most once
//! per state no matter how the requests interleave. The published result is the data the
//! client last received a result id for; only full and delta responses move it, so range
//! requests never disturb the baseline a later delta is computed against.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lsp_types::{
    Position, Range, SemanticToken, SemanticTokens, SemanticTokensDelta, SemanticTokensEdit,
    SemanticTokensFullDeltaResult, Url,
};

use crate::edit::{DocumentVersion, PositionEncoding};
use crate::session::DocumentSnapshot;

/// Number of integers the wire encoding spends on one token.
const TOKEN_WIDTH: usize = 5;

/// The document state a memo was tokenised from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TokensKey {
    version: DocumentVersion,
    document_id: usize,
    settings_epoch: u64,
    workspace_epoch: Option<u64>,
    environment_generation: u64,
    encoding: PositionEncoding,
}

/// The last full tokenisation of one open document.
#[derive(Clone, Debug)]
struct Memo {
    key: TokensKey,
    data: Arc<[SemanticToken]>,
}

/// The last result the client received a result id for.
#[derive(Clone, Debug)]
struct Published {
    result_id: String,
    data: Arc<[SemanticToken]>,
}

impl Published {
    fn into_full(self) -> SemanticTokens {
        SemanticTokens {
            result_id: Some(self.result_id),
            data: self.data.to_vec(),
        }
    }
}

#[derive(Default, Debug)]
struct DocumentTokens {
    memo: Option<Memo>,
    published: Option<Published>,
}

type DocumentSlot = Arc<Mutex<DocumentTokens>>;

/// Cache of the last semantic token result for every open document.
#[derive(Default)]
pub(crate) struct SemanticTokensCache {
    documents: Mutex<HashMap<Url, DocumentSlot>>,
    next_result_id: AtomicU64,
    tokenisations: AtomicU64,
}

impl SemanticTokensCache {
    /// Full tokens for the snapshot, reused from the memo when the document state is unchanged.
    pub(crate) fn full(
        &self,
        snapshot: &DocumentSnapshot,
    ) -> crate::server::Result<Option<SemanticTokens>> {
        let slot = self.slot(snapshot.query().file_url());
        let mut guard = lock_or_recover(&slot);
        let Some(data) = self.memo(&mut guard, snapshot)? else {
            return Ok(None);
        };
        Ok(Some(self.publish(&mut guard, data).into_full()))
    }

    /// Tokens intersecting `range`, encoded relative to the document start.
    pub(crate) fn range(
        &self,
        snapshot: &DocumentSnapshot,
        range: Range,
    ) -> crate::server::Result<Option<SemanticTokens>> {
        let slot = self.slot(snapshot.query().file_url());
        let mut guard = lock_or_recover(&slot);
        Ok(self.memo(&mut guard, snapshot)?.map(|data| SemanticTokens {
            result_id: None,
            data: tokens_in_range(&data, range),
        }))
    }

    /// Edits turning the result identified by `previous_result_id` into the current tokens,
    /// or a full result when that id is not the one last published for the document.
    pub(crate) fn delta(
        &self,
        snapshot: &DocumentSnapshot,
        previous_result_id: &str,
    ) -> crate::server::Result<Option<SemanticTokensFullDeltaResult>> {
        let slot = self.slot(snapshot.query().file_url());
        let mut guard = lock_or_recover(&slot);
        let Some(data) = self.memo(&mut guard, snapshot)? else {
            return Ok(None);
        };
        let baseline = guard
            .published
            .as_ref()
            .filter(|published| published.result_id == previous_result_id)
            .map(|published| published.data.clone());
        let Some(baseline) = baseline else {
            return Ok(Some(SemanticTokensFullDeltaResult::Tokens(
                self.publish(&mut guard, data).into_full(),
            )));
        };
        let edits = if Arc::ptr_eq(&baseline, &data) {
            Vec::new()
        } else {
            semantic_tokens_edits(&baseline, &data)
        };
        let result_id = if edits.is_empty() {
            previous_result_id.to_owned()
        } else {
            self.next_result_id()
        };
        guard.published = Some(Published {
            result_id: result_id.clone(),
            data,
        });
        Ok(Some(SemanticTokensFullDeltaResult::TokensDelta(
            SemanticTokensDelta {
                result_id: Some(result_id),
                edits,
            },
        )))
    }

    /// Drop everything cached for a document that is no longer open.
    pub(crate) fn evict(&self, uri: &Url) {
        lock_or_recover(&self.documents).remove(uri);
    }

    /// Number of documents currently holding a slot.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        lock_or_recover(&self.documents).len()
    }

    /// Number of times a document was tokenised since the cache was created.
    #[cfg(test)]
    pub(crate) fn tokenisations(&self) -> u64 {
        self.tokenisations.load(Ordering::Acquire)
    }

    /// The memoised tokens for the snapshot's document state, tokenising when the memo is
    /// empty or was computed from another state. The caller holds the document's lock, so
    /// concurrent requests for the same document wait for one tokenisation instead of
    /// repeating it.
    fn memo(
        &self,
        tokens: &mut DocumentTokens,
        snapshot: &DocumentSnapshot,
    ) -> crate::server::Result<Option<Arc<[SemanticToken]>>> {
        let key = tokens_key(snapshot);
        if let Some(memo) = &tokens.memo
            && memo.key == key
        {
            return Ok(Some(memo.data.clone()));
        }
        let Some(fresh) = super::semantic_tokens::semantic_tokens_full(snapshot)? else {
            tokens.memo = None;
            tokens.published = None;
            return Ok(None);
        };
        self.tokenisations.fetch_add(1, Ordering::AcqRel);
        let data: Arc<[SemanticToken]> = fresh.data.into();
        tokens.memo = Some(Memo {
            key,
            data: data.clone(),
        });
        Ok(Some(data))
    }

    /// Record `data` as the result the client holds, keeping the result id when the client
    /// already received exactly this data.
    fn publish(&self, tokens: &mut DocumentTokens, data: Arc<[SemanticToken]>) -> Published {
        if let Some(published) = &tokens.published
            && Arc::ptr_eq(&published.data, &data)
        {
            return published.clone();
        }
        let published = Published {
            result_id: self.next_result_id(),
            data,
        };
        tokens.published = Some(published.clone());
        published
    }

    fn slot(&self, uri: &Url) -> DocumentSlot {
        lock_or_recover(&self.documents)
            .entry(uri.clone())
            .or_default()
            .clone()
    }

    fn next_result_id(&self) -> String {
        self.next_result_id
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .to_string()
    }
}

fn tokens_key(snapshot: &DocumentSnapshot) -> TokensKey {
    let query = snapshot.query();
    TokensKey {
        version: query.document().version(),
        document_id: Arc::as_ptr(query.document()) as usize,
        settings_epoch: snapshot.analysis_settings_epoch(),
        workspace_epoch: snapshot.workspace_epoch(),
        environment_generation: snapshot.environment_generation(),
        encoding: snapshot.encoding(),
    }
}

/// Selects the tokens that overlap `range` and re-encodes them relative to the document
/// start, so the first token carries its absolute line and character.
fn tokens_in_range(data: &[SemanticToken], range: Range) -> Vec<SemanticToken> {
    let (range_start, range_end) = if range.start <= range.end {
        (range.start, range.end)
    } else {
        (range.end, range.start)
    };
    let mut line = 0u32;
    let mut character = 0u32;
    let mut previous: Option<(u32, u32)> = None;
    let mut selected = Vec::new();
    for token in data {
        line += token.delta_line;
        character = if token.delta_line == 0 {
            character + token.delta_start
        } else {
            token.delta_start
        };
        let start = Position::new(line, character);
        let end = Position::new(line, character + token.length);
        if start >= range_end || end <= range_start {
            continue;
        }
        let (delta_line, delta_start) = match previous {
            Some((previous_line, previous_character)) if previous_line == line => {
                (0, character - previous_character)
            }
            Some((previous_line, _)) => (line - previous_line, character),
            None => (line, character),
        };
        selected.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.token_modifiers_bitset,
        });
        previous = Some((line, character));
    }
    selected
}

/// Computes the edits that turn `old` into `new`: the longest common prefix and suffix are
/// kept, and the differing middle becomes a single replacement expressed in wire integers.
pub(crate) fn semantic_tokens_edits(
    old: &[SemanticToken],
    new: &[SemanticToken],
) -> Vec<SemanticTokensEdit> {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(before, after)| before == after)
        .count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(before, after)| before == after)
        .count();
    let deleted = old.len() - prefix - suffix;
    let inserted = &new[prefix..new.len() - suffix];
    if deleted == 0 && inserted.is_empty() {
        return Vec::new();
    }
    vec![SemanticTokensEdit {
        start: to_wire_count(prefix),
        delete_count: to_wire_count(deleted),
        data: (!inserted.is_empty()).then(|| inserted.to_vec()),
    }]
}

fn to_wire_count(tokens: usize) -> u32 {
    u32::try_from(tokens * TOKEN_WIDTH).unwrap_or(u32::MAX)
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
#[path = "../../tests/semantic_tokens/diff.rs"]
mod diff_tests;
