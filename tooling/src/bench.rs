use anyhow::{Context, Result, bail};
use colored::Colorize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::runner::{
    RunOptions, find_repo_root, print_error, print_section, print_step, print_success, run_command,
};

/// Available Criterion benchmark suites.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum BenchTarget {
    Parser,
    Arithmetic,
    Lexer,
    Semantic,
    Linter,
    Formatter,
    Lsp,
    LargeCorpusHotspots,
    All,
}

impl BenchTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Parser => "parser",
            Self::Arithmetic => "arithmetic",
            Self::Lexer => "lexer",
            Self::Semantic => "semantic",
            Self::Linter => "linter",
            Self::Formatter => "formatter",
            Self::Lsp => "lsp",
            Self::LargeCorpusHotspots => "large_corpus_hotspots",
            Self::All => "all",
        }
    }
}

/// Available memory benchmark suites.
#[derive(Debug, Clone, Copy, clap::ValueEnum, Default)]
pub enum MemoryTarget {
    #[default]
    All,
    Parser,
    Linter,
    Semantic,
}

impl MemoryTarget {
    pub fn examples(&self) -> Vec<&'static str> {
        match self {
            Self::All => vec!["parser_memory", "linter_memory", "semantic_memory"],
            Self::Parser => vec!["parser_memory"],
            Self::Linter => vec!["linter_memory"],
            Self::Semantic => vec!["semantic_memory"],
        }
    }
}

/// Run Criterion benchmarks.
pub fn run_bench(
    target: Option<BenchTarget>,
    save_baseline: Option<&str>,
    baseline: Option<&str>,
    filter: Option<&str>,
    extra_args: &[String],
) -> Result<()> {
    let repo_root = find_repo_root()?;
    let selected_target = target.unwrap_or(BenchTarget::All);

    print_section(&format!("Benchmark Runner: {}", selected_target.as_str()));

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    let mut args = vec!["bench", "-p", "shucked-benchmark"];

    match selected_target {
        BenchTarget::All => {}
        BenchTarget::LargeCorpusHotspots => {
            args.extend(&[
                "--features",
                "large-corpus-hotspots",
                "--bench",
                "large_corpus_hotspots",
            ]);
        }
        other => {
            args.extend(&["--bench", other.as_str()]);
        }
    }

    for extra in extra_args {
        args.push(extra);
    }

    let mut criterion_flags = Vec::new();
    if let Some(f) = filter {
        criterion_flags.push(f);
    }
    if let Some(sb) = save_baseline {
        criterion_flags.extend(&["--save-baseline", sb]);
    }
    if let Some(b) = baseline {
        criterion_flags.extend(&["--baseline", b]);
    }

    if !criterion_flags.is_empty() {
        args.push("--");
        args.extend(criterion_flags);
    }

    print_step(&format!("Running cargo {}", args.join(" ")));
    run_command("cargo", &args, &opts)?;

    print_success("Benchmark run completed.");
    Ok(())
}

fn format_metric_change(current: f64, baseline: f64) -> String {
    if baseline == 0.0 {
        return if current == 0.0 { "n/a".to_string() } else { "+inf".to_string() };
    }
    let change = ((current / baseline) - 1.0) * 100.0;
    let sign = if change > 0.0 { "+" } else { "" };
    let text = format!("{sign}{change:.2}%");
    if change > 5.0 {
        text.red().bold().to_string()
    } else if change < -5.0 {
        text.green().bold().to_string()
    } else {
        text.dimmed().to_string()
    }
}

fn compare_metric_map(
    current: &serde_json::Map<String, Value>,
    baseline: &serde_json::Map<String, Value>,
    indent: &str,
) {
    let metrics = [
        "total_allocated_bytes",
        "total_reallocated_bytes",
        "allocation_count",
        "reallocation_count",
        "peak_live_bytes",
        "final_live_bytes",
    ];

    for metric in metrics {
        let cur_val = current.get(metric).and_then(Value::as_f64).unwrap_or(0.0);
        let base_val = baseline.get(metric).and_then(Value::as_f64).unwrap_or(0.0);
        let change_str = format_metric_change(cur_val, base_val);
        println!("{indent}{metric}: {base_val:.0} -> {cur_val:.0} ({change_str})");
    }
}

/// Run memory profiling benchmarks and optionally compare or save baselines.
pub fn run_bench_memory(
    target: MemoryTarget,
    save_baseline: Option<&str>,
    baseline: Option<&str>,
    release: bool,
    case_filter: Option<&str>,
) -> Result<()> {
    let repo_root = find_repo_root()?;
    let target_dir = repo_root.join("target");

    print_section("Memory Benchmarks");

    for example in target.examples() {
        print_step(&format!("Running memory benchmark example: {example}"));
        let mut cmd = Command::new("cargo");
        cmd.args(["run", "-p", "shucked-benchmark", "--example", example]);
        if release {
            cmd.arg("--release");
        }
        cmd.arg("--quiet");
        cmd.current_dir(&repo_root);

        if let Some(case) = case_filter {
            cmd.args(["--", case]);
        }

        let output = cmd.output().with_context(|| format!("Failed to run example {example}"))?;
        if !output.status.success() {
            print_error(&format!("Example {example} failed:"));
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            bail!("Memory benchmark execution failed");
        }

        let current_json: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("Failed to parse JSON output from {example}"))?;

        if let Some(sb) = save_baseline {
            let baseline_dir = target_dir.join(format!("{example}-baselines"));
            fs::create_dir_all(&baseline_dir)?;
            let baseline_file = baseline_dir.join(format!("{sb}.json"));
            fs::write(
                &baseline_file,
                serde_json::to_string_pretty(&current_json)? + "\n",
            )?;
            print_success(&format!(
                "Saved {example} baseline `{sb}` to {}",
                baseline_file.display()
            ));
            continue;
        }

        if let Some(b) = baseline {
            let baseline_file = target_dir.join(format!("{example}-baselines")).join(format!("{b}.json"));
            if !baseline_file.is_file() {
                bail!("Missing memory baseline file: {}", baseline_file.display());
            }
            let baseline_content = fs::read_to_string(&baseline_file)?;
            let baseline_json: Value = serde_json::from_str(&baseline_content)?;

            println!("\n{}", format!("=== Comparison against baseline `{b}` for {example} ===").bold());

            let current_cases = current_json.as_array().cloned().unwrap_or_default();
            let baseline_cases = baseline_json.as_array().cloned().unwrap_or_default();

            for cur_case in &current_cases {
                let case_name = cur_case["case"].as_str().unwrap_or("unknown");
                if let Some(base_case) = baseline_cases.iter().find(|c| c["case"].as_str() == Some(case_name)) {
                    println!("\n{} {case_name}:", "●".cyan());
                    // Check if nested groups (facts_metrics / check_metrics) or flat metrics
                    if let Some(groups) = cur_case.as_object() {
                        for (key, val) in groups {
                            if key.ends_with("_metrics") && val.is_object() {
                                println!("  {key}:");
                                if let (Some(cur_m), Some(base_m)) = (val.as_object(), base_case[key].as_object()) {
                                    compare_metric_map(cur_m, base_m, "    ");
                                }
                            }
                        }
                        if cur_case.get("metrics").is_some()
                            && let (Some(cur_m), Some(base_m)) = (cur_case["metrics"].as_object(), base_case["metrics"].as_object())
                        {
                            compare_metric_map(cur_m, base_m, "  ");
                        }
                    }
                } else {
                    println!("  {} {case_name} (new case, not in baseline)", "•".yellow());
                }
            }
            continue;
        }

        // Default display if neither saving nor comparing baseline
        if let Some(cases) = current_json.as_array() {
            println!("\n{}", format!("--- Results for {example} ---").bold());
            for c in cases {
                let name = c["case"].as_str().unwrap_or("unknown");
                let peak = c["metrics"]["peak_live_bytes"]
                    .as_u64()
                    .or_else(|| c["check_metrics"]["peak_live_bytes"].as_u64())
                    .unwrap_or(0);
                let total = c["metrics"]["total_allocated_bytes"]
                    .as_u64()
                    .or_else(|| c["check_metrics"]["total_allocated_bytes"].as_u64())
                    .unwrap_or(0);
                println!(
                    "  {:<35} peak live: {:>10} bytes | total allocated: {:>12} bytes",
                    name.cyan(),
                    peak.to_string().yellow(),
                    total.to_string().dimmed()
                );
            }
        }
    }

    print_success("Memory benchmark suite finished.");
    Ok(())
}
