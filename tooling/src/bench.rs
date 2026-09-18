use anyhow::Result;

use crate::runner::{
    RunOptions, find_repo_root, print_section, print_step, print_success, run_command,
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
