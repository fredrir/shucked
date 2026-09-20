//! Static feedback and debounced environment analysis use independent workers.
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::session::{Client, DocumentSnapshot, RequestCancellationToken};
use lsp_types::{Diagnostic, Url};

pub(crate) struct DiagnosticWorker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct Queue {
    pending: HashMap<Url, Pending>,
    cancellations: HashMap<Url, RequestCancellationToken>,
    stopped: bool,
}

struct Pending {
    due: Instant,
    snapshot: DocumentSnapshot,
    syntax_pending: bool,
    environment_pending: bool,
}

#[derive(Debug)]
pub struct DiagnosticsReady {
    pub uri: Url,
    pub version: i32,
    pub settings_epoch: u64,
    pub workspace_epoch: Option<u64>,
    pub source_fingerprint: u64,
    pub environment_generation: u64,
    pub environment_complete: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl Queue {
    fn schedule(&mut self, snapshot: DocumentSnapshot) {
        let uri = snapshot.query().file_url().clone();
        let cancellation = RequestCancellationToken::default();
        if let Some(previous) = self.cancellations.insert(uri.clone(), cancellation.clone()) {
            previous.cancel();
        }
        self.pending.insert(
            uri,
            Pending {
                due: Instant::now() + Duration::from_millis(400),
                snapshot: snapshot.with_analysis_cancellation(cancellation),
                syntax_pending: true,
                environment_pending: true,
            },
        );
    }

    fn cancel(&mut self, uri: &Url) {
        self.pending.remove(uri);
        if let Some(cancellation) = self.cancellations.remove(uri) {
            cancellation.cancel();
        }
    }

    fn next(
        &mut self,
        environment: bool,
        now: Instant,
    ) -> Result<Option<DocumentSnapshot>, Duration> {
        let next = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                if environment {
                    pending.environment_pending
                } else {
                    pending.syntax_pending
                }
            })
            .min_by_key(|(_, pending)| pending.due)
            .map(|(uri, pending)| (uri.clone(), pending.due));
        let Some((uri, due)) = next else {
            return Ok(None);
        };
        if environment && due > now {
            return Err(due.saturating_duration_since(now));
        }
        let pending = self
            .pending
            .get_mut(&uri)
            .expect("selected pending diagnostic");
        if environment {
            pending.environment_pending = false;
        } else {
            pending.syntax_pending = false;
        }
        let snapshot = pending.snapshot.clone();
        if !pending.syntax_pending && !pending.environment_pending {
            self.pending.remove(&uri);
        }
        Ok(Some(snapshot))
    }
}

impl DiagnosticWorker {
    pub fn new(client: Client) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let mut threads = Vec::new();
        for environment in [false, true] {
            let shared = queue.clone();
            let client = client.clone();
            threads.push(
                std::thread::Builder::new()
                    .name(
                        if environment {
                            "shucked-environment-diagnostics"
                        } else {
                            "shucked-syntax-diagnostics"
                        }
                        .into(),
                    )
                    .spawn(move || {
                        loop {
                            let snapshot = {
                                let (lock, wake) = &*shared;
                                let mut queue = lock
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                loop {
                                    if queue.stopped {
                                        return;
                                    }
                                    match queue.next(environment, Instant::now()) {
                                        Ok(Some(snapshot)) => break snapshot,
                                        Ok(None) => {
                                            queue = wake
                                                .wait(queue)
                                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        }
                                        Err(duration) => {
                                            queue = wake
                                                .wait_timeout(queue, duration)
                                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                                .0
                                        }
                                    }
                                }
                            };
                            if snapshot.analysis_cancellation().is_cancelled() {
                                continue;
                            }
                            let diagnostics = if environment {
                                crate::lint::generate_diagnostics(&snapshot)
                            } else {
                                crate::lint::generate_static_diagnostics(&snapshot)
                            };
                            if snapshot.analysis_cancellation().is_cancelled() {
                                continue;
                            }
                            let result = DiagnosticsReady {
                                uri: snapshot.query().file_url().clone(),
                                version: snapshot.query().document().version(),
                                settings_epoch: snapshot.analysis_settings_epoch(),
                                workspace_epoch: snapshot.workspace_epoch(),
                                source_fingerprint: crate::handlers::commands::source_fingerprint(
                                    &snapshot,
                                ),
                                environment_generation: snapshot.environment_generation(),
                                environment_complete: environment,
                                diagnostics,
                            };
                            if client.queue_diagnostics(result).is_err() {
                                return;
                            }
                        }
                    })
                    .expect("diagnostic worker thread"),
            );
        }
        Self { queue, threads }
    }

    pub fn schedule(&self, snapshot: DocumentSnapshot) {
        let (lock, wake) = &*self.queue;
        lock.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .schedule(snapshot);
        wake.notify_all();
    }

    pub fn cancel(&self, uri: &Url) {
        self.queue
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancel(uri);
    }
}

impl Drop for DiagnosticWorker {
    fn drop(&mut self) {
        let mut queue = self
            .queue
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.stopped = true;
        for token in queue.cancellations.values() {
            token.cancel();
        }
        self.queue.1.notify_all();
        drop(queue);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
#[path = "../../tests/server/diagnostic_worker.rs"]
mod tests;
