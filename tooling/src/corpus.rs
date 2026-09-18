use anyhow::{Context, Result, bail};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

const DEFAULT_ARCHIVE_URL: &str =
    "https://github.com/ewhauser/shuck/releases/download/v0.0.0-test-files/shuck-cache-v3.tar.zst";
const DEFAULT_ARCHIVE_SHA256: &str =
    "880356ce713decb75c894a488c7a5f9cbeef2e3f76e43bb04525ebab4211ccd7";

const DEFAULT_ZSH_ARCHIVE_URL: &str = "https://github.com/ewhauser/shuck/releases/download/v0.0.0-test-files/shuck-zsh-diagnostic-corpus-v1.tar.zst";
const DEFAULT_ZSH_ARCHIVE_SHA256: &str =
    "6fefa7c1aea0aba37e0111a73d2dea2e0ba08df2ed1a8704a3464798ac413dda";

const MAX_BLOCKS_PER_SECTION: usize = 8;

static SECTION_HEADERS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s = HashSet::new();
    s.insert("Implementation Diffs:");
    s.insert("Mapping Issues:");
    s.insert("Reviewed Divergence:");
    s.insert("Harness Warnings:");
    s.insert("Harness Failures:");
    s.insert("Zsh Diagnostic Corpus Drift:");
    s
});

static FIXTURE_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:/.*|repo `.*`:\s*)$").expect("valid regex"));

static IMPORTANT_LINE_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"^running \d+ tests$").unwrap(),
        Regex::new(r"^large corpus: processed \d+/\d+ fixtures").unwrap(),
        Regex::new(r"^large corpus compatibility summary: ").unwrap(),
        Regex::new(r"^large corpus (compatibility|zsh parse) note: ").unwrap(),
        Regex::new(r"^large corpus .* had \d+ blocking issue\(s\) ").unwrap(),
        Regex::new(r"^large corpus test skipped ").unwrap(),
        Regex::new(r"^large corpus zsh parse skipped ").unwrap(),
        Regex::new(r"^zsh diagnostic corpus test skipped ").unwrap(),
        Regex::new(r"^zsh diagnostic corpus drifted ").unwrap(),
        Regex::new(r"^thread 'large_corpus").unwrap(),
        Regex::new(r"^thread 'zsh_diagnostic_corpus_matches_baseline").unwrap(),
        Regex::new(r"^test large_corpus_").unwrap(),
        Regex::new(r"^test zsh_diagnostic_").unwrap(),
        Regex::new(r"^failures:$").unwrap(),
        Regex::new(r"^test result: ").unwrap(),
        Regex::new(r"^error: test failed").unwrap(),
        Regex::new(r"^make: \*\*\*").unwrap(),
        Regex::new(r"^Nonblocking issue buckets were omitted ").unwrap(),
    ]
});

struct SectionState {
    name: String,
    printed_blocks: usize,
    suppressed_blocks: usize,
    current_block: Vec<String>,
}

impl SectionState {
    fn new(name: String) -> Self {
        Self {
            name,
            printed_blocks: 0,
            suppressed_blocks: 0,
            current_block: Vec::new(),
        }
    }
}

fn should_print_line(line: &str) -> bool {
    let stripped = line.trim_end_matches(['\r', '\n']);
    if SECTION_HEADERS.contains(stripped) {
        return true;
    }
    IMPORTANT_LINE_RES.iter().any(|re| re.is_match(stripped))
}

fn flush_block(state: &mut SectionState) -> Vec<String> {
    if state.current_block.is_empty() {
        return Vec::new();
    }

    let block = std::mem::take(&mut state.current_block);
    if state.printed_blocks < MAX_BLOCKS_PER_SECTION {
        state.printed_blocks += 1;
        let mut res = block;
        res.push("\n".to_string());
        res
    } else {
        state.suppressed_blocks += 1;
        Vec::new()
    }
}

fn finish_section(state: &mut SectionState) -> Vec<String> {
    let mut output = flush_block(state);
    if state.suppressed_blocks > 0 {
        output.push(format!(
            "... omitted {} additional entries from {}\n",
            state.suppressed_blocks, state.name
        ));
    }
    output
}

/// Compact a list of log lines according to high-signal filtering rules.
pub fn compact_lines(lines: &[String]) -> Vec<String> {
    let mut output = Vec::new();
    let mut section: Option<SectionState> = None;
    let mut index = 0;

    while index < lines.len() {
        let line = &lines[index];
        let stripped = line.trim_end_matches(['\r', '\n']);

        if let Some(ref mut sec) = section {
            let section_end = SECTION_HEADERS.contains(stripped)
                || (should_print_line(line) && !stripped.is_empty() && stripped != sec.name);

            if section_end {
                let mut finished = finish_section(sec);
                output.append(&mut finished);
                section = None;
                // re-evaluate the current line without incrementing index
                continue;
            }

            if FIXTURE_HEADER_RE.is_match(stripped) {
                let mut flushed = flush_block(sec);
                output.append(&mut flushed);
                sec.current_block = vec![line.clone()];
            } else {
                if stripped.is_empty() && sec.current_block.is_empty() {
                    index += 1;
                    continue;
                }
                sec.current_block.push(line.clone());
            }
            index += 1;
            continue;
        }

        if SECTION_HEADERS.contains(stripped) {
            output.push(line.clone());
            let name = stripped.strip_suffix(':').unwrap_or(stripped).to_string();
            section = Some(SectionState::new(name));
        } else if should_print_line(line) {
            output.push(line.clone());
        }

        index += 1;
    }

    if let Some(ref mut sec) = section {
        let mut finished = finish_section(sec);
        output.append(&mut finished);
    }

    output
}

/// Stream compact log from input to output.
pub fn run_compact_log(input_path: Option<&Path>, output_path: Option<&Path>) -> Result<()> {
    let lines: Vec<String> = if let Some(path) = input_path {
        let file = File::open(path).with_context(|| format!("Failed to open input: {:?}", path))?;
        BufReader::new(file)
            .lines()
            .collect::<Result<_, _>>()
            .map(|l: Vec<String>| l.into_iter().map(|s| format!("{s}\n")).collect())?
    } else {
        let stdin = std::io::stdin();
        BufReader::new(stdin.lock())
            .lines()
            .collect::<Result<_, _>>()
            .map(|l: Vec<String>| l.into_iter().map(|s| format!("{s}\n")).collect())?
    };

    let compacted = compact_lines(&lines);

    if let Some(path) = output_path {
        let mut file =
            File::create(path).with_context(|| format!("Failed to create output: {:?}", path))?;
        for line in compacted {
            file.write_all(line.as_bytes())?;
        }
    } else {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        for line in compacted {
            handle.write_all(line.as_bytes())?;
        }
    }

    Ok(())
}

fn compute_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("Failed to open {:?}", path))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn download_and_extract(
    url: &str,
    expected_sha256: &str,
    dest_dir: &Path,
    label: &str,
    repo_root: &Path,
) -> Result<()> {
    if dest_dir.is_dir() {
        let count = walkdir::WalkDir::new(dest_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .count();
        if count > 0 {
            print_success(&format!(
                "{label} already present ({count} files in {})",
                dest_dir.display()
            ));
            return Ok(());
        }
    }

    print_step(&format!("Downloading {label} from {url}..."));
    let temp_archive = std::env::temp_dir().join(format!(
        "shuck-{}-{}.tar.zst",
        label.to_lowercase().replace(' ', "-"),
        std::process::id()
    ));

    let run_opts = RunOptions::default();
    let curl_args = [
        "-L",
        "--fail",
        "--retry",
        "3",
        "-o",
        temp_archive.to_str().unwrap(),
        url,
    ];
    run_command("curl", &curl_args, &run_opts)
        .context(format!("Failed to download {label} archive"))?;

    print_step(&format!("Verifying SHA-256 for {label}..."));
    let actual_sha256 = compute_sha256(&temp_archive)?;
    if actual_sha256 != expected_sha256 {
        let _ = std::fs::remove_file(&temp_archive);
        bail!(
            "Checksum mismatch for {label}!\nExpected: {expected_sha256}\nActual:   {actual_sha256}"
        );
    }
    print_success("Checksum verified.");

    print_step(&format!(
        "Extracting {label} into {}...",
        repo_root.display()
    ));
    let tar_args = [
        "--zstd",
        "-xf",
        temp_archive.to_str().unwrap(),
        "-C",
        repo_root.to_str().unwrap(),
    ];
    run_command("tar", &tar_args, &run_opts)?;
    let _ = std::fs::remove_file(&temp_archive);

    let count = walkdir::WalkDir::new(dest_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count();
    print_success(&format!("Successfully installed {label} ({count} files)."));
    Ok(())
}

/// Download the large corpus archives.
pub fn run_download(clone: bool, dry_run: bool, custom_corpus_dir: Option<PathBuf>) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Corpus Download");

    if clone {
        print_step("Running repository clone mode via scripts/corpus-download.sh...");
        let script = repo_root.join("scripts/corpus-download.sh");
        let mut args = vec!["-c"];
        if dry_run {
            args.push("-l");
        }
        let opts = RunOptions {
            cwd: Some(&repo_root),
            ..Default::default()
        };
        run_command(script.to_str().unwrap(), &args, &opts)?;
        return Ok(());
    }

    let corpus_dir = custom_corpus_dir.unwrap_or_else(|| repo_root.join(".cache/large-corpus"));
    let zsh_corpus_dir = repo_root.join(".cache/zsh-diagnostic-corpus");

    download_and_extract(
        DEFAULT_ARCHIVE_URL,
        DEFAULT_ARCHIVE_SHA256,
        &corpus_dir.join("scripts"),
        "Large Corpus",
        &repo_root,
    )?;

    download_and_extract(
        DEFAULT_ZSH_ARCHIVE_URL,
        DEFAULT_ZSH_ARCHIVE_SHA256,
        &zsh_corpus_dir.join("repos"),
        "Zsh Diagnostic Corpus",
        &repo_root,
    )?;

    Ok(())
}

/// Run large corpus conformance tests.
#[allow(clippy::too_many_arguments)]
pub fn run_test(
    timeout_secs: u64,
    shuck_timeout_secs: Option<u64>,
    sample_percent: u8,
    mapped_only: bool,
    keep_going: bool,
    timing: bool,
    rules: Option<String>,
    zsh: bool,
    compact: bool,
) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Large Corpus Test Execution");

    let cache_dir = repo_root.join(".cache");
    if !cache_dir.exists() {
        print_warning("No .cache found. Running corpus download first...");
        run_download(false, false, None)?;
    }

    let mut envs = std::collections::HashMap::new();
    let timeout_str = timeout_secs.to_string();
    let sample_str = sample_percent.to_string();
    let timing_str = if timing { "1" } else { "0" };
    let mapped_str = if mapped_only { "1" } else { "0" };
    let keep_going_str = if keep_going { "1" } else { "0" };

    envs.insert("SHUCK_TEST_LARGE_CORPUS", "1");
    envs.insert("SHUCK_LARGE_CORPUS_TIMEOUT_SECS", timeout_str.as_str());
    envs.insert("SHUCK_LARGE_CORPUS_SAMPLE_PERCENT", sample_str.as_str());
    envs.insert("SHUCK_LARGE_CORPUS_MAPPED_ONLY", mapped_str);
    envs.insert("SHUCK_LARGE_CORPUS_KEEP_GOING", keep_going_str);
    envs.insert("SHUCK_LARGE_CORPUS_TIMING", timing_str);

    let shuck_timeout_str;
    if let Some(st) = shuck_timeout_secs {
        shuck_timeout_str = st.to_string();
        envs.insert(
            "SHUCK_LARGE_CORPUS_SHUCK_TIMEOUT_SECS",
            shuck_timeout_str.as_str(),
        );
    }

    let rules_str;
    if let Some(r) = rules {
        rules_str = r;
        envs.insert("SHUCK_LARGE_CORPUS_RULES", rules_str.as_str());
    }

    let mut cargo_args = vec!["test", "-p", "shucked-cli", "--test", "large_corpus"];

    if zsh {
        cargo_args.push("large_corpus_zsh_fixtures_parse");
        cargo_args.push("--");
        cargo_args.push("--ignored");
        cargo_args.push("--exact");
        cargo_args.push("--nocapture");
    } else if timing {
        cargo_args.push("large_corpus_conforms_with_shellcheck");
        cargo_args.push("--");
        cargo_args.push("--ignored");
        cargo_args.push("--exact");
        cargo_args.push("--nocapture");
    } else {
        cargo_args.push("--");
        cargo_args.push("--ignored");
        cargo_args.push("--nocapture");
    }

    let use_nix = is_tool_available("nix");
    let program: &str;
    let mut final_args: Vec<&str> = Vec::new();

    if use_nix {
        program = "nix";
        final_args.extend(&[
            "--extra-experimental-features",
            "nix-command flakes",
            "develop",
            "--command",
            "cargo",
        ]);
        final_args.extend(cargo_args);
    } else {
        program = "cargo";
        final_args.extend(cargo_args);
    }

    let opts = RunOptions {
        cwd: Some(&repo_root),
        envs,
        quiet: false,
    };

    if compact {
        let output = crate::runner::run_command_captured(program, &final_args, &opts)?;
        let lines: Vec<String> = output
            .stdout
            .lines()
            .chain(output.stderr.lines())
            .map(|l| format!("{l}\n"))
            .collect();
        let compacted = compact_lines(&lines);
        for l in compacted {
            print!("{l}");
        }
        if !output.status.success() {
            bail!("Large corpus test failed with status {}", output.status);
        }
    } else {
        run_command(program, &final_args, &opts)?;
    }

    print_success("Large corpus test execution finished.");
    Ok(())
}

/// Generate HTML compatibility report from large corpus test log.
pub fn run_report(log: Option<&Path>, output: Option<&Path>, open: bool) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Large Corpus Report");
    let script = repo_root.join("tooling/scripts/large_corpus_report.py");
    let log_path = log
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo_root.join("target/large-corpus.log"));
    let out_path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo_root.join("target/large-corpus-report.html"));

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    let log_str = log_path.to_str().context("invalid log path")?;
    let out_str = out_path.to_str().context("invalid output path")?;

    let args = [
        script.to_str().context("invalid script path")?,
        "--log",
        log_str,
        "--output",
        out_str,
    ];
    run_command("python3", &args, &opts)?;
    print_success(&format!("Large corpus HTML report: {}", out_path.display()));

    if open {
        let open_cmd = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = run_command(open_cmd, &[out_str], &opts);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keeps_high_signal_lines_and_drops_chatter() {
        let lines = vec![
            "Compiling shuck v0.0.7\n".to_string(),
            "running 2 tests\n".to_string(),
            "large corpus: processed 41/818 fixtures (5%)\n".to_string(),
            "large corpus compatibility summary: blocking=1 warnings=2 fixtures=3 unsupported_shells=0 implementation_diffs=1 mapping_issues=1 reviewed_divergences=1 harness_warnings=0 harness_failures=0\n".to_string(),
            "test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 72 filtered out; finished in 267.03s\n".to_string(),
        ];

        let compacted = compact_lines(&lines).join("");

        assert!(!compacted.contains("Compiling shuck"));
        assert!(compacted.contains("running 2 tests"));
        assert!(compacted.contains("large corpus: processed 41/818 fixtures (5%)"));
        assert!(compacted.contains("large corpus compatibility summary:"));
        assert!(compacted.contains("test result: FAILED."));
    }

    #[test]
    fn test_truncates_large_sections_after_eight_blocks() {
        let mut lines = vec!["Implementation Diffs:\n".to_string()];
        for i in 0..10 {
            lines.push(format!("/tmp/fixture-{i}.sh\n"));
            lines.push(format!(
                "  shellcheck-only C001/SC2000 {}:1-{}:5 error example {i}\n",
                i + 1,
                i + 1
            ));
            lines.push("\n".to_string());
        }
        lines.push("test large_corpus_conforms_with_shellcheck ... FAILED\n".to_string());

        let compacted = compact_lines(&lines).join("");

        for i in 0..8 {
            assert!(compacted.contains(&format!("/tmp/fixture-{i}.sh")));
        }
        assert!(!compacted.contains("/tmp/fixture-8.sh"));
        assert!(!compacted.contains("/tmp/fixture-9.sh"));
        assert!(compacted.contains("... omitted 2 additional entries from Implementation Diffs"));
        assert!(compacted.contains("test large_corpus_conforms_with_shellcheck ... FAILED"));
    }

    #[test]
    fn test_blank_lines_inside_one_fixture_do_not_hide_later_fixtures() {
        let mut lines = vec![
            "Implementation Diffs:\n".to_string(),
            "/tmp/noisy.sh\n".to_string(),
        ];
        for i in 0..8 {
            lines.push(format!(
                "  shellcheck-only C001/SC2000 {}:1-{}:5 error example {i}\n",
                i + 1,
                i + 1
            ));
            lines.push("\n".to_string());
        }
        lines.push("/tmp/second.sh\n".to_string());
        lines.push("  shellcheck-only C001/SC2000 9:1-9:5 error second\n".to_string());
        lines.push("\n".to_string());
        lines.push("test large_corpus_conforms_with_shellcheck ... FAILED\n".to_string());

        let compacted = compact_lines(&lines).join("");

        assert!(compacted.contains("/tmp/noisy.sh"));
        assert!(compacted.contains("/tmp/second.sh"));
        assert!(!compacted.contains("additional entries from Implementation Diffs"));
    }
}
