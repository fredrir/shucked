//! Bounded requests to an explicitly attached editor-owned shell session.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
use crate::session::{Client, DocumentSnapshot, RequestCancellationToken, Session};
use crossbeam::channel::{self, Sender};
use lsp_server::{Message, RequestId};
use lsp_types::Url;
use serde::{Deserialize, Serialize};

pub(crate) const METHOD: &str = "shucked/liveCompletion";
const DEADLINE: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Params {
    pub uri: Url,
    pub version: i32,
    pub session_id: String,
    pub generation: u64,
    pub dialect: String,
    pub words: Vec<String>,
    pub prefix: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Candidate {
    pub text: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Response {
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub partial: bool,
    pub reason: Option<String>,
}
#[derive(Debug)]
pub(crate) enum Reply {
    Completed(Option<Response>),
}
#[derive(Debug)]
pub struct Pending {
    id: RequestId,
    params: Params,
    environment_generation: u64,
    settings_epoch: u64,
    fingerprint: u64,
    deadline: Instant,
    cancellation: RequestCancellationToken,
    reply: Sender<Reply>,
}
impl Pending {
    fn current(&self, session: &Session) -> bool {
        if self.deadline <= Instant::now() || self.cancellation.is_cancelled() {
            return false;
        }
        let Some(snapshot) = session.take_snapshot(self.params.uri.clone()) else {
            return false;
        };
        snapshot.query().document().version() == self.params.version
            && snapshot.environment_generation() == self.environment_generation
            && snapshot.analysis_settings_epoch() == self.settings_epoch
            && crate::handlers::commands::source_fingerprint(&snapshot) == self.fingerprint
            && snapshot
                .client_settings()
                .environment()
                .session_id
                .as_deref()
                == Some(&self.params.session_id)
            && snapshot
                .command_service
                .session(&self.params.session_id)
                .is_some_and(|state| {
                    state.connected
                        && state.live_completion
                        && state.generation == self.params.generation
                })
    }
}

pub(crate) fn request(
    client: &Client,
    snapshot: &DocumentSnapshot,
    words: &[String],
    prefix: &str,
    cancellation: &RequestCancellationToken,
) -> Option<Response> {
    if words.is_empty()
        || words.len() > 256
        || words
            .iter()
            .any(|word| word.len() > 8192 || word.contains('\0'))
        || prefix.len() > 8192
        || prefix.contains('\0')
    {
        return None;
    }
    let analysis = snapshot.command_service.analysis(snapshot);
    if analysis.context.mode != shucked_command::ExecutionMode::InteractiveSession
        || !analysis.local_environment
    {
        return None;
    }
    let id = snapshot
        .client_settings()
        .environment()
        .session_id
        .as_deref()?;
    let state = snapshot.command_service.session(id)?;
    let dialect = crate::handlers::commands::dialect(snapshot);
    if !state.connected || !state.live_completion || state.shell.as_deref() != Some(dialect) {
        return None;
    }
    let (reply, receive) = channel::bounded(1);
    let id: RequestId = format!("shucked/live/{}", NEXT_ID.fetch_add(1, Ordering::Relaxed)).into();
    let deadline = Instant::now() + DEADLINE;
    let pending = Pending {
        id: id.clone(),
        params: Params {
            uri: snapshot.query().file_url().clone(),
            version: snapshot.query().document().version(),
            session_id: state.id.clone(),
            generation: state.generation,
            dialect: dialect.into(),
            words: words.to_vec(),
            prefix: prefix.into(),
        },
        environment_generation: snapshot.environment_generation(),
        settings_epoch: snapshot.analysis_settings_epoch(),
        fingerprint: crate::handlers::commands::source_fingerprint(snapshot),
        deadline,
        cancellation: cancellation.clone(),
        reply,
    };
    client.queue_live_completion(pending).ok()?;
    while !cancellation.is_cancelled() && Instant::now() < deadline {
        match receive.recv_timeout(Duration::from_millis(10)) {
            Ok(Reply::Completed(response)) => return response,
            Err(channel::RecvTimeoutError::Disconnected) => break,
            Err(channel::RecvTimeoutError::Timeout) => {}
        }
    }
    let _ = client.cancel_live_completion(id);
    None
}

pub(crate) fn start(pending: Pending, session: &Session, client: &Client) -> crate::Result<()> {
    if !pending.current(session) {
        let _ = pending.reply.send(Reply::Completed(None));
        return Ok(());
    }
    let params = serde_json::to_value(&pending.params)?;
    let id = pending.id.clone();
    let handler = Box::new(
        move |_: &Client, session: &mut Session, response: lsp_server::Response| {
            let response = if pending.current(session) && response.error.is_none() {
                response
                    .result
                    .and_then(|result| serde_json::from_value::<Response>(result).ok())
                    .filter(|result| {
                        result.candidates.len() <= 2000
                            && result
                                .candidates
                                .iter()
                                .map(|candidate| candidate.text.len() + candidate.description.len())
                                .sum::<usize>()
                                <= 1024 * 1024
                            && result.candidates.iter().all(|candidate| {
                                candidate.text.len() <= 8192
                                    && !candidate.text.contains('\0')
                                    && candidate.description.len() <= 16384
                            })
                    })
            } else {
                None
            };
            let _ = pending.reply.send(Reply::Completed(response));
        },
    );
    session
        .request_queue()
        .outgoing()
        .register_named(id.clone(), handler);
    client.send_raw_request(Message::Request(lsp_server::Request {
        id,
        method: METHOD.into(),
        params,
    }))
}
