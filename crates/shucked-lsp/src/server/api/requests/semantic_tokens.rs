//! Semantic token requests. All three share the per-document result cache on the snapshot,
//! so a document is tokenised once per state and deltas diff against the last full result.
use lsp_types::{self as types, request as req};

use crate::session::{Client, DocumentSnapshot};

pub(crate) struct SemanticTokensFull;

impl super::RequestHandler for SemanticTokensFull {
    type RequestType = req::SemanticTokensFullRequest;
}

impl super::BackgroundDocumentRequestHandler for SemanticTokensFull {
    super::define_document_url!(params: &types::SemanticTokensParams);

    fn run_with_snapshot(
        snapshot: DocumentSnapshot,
        _client: &Client,
        _params: types::SemanticTokensParams,
    ) -> crate::server::Result<Option<types::SemanticTokensResult>> {
        let tokens = snapshot.semantic_tokens().full(&snapshot)?;
        Ok(tokens.map(types::SemanticTokensResult::Tokens))
    }
}

pub(crate) struct SemanticTokensRange;

impl super::RequestHandler for SemanticTokensRange {
    type RequestType = req::SemanticTokensRangeRequest;
}

impl super::BackgroundDocumentRequestHandler for SemanticTokensRange {
    super::define_document_url!(params: &types::SemanticTokensRangeParams);

    fn run_with_snapshot(
        snapshot: DocumentSnapshot,
        _client: &Client,
        params: types::SemanticTokensRangeParams,
    ) -> crate::server::Result<Option<types::SemanticTokensRangeResult>> {
        let tokens = snapshot.semantic_tokens().range(&snapshot, params.range)?;
        Ok(tokens.map(types::SemanticTokensRangeResult::Tokens))
    }
}

pub(crate) struct SemanticTokensFullDelta;

impl super::RequestHandler for SemanticTokensFullDelta {
    type RequestType = req::SemanticTokensFullDeltaRequest;
}

impl super::BackgroundDocumentRequestHandler for SemanticTokensFullDelta {
    super::define_document_url!(params: &types::SemanticTokensDeltaParams);

    fn run_with_snapshot(
        snapshot: DocumentSnapshot,
        _client: &Client,
        params: types::SemanticTokensDeltaParams,
    ) -> crate::server::Result<Option<types::SemanticTokensFullDeltaResult>> {
        snapshot
            .semantic_tokens()
            .delta(&snapshot, &params.previous_result_id)
    }
}

#[cfg(test)]
#[path = "../../../../tests/semantic_tokens/requests.rs"]
mod tests;
