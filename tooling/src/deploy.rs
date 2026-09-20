use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::runner::{
    RunOptions, find_repo_root, print_section, print_step, print_success, print_warning,
    run_command, run_command_captured,
};
use crate::vscode::{binary_names, run_vscode_package};

const EXTENSION_ID: &str = "fredrir.shucked";

/// Build release binaries and install them into the local environment.
pub fn run_deploy(vscode: bool, shucked: bool) -> Result<()> {
    let (deploy_vscode, deploy_shucked) = match (vscode, shucked) {
        (false, false) => (true, true),
        selected => selected,
    };

    let repo_root = find_repo_root()?;
    print_section("Deploy");
    build_release(&repo_root)?;

    if deploy_shucked {
        install_binaries(&repo_root)?;
    }
    if deploy_vscode {
        deploy_extension(&repo_root)?;
    }

    print_success("Deploy finished.");
    Ok(())
}

fn build_release(repo_root: &Path) -> Result<()> {
    print_step("Building release binaries...");
    let opts = RunOptions {
        cwd: Some(repo_root),
        ..Default::default()
    };
    run_command(
        "cargo",
        &[
            "build",
            "--release",
            "--locked",
            "-p",
            "shucked-cli",
            "-p",
            "shucked-server",
        ],
        &opts,
    )?;
    Ok(())
}

fn install_binaries(repo_root: &Path) -> Result<()> {
    print_section("Deploy: Shucked Binaries");

    let install_dir = local_bin_dir()?;
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create {}", install_dir.display()))?;

    let release_dir = repo_root.join("target/release");
    let (cli_bin, server_bin) = binary_names();

    for binary in [cli_bin, server_bin] {
        let src = release_dir.join(binary);
        if !src.is_file() {
            bail!("Failed to find built binary at {}", src.display());
        }

        let dst = install_dir.join(binary);
        // Replacing the file avoids ETXTBSY when the old binary is running
        let _ = std::fs::remove_file(&dst);
        std::fs::copy(&src, &dst)
            .with_context(|| format!("Failed to install {}", dst.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))?;
        }

        print_step(&format!("Installed {}", dst.display()));
    }

    if !is_on_path(&install_dir) {
        print_warning(&format!(
            "{} is not on PATH; add it to use the installed binaries.",
            install_dir.display()
        ));
    }

    print_success("Shucked binaries installed.");
    Ok(())
}

fn deploy_extension(repo_root: &Path) -> Result<()> {
    let code = code_command();
    if run_command_captured(&code, &["--version"], &RunOptions::default()).is_err() {
        bail!("VS Code CLI `{code}` not found; set SHUCKED_CODE_COMMAND to override.");
    }

    run_vscode_package()?;

    let vscode_dir = repo_root.join("editors/vscode");
    let vsix = locate_vsix(&vscode_dir)?;

    print_section("Deploy: VS Code Extension");
    print_step(&format!("Uninstalling {EXTENSION_ID}..."));
    let uninstall = run_command_captured(
        &code,
        &["--uninstall-extension", EXTENSION_ID],
        &RunOptions::default(),
    )?;
    if !uninstall.status.success() {
        print_warning(&format!("No installed {EXTENSION_ID} to uninstall."));
    }

    print_step(&format!("Installing {}...", vsix.display()));
    let vsix_path = vsix.to_string_lossy().to_string();
    run_command(
        &code,
        &["--install-extension", &vsix_path, "--force"],
        &RunOptions::default(),
    )?;

    print_success("VS Code extension installed. Reload VS Code to pick it up.");
    Ok(())
}

/// Pick the VSIX matching the extension manifest version, newest first.
fn locate_vsix(vscode_dir: &Path) -> Result<PathBuf> {
    let manifest = vscode_dir.join("package.json");
    let contents = std::fs::read_to_string(&manifest)
        .with_context(|| format!("Failed to read {}", manifest.display()))?;
    let parsed: serde_json::Value = serde_json::from_str(&contents)
        .with_context(|| format!("Invalid {}", manifest.display()))?;
    let version = parsed["version"]
        .as_str()
        .context("Missing version in VS Code extension package.json")?;

    let suffix = format!("-{version}.vsix");
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(vscode_dir)
        .with_context(|| format!("Failed to read {}", vscode_dir.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(&suffix))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();

    candidates.sort_by_key(|(modified, _)| *modified);
    match candidates.pop() {
        Some((_, path)) => Ok(path),
        None => bail!(
            "No VSIX for version {version} found in {}",
            vscode_dir.display()
        ),
    }
}

fn code_command() -> String {
    std::env::var("SHUCKED_CODE_COMMAND").unwrap_or_else(|_| "code".to_string())
}

fn local_bin_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .context("Could not resolve the home directory")?;
    Ok(PathBuf::from(home).join(".local/bin"))
}

fn is_on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|entry| entry == dir))
        .unwrap_or(false)
}
