#![cfg(windows)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn fixture(mode: &str, pid_file: &std::path::Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "windows_process_fixture", "--nocapture"])
        .env("SHUCKED_PROCESS_FIXTURE_MODE", mode)
        .env("SHUCKED_PROCESS_FIXTURE_PID", pid_file);
    command
}

#[test]
fn windows_process_fixture() {
    let Ok(mode) = std::env::var("SHUCKED_PROCESS_FIXTURE_MODE") else {
        return;
    };
    if mode != "descendant" {
        let pid_file = std::env::var_os("SHUCKED_PROCESS_FIXTURE_PID").unwrap();
        let mut child = fixture("descendant", std::path::Path::new(&pid_file))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::fs::write(pid_file, child.id().to_string()).unwrap();
        if mode == "framed" {
            std::io::stdout().write_all(b"P\0fixture\0E\0").unwrap();
            std::io::stdout().flush().unwrap();
        }
        child.wait().unwrap();
    }
    loop {
        std::thread::park_timeout(Duration::from_secs(60));
    }
}

fn assert_descendant_stopped(pid_file: &std::path::Path) {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    let pid = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) };
    if !handle.is_null() {
        let status = unsafe { WaitForSingleObject(handle, 1000) };
        unsafe { CloseHandle(handle) };
        assert_eq!(status, 0, "worker descendant survived job cleanup");
    }
}

#[test]
fn timeout_terminates_an_immediately_spawned_windows_descendant() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("child.pid");
    assert!(
        shucked_command::process::capture(
            &mut fixture("timeout", &pid_file),
            Duration::from_secs(1),
            &|| false,
            false
        )
        .is_none()
    );
    assert_descendant_stopped(&pid_file);
}

#[test]
fn framed_completion_terminates_windows_workers_without_waiting_for_exit() {
    let root = tempfile::tempdir().unwrap();
    let pid_file = root.path().join("child.pid");
    let output = shucked_command::process::capture(
        &mut fixture("framed", &pid_file),
        Duration::from_secs(3),
        &|| false,
        true,
    )
    .unwrap();
    assert!(output.ends_with(b"\0E\0"));
    assert_descendant_stopped(&pid_file);
}
