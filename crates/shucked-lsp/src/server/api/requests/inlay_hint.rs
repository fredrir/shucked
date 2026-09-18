use lsp_types::{self as types, request as req};

use crate::handlers::inlay_hints;
use crate::session::{Client, DocumentSnapshot};

pub(crate) struct InlayHint;

impl super::RequestHandler for InlayHint {
    type RequestType = req::InlayHintRequest;
}

impl super::BackgroundDocumentRequestHandler for InlayHint {
    super::define_document_url!(params: &types::InlayHintParams);

    fn run_without_snapshot(
        _client: &Client,
        _params: types::InlayHintParams,
    ) -> crate::server::Result<<req::InlayHintRequest as req::Request>::Result> {
        Ok(None)
    }

    fn run_with_snapshot(
        snapshot: DocumentSnapshot,
        _client: &Client,
        params: types::InlayHintParams,
    ) -> crate::server::Result<<req::InlayHintRequest as req::Request>::Result> {
        inlay_hints::inlay_hints(snapshot, params)
    }
}
