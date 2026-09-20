use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
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
#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    id: u32,
    process_id: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
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
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> Handle;
    fn Thread32First(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snapshot: Handle, entry: *mut ThreadEntry) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> Handle;
    fn ResumeThread(thread: Handle) -> u32;

}
pub(super) struct Running {
    pub(super) child: std::process::Child,
    job: Handle,
}
impl Running {
    pub(super) fn spawn(command: &mut Command, stderr: bool) -> Option<Self> {
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
            .stdin(Stdio::piped())
            .stdout(if stderr {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stderr(if stderr {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            // Suspend before assignment so even an immediate child cannot escape the job.
            .creation_flags(0x0800_0000 | 0x0000_0004)
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
        if !resume_initial_thread(running.child.id()) {
            return None;
        }
        Some(running)
    }
}

fn resume_initial_thread(process_id: u32) -> bool {
    // std retains only the process handle. The suspended child has not run
    // any user code, so its initial thread can be found without a spawn race.
    let snapshot = unsafe { CreateToolhelp32Snapshot(0x0000_0004, 0) };
    if snapshot as isize == -1 {
        return false;
    }
    let mut entry = ThreadEntry {
        size: std::mem::size_of::<ThreadEntry>() as u32,
        usage: 0,
        id: 0,
        process_id: 0,
        base_priority: 0,
        delta_priority: 0,
        flags: 0,
    };
    let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    let mut resumed = false;
    while found {
        if entry.process_id == process_id {
            let thread = unsafe { OpenThread(0x0002, 0, entry.id) };
            if !thread.is_null() {
                resumed = unsafe { ResumeThread(thread) } != u32::MAX;
                unsafe { CloseHandle(thread) };
            }
            break;
        }
        entry.size = std::mem::size_of::<ThreadEntry>() as u32;
        found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    resumed
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

pub(super) struct Job(usize);
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as Handle);
        }
    }
}

pub(super) fn spawn(command: &mut Command) -> Option<(std::process::Child, Job)> {
    let running = std::mem::ManuallyDrop::new(Running::spawn(command, false)?);
    // Ownership moves together; ManuallyDrop prevents the temporary guard from
    // closing the job or waiting for the child after this transfer.
    let child = unsafe { std::ptr::read(&running.child) };
    Some((child, Job(running.job as usize)))
}
