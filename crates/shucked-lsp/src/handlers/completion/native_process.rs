use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use crate::session::RequestCancellationToken;

#[cfg(unix)]
pub(super) const MAX_OUTPUT: usize = 4 * 1024 * 1024;

// Drain the pipe while polling cancellation; neither a full pipe nor a helper
// retaining stdout can keep a completion request alive past its deadline.
#[cfg(unix)]
pub(super) fn capture(
    command: &mut Command,
    timeout: Duration,
    cancellation: &RequestCancellationToken,
    zpty: bool,
) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    if cancellation.is_cancelled() {
        return None;
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = Running {
        child: command.spawn().ok()?,
        worker: None,
    };
    let mut stdout = child.child.stdout.take()?;
    // The owned pipe remains alive and has no other readers.
    if unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
        return None;
    }
    let started = Instant::now();
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if cancellation.is_cancelled() || started.elapsed() >= timeout {
            return None;
        }
        match stdout.read(&mut chunk) {
            Ok(0) => {
                if zpty {
                    return Some(output);
                }
                if let Some(status) = child.child.try_wait().ok()? {
                    return status.success().then_some(output);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(size) => {
                output.extend_from_slice(&chunk[..size]);
                if output.len() > MAX_OUTPUT {
                    return None;
                }
                if zpty {
                    if child.worker.is_none() {
                        let mut fields = output.split(|byte| *byte == 0);
                        if fields.next() == Some(b"P")
                            && let Some(pid) = fields.next()
                            && output.iter().filter(|byte| **byte == 0).count() >= 2
                        {
                            child.worker = std::str::from_utf8(pid)
                                .ok()
                                .and_then(|pid| pid.parse::<i32>().ok())
                                .filter(|pid| *pid > 1);
                        }
                    }
                    if output.ends_with(b"\0E\0") {
                        return Some(output);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(_) => return None,
        }
    }
}

#[cfg(not(unix))]
pub(super) fn capture(
    _command: &mut Command,
    _timeout: Duration,
    _cancellation: &RequestCancellationToken,
    _zpty: bool,
) -> Option<Vec<u8>> {
    None
}

#[cfg(unix)]
struct Running {
    child: std::process::Child,
    worker: Option<i32>,
}

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        // zpty creates a separate process group. Include its completion helpers.
        unsafe {
            if let Some(pid) = self.worker {
                libc::kill(-pid, libc::SIGKILL);
            }
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
