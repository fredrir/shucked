#![cfg(unix)]
use shucked_command::*;
use std::os::unix::fs::PermissionsExt;

fn executable(path: &std::path::Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn context(root: &std::path::Path) -> ExecutionContext {
    ExecutionContext {
        target_id: "captured-host".into(),
        cwd: Some(root.into()),
        cwd_known: true,
        native_execution_allowed: true,
        ..Default::default()
    }
}

#[test]
fn captured_capabilities_survive_tool_upgrade_and_removal_without_host_fallback() {
    let root = tempfile::tempdir().unwrap();
    let tool = root.path().join("eza");
    executable(
        &tool,
        "[ \"$*\" = --version ] || exit 99\nprintf 'eza fixture\\nv0.23.5\\n'",
    );
    let context = context(root.path());
    let mut environment = host::capture(&context, vec![root.path().into()], 0);
    let report = metadata::capture_capabilities(&context, &mut environment, &|| false);
    assert!(report.recorded.contains("eza"));
    let old = TargetInventory::capture("audited version", &context, &environment);
    let old = TargetInventory::from_json(&old.to_json().unwrap()).unwrap();
    executable(
        &tool,
        "[ \"$*\" = --version ] || exit 99\nprintf 'eza fixture\\nv99.0.0\\n'",
    );
    let mut updated = host::capture(&context, vec![root.path().into()], 0);
    let report = metadata::capture_capabilities(&context, &mut updated, &|| false);
    assert!(report.unknown.contains("eza"));
    let future = TargetInventory::capture("uncovered version", &context, &updated);
    std::fs::remove_file(&tool).unwrap();
    let comparison = compare_targets(
        &[old, future],
        &[CommandSite {
            arguments: vec!["--icnos".into()],
            ..CommandSite::literal("eza")
        }],
    );
    assert!(matches!(
        comparison.commands[0].validation[0],
        ValidationResult::Invalid(_)
    ));
    assert!(matches!(
        comparison.commands[0].validation[1],
        ValidationResult::Unknown(_)
    ));
    assert!(
        comparison.commands[0]
            .results
            .iter()
            .all(|resolution| resolution.resolved().is_some())
    );
}

#[test]
fn permission_and_frozen_contexts_prevent_metadata_execution() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    executable(
        &root.path().join("eza"),
        &format!("printf x > '{}'", marker.display()),
    );
    for (allowed, policy) in [
        (false, ValidationPolicy::Workspace),
        (true, ValidationPolicy::Captured),
        (true, ValidationPolicy::Portable),
    ] {
        let mut context = context(root.path());
        let mut snapshot = host::capture(&context, vec![root.path().into()], 0);
        context.native_execution_allowed = allowed;
        context.policy = policy;
        assert!(
            metadata::capture_capabilities(&context, &mut snapshot, &|| false)
                .recorded
                .is_empty()
        );
    }
    assert!(!marker.exists());
}

#[test]
fn curl_capture_disables_personal_configuration_with_fixed_query_arguments() {
    let root = tempfile::tempdir().unwrap();
    executable(
        &root.path().join("curl"),
        "[ \"$*\" = '-q --version' ] || exit 99\nprintf 'curl 8.7.1 fixture\\n'",
    );
    let context = context(root.path());
    let mut environment = host::capture(&context, vec![root.path().into()], 0);
    assert!(
        metadata::capture_capabilities(&context, &mut environment, &|| false)
            .recorded
            .contains("curl")
    );
}

#[test]
fn bounded_stderr_capture_supports_version_interfaces() {
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "printf stdout; printf version >&2"]);
    assert_eq!(
        process::capture_stderr(&mut command, std::time::Duration::from_secs(1), &|| false),
        Some(b"version".to_vec())
    );
}

#[test]
fn ssh_version_query_validates_local_options_without_touching_hosts_or_config() {
    let root = tempfile::tempdir().unwrap();
    executable(
        &root.path().join("ssh"),
        "[ \"$*\" = -V ] || exit 99\nprintf 'OpenSSH_10.3p1, fixture\\n' >&2",
    );
    let context = context(root.path());
    let mut environment = host::capture(&context, vec![root.path().into()], 0);
    assert!(
        metadata::capture_capabilities(&context, &mut environment, &|| false)
            .recorded
            .contains("ssh")
    );
    let target = TargetInventory::capture("SSH target", &context, &environment);
    let sites = [
        CommandSite {
            arguments: vec!["--typo".into(), "some-host".into()],
            ..CommandSite::literal("ssh")
        },
        CommandSite {
            arguments: vec!["some-host".into(), "--remote-argument".into()],
            ..CommandSite::literal("ssh")
        },
        CommandSite {
            arguments: vec![
                "-p22".into(),
                "-o".into(),
                "ProxyCommand=arbitrary-editor-text".into(),
            ],
            ..CommandSite::literal("ssh")
        },
    ];
    let comparison = compare_targets(&[target], &sites);
    assert!(matches!(
        comparison.commands[0].validation[0],
        ValidationResult::Invalid(_)
    ));
    assert!(matches!(
        comparison.commands[1].validation[0],
        ValidationResult::Unknown(_)
    ));
    assert!(matches!(
        comparison.commands[2].validation[0],
        ValidationResult::Valid
    ));
}

#[test]
fn apple_ls_embedded_version_records_bsd_capabilities_without_running_the_binary() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    executable(
        &root.path().join("ls"),
        &format!(
            "# @(#)PROGRAM:ls  PROJECT:file_cmds-479\nprintf x > '{}'",
            marker.display()
        ),
    );
    let context = context(root.path());
    let mut environment = host::capture(&context, vec![root.path().into()], 0);
    environment.platform = "macos".into();
    assert!(
        metadata::capture_capabilities(&context, &mut environment, &|| false)
            .recorded
            .contains("ls")
    );
    assert!(!marker.exists());
    let target = TargetInventory::capture("BSD target", &context, &environment);
    std::fs::remove_file(root.path().join("ls")).unwrap();
    let comparison = compare_targets(
        &[target],
        &[
            CommandSite {
                arguments: vec!["--time-style=long-iso".into()],
                ..CommandSite::literal("ls")
            },
            CommandSite {
                arguments: vec!["-@e".into()],
                ..CommandSite::literal("ls")
            },
        ],
    );
    assert!(matches!(
        comparison.commands[0].validation[0],
        ValidationResult::Invalid(_)
    ));
    assert!(matches!(
        comparison.commands[1].validation[0],
        ValidationResult::Valid
    ));
    assert!(known_tool_grammar("apple-ls", "487.0.1").is_none());
}
