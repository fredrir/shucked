use anyhow::{Result, bail};
use std::fs;
use std::path::PathBuf;

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum FlameTarget {
    Parser,
    Arithmetic,
    Formatter,
    Linter,
    Cli,
}

impl FlameTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parser => "parser",
            Self::Arithmetic => "arithmetic",
            Self::Formatter => "formatter",
            Self::Linter => "linter",
            Self::Cli => "cli",
        }
    }
}

pub fn run_flame(
    target: FlameTarget,
    case: Option<&str>,
    file: Option<&str>,
    output: Option<&str>,
    open: bool,
) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section(&format!("Flamegraph Generator: {}", target.as_str()));

    if !is_tool_available("cargo-flamegraph") && !is_tool_available("flamegraph") {
        print_warning("cargo-flamegraph is not detected. Install with: cargo install flamegraph");
    }

    let profile_dir = repo_root.join(".cache/profiles");
    fs::create_dir_all(&profile_dir)?;

    let case_name = case.unwrap_or("nvm");
    let svg_path = output.map(PathBuf::from).unwrap_or_else(|| {
        if matches!(target, FlameTarget::Cli) {
            profile_dir.join("flame-cli.svg")
        } else {
            profile_dir.join(format!("flame-{}-{}.svg", target.as_str(), case_name))
        }
    });

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    match target {
        FlameTarget::Cli => {
            let target_file = file.map(PathBuf::from).unwrap_or_else(|| {
                repo_root.join(format!(
                    "crates/shucked-benchmark/resources/files/{case_name}.sh"
                ))
            });

            if !target_file.exists() {
                bail!("Target file does not exist: {}", target_file.display());
            }

            print_step(&format!(
                "Generating flamegraph for shucked CLI on {} -> {}...",
                target_file.display(),
                svg_path.display()
            ));

            let flame_args = [
                "flamegraph",
                "--profile",
                "profiling",
                "-p",
                "shucked-cli",
                "-o",
                svg_path.to_str().unwrap(),
                "--",
                "check",
                "--no-cache",
                target_file.to_str().unwrap(),
            ];
            run_command("cargo", &flame_args, &opts)?;
        }
        bench_target => {
            let bench_name = bench_target.as_str();
            print_step(&format!(
                "Generating flamegraph for {bench_name}/{case_name} -> {}...",
                svg_path.display()
            ));

            let flame_args = [
                "flamegraph",
                "--profile",
                "profiling",
                "-p",
                "shucked-benchmark",
                "--bench",
                bench_name,
                "-o",
                svg_path.to_str().unwrap(),
                "--",
                "--bench",
                case_name,
                "--noplot",
            ];
            run_command("cargo", &flame_args, &opts)?;
        }
    }

    print_success(&format!("Flamegraph generated: {}", svg_path.display()));

    if open {
        print_step(&format!(
            "Opening {} in default viewer...",
            svg_path.display()
        ));
        let open_cmd = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "start"
        } else {
            "xdg-open"
        };
        let _ = run_command(open_cmd, &[svg_path.to_str().unwrap()], &opts);
    }

    Ok(())
}
