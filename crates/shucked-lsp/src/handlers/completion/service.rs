//! Bounded background completion work shared across document versions.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam::channel::{Receiver, Sender, TrySendError, bounded};
use lsp_types::{Position, Url};

use super::{environment::Directory, native_zsh::Candidate};
use crate::session::{Client, RequestCancellationToken};

const MAX_CACHE: usize = 128;
const MAX_CANDIDATES: usize = 40_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Native(String),
    Preparation(String),
    Directory(std::path::PathBuf),
}

#[derive(Clone)]
pub(super) enum Output {
    Candidates(Arc<Vec<Candidate>>),
    Directory(Arc<Directory>),
    Invalidated,
    Prepared,
}

impl Output {
    fn len(&self) -> usize {
        match self {
            Self::Candidates(items) => items.len(),
            Self::Directory(directory) => directory.entries.len(),
            Self::Invalidated | Self::Prepared => 0,
        }
    }
}

#[derive(Clone)]
pub(super) struct Notice {
    pub client: Client,
    pub uri: Url,
    pub version: i32,
    pub position: Position,
}

struct Cached {
    key: Key,
    created: Instant,
    output: Output,
}

struct Pending {
    key: Key,
    generation: u64,
    cancellation: RequestCancellationToken,
    listeners: Vec<Notice>,
    prewarm: bool,
}

#[derive(Default)]
struct State {
    cache: VecDeque<Cached>,
    pending: Vec<Pending>,
    failures: VecDeque<(Key, Instant)>,
    watched: VecDeque<std::path::PathBuf>,
    recent: VecDeque<Notice>,
}

type Work = Box<dyn FnOnce(&RequestCancellationToken) -> Option<Output> + Send>;
struct Job {
    state: Arc<Mutex<State>>,
    generation: u64,
    cancellation: RequestCancellationToken,
    work: Work,
}

pub(super) struct Service {
    state: Arc<Mutex<State>>,
    next: AtomicU64,
    sender: Sender<Job>,
    queued: Receiver<Job>,
    directory_sender: Sender<Job>,
    directory_queued: Receiver<Job>,
}

impl Default for Service {
    fn default() -> Self {
        let (sender, receiver) = bounded::<Job>(32);
        let (directory_sender, directory_receiver) = bounded::<Job>(32);
        for (name, receiver) in [
            ("provider-1", receiver.clone()),
            ("provider-2", receiver.clone()),
            ("directory", directory_receiver.clone()),
        ] {
            std::thread::Builder::new()
                .name(format!("shucked-completion-{name}"))
                .spawn(move || {
                    while let Ok(job) = receiver.recv() {
                        job.run();
                    }
                })
                .expect("completion worker thread");
        }
        Self {
            state: Arc::default(),
            next: AtomicU64::new(1),
            sender,
            queued: receiver,
            directory_sender,
            directory_queued: directory_receiver,
        }
    }
}

impl Service {
    pub fn watch_directories(&self) -> Vec<std::path::PathBuf> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.watched.iter().cloned().collect()
    }

    pub fn remember(&self, notice: Notice) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.recent.retain(|old| old.uri != notice.uri);
        state.recent.push_back(notice);
        while state.recent.len() > 8 {
            state.recent.pop_front();
        }
    }

    pub fn recent(&self) -> Vec<Notice> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recent
            .iter()
            .cloned()
            .collect()
    }

    pub fn invalidate(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.cache.clear();
        state.failures.clear();
        let mut listeners = Vec::new();
        for pending in state.pending.drain(..) {
            pending.cancellation.cancel();
            for listener in pending.listeners {
                if !listeners.iter().any(|old: &Notice| {
                    old.uri == listener.uri
                        && old.version == listener.version
                        && old.position == listener.position
                }) {
                    listeners.push(listener);
                }
            }
        }
        while self.queued.try_recv().is_ok() {}
        while self.directory_queued.try_recv().is_ok() {}
        drop(state);
        for notice in listeners {
            notify_invalidated(notice, self.next.fetch_add(1, Ordering::Relaxed));
        }
    }

    pub fn cancel_document(&self, uri: &Url, current: Option<(i32, Position)>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.recent.retain(|notice| {
            &notice.uri != uri || current == Some((notice.version, notice.position))
        });
        for pending in &mut state.pending {
            pending.listeners.retain(|notice| {
                &notice.uri != uri || current == Some((notice.version, notice.position))
            });
            if pending.listeners.is_empty() && !pending.prewarm {
                pending.cancellation.cancel();
            }
        }
        state
            .pending
            .retain(|pending| !pending.cancellation.is_cancelled());
    }

    /// Returns cached data immediately; an expired entry is refreshed once in the background.
    pub fn query(
        &self,
        key: Key,
        notice: Option<Notice>,
        work: impl FnOnce(&RequestCancellationToken) -> Option<Output> + Send + 'static,
    ) -> (Option<Output>, bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Key::Directory(path) = &key {
            state.watched.retain(|old| old != path);
            state.watched.push_back(path.clone());
            while state.watched.len() > 64 {
                state.watched.pop_front();
            }
        }
        let cached = state.cache.iter().find(|entry| entry.key == key);
        let fresh = cached.is_some_and(|entry| entry.created.elapsed() < Duration::from_secs(2));
        let output = cached
            .filter(|entry| entry.created.elapsed() < Duration::from_secs(30))
            .map(|entry| entry.output.clone());
        if fresh {
            return (output, false);
        }
        if let Some(pending) = state.pending.iter_mut().find(|pending| pending.key == key) {
            if let Some(notice) = notice {
                pending.prewarm = false;
                if !pending.listeners.iter().any(|old| {
                    old.uri == notice.uri
                        && old.version == notice.version
                        && old.position == notice.position
                }) {
                    pending.listeners.push(notice);
                }
            }
            return (output, true);
        }
        state
            .failures
            .retain(|(_, when)| when.elapsed() < Duration::from_millis(250));
        if state.failures.iter().any(|(failed, _)| failed == &key) {
            return (output, true);
        }
        // Speculation cannot fill the demand queue while the user is typing.
        if notice.is_none() && state.pending.len() >= 8 {
            return (output, true);
        }
        let (sender, queued) = if matches!(key, Key::Directory(_)) {
            (&self.directory_sender, &self.directory_queued)
        } else {
            (&self.sender, &self.queued)
        };
        if state.pending.len() >= 32 {
            if let Ok(old) = queued.try_recv().or_else(|_| self.queued.try_recv()) {
                old.cancellation.cancel();
                state
                    .pending
                    .retain(|pending| pending.generation != old.generation);
            } else {
                return (output, true);
            }
        }
        let generation = self.next.fetch_add(1, Ordering::Relaxed);
        let cancellation = RequestCancellationToken::default();
        let prewarm = notice.is_none();
        state.pending.push(Pending {
            key,
            generation,
            cancellation: cancellation.clone(),
            listeners: notice.into_iter().collect(),
            prewarm,
        });
        let job = Job {
            state: self.state.clone(),
            generation,
            cancellation,
            work: Box::new(work),
        };
        tracing::debug!(generation, prewarm, "completion work queued");
        if let Err(error) = sender.try_send(job) {
            match error {
                TrySendError::Full(job) => {
                    if let Ok(old) = queued.try_recv() {
                        old.cancellation.cancel();
                        state
                            .pending
                            .retain(|pending| pending.generation != old.generation);
                    }
                    if sender.try_send(job).is_err() {
                        state
                            .pending
                            .retain(|pending| pending.generation != generation);
                    }
                }
                TrySendError::Disconnected(_) => state
                    .pending
                    .retain(|pending| pending.generation != generation),
            }
        }
        (output, true)
    }
}

impl Job {
    fn run(self) {
        let started = Instant::now();
        tracing::debug!(
            generation = self.generation,
            cancelled = self.cancellation.is_cancelled(),
            "completion work started"
        );
        let output = if self.cancellation.is_cancelled() {
            None
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.work)(&self.cancellation)
            }))
            .unwrap_or_else(|_| {
                tracing::warn!("completion provider panicked");
                None
            })
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(index) = state
            .pending
            .iter()
            .position(|entry| entry.generation == self.generation)
        else {
            return;
        };
        let pending = state.pending.remove(index);
        if self.cancellation.is_cancelled() {
            return;
        }
        let Some(output) = output else {
            tracing::debug!(
                generation = self.generation,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "completion provider unavailable"
            );
            state.failures.push_back((pending.key, Instant::now()));
            while state.failures.len() > MAX_CACHE {
                state.failures.pop_front();
            }
            return;
        };
        if matches!(output, Output::Invalidated) {
            state.cache.retain(|entry| entry.key != pending.key);
            drop(state);
            for notice in pending.listeners {
                notify_invalidated(notice, self.generation);
            }
            return;
        }
        if matches!(output, Output::Prepared) {
            // Readiness lives in the shared analysis caches, which can evict independently.
            drop(state);
            for notice in pending.listeners {
                let _ = notice.client.send_notification_value("shucked/completionReady", serde_json::json!({
                    "uri": notice.uri, "version": notice.version, "position": notice.position,
                    "generation": self.generation, "elapsedMs": started.elapsed().as_millis() as u64,
                    "candidateCount": 0, "reason": "analysisReady",
                }));
            }
            return;
        }
        let count = output.len();
        let previous_count = state
            .cache
            .iter()
            .find(|entry| entry.key == pending.key)
            .map_or(0, |entry| entry.output.len());
        state.cache.retain(|entry| {
            entry.key != pending.key && entry.created.elapsed() < Duration::from_secs(30)
        });
        state.cache.push_back(Cached {
            key: pending.key,
            created: Instant::now(),
            output,
        });
        while state.cache.len() > MAX_CACHE
            || state
                .cache
                .iter()
                .map(|entry| entry.output.len())
                .sum::<usize>()
                > MAX_CANDIDATES
        {
            state.cache.pop_front();
        }
        drop(state);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        tracing::debug!(
            elapsed_ms,
            candidates = count,
            "completion background result"
        );
        if count > 0 || previous_count > 0 {
            for notice in pending.listeners {
                let _ = notice.client.send_notification_value("shucked/completionReady", serde_json::json!({
                    "uri": notice.uri, "version": notice.version, "position": notice.position,
                    "generation": self.generation, "elapsedMs": elapsed_ms, "candidateCount": count,
                }));
            }
        }
    }
}

fn notify_invalidated(notice: Notice, generation: u64) {
    let _ = notice.client.send_notification_value(
        "shucked/completionReady",
        serde_json::json!({
            "uri": notice.uri, "version": notice.version, "position": notice.position,
            "generation": generation, "reason": "environmentChanged", "candidateCount": 0,
        }),
    );
}

#[cfg(test)]
#[path = "../../../tests/completion/service.rs"]
mod tests;
