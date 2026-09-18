use anyhow::Result;

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    run_command,
};

/// Format workspace files.
pub fn run_format(check: bool, vscode: bool) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Code Formatting");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    let mut args = vec!["fmt", "--all"];
    if check {
        args.extend(&["--", "--check"]);
        print_step("Checking Rust formatting with cargo fmt...");
    } else {
        print_step("Formatting Rust code with cargo fmt...");
    }

    run_command("cargo", &args, &opts)?;

    if vscode {
        let vscode_dir = repo_root.join("editors/vscode");
        if vscode_dir.is_dir() {
            print_step("Formatting VS Code extension files...");
            let vscode_opts = RunOptions {
                cwd: Some(&vscode_dir),
                ..Default::default()
            };
            if is_tool_available("bun") {
                let script = if check { "lint" } else { "lint:fix" };
                run_command("bun", &["run", script], &vscode_opts)?;
            } else if is_tool_available("npm") {
                let script = if check { "lint" } else { "lint:fix" };
                run_command("npm", &["run", script], &vscode_opts)?;
            }
        }
    }

    if check {
        print_success("Formatting check passed.");
    } else {
        print_success("Code formatting complete.");
    }

    Ok(())
}
