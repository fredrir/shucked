use lsp_server::ErrorCode;
use lsp_types as types;
use lsp_types::notification as notif;

use crate::server::Result;
use crate::server::api::LSPResult;
use crate::session::{Client, Session};

pub(crate) struct DidChange;

impl super::super::traits::NotificationHandler for DidChange {
    type NotificationType = notif::DidChangeTextDocument;
}

impl super::super::traits::SyncNotificationHandler for DidChange {
    fn run(
        session: &mut Session,
        _client: &Client,
        types::DidChangeTextDocumentParams {
            text_document:
                types::VersionedTextDocumentIdentifier {
                    uri,
                    version: new_version,
                },
            content_changes,
        }: types::DidChangeTextDocumentParams,
    ) -> Result<()> {
        let key = session.key_from_url(uri);
        session
            .update_text_document(&key, content_changes, new_version)
            .with_failure_code(ErrorCode::InternalError)?;

        session.schedule_diagnostics(key.into_url());

        Ok(())
    }
}
