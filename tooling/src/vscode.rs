use std::path::Path;

use anyhow::{Result, bail};

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    run_command,
};

/// Platform-specific names of the built Shucked binaries.
pub fn binary_names() -> (&'static str, &'static str) {
    if cfg!(target_os = "windows") {
        ("shucked.exe", "shucked-server.exe")
    } else {
        ("shucked", "shucked-server")
    }
}

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
    // vsce runs vscode:prepublish, which builds and bundles the binaries and extension.
    print_step("Creating platform VSIX artifact...");
    run_command(runner, &["run", "vsix"], &opts)?;

    print_success("VS Code extension packaging finished.");
    Ok(())
}

/// Selection for `just vscode test`.
pub struct TestOptions<'a> {
    pub e2e: bool,
    pub vsix: Option<&'a Path>,
    pub build_vsix: bool,
    pub pytest_args: &'a [String],
}

pub fn run_vscode_test(options: &TestOptions) -> Result<()> {
    let repo_root = find_repo_root()?;
    let vscode_dir = repo_root.join("editors/vscode");
    print_section("VS Code Extension: Test");

    // `just` runs from the repository root, so relative paths resolve there.
    let vsix = options.vsix.map(std::path::absolute).transpose()?;
    if let Some(path) = &vsix
        && !path.is_file()
    {
        bail!(
            "VSIX not found: {} (relative paths resolve from the repository root; pass an absolute path)",
            path.display()
        );
    }

    if !vscode_dir.join("node_modules").is_dir() {
        bail!(
            "Extension dependencies are missing; run `bun install` in {}",
            vscode_dir.display()
        );
    }
    let runner = detect_node_runner();
    print_step(&format!("Running extension unit tests using {runner}..."));
    run_command(
        runner,
        &["run", "test"],
        &RunOptions {
            cwd: Some(&vscode_dir),
            ..Default::default()
        },
    )?;

    if !is_tool_available("uv") {
        bail!("uv is required for the extension's pytest suites (https://docs.astral.sh/uv/)");
    }
    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };
    if options.e2e && options.vsix.is_none() {
        // Development-mode editor tests run the debug language server.
        print_step("Building the debug language server...");
        run_command(
            "cargo",
            &["build", "-p", "shucked-cli", "-p", "shucked-server"],
            &opts,
        )?;
    }

    let vsix = vsix.map(|path| path.display().to_string());
    let mut args = vec![
        "run",
        "--project",
        "tests",
        "pytest",
        "tests/editors/vscode",
    ];
    if options.e2e {
        args.push("--e2e");
    }
    if let Some(vsix) = vsix.as_deref() {
        args.extend(["--vsix", vsix]);
    }
    if options.build_vsix {
        args.push("--build-vsix");
    }
    args.extend(options.pytest_args.iter().map(String::as_str));
    print_step("Running extension pytest suites...");
    run_command("uv", &args, &opts)?;

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
