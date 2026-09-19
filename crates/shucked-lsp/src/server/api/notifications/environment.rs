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
