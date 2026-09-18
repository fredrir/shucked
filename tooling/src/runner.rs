use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Result of running a captured command.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    #[allow(dead_code)]
    pub elapsed: Duration,
}

impl CommandOutput {
    #[allow(dead_code)]
    pub fn success(&self) -> bool {
        self.status.success()
    }
}

/// Helper to print a top-level section header.
pub fn print_section(title: &str) {
    println!("\n{}", format!("=== {title} ===").bold().cyan());
}

/// Helper to print an action step.
pub fn print_step(msg: &str) {
    println!("{} {msg}", "▸".cyan().bold());
}

/// Helper to print a success message.
pub fn print_success(msg: &str) {
    println!("{} {msg}", "✔".green().bold());
}

/// Helper to print an error message.
pub fn print_error(msg: &str) {
    eprintln!("{} {msg}", "✘".red().bold());
}

/// Helper to print a warning message.
pub fn print_warning(msg: &str) {
    println!("{} {msg}", "⚠".yellow().bold());
}

/// Locate the workspace repository root directory.
pub fn find_repo_root() -> Result<PathBuf> {
    let mut current = std::env::current_dir().context("Failed to get current working directory")?;
    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.is_file()
            && let Ok(content) = std::fs::read_to_string(&cargo_toml)
            && content.contains("[workspace]")
        {
            return Ok(current);
        }
        if !current.pop() {
            break;
        }
    }
    std::env::current_dir().context("Fallback to current directory")
}

/// Check if a binary tool is available on the system PATH.
pub fn is_tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Options for command execution.
#[derive(Debug, Default)]
pub struct RunOptions<'a> {
    pub cwd: Option<&'a Path>,
    pub envs: HashMap<&'a str, &'a str>,
    pub quiet: bool,
}

/// Execute a command streaming its output directly to stdout/stderr.
pub fn run_command(program: &str, args: &[&str], opts: &RunOptions) -> Result<Duration> {
    let full_cmd = format!("{program} {}", args.join(" "));
    if !opts.quiet {
        println!("{} {full_cmd}", "▸ Executing:".cyan().bold());
    }

    let start = Instant::now();
    let mut cmd = Command::new(program);
    cmd.args(args);

    if let Some(cwd) = opts.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &opts.envs {
        cmd.env(k, v);
    }

    let status = cmd
        .status()
        .with_context(|| format!("Failed to spawn command: {full_cmd}"))?;
    let elapsed = start.elapsed();

    if status.success() {
        if !opts.quiet {
            println!(
                "{} {full_cmd} ({})",
                "✔".green().bold(),
                format!("{:.2?}", elapsed).dimmed()
            );
        }
        Ok(elapsed)
    } else {
        let code = status.code().unwrap_or(-1);
        print_error(&format!("{full_cmd} exited with status {code}"));
        bail!("Command failed: {full_cmd} (exit status {code})");
    }
}

/// Execute a command and capture its stdout and stderr.
pub fn run_command_captured(
    program: &str,
    args: &[&str],
    opts: &RunOptions,
) -> Result<CommandOutput> {
    let start = Instant::now();
    let mut cmd = Command::new(program);
    cmd.args(args);

    if let Some(cwd) = opts.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &opts.envs {
        cmd.env(k, v);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd
        .output()
        .with_context(|| format!("Failed to spawn command: {program} {}", args.join(" ")))?;
    let elapsed = start.elapsed();

    Ok(CommandOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        elapsed,
    })
}
