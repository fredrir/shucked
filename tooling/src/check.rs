use anyhow::{Result, bail};
use colored::Colorize;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::release::{DEFAULT_WORKFLOW_PATH, check_security_content, run_check_config};
use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, run_command_captured,
};

struct CheckResult {
    name: &'static str,
    success: bool,
    elapsed: Duration,
    output: String,
}

fn check_fmt(repo_root: &Path) -> CheckResult {
    let start = Instant::now();
    let opts = RunOptions {
        cwd: Some(repo_root),
        quiet: true,
        ..Default::default()
    };
    let res = run_command_captured("cargo", &["fmt", "--all", "--", "--check"], &opts);
    let elapsed = start.elapsed();
    match res {
        Ok(out) => CheckResult {
            name: "Formatting (cargo fmt)",
            success: out.status.success(),
            elapsed,
            output: if out.status.success() {
                out.stdout
            } else {
                format!("{}\n{}", out.stdout, out.stderr)
            },
        },
        Err(e) => CheckResult {
            name: "Formatting (cargo fmt)",
            success: false,
            elapsed,
            output: e.to_string(),
        },
    }
}

fn check_clippy(repo_root: &Path) -> CheckResult {
    let start = Instant::now();
    let opts = RunOptions {
        cwd: Some(repo_root),
        quiet: true,
        ..Default::default()
    };
    let res = run_command_captured(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        &opts,
    );
    let elapsed = start.elapsed();
    match res {
        Ok(out) => CheckResult {
            name: "Linter (cargo clippy)",
            success: out.status.success(),
            elapsed,
            output: if out.status.success() {
                out.stdout
            } else {
                format!("{}\n{}", out.stdout, out.stderr)
            },
        },
        Err(e) => CheckResult {
            name: "Linter (cargo clippy)",
            success: false,
            elapsed,
            output: e.to_string(),
        },
    }
}

fn check_dependencies(repo_root: &Path) -> CheckResult {
    let start = Instant::now();
    let mut logs = Vec::new();
    let mut all_success = true;

    // 1. Cargo shear check if installed
    if is_tool_available("cargo-shear") {
        let opts = RunOptions {
            cwd: Some(repo_root),
            quiet: true,
            ..Default::default()
        };
        match run_command_captured("cargo", &["shear"], &opts) {
            Ok(out) => {
                if !out.status.success() {
                    all_success = false;
                    logs.push(format!(
                        "cargo shear failed:\n{}\n{}",
                        out.stdout, out.stderr
                    ));
                } else {
                    logs.push("cargo shear: OK".to_string());
                }
            }
            Err(e) => {
                all_success = false;
                logs.push(format!("cargo shear execution error: {e}"));
            }
        }
    } else {
        logs.push("cargo shear: skipped (not installed)".to_string());
    }

    // 2. Release-please configuration check
    match run_check_config() {
        Ok(()) => {
            logs.push("release-please configuration: OK".to_string());
        }
        Err(e) => {
            all_success = false;
            logs.push(format!("release-please configuration check failed: {e}"));
        }
    }

    let elapsed = start.elapsed();
    CheckResult {
        name: "Dependencies & Packaging Config",
        success: all_success,
        elapsed,
        output: logs.join("\n"),
    }
}

fn check_security(repo_root: &Path) -> CheckResult {
    let start = Instant::now();
    let workflow_path = repo_root.join(DEFAULT_WORKFLOW_PATH);
    if !workflow_path.exists() {
        return CheckResult {
            name: "Workflow Security Hardening",
            success: true,
            elapsed: start.elapsed(),
            output: "Workflow file not found, skipped.".to_string(),
        };
    }

    match std::fs::read_to_string(&workflow_path) {
        Ok(content) => {
            let issues = check_security_content(&content);
            let elapsed = start.elapsed();
            if issues.is_empty() {
                CheckResult {
                    name: "Workflow Security Hardening",
                    success: true,
                    elapsed,
                    output: "All security checks passed.".to_string(),
                }
            } else {
                let mut out = format!("Found {} security issues:\n", issues.len());
                for iss in issues {
                    out.push_str(&format!("  - {iss}\n"));
                }
                CheckResult {
                    name: "Workflow Security Hardening",
                    success: false,
                    elapsed,
                    output: out,
                }
            }
        }
        Err(e) => CheckResult {
            name: "Workflow Security Hardening",
            success: false,
            elapsed: start.elapsed(),
            output: e.to_string(),
        },
    }
}

/// Run fast pre-push sanity checks in parallel using Rayon.
pub fn run_check() -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Pre-Push Sanity Check (Parallel Execution)");
    let overall_start = Instant::now();

    println!(
        "{} Dispatching 4 sanity checks in parallel...",
        "▸".cyan().bold()
    );

    let (r1, (r2, (r3, r4))) = rayon::join(
        || check_fmt(&repo_root),
        || {
            rayon::join(
                || check_clippy(&repo_root),
                || {
                    rayon::join(
                        || check_dependencies(&repo_root),
                        || check_security(&repo_root),
                    )
                },
            )
        },
    );

    let results = vec![r1, r2, r3, r4];
    let overall_elapsed = overall_start.elapsed();

    println!("\n{}", "Results:".bold());
    println!("{:-<70}", "");

    let mut any_failed = false;
    for res in &results {
        let status_badge = if res.success {
            "✔ PASSED".green().bold()
        } else {
            any_failed = true;
            "✘ FAILED".red().bold()
        };

        println!(
            " {:<38} {:>12}  {}",
            res.name.bold(),
            status_badge,
            format!("{:.2?}", res.elapsed).dimmed()
        );
    }
    println!("{:-<70}", "");
    println!(
        "Total wall clock time: {}",
        format!("{:.2?}", overall_elapsed).cyan().bold()
    );

    if any_failed {
        println!("\n{}", "Failure Details:".red().bold());
        for res in &results {
            if !res.success {
                println!("\n{} {}:", "✘".red().bold(), res.name.bold());
                println!("{}", res.output.trim());
            }
        }
        bail!("Pre-push sanity checks failed!");
    } else {
        println!(
            "\n{}",
            "✔ All pre-push checks passed successfully!".green().bold()
        );
    }

    Ok(())
}
