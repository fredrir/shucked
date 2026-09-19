#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use shucked_command::*;

fn executable(directory: &Path, name: &str) {
    let path = directory.join(name);
    fs::write(&path, "#!/bin/sh\nexit 99\n").expect("fixture file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("executable fixture");
}

fn context(cwd: &Path) -> ExecutionContext {
    ExecutionContext {
        cwd: Some(cwd.to_owned()),
        cwd_known: true,
        ..ExecutionContext::default()
    }
}

#[test]
fn exact_path_does_not_inherit_system_or_helper_directories() {
    let temporary = tempfile::tempdir().expect("fixture directory");
    executable(temporary.path(), "fixture-command");
    let context = context(temporary.path());
    let snapshot = host::capture(&context, vec![temporary.path().into()], 8);
    assert!(matches!(
        resolve(
            &context,
            &snapshot,
            &CommandSite::literal("fixture-command")
        ),
        CommandResolution::Resolved(_)
    ));
    assert!(matches!(
        resolve(&context, &snapshot, &CommandSite::literal("cargo")),
        CommandResolution::Missing(_)
    ));
    assert_eq!(snapshot.generation, 8);
}

#[test]
fn empty_and_relative_path_entries_resolve_against_known_launch_cwd() {
    let temporary = tempfile::tempdir().expect("fixture directory");
    fs::create_dir(temporary.path().join("tools")).expect("tools directory");
    executable(temporary.path(), "from-cwd");
    executable(&temporary.path().join("tools"), "from-tools");
    let context = context(temporary.path());
    let snapshot = host::capture(&context, vec![PathBuf::new(), "tools".into()], 1);
    for name in ["from-cwd", "from-tools"] {
        assert!(matches!(
            resolve(&context, &snapshot, &CommandSite::literal(name)),
            CommandResolution::Resolved(_)
        ));
    }
    let mut assumed = context.clone();
    assumed.cwd_known = false;
    let snapshot = host::capture(&assumed, vec![PathBuf::new(), "tools".into()], 2);
    assert!(matches!(
        resolve(&assumed, &snapshot, &CommandSite::literal("missing")),
        CommandResolution::Unknown(_)
    ));
}

#[test]
fn exact_lookup_can_find_explicit_relative_path_but_requires_known_cwd() {
    let temporary = tempfile::tempdir().expect("fixture directory");
    executable(temporary.path(), "fixture-command");
    let context = context(temporary.path());
    let mut snapshot = host::capture(&context, Vec::new(), 1);
    host::refresh_exact(&context, &mut snapshot, &["./fixture-command".into()]);
    assert!(matches!(
        resolve(
            &context,
            &snapshot,
            &CommandSite::literal("./fixture-command")
        ),
        CommandResolution::Resolved(_)
    ));
}

#[test]
fn a_nonexecutable_file_and_missing_directory_do_not_make_a_command_available() {
    let temporary = tempfile::tempdir().expect("fixture directory");
    fs::write(temporary.path().join("plain-file"), "not executable").expect("fixture file");
    let context = context(temporary.path());
    let snapshot = host::capture(
        &context,
        vec![temporary.path().join("absent"), temporary.path().into()],
        1,
    );
    assert!(matches!(
        resolve(&context, &snapshot, &CommandSite::literal("plain-file")),
        CommandResolution::Missing(_)
    ));
}

#[test]
fn filesystem_errors_are_unknown_and_an_exact_query_can_bypass_unrelated_bad_entries() {
    let temporary = tempfile::tempdir().expect("fixture directory");
    symlink("loop", temporary.path().join("loop")).expect("loop fixture");
    executable(temporary.path(), "known");
    let context = context(temporary.path());
    let mut snapshot = host::capture(&context, vec![temporary.path().into()], 1);
    assert!(!snapshot.is_complete());
    assert!(matches!(
        resolve(&context, &snapshot, &CommandSite::literal("missing")),
        CommandResolution::Unknown(_)
    ));
    host::refresh_exact(&context, &mut snapshot, &["missing".into(), "loop".into()]);
    assert!(matches!(
        resolve(&context, &snapshot, &CommandSite::literal("missing")),
        CommandResolution::Missing(_)
    ));
    assert!(matches!(
        resolve(&context, &snapshot, &CommandSite::literal("loop")),
        CommandResolution::Unknown(_)
    ));
}

#[test]
fn captured_target_cannot_be_refreshed_from_local_filesystem() {
    let temporary = tempfile::tempdir().expect("fixture directory");
    executable(temporary.path(), "local-only");
    let mut context = context(temporary.path());
    context.policy = ValidationPolicy::Captured;
    let mut snapshot = EnvironmentSnapshot::empty(&context);
    host::refresh_exact(&context, &mut snapshot, &["local-only".into()]);
    assert!(snapshot.exact_lookups.is_empty());
    assert!(matches!(
        host::exact_lookup(&context, &snapshot, "local-only"),
        LookupEvidence::Unknown(_)
    ));
}
