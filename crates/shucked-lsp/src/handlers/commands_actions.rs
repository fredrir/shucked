//! Explicit command corrections retained and checked by the server until use.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use lsp_server::ErrorCode;
use lsp_types as types;
use serde::{Deserialize, Serialize};
use shucked_command::{CommandResolution, ResolutionSnapshotKey};

use crate::session::{Client, DocumentSnapshot, Session};

pub(crate) const APPLY_COMMAND: &str = "shucked.applyCommandCorrection";
const KIND: &str = "commandCorrection";
const OFFER_LIFETIME: Duration = Duration::from_secs(30);
const MAX_OFFERS: usize = 2048;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static OFFERS: OnceLock<Mutex<VecDeque<OfferedCorrection>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorrectionData {
    kind: String,
    id: u64,
    key: ResolutionSnapshotKey,
    source_fingerprint: String,
}

struct OfferedCorrection {
    data: CorrectionData,
    created: Instant,
    edit: types::TextEdit,
    start: usize,
    end: usize,
    original: String,
}

pub(crate) fn code_actions(
    snapshot: &DocumentSnapshot,
    requested: &types::Range,
) -> Vec<types::CodeActionOrCommand> {
    if !snapshot.resolved_client_capabilities().apply_edit {
        return Vec::new();
    }
    let analysis = snapshot.command_service.analysis(snapshot);
    let mut candidates = Vec::new();
    for (site, resolution) in &analysis.sites {
        let CommandResolution::Missing(missing) = resolution else {
            continue;
        };
        if missing.declaration.is_some()
            || !site.aliases.is_empty()
            || site.name() != Some(missing.name.as_str())
        {
            continue;
        }
        for suggestion in &missing.suggestions {
            candidates.push((site.name_span(), suggestion.clone()));
        }
    }
    for finding in super::commands::validation(snapshot, &analysis).iter() {
        for suggestion in &finding.suggestions {
            candidates.push((finding.span, suggestion.clone()));
        }
    }
    let key = ResolutionSnapshotKey {
        document_uri: snapshot.query().file_url().to_string(),
        document_version: snapshot.query().document().version(),
        analysis_generation: snapshot.analysis_settings_epoch(),
        target_id: analysis.context.target_id.clone(),
        environment_generation: snapshot.environment_generation(),
        // Provider invalidation advances the same environment generation.
        provider_generation: snapshot.environment_generation(),
    };
    let fingerprint = super::commands::source_fingerprint(snapshot);
    let source = snapshot.query().document().contents();
    let mut actions = Vec::new();
    for (span, suggestion) in candidates {
        if suggestion.is_empty()
            || !suggestion
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-+".contains(&byte))
        {
            continue;
        }
        let range = super::commands::range(snapshot, span);
        if !overlaps(&range, requested) {
            continue;
        }
        let Some(original) = source.get(span.start.offset()..span.end.offset()) else {
            continue;
        };
        let data = CorrectionData {
            kind: KIND.into(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            key: key.clone(),
            source_fingerprint: format!("{fingerprint:016x}"),
        };
        let Ok(payload) = serde_json::to_value(&data) else {
            continue;
        };
        let edit = types::TextEdit {
            range,
            new_text: suggestion.clone(),
        };
        let offer = OfferedCorrection {
            data,
            created: Instant::now(),
            edit,
            start: span.start.offset(),
            end: span.end.offset(),
            original: original.into(),
        };
        let mut offers = OFFERS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        offers.retain(|offer| offer.created.elapsed() < OFFER_LIFETIME);
        offers.push_back(offer);
        while offers.len() > MAX_OFFERS {
            offers.pop_front();
        }
        drop(offers);
        let title = format!("Replace with `{suggestion}`");
        actions.push(types::CodeActionOrCommand::CodeAction(types::CodeAction {
            title: title.clone(),
            kind: Some(types::CodeActionKind::QUICKFIX),
            is_preferred: Some(false),
            command: Some(types::Command {
                title,
                command: APPLY_COMMAND.into(),
                arguments: Some(vec![payload.clone()]),
            }),
            data: snapshot
                .resolved_client_capabilities()
                .code_action_deferred_edit_resolution
                .then_some(payload),
            ..Default::default()
        }));
    }
    actions
}

pub(crate) fn is_command_action(action: &types::CodeAction) -> bool {
    action
        .data
        .as_ref()
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        == Some(KIND)
}

/// Resolve checks do not materialize a raw edit: execution checks again so a
/// target switch after resolution cannot apply an old environment correction.
pub(crate) fn resolve(session: &Session, mut action: types::CodeAction) -> types::CodeAction {
    action.edit = None;
    let valid = action
        .data
        .clone()
        .and_then(|value| serde_json::from_value::<CorrectionData>(value).ok())
        .and_then(|data| checked_edit(session, &data).ok());
    if let Some((_, edit)) = valid {
        let title = format!("Replace with `{}`", edit.new_text);
        action.title = title.clone();
        action.kind = Some(types::CodeActionKind::QUICKFIX);
        action.command = Some(types::Command {
            title,
            command: APPLY_COMMAND.into(),
            arguments: action.data.clone().map(|payload| vec![payload]),
        });
        action.disabled = None;
    } else {
        action.command = None;
        action.disabled = Some(types::CodeActionDisabled {
            reason: "The command context changed or this correction expired; request fixes again"
                .into(),
        });
    }
    action
}

pub(crate) fn execute(
    session: &Session,
    client: &Client,
    arguments: &[serde_json::Value],
) -> crate::server::Result<Option<serde_json::Value>> {
    if arguments.len() != 1 {
        return Err(error(
            ErrorCode::InvalidParams,
            "Expected one issued command correction",
        ));
    }
    let data: CorrectionData = serde_json::from_value(arguments[0].clone()).map_err(|_| {
        error(
            ErrorCode::InvalidParams,
            "Invalid command correction payload",
        )
    })?;
    let (snapshot, edit) = checked_edit(session, &data)?;
    if !snapshot.resolved_client_capabilities().apply_edit {
        return Err(error(
            ErrorCode::InvalidRequest,
            "The client does not support workspace edits",
        ));
    }
    OFFERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|offer| offer.data.id != data.id);
    client.send_request::<types::request::ApplyWorkspaceEdit>(
        session,
        types::ApplyWorkspaceEditParams {
            label: Some("Shucked: correct command spelling".into()),
            edit: super::fix::workspace_edit_for_document(&snapshot, vec![edit]),
        },
        |_, _, response| {
            if !response.applied {
                tracing::debug!("The client declined a command correction");
            }
        },
    )?;
    Ok(None)
}

fn checked_edit(
    session: &Session,
    data: &CorrectionData,
) -> crate::server::Result<(DocumentSnapshot, types::TextEdit)> {
    let stale = || {
        error(
            ErrorCode::ContentModified,
            "The command context changed or this correction expired; request fixes again",
        )
    };
    let uri = types::Url::parse(&data.key.document_uri).map_err(|_| stale())?;
    let snapshot = session.take_snapshot(uri).ok_or_else(stale)?;
    let offers = OFFERS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let offer = offers
        .iter()
        .find(|offer| {
            offer.data.id == data.id
                && offer.data == *data
                && offer.created.elapsed() < OFFER_LIFETIME
        })
        .ok_or_else(stale)?;
    if data.kind != KIND
        || data.key.document_version != snapshot.query().document().version()
        || data.key.analysis_generation != snapshot.analysis_settings_epoch()
        || data.key.environment_generation != snapshot.environment_generation()
        || data.key.provider_generation != snapshot.environment_generation()
        || data.source_fingerprint
            != format!("{:016x}", super::commands::source_fingerprint(&snapshot))
        || snapshot
            .query()
            .document()
            .contents()
            .get(offer.start..offer.end)
            != Some(offer.original.as_str())
    {
        return Err(stale());
    }
    // The range and replacement come exclusively from a server-issued offer;
    // caller-provided edits are never accepted or executed.
    Ok((snapshot, offer.edit.clone()))
}

fn overlaps(actual: &types::Range, requested: &types::Range) -> bool {
    if requested.start == requested.end {
        actual.start <= requested.start && requested.start < actual.end
    } else {
        actual.start < requested.end && requested.start < actual.end
    }
}

fn error(code: ErrorCode, message: &str) -> crate::server::Error {
    crate::server::Error::new(anyhow!("{message}"), code)
}

#[cfg(test)]
#[path = "../../tests/commands/actions.rs"]
mod tests;
