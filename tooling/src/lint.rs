use anyhow::Result;
use std::time::Instant;

use crate::release::run_check_security;
use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

/// Run workspace linting suite.
pub fn run_lint(
    fix: bool,
    all_features: bool,
    skip_shear: bool,
    skip_security: bool,
) -> Result<()> {
    let repo_root = find_repo_root()?;
    let start_total = Instant::now();
    print_section("Linter & Code Health Checks");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    // 1. Formatting check
    print_step("Checking code formatting (cargo fmt)...");
    let mut fmt_args = vec!["fmt", "--all"];
    if !fix {
        fmt_args.extend(&["--", "--check"]);
    }
    run_command("cargo", &fmt_args, &opts)?;

    // 2. Clippy check
    print_step("Running static analysis (cargo clippy)...");
    let mut clippy_args = vec!["clippy", "--workspace", "--all-targets"];
    if all_features {
        clippy_args.push("--all-features");
    }
    if fix {
        clippy_args.extend(&["--fix", "--allow-dirty", "--allow-staged"]);
    }
    clippy_args.extend(&["--", "-D", "warnings"]);
    run_command("cargo", &clippy_args, &opts)?;

    // 3. Cargo shear (unused dependencies)
    if !skip_shear {
        if is_tool_available("cargo-shear") {
            print_step("Checking for unused dependencies (cargo shear)...");
            run_command("cargo", &["shear"], &opts)?;
        } else {
            print_warning("cargo-shear is not installed, skipping unused dependency check.");
        }
    }

    // 4. Release workflow security audit
    if !skip_security {
        print_step("Verifying release workflow security hardening...");
        run_check_security(fix, None)?;
    }

    // 5. Scripts check (dogfood shucked on repo's scripts)
    print_step("Dogfooding shucked on repo scripts...");
    let scripts_check_res = run_command(
        "cargo",
        &[
            "run",
            "-q",
            "-p",
            "shucked-cli",
            "--",
            "check",
            "--no-cache",
            "scripts",
        ],
        &opts,
    );
    if let Err(e) = scripts_check_res {
        print_warning(&format!("Scripts check noted findings: {e}"));
    }

    print_success(&format!(
        "All linter checks completed successfully in {:.2?}.",
        start_total.elapsed()
    ));
    Ok(())
}
