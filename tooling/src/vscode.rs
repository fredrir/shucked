use anyhow::{Result, bail};

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
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

    // Build release binaries for shucked and shucked-server
    print_step("Building shucked and shucked-server release binaries...");
    let build_opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };
    run_command(
        "cargo",
        &[
            "build",
            "--release",
            "-p",
            "shucked-cli",
            "-p",
            "shucked-server",
        ],
        &build_opts,
    )?;

    // Ensure bin directory in editors/vscode and copy the binaries
    let bin_dir = vscode_dir.join("bin");
    std::fs::create_dir_all(&bin_dir)?;

    let (cli_bin, server_bin) = binary_names();

    let target_dir = repo_root.join("target/release");
    let cli_src = target_dir.join(cli_bin);
    let server_src = target_dir.join(server_bin);

    if !cli_src.is_file() {
        bail!("Failed to find built CLI binary at {}", cli_src.display());
    }
    if !server_src.is_file() {
        bail!(
            "Failed to find built server binary at {}",
            server_src.display()
        );
    }

    let cli_dst = bin_dir.join(cli_bin);
    let server_dst = bin_dir.join(server_bin);

    print_step(&format!("Bundling binary: {cli_bin}"));
    std::fs::copy(&cli_src, &cli_dst)?;
    print_step(&format!("Bundling binary: {server_bin}"));
    std::fs::copy(&server_src, &server_dst)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&cli_dst, perms.clone())?;
        std::fs::set_permissions(&server_dst, perms)?;
    }

    let runner = detect_node_runner();
    print_step(&format!("Packaging extension using {runner}..."));
    let opts = RunOptions {
        cwd: Some(&vscode_dir),
        ..Default::default()
    };
    run_command(runner, &["run", "package"], &opts)?;

    print_step("Creating platform VSIX artifact...");
    run_command(runner, &["run", "vsix"], &opts)?;

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
