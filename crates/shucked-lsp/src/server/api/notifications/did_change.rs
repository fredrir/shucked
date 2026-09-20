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
        client: &Client,
        types::DidChangeTextDocumentParams {
            text_document:
                types::VersionedTextDocumentIdentifier {
                    uri,
                    version: new_version,
                },
            content_changes,
        }: types::DidChangeTextDocumentParams,
    ) -> Result<()> {
        session.completion_environment.cancel_document(&uri);
        let position = content_changes.last().map(|change| {
            let start = change
                .range
                .map_or(types::Position::new(0, 0), |range| range.start);
            let lines = change.text.bytes().filter(|byte| *byte == b'\n').count() as u32;
            let tail = change.text.rsplit('\n').next().unwrap_or_default();
            let columns = match session.encoding() {
                crate::PositionEncoding::UTF8 => tail.len(),
                crate::PositionEncoding::UTF16 => tail.encode_utf16().count(),
                crate::PositionEncoding::UTF32 => tail.chars().count(),
            } as u32;
            types::Position::new(
                start.line + lines,
                if lines == 0 {
                    start.character + columns
                } else {
                    columns
                },
            )
        });
        let key = session.key_from_url(uri.clone());
        session
            .update_text_document(&key, content_changes, new_version)
            .with_failure_code(ErrorCode::InternalError)?;

        if let Some(position) = position
            && let Some(snapshot) = session.take_snapshot(uri)
        {
            crate::handlers::completion::background::prewarm(
                session.completion_environment.clone(),
                snapshot,
                client.clone(),
                position,
            );
        }

        session.schedule_all_diagnostics();

        Ok(())
    }
}
