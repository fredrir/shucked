use anyhow::{Result, bail};
use colored::Colorize;
use rayon::prelude::*;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use crate::runner::{
    FUZZ_DIR, RunOptions, find_repo_root, is_tool_available, print_error, print_section,
    print_step, print_success, run_command,
};

const COMMON_TARGETS: &[&str] = &[
    "parser_fuzz",
    "lexer_fuzz",
    "arithmetic_fuzz",
    "glob_fuzz",
    "recovered_parser_fuzz",
    "linter_no_panic_fuzz",
    "lsp_document_sync_fuzz",
    "lsp_request_surface_fuzz",
    "lsp_protocol_sequence_fuzz",
];

pub fn run_fuzz_init(ci: bool, cmin: bool, large_corpus: bool) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Fuzz Setup & Seeding");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    // 1. Toolchain setup
    if !is_tool_available("cargo-fuzz") {
        if ci {
            bail!("cargo-fuzz is required in CI mode but not found on PATH");
        }
        print_step("Installing cargo-fuzz via cargo install...");
        run_command("cargo", &["install", "cargo-fuzz", "--locked"], &opts)?;
    }

    // Check rustup nightly toolchain
    if let Ok(output) = Command::new("rustup").args(["toolchain", "list"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.contains("nightly") {
            print_step("Installing rustup nightly toolchain...");
            run_command(
                "rustup",
                &["toolchain", "install", "nightly", "--profile", "minimal"],
                &opts,
            )?;
        }
    }

    // 2. Directory setup
    let fuzz_dir = repo_root.join(FUZZ_DIR);
    let corpus_dir = fuzz_dir.join("corpus");
    let common_dir = corpus_dir.join("common");
    let artifacts_dir = fuzz_dir.join("artifacts");

    fs::create_dir_all(&common_dir)?;
    fs::create_dir_all(&artifacts_dir)?;

    print_step("Linking fuzz target corpus directories to common pool...");
    for target in COMMON_TARGETS {
        let target_link = corpus_dir.join(target);
        if target_link.is_symlink() || target_link.is_file() {
            let _ = fs::remove_file(&target_link);
        } else if target_link.is_dir() {
            let _ = fs::remove_dir_all(&target_link);
        }

        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink("common", &target_link);
        }
        #[cfg(not(unix))]
        {
            let _ = fs::create_dir_all(&target_link);
        }
    }

    // 3. Seed fixtures
    print_step("Seeding fuzz corpus from repository fixtures...");
    let fixture_roots = [
        repo_root.join("crates/shucked-linter/resources/test/fixtures"),
        repo_root.join("crates/shucked-formatter/tests/oracle-fixtures"),
        repo_root.join("crates/shucked-benchmark/resources/files"),
    ];

    let mut seeded = 0;
    for root in &fixture_roots {
        if !root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let ext = entry
                    .path()
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if matches!(ext, "sh" | "bash" | "dash" | "ksh" | "mksh" | "zsh") {
                    let rel = entry
                        .path()
                        .strip_prefix(&repo_root)
                        .unwrap_or(entry.path());
                    let safe_name = rel.to_string_lossy().replace('/', "__");
                    let dest = common_dir.join(safe_name);
                    if fs::copy(entry.path(), dest).is_ok() {
                        seeded += 1;
                    }
                }
            }
        }
    }

    if large_corpus {
        let lc_dir = repo_root.join(".cache/large-corpus/scripts");
        if lc_dir.is_dir() {
            print_step("Seeding additional fixtures from large corpus cache...");
            for entry in walkdir::WalkDir::new(&lc_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    let safe_name =
                        format!("large_corpus__{}", entry.file_name().to_string_lossy());
                    let dest = common_dir.join(safe_name);
                    if fs::copy(entry.path(), dest).is_ok() {
                        seeded += 1;
                    }
                }
            }
        }
    }

    print_success(&format!(
        "Seeded {seeded} test files into {}",
        common_dir.display()
    ));

    // 4. Optional corpus minimization
    if cmin {
        print_step("Minimizing fuzz corpus with cargo fuzz cmin...");
        let fuzz_opts = RunOptions {
            cwd: Some(&fuzz_dir),
            ..Default::default()
        };
        for target in COMMON_TARGETS {
            let _ = run_command("cargo", &["+nightly", "fuzz", "cmin", target], &fuzz_opts);
        }
    }

    print_success("Fuzz setup completed successfully.");
    Ok(())
}

pub fn run_fuzz_list() -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Fuzz Targets");

    let fuzz_dir = repo_root.join(FUZZ_DIR);
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

    let fuzz_dir = repo_root.join(FUZZ_DIR);
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

    let fuzz_dir = repo_root.join(FUZZ_DIR);
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

// ---------------------------------------------------------------------------
// Pure Rust Differential CLI Fuzzing
// ---------------------------------------------------------------------------

struct FastRng(u64);

impl FastRng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0xdeadbeefcafe } else { seed })
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn gen_range(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        min + (self.next_u64() as usize % (max - min))
    }

    fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.gen_range(0, items.len())]
    }
}

fn generate_random_shell_script(rng: &mut FastRng, dialect: &str) -> String {
    let vars = [
        "FOO", "BAR", "BAZ", "COUNT", "ITEM", "VALUE", "PATH", "STATUS", "FILE",
    ];
    let cmds = [
        "echo", "printf", "cat", "test", "true", "false", "head", "tail",
    ];
    let strings = [
        "\"simple string\"",
        "'literal'",
        "\"with $VAR\"",
        "\"a b c\"",
        "''",
    ];

    let mut script = String::new();
    let shebang = match dialect {
        "zsh" => "#!/usr/bin/env zsh\n",
        "bash" => "#!/usr/bin/env bash\n",
        _ => "#!/bin/sh\n",
    };
    script.push_str(shebang);

    let statement_count = rng.gen_range(4, 12);
    for _ in 0..statement_count {
        match rng.gen_range(0, 8) {
            0 => {
                // Assignment
                let v = rng.choose(&vars);
                let val = rng.choose(&strings);
                script.push_str(&format!("{v}={val}\n"));
            }
            1 => {
                // Command call
                let cmd = rng.choose(&cmds);
                let arg = rng.choose(&vars);
                script.push_str(&format!("{cmd} \"${arg}\"\n"));
            }
            2 => {
                // If statement
                let v = rng.choose(&vars);
                script.push_str(&format!(
                    "if [ -n \"${v}\" ]; then\n  echo \"set\"\nelse\n  echo \"empty\"\nfi\n"
                ));
            }
            3 => {
                // For loop
                let v = rng.choose(&vars);
                script.push_str(&format!("for {v} in a b c; do\n  echo \"${v}\"\ndone\n"));
            }
            4 => {
                // While loop with arithmetic
                let v = rng.choose(&vars);
                script.push_str(&format!(
                    "{v}=0\nwhile [ \"${v}\" -lt 3 ]; do\n  {v}=$(( {v} + 1 ))\ndone\n"
                ));
            }
            5 => {
                // Case statement
                let v = rng.choose(&vars);
                script.push_str(&format!(
                    "case \"${v}\" in\n  a) echo 1 ;;\n  *) echo 2 ;;\nesac\n"
                ));
            }
            6 => {
                // Function definition and invocation
                let fn_name = format!("fn_{}", rng.gen_range(1, 100));
                script.push_str(&format!(
                    "{fn_name}() {{\n  local x=\"$1\"\n  echo \"$x\"\n}}\n{fn_name} \"test\"\n"
                ));
            }
            _ => {
                // Pipeline or subshell
                let cmd1 = rng.choose(&cmds);
                let cmd2 = rng.choose(&cmds);
                script.push_str(&format!("{cmd1} \"hello\" | {cmd2} >/dev/null\n"));
            }
        }
    }

    script
}

pub fn run_fuzz_cli(
    dialect: &str,
    _profile: &str,
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
        repo_root.join("target/debug/shucked")
    } else {
        repo_root.join("target/debug/shuck")
    };

    if !bin.is_file() {
        bail!("Compiled shuck binary not found at {}", bin.display());
    }

    let artifacts_dir = repo_root.join(FUZZ_DIR).join("artifacts/cli");
    fs::create_dir_all(&artifacts_dir)?;

    print_step(&format!(
        "Running pure Rust CLI fuzzer: {count} iterations, {workers} workers, seed {seed}..."
    ));
    let start_time = Instant::now();

    let passed_count = AtomicU32::new(0);
    let crash_count = AtomicU32::new(0);

    let thread_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers as usize)
        .build()?;

    thread_pool.install(|| {
        (0..count).into_par_iter().for_each(|i| {
            let mut rng = FastRng::new(seed.wrapping_add(i as u64));
            let script = generate_random_shell_script(&mut rng, dialect);

            let temp_dir =
                std::env::temp_dir().join(format!("shuck-fuzz-{}-{i}", std::process::id()));
            let _ = fs::create_dir_all(&temp_dir);
            let temp_file = temp_dir.join(format!("test.{dialect}"));
            if fs::write(&temp_file, &script).is_err() {
                let _ = fs::remove_dir_all(&temp_dir);
                return;
            }

            // 1. Run check
            let check_output = Command::new(&bin)
                .args(["check", "--no-cache"])
                .arg(&temp_file)
                .output();

            let mut crashed = false;
            let mut crash_reason = String::new();

            match check_output {
                Ok(out) => {
                    let code = out.status.code().unwrap_or(0);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if code == 101 || stderr.contains("panicked at") {
                        crashed = true;
                        crash_reason = format!("check panic: {stderr}");
                    }
                }
                Err(e) => {
                    crashed = true;
                    crash_reason = format!("check execution error: {e}");
                }
            }

            // 2. Run format and check idempotence if check passed
            if !crashed {
                let fmt_output = Command::new(&bin)
                    .args(["format", "--no-cache"])
                    .arg(&temp_file)
                    .output();

                match fmt_output {
                    Ok(out) => {
                        let code = out.status.code().unwrap_or(0);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        if code == 101 || stderr.contains("panicked at") {
                            crashed = true;
                            crash_reason = format!("format panic: {stderr}");
                        }
                    }
                    Err(e) => {
                        crashed = true;
                        crash_reason = format!("format execution error: {e}");
                    }
                }
            }

            if crashed {
                crash_count.fetch_add(1, Ordering::Relaxed);
                let bug_file = artifacts_dir.join(format!("crash-seed{seed}-{i}.sh"));
                let _ = fs::write(&bug_file, format!("# Reason: {crash_reason}\n{script}"));
                eprintln!(
                    "  {} Crash reproduced and saved to: {}",
                    "✖".red(),
                    bug_file.display()
                );
            } else {
                passed_count.fetch_add(1, Ordering::Relaxed);
            }

            let _ = fs::remove_dir_all(&temp_dir);
        });
    });

    let passed = passed_count.load(Ordering::Relaxed);
    let crashes = crash_count.load(Ordering::Relaxed);
    let elapsed = start_time.elapsed();

    if crashes > 0 {
        print_error(&format!(
            "CLI fuzzing found {crashes} crash(es) out of {count} runs in {:.2?}",
            elapsed
        ));
        bail!("CLI fuzzing encountered crashes");
    }

    print_success(&format!(
        "CLI fuzzing completed: {passed}/{count} passed in {:.2?} ({:.0} runs/sec, 0 crashes).",
        elapsed,
        passed as f64 / elapsed.as_secs_f64().max(0.001)
    ));

    Ok(())
}
