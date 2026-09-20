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
use std::sync::Mutex;
use std::time::Instant;

/// A framework is initialized once; requests and candidate records remain data.
#[derive(Default)]
pub(super) struct Persistent {
    worker: Mutex<Option<Worker>>,
}

struct Worker {
    key: String,
    child: Child,
    #[cfg(windows)]
    job: Option<windows::Job>,
    input: Sender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
    zpty_pid: Option<i32>,
}

impl Persistent {
    pub(super) fn invalidate(&self) {
        if let Ok(mut worker) = self.worker.try_lock() {
            worker.take();
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
        let cold = slot.is_none();
        if cold {
            *slot = Worker::spawn(command, key);
        }
        let request_started = Instant::now();
        let budget = if cold { startup_budget } else { timeout };
        let worker = slot.as_mut()?;
        let mut payload = Vec::new();
        for field in fields {
            if field.contains('\0') {
                return None;
            }
            payload.extend_from_slice(field.as_bytes());
            payload.push(0);
        }
        let result = (|| {
            worker.input.try_send(payload).ok()?;
            let mut output = Vec::new();
            loop {
                if cancellation.is_cancelled()
                    || started.elapsed() >= startup_budget
                    || request_started.elapsed() >= budget
                {
                    return None;
                }
                match worker.output.recv_timeout(Duration::from_millis(5)) {
                    Ok(chunk) => {
                        output.extend_from_slice(&chunk);
                        if output.len() > 1024 * 1024 {
                            return None;
                        }
                        // ZLE runs in a separate process group owned by zpty.
                        if worker.zpty_pid.is_none() {
                            let mut fields = output.split(|byte| *byte == 0);
                            if fields.next() == Some(b"P")
                                && output.iter().filter(|byte| **byte == 0).count() >= 2
                            {
                                worker.zpty_pid = fields
                                    .next()
                                    .and_then(|pid| std::str::from_utf8(pid).ok())
                                    .and_then(|pid| pid.parse().ok())
                                    .filter(|pid| *pid > 1);
                            }
                        }
                        match frame_complete(&output) {
                            Some(true) => return Some(output),
                            Some(false) => {}
                            None => return None,
                        }
                    }
                    Err(channel::RecvTimeoutError::Timeout) => {
                        if worker.child.try_wait().ok()?.is_some() {
                            return None;
                        }
                    }
                    Err(channel::RecvTimeoutError::Disconnected) => return None,
                }
            }
        })();
        if result.is_none() {
            slot.take();
        }
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            success = result.is_some(),
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
            zpty_pid: None,
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            if let Some(pid) = self.zpty_pid {
                libc::kill(-pid, libc::SIGKILL);
            }
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/completion/native_process.rs"]
mod tests;
