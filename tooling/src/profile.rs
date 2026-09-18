use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProfileTarget {
    Parser,
    Arithmetic,
    Formatter,
    Linter,
    Cli,
    LargeCorpus,
}

impl ProfileTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parser => "parser",
            Self::Arithmetic => "arithmetic",
            Self::Formatter => "formatter",
            Self::Linter => "linter",
            Self::Cli => "cli",
            Self::LargeCorpus => "large-corpus",
        }
    }
}

fn find_bench_binary(repo_root: &Path, bench_name: &str) -> Result<PathBuf> {
    let deps_dir = repo_root.join("target/profiling/deps");
    if !deps_dir.is_dir() {
        bail!("Directory {:?} does not exist", deps_dir);
    }

    let prefix = format!("{bench_name}-");
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in fs::read_dir(&deps_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(file_name) = path.file_name().and_then(|n| n.to_str())
            && file_name.starts_with(&prefix)
            && !file_name.ends_with(".d")
        {
            let mtime = entry.metadata()?.modified()?;
            candidates.push((path, mtime));
        }
    }

    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    candidates
        .into_iter()
        .next()
        .map(|(p, _)| p)
        .context(format!(
            "Could not locate compiled benchmark binary for {bench_name} in {:?}",
            deps_dir
        ))
}

pub fn run_profile(
    target: ProfileTarget,
    case: Option<&str>,
    file: Option<&str>,
    output_dir: Option<&str>,
    rate: u32,
    iterations: u32,
    view: bool,
) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section(&format!("Profiling Runner: {}", target.as_str()));

    if !is_tool_available("samply") {
        print_warning("samply is not installed. Install with `cargo install --locked samply`.");
    }

    let out_dir = output_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join(".cache/profiles").join(target.as_str()));
    fs::create_dir_all(&out_dir)?;

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    match target {
        ProfileTarget::Cli => {
            let target_case = case.unwrap_or("nvm");
            let target_file = file.map(PathBuf::from).unwrap_or_else(|| {
                repo_root.join(format!(
                    "crates/shucked-benchmark/resources/files/{target_case}.sh"
                ))
            });

            if !target_file.exists() {
                bail!("Target file does not exist: {}", target_file.display());
            }

            print_step("Building shucked CLI with profiling profile...");
            run_command(
                "cargo",
                &["build", "--profile", "profiling", "-p", "shucked-cli"],
                &opts,
            )?;

            let binary = if repo_root.join("target/profiling/shuck").exists() {
                repo_root.join("target/profiling/shuck")
            } else {
                repo_root.join("target/profiling/shucked")
            };

            let output_file = out_dir.join(format!("{target_case}.json.gz"));
            let rate_str = rate.to_string();
            let iter_str = iterations.to_string();
            let profile_name = format!("cli/{target_case}");

            print_step(&format!(
                "Recording profile with samply to {}...",
                output_file.display()
            ));
            let samply_args = [
                "record",
                "--save-only",
                "--output",
                output_file.to_str().unwrap(),
                "--rate",
                &rate_str,
                "--iteration-count",
                &iter_str,
                "--profile-name",
                &profile_name,
                "--",
                binary.to_str().unwrap(),
                "check",
                "--no-cache",
                target_file.to_str().unwrap(),
            ];
            run_command("samply", &samply_args, &opts)?;

            if view || std::env::var("SAMPLY_VIEW").is_ok_and(|v| v == "1") {
                print_step("Opening profile in samply viewer...");
                run_command("samply", &["load", output_file.to_str().unwrap()], &opts)?;
            } else {
                print_success(&format!("Profile saved to: {}", output_file.display()));
                println!(
                    "  Open with: {}",
                    format!("samply load {}", output_file.display()).cyan()
                );
            }
        }
        ProfileTarget::LargeCorpus => {
            let script = repo_root.join("scripts/profiling/profile_large_corpus.sh");
            let case_name = case.unwrap_or("xwmx__nb__nb");
            let rate_str = rate.to_string();
            let iter_str = iterations.to_string();
            let mut envs = std::collections::HashMap::new();
            if view {
                envs.insert("SAMPLY_VIEW", "1");
            }
            let profile_opts = RunOptions {
                cwd: Some(&repo_root),
                envs,
                ..Default::default()
            };
            run_command(
                script.to_str().unwrap(),
                &[
                    case_name,
                    out_dir.to_str().unwrap(),
                    &rate_str,
                    &iter_str,
                    &iter_str,
                ],
                &profile_opts,
            )?;
        }
        bench_target => {
            let bench_name = bench_target.as_str();
            let case_name = case.unwrap_or("nvm");

            print_step(&format!(
                "Building {bench_name} benchmark with profiling profile..."
            ));
            run_command(
                "cargo",
                &[
                    "build",
                    "--profile",
                    "profiling",
                    "-p",
                    "shucked-benchmark",
                    "--bench",
                    bench_name,
                ],
                &opts,
            )?;

            let binary = find_bench_binary(&repo_root, bench_name)?;
            let output_file = out_dir.join(format!("{case_name}.json.gz"));
            let rate_str = rate.to_string();
            let iter_str = iterations.to_string();
            let profile_name = format!("{bench_name}/{case_name}");

            print_step(&format!(
                "Recording {bench_name}/{case_name} profile to {}...",
                output_file.display()
            ));
            let samply_args = [
                "record",
                "--save-only",
                "--output",
                output_file.to_str().unwrap(),
                "--rate",
                &rate_str,
                "--iteration-count",
                &iter_str,
                "--profile-name",
                &profile_name,
                "--",
                binary.to_str().unwrap(),
                "--bench",
                case_name,
                "--noplot",
            ];
            run_command("samply", &samply_args, &opts)?;

            if view || std::env::var("SAMPLY_VIEW").is_ok_and(|v| v == "1") {
                print_step("Opening profile in samply viewer...");
                run_command("samply", &["load", output_file.to_str().unwrap()], &opts)?;
            } else {
                print_success(&format!("Profile saved to: {}", output_file.display()));
                println!(
                    "  Open with: {}",
                    format!("samply load {}", output_file.display()).cyan()
                );
            }
        }
    }

    Ok(())
}
