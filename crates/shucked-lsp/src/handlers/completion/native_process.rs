use crate::session::RequestCancellationToken;
use std::process::Command;
use std::time::Duration;

#[cfg(test)]
pub(crate) fn capture(
    command: &mut Command,
    timeout: Duration,
    cancellation: &RequestCancellationToken,
    zpty: bool,
) -> Option<Vec<u8>> {
    shucked_command::process::capture(command, timeout, &|| cancellation.is_cancelled(), zpty)
}

use crossbeam::channel::{self, Receiver, Sender};
#[cfg(windows)]
#[path = "native_windows.rs"]
mod windows;
use std::io::{Read, Write};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const MAX_FRAME: usize = 1024 * 1024;
/// How long a response nobody waits for any more may keep arriving before the
/// worker is considered stuck and killed.
const DRAIN_CAP: Duration = Duration::from_secs(10);

/// A framework is initialized once; requests and candidate records remain data.
///
/// A request that is cancelled or exceeds its budget does not kill the worker:
/// the pending response keeps arriving in the background so the initialized
/// worker stays warm, and an identical request that follows adopts that
/// response instead of running the completer again.
pub(super) struct Persistent {
    worker: Mutex<Option<Worker>>,
    drain_cap: Duration,
}

impl Default for Persistent {
    fn default() -> Self {
        Self {
            worker: Mutex::default(),
            drain_cap: DRAIN_CAP,
        }
    }
}

struct Worker {
    key: String,
    child: Child,
    #[cfg(windows)]
    job: Option<windows::Job>,
    input: Sender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
    /// Shared with the thread finishing an abandoned response.
    shared: Arc<Mutex<Shared>>,
}

#[derive(Default)]
struct Shared {
    /// ZLE runs in a separate process group owned by zpty.
    zpty_pid: Option<i32>,
    drain: Drain,
}

#[derive(Default)]
enum Drain {
    #[default]
    Idle,
    /// A response is still arriving after its requester gave up.
    Draining,
    /// A finished response kept for an identical request.
    Late { payload: Vec<u8>, output: Vec<u8> },
    /// The worker exceeded the drain cap or broke the protocol.
    Dead,
}

enum Exchange {
    Complete(Vec<u8>),
    /// The requester stopped waiting; the partial response so far.
    Abandoned(Vec<u8>),
    Broken,
}

enum Settled {
    Ready,
    Adopted(Vec<u8>),
    Busy,
    Dead,
}

impl Persistent {
    #[cfg(test)]
    pub(super) fn with_drain_cap(drain_cap: Duration) -> Self {
        Self {
            worker: Mutex::default(),
            drain_cap,
        }
    }

    pub(super) fn invalidate(&self) {
        if let Ok(mut worker) = self.worker.try_lock() {
            worker.take();
        }
    }

    /// Whether a request is in progress or an abandoned response is still
    /// arriving, so that a request returning `None` may be retried.
    pub(super) fn busy(&self) -> bool {
        match self.worker.try_lock() {
            Ok(slot) => slot.as_ref().is_some_and(|worker| {
                matches!(
                    worker
                        .shared
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .drain,
                    Drain::Draining
                )
            }),
            Err(std::sync::TryLockError::WouldBlock) => true,
            Err(std::sync::TryLockError::Poisoned(_)) => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn request(
        &self,
        command: &mut Command,
        key: String,
        fields: &[String],
        timeout: Duration,
        startup_timeout: Duration,
        cancellation: &RequestCancellationToken,
    ) -> Option<Vec<u8>> {
        let started = Instant::now();
        let startup_budget = timeout.max(startup_timeout);
        let mut payload = Vec::new();
        for field in fields {
            if field.contains('\0') {
                return None;
            }
            payload.extend_from_slice(field.as_bytes());
            payload.push(0);
        }
        let mut slot = loop {
            if cancellation.is_cancelled() || started.elapsed() >= startup_budget {
                return None;
            }
            if let Ok(slot) = self.worker.try_lock() {
                break slot;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        if slot.as_ref().is_some_and(|worker| worker.key != key) {
            slot.take();
        }
        if let Some(worker) = slot.as_mut() {
            match worker.settle(&payload, started + timeout, cancellation) {
                Settled::Ready => {}
                Settled::Adopted(output) => {
                    tracing::debug!(
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "native completion worker adopted a late response"
                    );
                    return Some(output);
                }
                Settled::Busy => {
                    tracing::debug!(
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        "native completion worker still finishing an abandoned response"
                    );
                    return None;
                }
                Settled::Dead => {
                    slot.take();
                }
            }
        }
        let cold = slot.is_none();
        if cold {
            *slot = Worker::spawn(command, key);
        }
        let budget = if cold { startup_budget } else { timeout };
        let deadline = (Instant::now() + budget).min(started + startup_budget);
        let worker = slot.as_mut()?;
        let result = match worker.exchange(payload.clone(), deadline, cancellation) {
            Exchange::Complete(output) => Some(output),
            Exchange::Abandoned(partial) => {
                worker.drain(payload, partial, self.drain_cap);
                None
            }
            Exchange::Broken => {
                slot.take();
                None
            }
        };
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            success = result.is_some(),
            cold,
            "native completion worker request"
        );
        result
    }
}

// Count fields rather than looking for an E byte inside candidate text.
fn frame_complete(bytes: &[u8]) -> Option<bool> {
    let mut fields = bytes.split_inclusive(|byte| *byte == 0);
    let mut next = || {
        fields
            .next()
            .filter(|field| field.ends_with(&[0]))
            .map(|field| &field[..field.len() - 1])
    };
    match next() {
        None => return Some(false),
        Some(b"P") => {}
        _ => return None,
    }
    if next().is_none() {
        return Some(false);
    }
    loop {
        let count = match next() {
            None => return Some(false),
            Some(b"E") => return Some(true),
            Some(b"M" | b"B") => 2,
            Some(b"C" | b"Q") => 4,
            _ => return None,
        };
        for _ in 0..count {
            if next().is_none() {
                return Some(false);
            }
        }
    }
}

/// Record the worker's process id from the response header once.
fn note_pid(shared: &Mutex<Shared>, output: &[u8]) {
    let mut shared = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if shared.zpty_pid.is_some() {
        return;
    }
    let mut fields = output.split(|byte| *byte == 0);
    if fields.next() == Some(b"P") && output.iter().filter(|byte| **byte == 0).count() >= 2 {
        shared.zpty_pid = fields
            .next()
            .and_then(|pid| std::str::from_utf8(pid).ok())
            .and_then(|pid| pid.parse().ok())
            .filter(|pid| *pid > 1);
    }
}

fn kill_tree(child_pid: u32, zpty_pid: Option<i32>) {
    #[cfg(unix)]
    unsafe {
        if let Some(pid) = zpty_pid {
            libc::kill(-pid, libc::SIGKILL);
        }
        libc::kill(-(child_pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = (child_pid, zpty_pid);
}

impl Worker {
    fn spawn(command: &mut Command, key: String) -> Option<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(not(windows))]
        let mut child = command.spawn().ok()?;
        #[cfg(windows)]
        let (mut child, job) = windows::spawn(command)?;
        let mut stdin = child.stdin.take()?;
        let mut stdout = child.stdout.take()?;
        let (input, requests) = channel::bounded::<Vec<u8>>(1);
        let (responses, output) = channel::bounded(8);
        std::thread::spawn(move || {
            while let Ok(payload) = requests.recv() {
                if stdin.write_all(&payload).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => {
                        if responses.send(buffer[..size].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Some(Self {
            key,
            child,
            #[cfg(windows)]
            job: Some(job),
            input,
            output,
            shared: Arc::default(),
        })
    }

    /// Wait for an abandoned response to finish before sending new input.
    fn settle(
        &mut self,
        payload: &[u8],
        deadline: Instant,
        cancellation: &RequestCancellationToken,
    ) -> Settled {
        loop {
            {
                let mut shared = self
                    .shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match &shared.drain {
                    Drain::Idle => return Settled::Ready,
                    Drain::Dead => return Settled::Dead,
                    Drain::Late { .. } => {
                        if let Drain::Late {
                            payload: late,
                            output,
                        } = std::mem::take(&mut shared.drain)
                        {
                            return if late == payload {
                                Settled::Adopted(output)
                            } else {
                                Settled::Ready
                            };
                        }
                    }
                    Drain::Draining => {}
                }
            }
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                return Settled::Busy;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn exchange(
        &mut self,
        payload: Vec<u8>,
        deadline: Instant,
        cancellation: &RequestCancellationToken,
    ) -> Exchange {
        if self.input.try_send(payload).is_err() {
            return Exchange::Broken;
        }
        let mut output = Vec::new();
        let mut exited = false;
        loop {
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                return Exchange::Abandoned(output);
            }
            match self.output.recv_timeout(Duration::from_millis(5)) {
                Ok(chunk) => {
                    output.extend_from_slice(&chunk);
                    if output.len() > MAX_FRAME {
                        return Exchange::Broken;
                    }
                    note_pid(&self.shared, &output);
                    match frame_complete(&output) {
                        Some(true) => return Exchange::Complete(output),
                        Some(false) => {}
                        None => return Exchange::Broken,
                    }
                }
                // An exited worker may still have its final chunk in flight;
                // the reader closes the channel once the pipe is drained.
                Err(channel::RecvTimeoutError::Timeout) if !exited => {
                    exited = !matches!(self.child.try_wait(), Ok(None));
                }
                Err(channel::RecvTimeoutError::Timeout) => {}
                Err(channel::RecvTimeoutError::Disconnected) => return Exchange::Broken,
            }
        }
    }

    /// Finish an abandoned response in the background, keeping it for an
    /// identical request. A worker that cannot finish within `cap` is killed.
    fn drain(&self, payload: Vec<u8>, mut output: Vec<u8>, cap: Duration) {
        let shared = self.shared.clone();
        shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain = Drain::Draining;
        let receiver = self.output.clone();
        let child_pid = self.child.id();
        let deadline = Instant::now() + cap;
        let thread = std::thread::Builder::new()
            .name("shucked-completion-drain".into())
            .spawn(move || {
                let outcome = loop {
                    let now = Instant::now();
                    if now >= deadline {
                        break Drain::Dead;
                    }
                    match receiver.recv_timeout((deadline - now).min(Duration::from_millis(5))) {
                        Ok(chunk) => {
                            output.extend_from_slice(&chunk);
                            if output.len() > MAX_FRAME {
                                break Drain::Dead;
                            }
                            note_pid(&shared, &output);
                            match frame_complete(&output) {
                                Some(true) => break Drain::Late { payload, output },
                                Some(false) => {}
                                None => break Drain::Dead,
                            }
                        }
                        Err(channel::RecvTimeoutError::Timeout) => {}
                        Err(channel::RecvTimeoutError::Disconnected) => break Drain::Dead,
                    }
                };
                let mut shared = shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if matches!(outcome, Drain::Dead) {
                    tracing::debug!(
                        "native completion worker did not finish an abandoned response"
                    );
                    kill_tree(child_pid, shared.zpty_pid);
                }
                shared.drain = outcome;
            });
        if thread.is_err() {
            self.shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .drain = Drain::Dead;
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let zpty_pid = self
            .shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .zpty_pid;
        kill_tree(self.child.id(), zpty_pid);
        #[cfg(windows)]
        self.job.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/completion/native_process.rs"]
mod tests;
