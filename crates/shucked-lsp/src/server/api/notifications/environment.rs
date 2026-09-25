use crate::session::{Client, Session};
use lsp_types::Url;
use serde::{Deserialize, Serialize};

pub(crate) enum SelectEnvironment {}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectEnvironmentParams {
    uri: Url,
    options: Option<crate::session::environment_options::EnvironmentOptions>,
}
impl lsp_types::notification::Notification for SelectEnvironment {
    type Params = SelectEnvironmentParams;
    const METHOD: &'static str = "shucked/selectEnvironment";
}
impl super::super::traits::NotificationHandler for SelectEnvironment {
    type NotificationType = Self;
}
impl super::super::traits::SyncNotificationHandler for SelectEnvironment {
    fn run(
        session: &mut Session,
        _: &Client,
        params: SelectEnvironmentParams,
    ) -> crate::server::Result<()> {
        session.select_environment(params.uri, params.options);
        Ok(())
    }
}

/// `shucked/environmentDetails`: a Markdown report of the execution context,
/// evidence provenance, provider availability and the command under the cursor.
pub(crate) enum EnvironmentDetailsRequest {}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvironmentDetailsParams {
    text_document: lsp_types::TextDocumentIdentifier,
    #[serde(default)]
    position: Option<lsp_types::Position>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnvironmentDetailsResult {
    markdown: String,
    trusted: bool,
    source: String,
}
impl lsp_types::request::Request for EnvironmentDetailsRequest {
    type Params = EnvironmentDetailsParams;
    type Result = EnvironmentDetailsResult;
    const METHOD: &'static str = "shucked/environmentDetails";
}
impl super::super::traits::RequestHandler for EnvironmentDetailsRequest {
    type RequestType = Self;
}
pub(crate) struct EnvironmentDetailsSnapshot {
    document: Option<crate::session::DocumentSnapshot>,
    trusted: bool,
}
impl super::super::traits::BackgroundRequestHandler for EnvironmentDetailsRequest {
    type Snapshot = EnvironmentDetailsSnapshot;

    fn snapshot(
        session: &Session,
        params: &EnvironmentDetailsParams,
        cancellation: crate::session::RequestCancellationToken,
    ) -> crate::server::Result<Self::Snapshot> {
        Ok(EnvironmentDetailsSnapshot {
            document: session
                .take_snapshot(params.text_document.uri.clone())
                .map(|snapshot| snapshot.with_analysis_cancellation(cancellation)),
            trusted: session.native_execution_allowed(),
        })
    }

    fn run_with_snapshot(
        snapshot: Self::Snapshot,
        _: &Client,
        params: EnvironmentDetailsParams,
    ) -> crate::server::Result<EnvironmentDetailsResult> {
        use crate::edit::PositionExt;
        let host = crate::handlers::commands::HostDetails::detect(snapshot.trusted);
        let Some(document) = snapshot.document else {
            return Ok(EnvironmentDetailsResult {
                markdown: format!(
                    "# Shucked execution context\n\n- Document `{}` is not open.\n- Workspace trust (native execution): {}\n",
                    params.text_document.uri,
                    if host.trusted { "trusted" } else { "untrusted" }
                ),
                trusted: host.trusted,
                source: String::new(),
            });
        };
        let offset = params.position.map(|position| {
            position.to_offset(
                document.query().document().contents(),
                document.query().document().index(),
                document.encoding(),
            )
        });
        let markdown = crate::handlers::commands::environment_details(&document, offset, &host);
        let source = document.command_service.analysis(&document).source.label();
        Ok(EnvironmentDetailsResult {
            markdown,
            trusted: host.trusted,
            source,
        })
    }
}

pub(crate) enum ShellSession {}
impl lsp_types::notification::Notification for ShellSession {
    type Params = crate::handlers::commands::ShellSessionState;
    const METHOD: &'static str = "shucked/shellSession";
}
impl super::super::traits::NotificationHandler for ShellSession {
    type NotificationType = Self;
}
impl super::super::traits::SyncNotificationHandler for ShellSession {
    fn run(
        session: &mut Session,
        _: &Client,
        params: crate::handlers::commands::ShellSessionState,
    ) -> crate::server::Result<()> {
        session.update_shell_session(params);
        Ok(())
    }
}
