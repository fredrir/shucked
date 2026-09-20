use anyhow::anyhow;
use crossbeam::select;
use lsp_server::Message;
use lsp_types::{
    self as types, DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher,
    notification::Notification as _,
};

use crate::server::{api, schedule};
use crate::{Server, session::Client};

/// Channel sender for dispatching events into the main server event loop.
pub type MainLoopSender = crossbeam::channel::Sender<Event>;
pub(crate) type MainLoopReceiver = crossbeam::channel::Receiver<Event>;

impl Server {
    pub(super) fn main_loop(&mut self) -> crate::Result<()> {
        self.session.update_environment_watches();
        let mut scheduler = schedule::Scheduler::new(self.worker_threads);
        let mut completed_diagnostics = std::collections::HashMap::new();
        let environment_tick = crossbeam::channel::tick(std::time::Duration::from_millis(100));
        let mut last_environment_refresh = std::time::Instant::now();
        let mut diagnostic_refresh_pending = false;
        let mut token_refresh_pending = false;
        while let Ok(next_event) = self.next_event(&environment_tick) {
            let Some(next_event) = next_event else {
                return Ok(());
            };

            match next_event {
                Event::Message(msg) => {
                    let client = Client::new(
                        self.main_loop_sender.clone(),
                        self.connection.sender.clone(),
                    );

                    let task = match msg {
                        Message::Request(req) => {
                            self.session
                                .request_queue_mut()
                                .incoming_mut()
                                .register(req.id.clone(), req.method.clone());

                            if self.session.is_shutdown_requested() {
                                client.respond_err(
                                    req.id,
                                    lsp_server::ResponseError {
                                        code: lsp_server::ErrorCode::InvalidRequest as i32,
                                        message: "shutdown already requested".to_owned(),
                                        data: None,
                                    },
                                )?;
                                continue;
                            }

                            api::request(req)
                        }
                        Message::Notification(notification) => {
                            if notification.method
                                == types::notification::DidCloseTextDocument::METHOD
                                && let Ok(params) =
                                    serde_json::from_value::<types::DidCloseTextDocumentParams>(
                                        notification.params.clone(),
                                    )
                            {
                                completed_diagnostics.remove(&params.text_document.uri);
                            }
                            if notification.method == lsp_types::notification::Exit::METHOD {
                                if !self.session.is_shutdown_requested() {
                                    return Err(anyhow!(
                                        "received exit notification before shutdown request"
                                    ));
                                }
                                return Ok(());
                            }

                            if notification.method == lsp_types::notification::Initialized::METHOD {
                                self.on_initialized(&client);
                            }

                            api::notification(notification)
                        }
                        Message::Response(response) => {
                            if let Some(handler) = self
                                .session
                                .request_queue_mut()
                                .outgoing_mut()
                                .complete(&response.id)
                            {
                                handler(&client, &mut self.session, response);
                            } else {
                                tracing::error!(
                                    "Received an unexpected response for request {}",
                                    response.id
                                );
                            }
                            continue;
                        }
                    };

                    scheduler.dispatch(task, &mut self.session, client);
                }
                Event::LiveCompletion(pending) => {
                    let client = Client::new(
                        self.main_loop_sender.clone(),
                        self.connection.sender.clone(),
                    );
                    crate::server::live_completion::start(pending, &self.session, &client)?;
                }
                Event::CancelLiveCompletion(id) => {
                    if self
                        .session
                        .request_queue_mut()
                        .outgoing_mut()
                        .complete(&id)
                        .is_some()
                    {
                        self.connection.sender.send(Message::Notification(
                            lsp_server::Notification::new(
                                "$/cancelRequest".into(),
                                serde_json::json!({"id": id}),
                            ),
                        ))?;
                    }
                }
                Event::EnvironmentTick => {
                    self.session.refresh_environment();
                    last_environment_refresh = std::time::Instant::now();
                }
                Event::RefreshTick => {
                    if last_environment_refresh.elapsed() >= std::time::Duration::from_secs(30) {
                        self.session.refresh_environment();
                        last_environment_refresh = std::time::Instant::now();
                    }
                    let client = Client::new(
                        self.main_loop_sender.clone(),
                        self.connection.sender.clone(),
                    );
                    if std::mem::take(&mut diagnostic_refresh_pending) {
                        client.send_request::<types::request::WorkspaceDiagnosticRefresh>(
                            &self.session,
                            (),
                            |_, _, ()| {},
                        )?;
                    }
                    if std::mem::take(&mut token_refresh_pending) {
                        client.send_request::<types::request::SemanticTokensRefresh>(
                            &self.session,
                            (),
                            |_, _, ()| {},
                        )?;
                    }
                }
                Event::DiagnosticsReady(result) => {
                    let Some(current) = self.session.take_snapshot(result.uri.clone()) else {
                        continue;
                    };
                    if current.query().document().version() != result.version
                        || crate::handlers::commands::source_fingerprint(&current)
                            != result.source_fingerprint
                        || current.workspace_epoch() != result.workspace_epoch
                        || current.analysis_settings_epoch() != result.settings_epoch
                        || current.environment_generation() != result.environment_generation
                    {
                        continue;
                    }
                    let diagnostic_key = (
                        result.version,
                        result.settings_epoch,
                        result.workspace_epoch,
                        result.source_fingerprint,
                        result.environment_generation,
                    );
                    if !result.environment_complete
                        && completed_diagnostics.get(&result.uri) == Some(&diagnostic_key)
                    {
                        continue;
                    }
                    if result.environment_complete {
                        completed_diagnostics.insert(result.uri.clone(), diagnostic_key);
                        self.session.update_environment_watches();
                    }
                    let client = Client::new(
                        self.main_loop_sender.clone(),
                        self.connection.sender.clone(),
                    );
                    let capabilities = self.session.resolved_client_capabilities();
                    if capabilities.pull_diagnostics {
                        if capabilities.diagnostic_refresh {
                            diagnostic_refresh_pending = true;
                        }
                    } else {
                        client.send_notification::<types::notification::PublishDiagnostics>(
                            types::PublishDiagnosticsParams {
                                uri: result.uri,
                                version: Some(result.version),
                                diagnostics: result.diagnostics,
                            },
                        )?;
                    }
                    if result.environment_complete && capabilities.semantic_token_refresh {
                        token_refresh_pending = true;
                    }
                }
                Event::SendResponse(response) => {
                    if self
                        .session
                        .request_queue_mut()
                        .incoming_mut()
                        .complete(&response.id)
                        .is_some()
                    {
                        self.connection.sender.send(Message::Response(response))?;
                    } else {
                        tracing::trace!("Ignoring response for cancelled request {}", response.id);
                    }
                }
            }
        }

        Ok(())
    }

    fn next_event(
        &self,
        environment_tick: &crossbeam::channel::Receiver<std::time::Instant>,
    ) -> Result<Option<Event>, crossbeam::channel::RecvError> {
        select!(
            recv(self.connection.receiver) -> msg => Ok(msg.ok().map(Event::Message)),
            recv(self.main_loop_receiver) -> event => event.map(Some),
            recv(environment_tick) -> _ => Ok(Some(Event::RefreshTick)),
        )
    }

    fn on_initialized(&mut self, client: &Client) {
        let dynamic_registration = self
            .client_capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files)
            .and_then(|watched_files| watched_files.dynamic_registration)
            .unwrap_or_default();

        if !dynamic_registration {
            tracing::warn!(
                "LSP client does not support dynamic watched-file registration; closed-file index updates and config reloads are disabled"
            );
            return;
        }

        let watchers = watched_files();

        let register_options =
            match serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers }) {
                Ok(value) => value,
                Err(error) => {
                    tracing::error!("Failed to serialize workspace watcher registration: {error}");
                    return;
                }
            };

        let params = lsp_types::RegistrationParams {
            registrations: vec![lsp_types::Registration {
                id: "shuck-server-watch".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: Some(register_options),
            }],
        };

        let response_handler = |_: &Client, session: &mut crate::Session, ()| {
            session.set_project_settings_cache_enabled(true);
            tracing::info!("Registered workspace file watcher");
        };

        if let Err(err) = client.send_request::<lsp_types::request::RegisterCapability>(
            &self.session,
            params,
            response_handler,
        ) {
            tracing::error!("Failed to register workspace file watcher: {err}");
        }
    }
}

fn watched_files() -> Vec<FileSystemWatcher> {
    let mut watchers = vec![
        // Call-hierarchy and workspace-symbol indexes include every discovered
        // shell file, including extensionless shebang and zsh startup files.
        // Watch the workspace broadly so closed-file edits, creates, deletes,
        // and VCS updates invalidate those indexes.
        FileSystemWatcher {
            glob_pattern: types::GlobPattern::String("**/*".into()),
            kind: None,
        },
        FileSystemWatcher {
            glob_pattern: types::GlobPattern::String("**/.shucked.toml".into()),
            kind: None,
        },
        FileSystemWatcher {
            glob_pattern: types::GlobPattern::String("**/shucked.toml".into()),
            kind: None,
        },
    ];
    // The user-level global config lives outside the workspace, so the
    // relative globs above never cover it; watch its candidate paths
    // explicitly so projects using the global fallback observe edits.
    watchers.extend(
        shucked_config::global_config_candidate_paths()
            .iter()
            .filter_map(|path| path.to_str())
            .map(|path| FileSystemWatcher {
                glob_pattern: types::GlobPattern::String(path.into()),
                kind: None,
            }),
    );
    watchers
}

#[derive(Debug)]
pub enum Event {
    Message(lsp_server::Message),
    SendResponse(lsp_server::Response),
    DiagnosticsReady(crate::server::diagnostic_worker::DiagnosticsReady),
    EnvironmentTick,
    RefreshTick,
    LiveCompletion(crate::server::live_completion::Pending),
    CancelLiveCompletion(lsp_server::RequestId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watched_files_include_all_workspace_paths_and_config_dotfiles() {
        let patterns = watched_files()
            .into_iter()
            .filter_map(|watcher| match watcher.glob_pattern {
                types::GlobPattern::String(pattern) => Some(pattern),
                types::GlobPattern::Relative(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(patterns.iter().any(|pattern| pattern == "**/*"));
        assert!(patterns.iter().any(|pattern| pattern == "**/.shucked.toml"));
        assert!(patterns.iter().any(|pattern| pattern == "**/shucked.toml"));
    }
}
