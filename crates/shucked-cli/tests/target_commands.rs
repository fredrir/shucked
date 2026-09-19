use std::process::Command;

fn command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_shucked"))
}

#[test]
fn capture_and_compare_are_offline_and_never_execute_target_commands() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let tool = bin.join("deployment-tool");
    std::fs::write(&tool, "#!/bin/sh\nexit 91\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let inventory = root.path().join("target.json");
    let capture = command()
        .env("PATH", &bin)
        .args(["target", "capture", "--label", "deployment", "--output"])
        .arg(&inventory)
        .output()
        .unwrap();
    assert!(
        capture.status.success(),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );
    std::fs::remove_file(tool).unwrap();
    let source = root.path().join("script.sh");
    std::fs::write(&source, "deployment-tool\nmissing-command\n").unwrap();
    let comparison = command()
        .env("PATH", "")
        .args(["target", "compare", "--target"])
        .arg(&inventory)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        comparison.status.success(),
        "{}",
        String::from_utf8_lossy(&comparison.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&comparison.stdout).unwrap();
    #[cfg(unix)]
    assert_eq!(
        report["comparison"]["commands"][0]["results"][0]["state"],
        "resolved"
    );
    assert_eq!(
        report["comparison"]["commands"][1]["results"][0]["state"],
        "missing"
    );
    assert_eq!(report["locations"][1]["line"], 2);
}

#[test]
fn imported_inventory_integrity_is_verified_and_export_wont_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let inventory = root.path().join("target.json");
    assert!(
        command()
            .env("PATH", root.path())
            .args(["target", "capture", "--output"])
            .arg(&inventory)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !command()
            .env("PATH", root.path())
            .args(["target", "capture", "--output"])
            .arg(&inventory)
            .output()
            .unwrap()
            .status
            .success()
    );
    let text = std::fs::read_to_string(&inventory)
        .unwrap()
        .replace("\"label\": \"workspace\"", "\"label\": \"tampered\"");
    std::fs::write(&inventory, text).unwrap();
    let result = command()
        .args(["target", "inspect"])
        .arg(&inventory)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("checksum"));
}

#[test]
fn comparison_recognizes_fish_shebang_and_source_functions() {
    let root = tempfile::tempdir().unwrap();
    let inventory = root.path().join("fish.json");
    assert!(
        command()
            .env("PATH", root.path())
            .args(["target", "capture", "--shell", "fish", "--output"])
            .arg(&inventory)
            .output()
            .unwrap()
            .status
            .success()
    );
    let source = root.path().join("script");
    std::fs::write(
        &source,
        "#!/usr/bin/env fish\nfunction greeting\n printf hello\nend\ngreeting\n",
    )
    .unwrap();
    let output = command()
        .args(["target", "compare", "--target"])
        .arg(&inventory)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let greeting = report["comparison"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|site| site["name"] == "greeting")
        .unwrap();
    assert_eq!(greeting["results"][0]["state"], "resolved");
    assert_eq!(greeting["results"][0]["command"]["kind"], "function");
}

#[test]
#[cfg(unix)]
fn explicit_capability_capture_preserves_validation_after_local_removal() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let tool = root.path().join("eza");
    let marker = root.path().join("queried");
    std::fs::write(&tool, format!("#!/bin/sh\n[ \"$*\" = --version ] || exit 99\nprintf x > '{}'\nprintf 'eza fixture\\nv0.23.5\\n'\n", marker.display())).unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plain = command()
        .env("PATH", root.path())
        .args(["target", "capture"])
        .output()
        .unwrap();
    assert!(plain.status.success());
    assert!(
        !marker.exists(),
        "filesystem-only capture must not execute metadata queries"
    );
    let inventory = root.path().join("target.json");
    let captured = command()
        .env("PATH", root.path())
        .args(["target", "capture", "--capabilities", "--output"])
        .arg(&inventory)
        .output()
        .unwrap();
    assert!(
        captured.status.success(),
        "{}",
        String::from_utf8_lossy(&captured.stderr)
    );
    assert!(marker.exists());
    std::fs::remove_file(tool).unwrap();
    let source = root.path().join("script.sh");
    std::fs::write(&source, "eza --icnos\neza $FLAGS --icnos\n").unwrap();
    let output = command()
        .env("PATH", "")
        .args(["target", "compare", "--target"])
        .arg(inventory)
        .arg(source)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["comparison"]["commands"][0]["validation"][0]["state"],
        "invalid"
    );
    assert_eq!(
        report["comparison"]["commands"][1]["validation"][0]["state"],
        "unknown"
    );
}

#[test]
#[cfg(unix)]
fn docker_and_kubectl_capture_plugins_without_running_them_or_connecting() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    let docker_config = root.path().join("docker");
    let plugins = docker_config.join("cli-plugins");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(
        docker_config.join("config.json"),
        r#"{"auths":{"private":"secret-must-not-export"}}"#,
    )
    .unwrap();
    for (path, body) in [
        (
            bin.join("docker"),
            "[ \"$*\" = --version ] || exit 99\nprintf 'Docker version 28.0.0, build fixture\\n'",
        ),
        (
            bin.join("kubectl"),
            "[ \"$*\" = 'version --client --output=json' ] || exit 99\nprintf '%s' '{\"clientVersion\":{\"gitVersion\":\"v1.34.0\"}}'",
        ),
        (bin.join("kubectl-foo_bar-sub"), "exit 99"),
        (plugins.join("docker-compose"), "exit 99"),
    ] {
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let inventory = root.path().join("target.json");
    let output = command()
        .env("PATH", &bin)
        .env("HOME", root.path())
        .env("DOCKER_CONFIG", &docker_config)
        .env("KUBERC", "off")
        .args(["target", "capture", "--capabilities", "--output"])
        .arg(&inventory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !std::fs::read_to_string(&inventory)
            .unwrap()
            .contains("secret-must-not-export")
    );
    std::fs::remove_dir_all(bin).unwrap();
    std::fs::remove_dir_all(plugins).unwrap();
    let script = root.path().join("script.sh");
    std::fs::write(&script, "docker invented-subcommand\ndocker compose up --plugin-option\ndocker --invented-flag\nkubectl invented-subcommand\nkubectl foo-bar sub --plugin-option\n").unwrap();
    let output = command()
        .env("PATH", "")
        .args(["target", "compare", "--target"])
        .arg(inventory)
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let states: Vec<_> = report["comparison"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["validation"][0]["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        ["invalid", "unknown", "invalid", "invalid", "unknown"]
    );
}

#[test]
#[cfg(unix)]
fn kuberc_preferences_prevent_strict_unrecognized_command_claims() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let tool = root.path().join("kubectl");
    std::fs::write(&tool, "#!/bin/sh\nexit 99\n").unwrap();
    std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir(root.path().join(".kube")).unwrap();
    std::fs::write(root.path().join(".kube/kuberc"), "aliases: []\n").unwrap();
    let output = command()
        .env("PATH", root.path())
        .env("HOME", root.path())
        .env_remove("KUBERC")
        .env_remove("KUBECTL_KUBERC")
        .args(["target", "capture", "--capabilities"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let inventory: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        inventory["inventory"]["snapshot"]["validators"]
            .get("kubectl")
            .is_none()
    );
}
