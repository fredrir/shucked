//! Range, delta and cache behaviour of the semantic token requests, driven through the
//! request handlers and the `didClose` notification exactly as the main loop calls them.
use crossbeam::channel;
use lsp_server::Message;
use lsp_types::{
    ClientCapabilities, DidCloseTextDocumentParams, PartialResultParams, Position, Range,
    SemanticToken, SemanticTokens, SemanticTokensDeltaParams, SemanticTokensEdit,
    SemanticTokensFullDeltaResult, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, Url, WorkDoneProgressParams,
};

use super::{SemanticTokensFull, SemanticTokensFullDelta, SemanticTokensRange};
use crate::server::Event;
use crate::server::api::notifications::DidClose;
use crate::server::api::traits::{BackgroundDocumentRequestHandler, SyncNotificationHandler};
use crate::session::{Client, GlobalOptions, Session, Workspace, Workspaces};
use crate::{PositionEncoding, TextDocument};

const SCRIPT: &str = "#!/bin/bash\n# note\nname=1\necho \"$name\"\nexit 0\n";
const SCRIPT_WITH_EXTRA_LINE: &str =
    "#!/bin/bash\n# note\nname=1\necho \"$name\"\necho done\nexit 0\n";
const FISH_SCRIPT: &str = "function greet\n    echo 'hi'\nend\ngreet\n";
const FISH_SCRIPT_WITH_EXTRA_LINE: &str = "function greet\n    echo 'hi'\nend\ngreet\necho 'bye'\n";

/// Absolute `(line, character, length, token type, modifiers)` view of encoded tokens.
type AbsoluteToken = (u32, u32, u32, u32, u32);

struct Harness {
    session: Session,
    client: Client,
    workspace: tempfile::TempDir,
    _events: channel::Receiver<Event>,
    _messages: channel::Receiver<Message>,
}

impl Harness {
    fn new() -> Self {
        let workspace = tempfile::tempdir().expect("workspace should be created");
        let workspace_url =
            Url::from_file_path(workspace.path()).expect("workspace path should convert");
        let (event_sender, events) = channel::unbounded();
        let (message_sender, messages) = channel::unbounded::<Message>();
        let client = Client::new(event_sender, message_sender);
        let workspaces = Workspaces::new(vec![Workspace::default(workspace_url)]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let session = Session::new(
            &ClientCapabilities::default(),
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");
        Self {
            session,
            client,
            workspace,
            _events: events,
            _messages: messages,
        }
    }

    fn open(&mut self, name: &str, language_id: &str, text: &str) -> Url {
        let uri = Url::from_file_path(self.workspace.path().join(name))
            .expect("document path should convert");
        self.session.open_text_document(
            uri.clone(),
            TextDocument::new(text.to_owned(), 1).with_language_id(language_id),
        );
        uri
    }

    fn replace(&mut self, uri: &Url, version: i32, text: &str) {
        let key = self.session.key_from_url(uri.clone());
        self.session
            .update_text_document(
                &key,
                vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.to_owned(),
                }],
                version,
            )
            .expect("document update should apply");
    }

    fn close(&mut self, uri: &Url) {
        DidClose::run(
            &mut self.session,
            &self.client,
            DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            },
        )
        .expect("didClose should succeed");
    }

    fn full(&self, uri: &Url) -> SemanticTokens {
        let snapshot = self
            .session
            .take_snapshot(uri.clone())
            .expect("document should be open");
        let result = SemanticTokensFull::run_with_snapshot(
            snapshot,
            &self.client,
            SemanticTokensParams {
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            },
        )
        .expect("full request should succeed")
        .expect("document should produce tokens");
        match result {
            SemanticTokensResult::Tokens(tokens) => tokens,
            SemanticTokensResult::Partial(_) => panic!("full request should not be partial"),
        }
    }

    fn range(&self, uri: &Url, range: Range) -> SemanticTokens {
        let snapshot = self
            .session
            .take_snapshot(uri.clone())
            .expect("document should be open");
        let result = SemanticTokensRange::run_with_snapshot(
            snapshot,
            &self.client,
            SemanticTokensRangeParams {
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range,
            },
        )
        .expect("range request should succeed")
        .expect("document should produce tokens");
        match result {
            SemanticTokensRangeResult::Tokens(tokens) => tokens,
            SemanticTokensRangeResult::Partial(_) => {
                panic!("range request should not be partial")
            }
        }
    }

    fn delta(&self, uri: &Url, previous_result_id: &str) -> SemanticTokensFullDeltaResult {
        let snapshot = self
            .session
            .take_snapshot(uri.clone())
            .expect("document should be open");
        SemanticTokensFullDelta::run_with_snapshot(
            snapshot,
            &self.client,
            SemanticTokensDeltaParams {
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                previous_result_id: previous_result_id.to_owned(),
            },
        )
        .expect("delta request should succeed")
        .expect("document should produce tokens")
    }

    fn tokenisations(&self) -> u64 {
        self.session.semantic_tokens().tokenisations()
    }

    fn cached_documents(&self) -> usize {
        self.session.semantic_tokens().len()
    }
}

fn decode(data: &[SemanticToken]) -> Vec<AbsoluteToken> {
    let mut line = 0;
    let mut character = 0;
    let mut decoded = Vec::with_capacity(data.len());
    for token in data {
        line += token.delta_line;
        character = if token.delta_line == 0 {
            character + token.delta_start
        } else {
            token.delta_start
        };
        decoded.push((
            line,
            character,
            token.length,
            token.token_type,
            token.token_modifiers_bitset,
        ));
    }
    decoded
}

fn overlaps(token: &AbsoluteToken, range: Range) -> bool {
    let start = Position::new(token.0, token.1);
    let end = Position::new(token.0, token.1 + token.2);
    start < range.end && end > range.start
}

fn wire(tokens: &[SemanticToken]) -> Vec<u32> {
    tokens
        .iter()
        .flat_map(|token| {
            [
                token.delta_line,
                token.delta_start,
                token.length,
                token.token_type,
                token.token_modifiers_bitset,
            ]
        })
        .collect()
}

/// Applies delta edits the way a client does: on the flat integer array, later edits first.
fn apply_edits(old: &[SemanticToken], edits: &[SemanticTokensEdit]) -> Vec<SemanticToken> {
    let mut data = wire(old);
    let mut ordered = edits.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    for edit in ordered {
        let start = edit.start as usize;
        let end = start + edit.delete_count as usize;
        let inserted = edit.data.as_deref().map(wire).unwrap_or_default();
        data.splice(start..end, inserted);
    }
    data.chunks_exact(5)
        .map(|chunk| SemanticToken {
            delta_line: chunk[0],
            delta_start: chunk[1],
            length: chunk[2],
            token_type: chunk[3],
            token_modifiers_bitset: chunk[4],
        })
        .collect()
}

fn result_id(tokens: &SemanticTokens) -> String {
    tokens
        .result_id
        .clone()
        .expect("full result should carry a result id")
}

#[test]
fn range_request_returns_only_the_tokens_inside_the_range() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let full = decode(&harness.full(&uri).data);
    let range = Range::new(Position::new(2, 0), Position::new(4, 0));
    let expected = full
        .iter()
        .copied()
        .filter(|token| token.0 == 2 || token.0 == 3)
        .collect::<Vec<_>>();
    assert!(!expected.is_empty(), "fixture lines should carry tokens");
    assert!(
        expected.len() < full.len(),
        "range should exclude other lines"
    );

    let ranged = harness.range(&uri, range);

    assert_eq!(ranged.result_id, None);
    assert_eq!(decode(&ranged.data), expected);
    let first = ranged.data.first().expect("range should contain tokens");
    assert_eq!(
        (first.delta_line, first.delta_start),
        (expected[0].0, expected[0].1),
        "the first token is encoded relative to the document start"
    );
}

#[test]
fn range_request_keeps_whole_tokens_that_intersect_a_partial_line_range() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let full = decode(&harness.full(&uri).data);
    // Inside `$name` on the `echo "$name"` line.
    let range = Range::new(Position::new(3, 7), Position::new(3, 9));
    let expected = full
        .iter()
        .copied()
        .filter(|token| overlaps(token, range))
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), 1, "only the variable reference overlaps");

    let ranged = harness.range(&uri, range);

    assert_eq!(decode(&ranged.data), expected);
    assert_eq!(ranged.data[0].length, expected[0].2);
}

#[test]
fn delta_after_an_edit_near_the_end_reproduces_the_new_full_data_with_one_edit() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let before = harness.full(&uri);
    let previous_id = result_id(&before);
    harness.replace(&uri, 2, SCRIPT_WITH_EXTRA_LINE);

    let SemanticTokensFullDeltaResult::TokensDelta(delta) = harness.delta(&uri, &previous_id)
    else {
        panic!("a known result id should produce a delta");
    };
    let after = harness.full(&uri);

    assert_ne!(delta.result_id.as_deref(), Some(previous_id.as_str()));
    assert_eq!(delta.result_id, after.result_id);
    assert_eq!(delta.edits.len(), 1);
    let edit = &delta.edits[0];
    assert!(edit.start > 0, "the unchanged prefix is kept");
    assert_eq!(edit.start % 5, 0);
    assert_eq!(edit.delete_count % 5, 0);
    assert_eq!(apply_edits(&before.data, &delta.edits), after.data);
    assert_ne!(before.data, after.data);
}

#[test]
fn delta_with_an_unknown_result_id_falls_back_to_a_full_result() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let before = harness.full(&uri);

    let SemanticTokensFullDeltaResult::Tokens(tokens) = harness.delta(&uri, "no-such-result")
    else {
        panic!("an unknown result id should produce a full result");
    };

    assert_eq!(tokens.data, before.data);
    assert_eq!(tokens.result_id, before.result_id);
}

#[test]
fn unchanged_document_is_answered_from_the_cache() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let first = harness.full(&uri);
    let second = harness.full(&uri);
    assert_eq!(first.result_id, second.result_id);
    assert_eq!(first.data, second.data);

    let SemanticTokensFullDeltaResult::TokensDelta(delta) = harness.delta(&uri, &result_id(&first))
    else {
        panic!("a known result id should produce a delta");
    };
    assert!(delta.edits.is_empty());
    assert_eq!(delta.result_id, first.result_id);
    assert_eq!(harness.tokenisations(), 1);

    harness.replace(&uri, 2, SCRIPT_WITH_EXTRA_LINE);
    let third = harness.full(&uri);
    assert_ne!(third.result_id, first.result_id);
    assert_eq!(harness.tokenisations(), 2);
}

#[test]
fn interleaved_range_and_full_requests_tokenise_once_per_version() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let whole = Range::new(Position::new(0, 0), Position::new(5, 0));

    let ranged = harness.range(&uri, whole);
    let full = harness.full(&uri);
    let ranged_again = harness.range(&uri, Range::new(Position::new(2, 0), Position::new(3, 0)));

    assert_eq!(harness.tokenisations(), 1);
    assert_eq!(
        ranged.data, full.data,
        "a range covering the document equals the full data"
    );
    assert!(!ranged_again.data.is_empty());
}

#[test]
fn did_close_clears_the_cached_result() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let before = harness.full(&uri);
    assert_eq!(harness.cached_documents(), 1);

    harness.close(&uri);

    assert_eq!(harness.cached_documents(), 0);
    assert!(harness.session.take_snapshot(uri.clone()).is_none());

    harness.open("script.sh", "shellscript", SCRIPT);
    let SemanticTokensFullDeltaResult::Tokens(tokens) = harness.delta(&uri, &result_id(&before))
    else {
        panic!("a result id from before the close should be forgotten");
    };
    assert_ne!(tokens.result_id, before.result_id);
    assert_eq!(harness.tokenisations(), 2);
}

#[test]
fn fish_documents_share_the_range_delta_and_eviction_paths() {
    let mut harness = Harness::new();
    let uri = harness.open("script.fish", "fish", FISH_SCRIPT);
    let before = harness.full(&uri);
    assert!(
        before.data.iter().any(|token| token.token_type == 1),
        "fish functions should be tokenised"
    );
    let decoded = decode(&before.data);

    let ranged = harness.range(&uri, Range::new(Position::new(3, 0), Position::new(4, 0)));
    let expected = decoded
        .iter()
        .copied()
        .filter(|token| token.0 == 3)
        .collect::<Vec<_>>();
    assert!(
        !expected.is_empty(),
        "the call site line should carry tokens"
    );
    assert_eq!(decode(&ranged.data), expected);
    assert_eq!(harness.tokenisations(), 1);

    let previous_id = result_id(&before);
    harness.replace(&uri, 2, FISH_SCRIPT_WITH_EXTRA_LINE);
    let SemanticTokensFullDeltaResult::TokensDelta(delta) = harness.delta(&uri, &previous_id)
    else {
        panic!("a known result id should produce a delta");
    };
    let after = harness.full(&uri);
    assert_eq!(delta.edits.len(), 1);
    assert_eq!(delta.result_id, after.result_id);
    assert_eq!(apply_edits(&before.data, &delta.edits), after.data);

    harness.close(&uri);
    assert_eq!(harness.cached_documents(), 0);
}

#[test]
fn range_requests_after_a_state_change_keep_the_delta_baseline() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let before = harness.full(&uri);
    let previous_id = result_id(&before);

    // The host environment changed without an edit, so the next request re-tokenises.
    harness
        .session
        .take_snapshot(uri.clone())
        .expect("document should be open")
        .command_service
        .invalidate();
    let ranged = harness.range(&uri, Range::new(Position::new(0, 0), Position::new(9, 0)));
    assert!(!ranged.data.is_empty());
    assert_eq!(harness.tokenisations(), 2);

    harness.replace(&uri, 2, SCRIPT_WITH_EXTRA_LINE);
    let SemanticTokensFullDeltaResult::TokensDelta(delta) = harness.delta(&uri, &previous_id)
    else {
        panic!("the delta baseline should survive interleaved range requests");
    };
    let after = harness.full(&uri);
    assert_eq!(delta.result_id, after.result_id);
    assert_eq!(apply_edits(&before.data, &delta.edits), after.data);
    assert_eq!(harness.tokenisations(), 3);
}

#[test]
fn delta_after_a_state_change_without_token_changes_keeps_the_result_id() {
    let mut harness = Harness::new();
    let uri = harness.open("script.sh", "shellscript", SCRIPT);
    let before = harness.full(&uri);
    let previous_id = result_id(&before);

    harness
        .session
        .take_snapshot(uri.clone())
        .expect("document should be open")
        .command_service
        .invalidate();
    let SemanticTokensFullDeltaResult::TokensDelta(delta) = harness.delta(&uri, &previous_id)
    else {
        panic!("a known result id should produce a delta");
    };

    assert_eq!(harness.tokenisations(), 2);
    assert!(delta.edits.is_empty());
    assert_eq!(delta.result_id.as_deref(), Some(previous_id.as_str()));
    let after = harness.full(&uri);
    assert_eq!(after.result_id, before.result_id);
    assert_eq!(after.data, before.data);
}
