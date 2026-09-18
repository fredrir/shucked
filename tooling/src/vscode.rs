use anyhow::{Result, bail};

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

fn detect_node_runner() -> &'static str {
    if is_tool_available("bun") {
        "bun"
    } else if is_tool_available("pnpm") {
        "pnpm"
    } else if is_tool_available("npm") {
        "npm"
    } else {
        "bun"
    }
}

pub fn run_vscode_compile() -> Result<()> {
    let repo_root = find_repo_root()?;
    let vscode_dir = repo_root.join("editors/vscode");
    print_section("VS Code Extension: Compile");

    if !vscode_dir.is_dir() {
        bail!(
            "VS Code extension directory not found: {}",
            vscode_dir.display()
        );
    }

    let runner = detect_node_runner();
    print_step(&format!("Running compile using {runner}..."));
    let opts = RunOptions {
        cwd: Some(&vscode_dir),
        ..Default::default()
    };
    run_command(runner, &["run", "compile"], &opts)?;

    print_success("VS Code extension compiled successfully.");
    Ok(())
}

pub fn run_vscode_package() -> Result<()> {
    let repo_root = find_repo_root()?;
    let vscode_dir = repo_root.join("editors/vscode");
    print_section("VS Code Extension: Package");

    if !vscode_dir.is_dir() {
        bail!(
            "VS Code extension directory not found: {}",
            vscode_dir.display()
        );
    }

    let runner = detect_node_runner();
    print_step(&format!("Packaging extension using {runner}..."));
    let opts = RunOptions {
        cwd: Some(&vscode_dir),
        ..Default::default()
    };
    run_command(runner, &["run", "package"], &opts)?;

    if is_tool_available("vsce") {
        print_step("Creating .vsix artifact with vsce...");
        run_command("vsce", &["package", "--no-dependencies"], &opts)?;
    } else {
        print_warning("vsce is not installed on PATH, vsix creation skipped.");
    }

    print_success("VS Code extension packaging finished.");
    Ok(())
}

pub fn run_vscode_test() -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("VS Code Extension: Test");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    if is_tool_available("uv") {
        print_step("Running VSIX packaging tests via pytest...");
        let py_args = [
            "run",
            "--project",
            "tests",
            "pytest",
            "tests/packaging/test_vsix.py",
            "-v",
        ];
        run_command("uv", &py_args, &opts)?;
    } else {
        print_warning("uv not installed, skipping python packaging test.");
    }

    print_success("VS Code extension tests passed.");
    Ok(())
}

pub fn run_vscode_lint() -> Result<()> {
    let repo_root = find_repo_root()?;
    let vscode_dir = repo_root.join("editors/vscode");
    print_section("VS Code Extension: Lint");

    let runner = detect_node_runner();
    let opts = RunOptions {
        cwd: Some(&vscode_dir),
        ..Default::default()
    };
    run_command(runner, &["run", "lint"], &opts)?;

    print_success("VS Code extension lint passed.");
    Ok(())
}
