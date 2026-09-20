use lsp_types as types;
use lsp_types::notification as notif;

use crate::TextDocument;
use crate::server::Result;
use crate::session::{Client, Session};

pub(crate) struct DidOpen;

impl super::super::traits::NotificationHandler for DidOpen {
    type NotificationType = notif::DidOpenTextDocument;
}

impl super::super::traits::SyncNotificationHandler for DidOpen {
    fn run(
        session: &mut Session,
        client: &Client,
        types::DidOpenTextDocumentParams {
            text_document:
                types::TextDocumentItem {
                    uri,
                    text,
                    version,
                    language_id,
                },
        }: types::DidOpenTextDocumentParams,
    ) -> Result<()> {
        let document = TextDocument::new(text, version).with_language_id(&language_id);
        session.open_text_document(uri.clone(), document);

        if let Some(snapshot) = session.take_snapshot(uri) {
            let source = snapshot.query().document().contents();
            let position = crate::edit::offset_to_position(
                source,
                snapshot.query().document().index(),
                source.len(),
                snapshot.encoding(),
            );
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
