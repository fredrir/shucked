use anyhow::{Result, bail};

use crate::runner::{
    RunOptions, find_repo_root, print_section, print_step, print_success, run_command,
};

pub fn run_fuzz_init(ci: bool, cmin: bool, large_corpus: bool) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Fuzz Setup & Seeding");

    let script = repo_root.join("scripts/fuzz-init.sh");
    if !script.is_file() {
        bail!("Fuzz init script not found at: {}", script.display());
    }

    let mut args = Vec::new();
    if ci {
        args.push("--ci");
    }
    if cmin {
        args.push("--cmin");
    }
    if large_corpus {
        args.push("--large-corpus");
    }

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    run_command("bash", &[script.to_str().unwrap(), &args.join(" ")], &opts)?;
    print_success("Fuzzing setup complete.");
    Ok(())
}

pub fn run_fuzz_list() -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Fuzz Targets");

    let fuzz_dir = repo_root.join("fuzz");
    if !fuzz_dir.is_dir() {
        bail!("fuzz directory not found at {}", fuzz_dir.display());
    }

    let opts = RunOptions {
        cwd: Some(&fuzz_dir),
        ..Default::default()
    };

    run_command("cargo", &["fuzz", "list"], &opts)?;
    Ok(())
}

pub fn run_fuzz_smoke(sanitizer: Option<&str>) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Fuzz Smoke Test");

    let fuzz_dir = repo_root.join("fuzz");
    let opts = RunOptions {
        cwd: Some(&fuzz_dir),
        ..Default::default()
    };

    let targets = [
        "parser_fuzz",
        "recovered_parser_fuzz",
        "linter_no_panic_fuzz",
        "lsp_document_sync_fuzz",
        "lsp_request_surface_fuzz",
        "lsp_protocol_sequence_fuzz",
    ];

    let default_sanitizer = if cfg!(target_os = "macos") {
        None
    } else {
        Some("none")
    };
    let chosen_sanitizer = sanitizer.or(default_sanitizer);

    for target in targets {
        print_step(&format!("Running smoke test for target: {target}..."));
        let mut args = vec!["+nightly", "fuzz", "run"];
        if let Some(s) = chosen_sanitizer {
            args.extend(&["-s", s]);
        }
        args.extend(&[target, "--", "-runs=1"]);
        run_command("cargo", &args, &opts)?;
    }

    print_success("Fuzz smoke test suite passed.");
    Ok(())
}

pub fn run_fuzz_target(
    target: &str,
    max_total_time: u32,
    sanitizer: Option<&str>,
    extra_args: &[String],
) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section(&format!("Fuzz Runner: {target}"));

    let fuzz_dir = repo_root.join("fuzz");
    let opts = RunOptions {
        cwd: Some(&fuzz_dir),
        ..Default::default()
    };

    let default_sanitizer = if cfg!(target_os = "macos") {
        None
    } else {
        Some("none")
    };
    let chosen_sanitizer = sanitizer.or(default_sanitizer);

    let time_arg = format!("-max_total_time={max_total_time}");
    let mut args = vec!["+nightly", "fuzz", "run"];
    if let Some(s) = chosen_sanitizer {
        args.extend(&["-s", s]);
    }
    args.extend(&[target, "--", &time_arg]);
    for extra in extra_args {
        args.push(extra);
    }

    run_command("cargo", &args, &opts)?;
    print_success(&format!("Fuzz run for {target} finished."));
    Ok(())
}

pub fn run_fuzz_cli(
    dialect: &str,
    profile: &str,
    count: u32,
    seed: u64,
    workers: u32,
) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("CLI Differential Fuzzing");

    print_step("Building shucked CLI...");
    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };
    run_command("cargo", &["build", "-p", "shucked-cli"], &opts)?;

    let bin = if repo_root.join("target/debug/shucked").exists() {
        "target/debug/shucked"
    } else {
        "target/debug/shuck"
    };

    let script = repo_root.join("scripts/fuzz_cli.py");
    let count_str = count.to_string();
    let seed_str = seed.to_string();
    let workers_str = workers.to_string();

    let args = [
        script.to_str().unwrap(),
        "--shucked-bin",
        bin,
        "--dialect",
        dialect,
        "--profile",
        profile,
        "--count",
        &count_str,
        "--seed",
        &seed_str,
        "--workers",
        &workers_str,
    ];

    print_step("Running Python CLI fuzz harness...");
    run_command("python3", &args, &opts)?;
    print_success("CLI fuzzing run complete.");
    Ok(())
}
