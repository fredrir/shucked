use std::process::Command;
#[cfg(any(unix, windows))]
use std::process::Stdio;
use std::time::Duration;
#[cfg(any(unix, windows))]
use std::time::Instant;

use crate::session::RequestCancellationToken;

#[cfg(any(unix, windows))]
pub(super) const MAX_OUTPUT: usize = 4 * 1024 * 1024;

// Drain the pipe while polling cancellation; neither a full pipe nor a helper
// retaining stdout can keep a completion request alive past its deadline.
#[cfg(unix)]
pub(crate) fn capture(
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

#[cfg(not(any(unix, windows)))]
pub(crate) fn capture(
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

#[cfg(windows)]
pub(crate) fn capture(
    command: &mut Command,
    timeout: Duration,
    cancellation: &RequestCancellationToken,
    zpty: bool,
) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    if zpty || cancellation.is_cancelled() {
        return None;
    }
    let mut running = windows::Running::spawn(command)?;
    let mut stdout = running.child.stdout.take()?;
    let started = Instant::now();
    let mut output = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if cancellation.is_cancelled() || started.elapsed() >= timeout {
            return None;
        }
        let mut available = 0;
        let ready = unsafe {
            windows::PeekNamedPipe(
                stdout.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if available > 0 {
            let limit = chunk.len().min(available as usize);
            let count = stdout.read(&mut chunk[..limit]).ok()?;
            output.extend_from_slice(&chunk[..count]);
            if output.len() > MAX_OUTPUT {
                return None;
            }
        } else if let Some(status) = running.child.try_wait().ok()? {
            return status.success().then_some(output);
        } else if ready == 0 {
            return None;
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    type Handle = *mut c_void;
    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            information: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        pub(super) fn PeekNamedPipe(
            pipe: Handle,
            buffer: *mut c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            remaining: *mut u32,
        ) -> i32;
    }
    pub(super) struct Running {
        pub(super) child: std::process::Child,
        job: Handle,
    }
    impl Running {
        pub(super) fn spawn(command: &mut Command) -> Option<Self> {
            let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if job.is_null() {
                return None;
            }
            let limits = ExtendedLimits {
                basic: BasicLimits {
                    flags: 0x2000,
                    ..Default::default()
                },
                ..Default::default()
            };
            if unsafe {
                SetInformationJobObject(
                    job,
                    9,
                    (&limits as *const ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                )
            } == 0
            {
                unsafe {
                    CloseHandle(job);
                }
                return None;
            }
            let child = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .spawn();
            let Ok(child) = child else {
                unsafe {
                    CloseHandle(job);
                }
                return None;
            };
            let running = Self { child, job };
            if unsafe { AssignProcessToJobObject(job, running.child.as_raw_handle()) } == 0 {
                return None;
            }
            Some(running)
        }
    }
    impl Drop for Running {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.job);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
