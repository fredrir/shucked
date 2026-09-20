use std::fs;
use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;

fn check(root: &Path, extra: &[&str]) -> Vec<Value> {
    let output = Command::cargo_bin("shucked")
        .unwrap()
        .current_dir(root)
        .env("SHUCKED_CACHE_DIR", root.join("cache"))
        .args(["check", ".", "--select", "C001", "--output-format", "json"])
        .args(extra)
        .output()
        .unwrap();
    assert!(matches!(output.status.code(), Some(0 | 1)), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

const COMMON: &str = r#"#!/usr/bin/env bash
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ADMIN_DIR="$ROOT_DIR/apps/admin"
BACKUP_DIR="$ROOT_DIR/.backups"
"#;
const CONSUMER: &str = r#"#!/usr/bin/env bash
source "$(dirname -- "${BASH_SOURCE[0]}")/_common.sh"
backup_sanity() {
  local target=production
  local backup_path="$BACKUP_DIR/$target/archive.tar.gz"
  ( cd "$ADMIN_DIR"; printf '%s\n' "$backup_path" )
}
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  backup_sanity "$@"
fi
"#;

#[test]
fn workspace_consumers_keep_shared_assignments_and_unsafe_fixes_intact() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("_common.sh"), COMMON).unwrap();
    fs::write(root.path().join("backup-sanity"), CONSUMER).unwrap();
    assert!(check(root.path(), &[]).is_empty());
    assert!(check(root.path(), &["--fix", "--unsafe-fixes"]).is_empty());
    Command::cargo_bin("shucked")
        .unwrap()
        .current_dir(root.path())
        .args(["check", ".", "--select", "C001", "--add-ignore"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(root.path().join("_common.sh")).unwrap(),
        COMMON
    );
}

#[test]
fn workspace_usage_refreshes_cached_helpers_when_consumers_change() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("_common.sh"), "ADMIN_DIR=/srv/admin\n").unwrap();
    let consumer = root.path().join("consumer.sh");
    fs::write(&consumer, "source ./_common.sh\n").unwrap();
    assert_eq!(check(root.path(), &[]).len(), 1);
    fs::write(&consumer, "source ./_common.sh\necho \"$ADMIN_DIR\"\n").unwrap();
    assert!(check(root.path(), &[]).is_empty());
    assert!(check(root.path(), &[]).is_empty());
    fs::write(&consumer, "source ./_common.sh\n").unwrap();
    assert_eq!(check(root.path(), &[]).len(), 1);
    fs::write(&consumer, "source ./_common.sh\necho \"$ADMIN_DIR\"\n").unwrap();
    assert!(check(root.path(), &[]).is_empty());
    fs::remove_file(consumer).unwrap();
    assert_eq!(check(root.path(), &[]).len(), 1);
}

#[test]
fn workspace_usage_respects_source_order_and_variable_shadows() {
    for consumer in [
        "echo \"$ADMIN_DIR\"\nsource ./_common.sh\n",
        "source ./_common.sh\nADMIN_DIR=other\necho \"$ADMIN_DIR\"\n",
        "source ./_common.sh\nf() { local ADMIN_DIR=other; echo \"$ADMIN_DIR\"; }; f\n",
        "echo \"$ADMIN_DIR\"\n",
        "( source ./_common.sh )\necho \"$ADMIN_DIR\"\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("_common.sh"), "ADMIN_DIR=/srv/admin\n").unwrap();
        fs::write(root.path().join("consumer.sh"), consumer).unwrap();
        let diagnostics = check(root.path(), &[]);
        assert!(
            diagnostics.iter().any(|d| d["filename"] == "_common.sh"),
            "{consumer}: {diagnostics:?}"
        );
    }
}

#[test]
fn workspace_usage_follows_transitive_and_sibling_sources_without_extensions() {
    for bridge in ["source ./values\n", "echo \"$ADMIN_DIR\"\n"] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("values"), "ADMIN_DIR=/srv/admin\n").unwrap();
        fs::write(root.path().join("bridge"), bridge).unwrap();
        let consumer = if bridge.starts_with("source") {
            "source ./bridge\necho \"$ADMIN_DIR\"\n"
        } else {
            "source ./values\nsource ./bridge\n"
        };
        fs::write(root.path().join("consumer.sh"), consumer).unwrap();
        // Include the extensionless helper explicitly so its diagnostics are reported.
        let output = Command::cargo_bin("shucked")
            .unwrap()
            .current_dir(root.path())
            .env("SHUCKED_CACHE_DIR", root.path().join("cache"))
            .args([
                "check",
                "consumer.sh",
                "values",
                "--select",
                "C001",
                "--output-format",
                "json",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn workspace_usage_counts_reads_before_the_assignment_takes_effect() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("_common.sh"), "ADMIN_DIR=/srv/admin\n").unwrap();
    fs::write(
        root.path().join("consumer.sh"),
        "source ./_common.sh\nADMIN_DIR=\"$ADMIN_DIR/subdir\"\necho \"$ADMIN_DIR\"\n",
    )
    .unwrap();
    assert!(check(root.path(), &[]).is_empty());
}

#[test]
fn workspace_usage_does_not_guess_unknown_source_options_or_targets() {
    for source in [
        "source \"$(dirname -z -- \"${BASH_SOURCE[0]}\")/_common.sh\"\necho \"$ADMIN_DIR\"\n",
        "source \"$dynamic\"\necho \"$ADMIN_DIR\"\n",
        "source ./_common.sh\nsource \"$dynamic\"\necho \"$ADMIN_DIR\"\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("_common.sh"), "ADMIN_DIR=/srv/admin\n").unwrap();
        fs::write(root.path().join("consumer.sh"), source).unwrap();
        assert_eq!(check(root.path(), &[]).len(), 1, "{source}");
    }
}

#[test]
fn workspace_usage_terminates_on_source_cycles() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("a.sh"),
        "ADMIN_DIR=/srv/admin\nsource ./b.sh\n",
    )
    .unwrap();
    fs::write(
        root.path().join("b.sh"),
        "source ./a.sh\necho \"$ADMIN_DIR\"\n",
    )
    .unwrap();
    // A recursive source cannot prove a completed import of ADMIN_DIR.
    assert_eq!(check(root.path(), &[]).len(), 1);
}

#[test]
fn workspace_sources_follow_derived_directories_and_cached_target_changes() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("scripts")).unwrap();
    fs::create_dir_all(root.path().join("lib one")).unwrap();
    fs::create_dir_all(root.path().join("lib two")).unwrap();
    for directory in ["lib one", "lib two"] {
        fs::write(
            root.path().join(directory).join("common.sh"),
            "ADMIN_DIR=/srv/admin\n",
        )
        .unwrap();
    }
    let consumer = root.path().join("scripts/backup-sanity");
    let source = r#"#!/usr/bin/env bash
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
LIB_DIR="${ROOT_DIR}/lib one"
source "$LIB_DIR/common.sh"
echo "$ADMIN_DIR"
"#;
    fs::write(&consumer, source).unwrap();
    for (content, unused_directory) in [
        (source.to_owned(), "lib two"),
        (source.replace("lib one", "lib two"), "lib one"),
    ] {
        fs::write(&consumer, content).unwrap();
        let diagnostics = check(root.path(), &[]);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(
            diagnostics[0]["filename"],
            format!("{unused_directory}/common.sh")
        );
    }
    Command::cargo_bin("shucked")
        .unwrap()
        .current_dir(root.path())
        .args([
            "check",
            "scripts/backup-sanity",
            "--select",
            "C006",
            "--no-cache",
        ])
        .assert()
        .success();
}

#[cfg(unix)]
#[test]
fn derived_directories_preserve_logical_parent_paths_across_symlinks() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("scripts")).unwrap();
    fs::create_dir_all(root.path().join("elsewhere/child")).unwrap();
    std::os::unix::fs::symlink(
        root.path().join("elsewhere/child"),
        root.path().join("scripts/link"),
    )
    .unwrap();
    for directory in ["scripts", "elsewhere"] {
        fs::write(
            root.path().join(directory).join("common.sh"),
            "ADMIN_DIR=/srv/admin\n",
        )
        .unwrap();
    }
    fs::write(
        root.path().join("scripts/consumer.sh"),
        r#"#!/usr/bin/env bash
SCRIPT_DIR="$(dirname -- "${BASH_SOURCE[0]}")"
LIB_DIR="$(cd -L -- "$SCRIPT_DIR/link/.." && pwd -L)"
source "$LIB_DIR/common.sh"
echo "$ADMIN_DIR"
"#,
    )
    .unwrap();
    let diagnostics = check(root.path(), &[]);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["filename"], "elsewhere/common.sh");
}
