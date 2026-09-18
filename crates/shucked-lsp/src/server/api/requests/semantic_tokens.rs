use lsp_types::{self as types, request as req};

use crate::handlers::semantic_tokens;
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
        let tokens = semantic_tokens::semantic_tokens_full(snapshot)?;
        Ok(tokens.map(types::SemanticTokensResult::Tokens))
    }
}
