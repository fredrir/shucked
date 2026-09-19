use lsp_types as types;

use crate::server::Result;
use crate::session::{Client, DocumentQuery};

pub(super) fn clear_diagnostics_for_document(query: &DocumentQuery, client: &Client) -> Result<()> {
    client.send_notification::<types::notification::PublishDiagnostics>(
        types::PublishDiagnosticsParams {
            uri: query.file_url().clone(),
            diagnostics: Vec::new(),
            version: Some(query.document().version()),
        },
    )?;
    Ok(())
}
