use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway repository layout accepted by `find_repo_root`.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(shucked: &str, vscode: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("shucked-tag-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&root);

        std::fs::create_dir_all(root.join("tooling/src")).unwrap();
        std::fs::create_dir_all(root.join("editors/vscode")).unwrap();

        write(
            &root,
            "Cargo.toml",
            &format!(
                "[workspace]\n\n\
                 [workspace.package]\nversion = \"{shucked}\"\n\n\
                 [workspace.dependencies]\n\
                 shucked-ast = {{ path = \"crates/shucked-ast\", version = \"{shucked}\" }}\n\
                 shucked-tooling = {{ path = \"tooling\", version = \"{shucked}\" }}\n"
            ),
        );
        write(
            &root,
            "tooling/Cargo.toml",
            &format!("[package]\nname = \"shucked-tooling\"\nversion = \"{shucked}\"\n"),
        );
        write(
            &root,
            "tooling/src/main.rs",
            &format!("#[command(\n    name = \"tooling\",\n    version = \"{shucked}\",\n)]\n"),
        );
        write(
            &root,
            "Cargo.lock",
            &format!(
                "[[package]]\nname = \"shucked-ast\"\nversion = \"{shucked}\"\n\n\
                 [[package]]\nname = \"shucked-tooling\"\nversion = \"{shucked}\"\n"
            ),
        );
        write(
            &root,
            ".release-please-manifest.json",
            &format!("{{\n  \".\": \"{shucked}\"\n}}\n"),
        );
        write(
            &root,
            "editors/vscode/package.json",
            &format!("{{\n  \"name\": \"shucked\",\n  \"version\": \"{vscode}\",\n}}\n"),
        );

        Fixture { root }
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative)).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tooling"))
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(root: &Path, relative: &str, contents: &str) {
    std::fs::write(root.join(relative), contents).unwrap();
}

fn assert_success(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
fn get_prints_both_versions() {
    let fixture = Fixture::new("0.0.2", "0.0.6");

    let stdout = assert_success(&fixture.run(&["tag"]));

    assert!(stdout.contains("shucked      0.0.2"), "stdout: {stdout}");
    assert!(stdout.contains("vscode       0.0.6"), "stdout: {stdout}");
}

#[test]
fn up_bumps_vscode_version_only() {
    let fixture = Fixture::new("0.0.2", "0.0.6");

    let stdout = assert_success(&fixture.run(&["tag", "vscode", "--up"]));

    assert_eq!(stdout.trim(), "0.0.6 --> 0.0.7");
    assert!(
        fixture
            .read("editors/vscode/package.json")
            .contains("\"0.0.7\"")
    );
    assert!(fixture.read("Cargo.toml").contains("\"0.0.2\""));
}

#[test]
fn up_bumps_shucked_across_tracked_files() {
    let fixture = Fixture::new("0.0.2", "0.0.6");

    let stdout = assert_success(&fixture.run(&["tag", "shucked", "--up"]));

    assert_eq!(stdout.trim(), "0.0.2 --> 0.0.3");
    for relative in [
        "Cargo.toml",
        "tooling/Cargo.toml",
        "tooling/src/main.rs",
        "Cargo.lock",
        ".release-please-manifest.json",
    ] {
        let contents = fixture.read(relative);
        assert!(
            contents.contains("0.0.3"),
            "{relative} not bumped: {contents}"
        );
        assert!(
            !contents.contains("0.0.2"),
            "{relative} retained old version: {contents}"
        );
    }
    assert!(
        fixture
            .read("editors/vscode/package.json")
            .contains("\"0.0.6\"")
    );
}

#[test]
fn down_refuses_to_go_below_zero() {
    let fixture = Fixture::new("0.0.0", "0.0.6");

    let output = fixture.run(&["tag", "shucked", "--down"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("below zero"), "stderr: {stderr}");
    assert!(fixture.read("Cargo.toml").contains("\"0.0.0\""));
}
