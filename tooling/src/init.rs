use anyhow::{Context, Result};
use colored::Colorize;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::runner::{
    RunOptions, find_repo_root, print_section, print_step, print_success, print_warning,
    run_command,
};

/// Get the list of installed rustup components.
fn get_installed_rustup_components() -> HashSet<String> {
    let mut installed = HashSet::new();
    if let Ok(output) = Command::new("rustup")
        .args(["component", "list", "--installed"])
        .output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            installed.insert(trimmed.to_string());
            if let Some((name, _)) = trimmed.split_once('-') {
                installed.insert(name.to_string());
            }
        }
    }
    installed
}

/// Check if a specific cargo subtool or binary is installed.
fn is_cargo_tool_installed(binary_name: &str, test_args: &[&str]) -> Option<String> {
    if let Ok(output) = Command::new(binary_name).args(test_args).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let first_line = stdout.lines().next().unwrap_or("").trim();
        return Some(first_line.to_string());
    }

    // Try finding via $CARGO_HOME/bin or ~/.cargo/bin
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        let path = Path::new(&cargo_home).join("bin").join(binary_name);
        if path.is_file()
            && let Ok(output) = Command::new(&path).args(test_args).output()
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let first_line = stdout.lines().next().unwrap_or("").trim();
            return Some(first_line.to_string());
        }
    } else if let Ok(home) = std::env::var("HOME") {
        let path = Path::new(&home).join(".cargo/bin").join(binary_name);
        if path.is_file()
            && let Ok(output) = Command::new(&path).args(test_args).output()
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let first_line = stdout.lines().next().unwrap_or("").trim();
            return Some(first_line.to_string());
        }
    }

    None
}

/// Retrieve tool version string if tool is available on PATH.
fn get_tool_version(tool: &str, args: &[&str]) -> Option<String> {
    if let Ok(output) = Command::new(tool).args(args).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().next().unwrap_or("").trim();
        if !line.is_empty() {
            return Some(line.to_string());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let err_line = stderr.lines().next().unwrap_or("").trim();
        if !err_line.is_empty() {
            return Some(err_line.to_string());
        }
        return Some("available".to_string());
    }
    None
}

/// Run idempotent developer environment setup.
pub fn run_init(skip_cargo_tools: bool) -> Result<()> {
    let total_start = Instant::now();
    let repo_root = find_repo_root()?;

    print_section("Developer Environment Initialization");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    // 1. Rust Toolchain Components
    print_step("Checking Rust toolchain components...");
    let installed_components = get_installed_rustup_components();
    let components = ["rustfmt", "clippy", "llvm-tools-preview"];

    for component in components {
        let comp_start = Instant::now();
        let is_installed = installed_components.iter().any(|c| {
            c == component
                || c.starts_with(&format!("{component}-"))
                || (component == "llvm-tools-preview" && c.starts_with("llvm-tools"))
        });

        if is_installed {
            println!(
                "  {} Rust component {} already installed ({:.2?})",
                "✔".green().bold(),
                component.cyan(),
                comp_start.elapsed()
            );
        } else {
            print_step(&format!("Installing Rust component {component}..."));
            run_command("rustup", &["component", "add", component], &opts)
                .with_context(|| format!("Failed to install rustup component {component}"))?;
            println!(
                "  {} Installed {} in {:.2?}",
                "✔".green().bold(),
                component.cyan(),
                comp_start.elapsed()
            );
        }
    }

    // 2. Cargo Binary Tools
    print_step("Checking Cargo developer binary tools...");
    let cargo_tools: [(&str, &str, &[&str], &str); 3] = [
        ("cargo-fuzz", "cargo-fuzz", &["--version"], "cargo-fuzz"),
        (
            "cargo-flamegraph",
            "flamegraph",
            &["--version"],
            "flamegraph",
        ),
        ("cargo-shear", "cargo-shear", &["--version"], "cargo-shear"),
    ];

    for (tool_name, binary_name, test_args, crate_name) in cargo_tools {
        let tool_start = Instant::now();
        if let Some(version_info) = is_cargo_tool_installed(binary_name, test_args) {
            println!(
                "  {} {tool_name} already installed: {} ({:.2?})",
                "✔".green().bold(),
                version_info.dimmed(),
                tool_start.elapsed()
            );
        } else if skip_cargo_tools {
            print_warning(&format!(
                "{tool_name} is not installed (skipped via --skip-cargo-tools)"
            ));
        } else {
            print_step(&format!("Installing {tool_name} via cargo install..."));
            let install_res = run_command("cargo", &["install", crate_name], &opts);
            if install_res.is_err() && crate_name == "cargo-shear" {
                print_step("Retrying cargo-shear with +nightly for rustc compatibility...");
                let _ = run_command(
                    "cargo",
                    &["+nightly", "install", "cargo-shear", "--locked"],
                    &opts,
                );
            }
            if let Some(version_info) = is_cargo_tool_installed(binary_name, test_args) {
                println!(
                    "  {} Installed {} ({}) in {:.2?}",
                    "✔".green().bold(),
                    tool_name.cyan(),
                    version_info.dimmed(),
                    tool_start.elapsed()
                );
            } else {
                print_warning(&format!(
                    "Could not automatically install {tool_name}. You can install it manually via: cargo install {crate_name}"
                ));
            }
        }
    }

    // 3. Dev Environment Prerequisites
    print_step("Checking development environment prerequisites...");
    let dev_tools: [(&str, &[&str], &str); 3] = [
        (
            "uv",
            &["--version"],
            "curl -LsSf https://astral.sh/uv/install.sh | sh  (or: brew install uv)",
        ),
        (
            "bun",
            &["--version"],
            "curl -fsSL https://bun.sh/install | bash  (or: brew install oven-sh/bun/bun)",
        ),
        (
            "wasm-pack",
            &["--version"],
            "cargo install wasm-pack  (or: brew install wasm-pack)",
        ),
    ];

    for (tool, version_args, install_instr) in dev_tools {
        let check_start = Instant::now();
        if let Some(ver) = get_tool_version(tool, version_args) {
            println!(
                "  {} {tool} found: {} ({:.2?})",
                "✔".green().bold(),
                ver.dimmed(),
                check_start.elapsed()
            );
        } else {
            print_warning(&format!("{tool} is not installed on PATH."));
            println!(
                "     {} Install via: {}",
                "↳".yellow(),
                install_instr.cyan()
            );
        }
    }

    // 4. Git Hooks Configuration
    print_step("Configuring Git hooks...");
    let hooks_start = Instant::now();
    let hooks_dir = repo_root.join(".githooks");
    if !hooks_dir.exists() {
        std::fs::create_dir_all(&hooks_dir)
            .with_context(|| format!("Failed to create directory {}", hooks_dir.display()))?;
    }

    run_command("git", &["config", "core.hooksPath", ".githooks"], &opts)
        .context("Failed to configure git core.hooksPath")?;

    println!(
        "  {} Git hooks configured at {} ({:.2?})",
        "✔".green().bold(),
        ".githooks".cyan(),
        hooks_start.elapsed()
    );

    print_success(&format!(
        "Environment initialization completed in {:.2?}.",
        total_start.elapsed()
    ));

    Ok(())
}
