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
    std::fs::write(&source, "eza --icnos\n").unwrap();
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
}
