use std::sync::Arc;

use lsp_types::Position;

use super::{
    environment::Environment,
    native_zsh::Candidate,
    service::{Key, Notice, Output},
};
use crate::edit::PositionExt;
use crate::session::{Client, DocumentSnapshot};
use crate::workspace_functions::{
    WorkspaceFunctionContext, cached_workspace_function_index, completion_workspace_function_index,
};

/// Keep workspace discovery and command analysis off the completion response path.
pub(crate) fn prepare(
    environment: &Environment,
    snapshot: &DocumentSnapshot,
    workspace: &WorkspaceFunctionContext,
    client: &Client,
    position: Position,
) -> bool {
    environment.service.cancel_document(
        snapshot.query().file_url(),
        Some((snapshot.query().document().version(), position)),
    );
    environment
        .service
        .remember(notice(snapshot, client, position));
    let fish = crate::handlers::commands::dialect(snapshot) == "fish";
    if cached_workspace_function_index(workspace).is_some()
        && snapshot.command_service.cached_analysis(snapshot).is_some()
        && (fish || snapshot.cached_analysis().is_some())
    {
        return true;
    }
    let key = Key::Preparation(format!(
        "{}:{}:{}:{}:{}",
        snapshot.query().file_url(),
        snapshot.query().document().version(),
        snapshot.environment_generation(),
        snapshot.analysis_settings_epoch(),
        workspace.epoch,
    ));
    let mut workspace = workspace.clone();
    let worker_snapshot = snapshot.clone();
    environment.service.query(
        key,
        Some(notice(snapshot, client, position)),
        move |cancel| {
            if cancel.is_cancelled() {
                return None;
            }
            workspace.cancellation = cancel.clone();
            let snapshot = worker_snapshot.with_analysis_cancellation(cancel.clone());
            if completion_workspace_function_index(&workspace).is_none() {
                return (!cancel.is_cancelled()).then_some(Output::Invalidated);
            }
            if let Some(analysis) = snapshot.analysis() {
                analysis.semantic();
            }
            snapshot.command_service.analysis(&snapshot);
            if cancel.is_cancelled() {
                return None;
            }
            if workspace.cache.current_epoch() != workspace.epoch
                || snapshot.command_service.generation() != snapshot.environment_generation()
            {
                return Some(Output::Invalidated);
            }
            Some(Output::Prepared)
        },
    );
    false
}

pub(crate) fn prewarm(
    environment: Arc<Environment>,
    snapshot: DocumentSnapshot,
    client: Client,
    position: Position,
) {
    warm(environment, snapshot, client, position, false);
}

pub(crate) fn refresh(
    environment: Arc<Environment>,
    snapshot: DocumentSnapshot,
    client: Client,
    position: Position,
) {
    warm(environment, snapshot, client, position, true);
}

fn warm(
    environment: Arc<Environment>,
    snapshot: DocumentSnapshot,
    client: Client,
    position: Position,
    refresh: bool,
) {
    let key = Key::Preparation(format!(
        "prewarm:{refresh}:{}:{}:{position:?}",
        snapshot.query().file_url(),
        snapshot.query().document().version()
    ));
    let service = environment.service.clone();
    service.query(
        key,
        Some(notice(&snapshot, &client, position)),
        move |cancel| {
            if cancel.is_cancelled() {
                return None;
            }
            let snapshot = snapshot.with_analysis_cancellation(cancel.clone());
            let options = snapshot.client_settings().completion();
            let analysis = snapshot.analysis()?;
            let offset = position.to_offset(
                analysis.source(),
                analysis.line_index(),
                snapshot.encoding(),
            );
            let site = super::context::at(analysis.source(), analysis.indexer(), offset)?;
            let command = snapshot.command_service.analysis(&snapshot);
            if cancel.is_cancelled() {
                return None;
            }
            if snapshot.command_service.generation() != snapshot.environment_generation() {
                return Some(Output::Invalidated);
            }
            let scoped = environment.scoped(&command.context, &command.environment);
            if command.local_environment && options.include_paths {
                scoped.directory_cached(
                    &crate::handlers::commands::cwd(&snapshot),
                    refresh.then(|| notice(&snapshot, &client, position)),
                );
            }
            if command.local_environment
                && scoped.native_allowed
                && options.include_native
                && options.include_command_arguments
                && !site.redirect
            {
                if site.command {
                    if super::command_names(&command.context, &command.environment)
                        .contains(&site.prefix)
                    {
                        prewarm_next(
                            &scoped,
                            &snapshot,
                            &client,
                            &site.words,
                            &site.prefix,
                            position,
                        );
                    }
                } else {
                    let permitted = command
                        .sites
                        .iter()
                        .rfind(|(facts, _)| facts.span.start.offset() <= offset)
                        .is_none_or(|(facts, resolution)| {
                            super::grammar_allowed(facts, resolution)
                        });
                    if permitted && (refresh || site.prefix.is_empty() || site.prefix == "-") {
                        candidates(
                            &scoped,
                            &snapshot,
                            &client,
                            &site.words,
                            &site.prefix,
                            &site.suffix,
                            position,
                            false,
                            !refresh,
                        );
                    }
                }
            }
            Some(Output::Candidates(Arc::new(Vec::new())))
        },
    );
}

pub(super) fn notice(snapshot: &DocumentSnapshot, client: &Client, position: Position) -> Notice {
    Notice {
        client: client.clone(),
        uri: snapshot.query().file_url().clone(),
        version: snapshot.query().document().version(),
        position,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn candidates(
    environment: &Environment,
    snapshot: &DocumentSnapshot,
    client: &Client,
    words: &[String],
    prefix: &str,
    suffix: &str,
    position: Position,
    live: bool,
    prewarm: bool,
) -> (Option<Arc<Vec<Candidate>>>, bool) {
    let command_analysis = snapshot.command_service.analysis(snapshot);
    let directory = crate::handlers::commands::cwd(snapshot);
    let dialect = crate::handlers::commands::dialect(snapshot).to_owned();
    let key = Key::Native(
        serde_json::json!({
            "context": command_analysis.context,
            "environment": snapshot.environment_generation(),
            "settings": snapshot.analysis_settings_epoch(),
            "path": environment.execution_path(),
            "words": words, "prefix": prefix, "suffix": suffix,
            "dialect": dialect, "live": live,
            "session": snapshot.client_settings().environment().session_id,
        })
        .to_string(),
    );
    let worker_environment = environment.clone();
    let words = words.to_vec();
    let prefix = prefix.to_owned();
    let suffix = suffix.to_owned();
    let worker_snapshot = snapshot.clone();
    let worker_client = client.clone();
    let (output, pending) = environment.service.query(
        key,
        (!prewarm).then(|| notice(snapshot, client, position)),
        move |cancel| {
            if worker_snapshot.command_service.generation()
                != worker_snapshot.environment_generation()
            {
                tracing::debug!("completion environment changed before provider started");
                return Some(Output::Invalidated);
            }
            let snapshot = worker_snapshot.with_analysis_cancellation(cancel.clone());
            let result = if live {
                crate::server::live_completion::request(
                    &worker_client,
                    &snapshot,
                    &words,
                    &prefix,
                    cancel,
                )
                .map(|reply| {
                    Arc::new(
                        reply
                            .candidates
                            .into_iter()
                            .map(|item| Candidate {
                                text: item.text,
                                description: item.description,
                                provider: "Attached shell".into(),
                                ..Default::default()
                            })
                            .collect(),
                    )
                })
            } else {
                worker_environment.native.complete_at(
                    &worker_environment,
                    &words,
                    &prefix,
                    &directory,
                    cancel,
                    false,
                    &dialect,
                    &suffix,
                )
            };
            if cancel.is_cancelled() {
                return None;
            }
            if snapshot.command_service.generation() != snapshot.environment_generation() {
                tracing::debug!("completion environment changed while provider ran");
                return Some(Output::Invalidated);
            }
            result.map(Output::Candidates)
        },
    );
    (
        match output {
            Some(Output::Candidates(items)) => Some(items),
            _ => None,
        },
        pending,
    )
}

/// Warm the next argument context as a command name is being completed.
pub(super) fn prewarm_next(
    environment: &Environment,
    snapshot: &DocumentSnapshot,
    client: &Client,
    words: &[String],
    current: &str,
    position: Position,
) {
    if current.is_empty() || current.starts_with('-') || current.contains('/') || words.len() > 8 {
        return;
    }
    let mut next = words.to_vec();
    next.push(current.to_owned());
    let _ = candidates(
        environment,
        snapshot,
        client,
        &next,
        "",
        "",
        position,
        false,
        true,
    );
}
