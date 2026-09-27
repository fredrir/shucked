//! Subcommand candidates for brew, git, docker and kubectl from the cached
//! one-shot inventories in `shucked_command::subcommands`.
//!
//! A cached inventory (memory or the shucked cache directory) answers on the
//! request thread. Otherwise, when native execution is trusted, the single
//! bounded query runs on the completion service; the response stays incomplete
//! and the readiness notice brings the editor back for the result.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use shucked_command::subcommands;

use super::native_zsh::Candidate;
use super::offline::Request;
use super::service::{Key, Output};

/// A tool whose query failed or timed out is not asked again for this long.
const RETRY_AFTER: Duration = Duration::from_secs(60);

static FAILURES: OnceLock<Mutex<VecDeque<(PathBuf, Instant)>>> = OnceLock::new();

fn failures() -> std::sync::MutexGuard<'static, VecDeque<(PathBuf, Instant)>> {
    FAILURES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn invalidate() {
    failures().clear();
}

/// Candidates for the first argument, and whether a query is still pending.
pub(super) fn candidates(request: &Request<'_>) -> (Vec<Candidate>, bool) {
    let Some(identity) = request.resolved.executable.as_ref() else {
        return (Vec::new(), false);
    };
    let Some(tool) = subcommands::tool_name(&identity.path) else {
        return (Vec::new(), false);
    };
    let cache_directory = super::native::assets::cache_directory();
    if let Some(inventory) = subcommands::cached(identity, cache_directory.as_deref()) {
        return (convert(&inventory, tool), false);
    }
    let context = &request.analysis.context;
    let environment = &request.analysis.environment;
    if !request.execution || !subcommands::execution_permitted(context, environment) {
        return (Vec::new(), false);
    }
    {
        let mut failures = failures();
        failures.retain(|(_, when)| when.elapsed() < RETRY_AFTER);
        if failures.iter().any(|(path, _)| *path == identity.path) {
            return (Vec::new(), false);
        }
    }
    let key = Key::Native(
        serde_json::json!({
            "inventory": tool,
            "path": identity.path,
            "size": identity.size,
            "modified": identity.modified_unix_ms,
            "environment": request.snapshot.environment_generation(),
        })
        .to_string(),
    );
    let analysis = request.analysis.clone();
    let identity = identity.clone();
    let worker_snapshot = request.snapshot.clone();
    let (output, pending) = request.environment.service.query(
        key,
        Some(super::background::notice(
            request.snapshot,
            request.client,
            request.position,
        )),
        move |cancel| {
            if worker_snapshot.command_service.generation()
                != worker_snapshot.environment_generation()
            {
                return Some(Output::Invalidated);
            }
            let inventory = subcommands::acquire(
                &analysis.context,
                &analysis.environment,
                &identity,
                cache_directory.as_deref(),
                &|| cancel.is_cancelled(),
            );
            if cancel.is_cancelled() {
                return None;
            }
            let Some(inventory) = inventory else {
                tracing::debug!(tool, "subcommand inventory unavailable");
                let mut failures = failures();
                failures.push_back((identity.path.clone(), Instant::now()));
                while failures.len() > 32 {
                    failures.pop_front();
                }
                return None;
            };
            tracing::debug!(
                tool,
                commands = inventory.commands.len(),
                "subcommand inventory acquired"
            );
            Some(Output::Candidates(Arc::new(convert(&inventory, tool))))
        },
    );
    match output {
        Some(Output::Candidates(items)) => (items.as_ref().clone(), pending),
        _ => (Vec::new(), pending),
    }
}

fn convert(inventory: &subcommands::SubcommandInventory, tool: &str) -> Vec<Candidate> {
    inventory
        .commands
        .iter()
        .map(|entry| Candidate {
            text: entry.name.clone(),
            description: entry.description.clone(),
            kind: None,
            no_space: false,
            provider: tool.to_owned(),
        })
        .collect()
}
