use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

mod bench;
mod build;
mod check;
mod clean;
mod corpus;
mod deploy;
mod flame;
mod format;
mod fuzz;
mod init;
mod lint;
mod profile;
mod release;
mod runner;
mod tag;
mod test_cmd;
mod vscode;

#[derive(Parser, Debug)]
#[command(
    name = "tooling",
    author = "Fredrik Carsten Hansteen <fhansteen@gmail.com>",
    version = "0.0.4",
    about = "Blazingly fast developer tooling and automation for Shuck",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize project tooling, toolchains, components, and hooks
    Init(InitArgs),

    /// Build workspace targets
    Build(BuildArgs),

    /// High performance workspace linting and static analysis
    Lint(LintArgs),

    /// Auto-formatting across the repository
    Format(FormatArgs),

    /// Test runner for unit, integration, python, and wasm test suites
    Test(TestArgs),

    /// Fast parallel pre-push sanity checks
    Check,

    /// Criterion benchmarks runner
    Bench(BenchArgs),

    /// Profiling runner using samply
    Profile(ProfileArgs),

    /// SVG flamegraph generator using cargo-flamegraph
    Flame(FlameArgs),

    /// Large corpus test and log management
    Corpus(CorpusArgs),

    /// Fuzzing tools and test runners
    Fuzz(FuzzArgs),

    /// VS Code extension development tooling
    Vscode(VscodeArgs),

    /// Build release binaries and install them locally
    Deploy(DeployArgs),

    /// Release verification and workflow security audits
    Release(ReleaseArgs),

    /// Show or bump the shucked and VS Code extension versions
    Tag(TagArgs),

    /// Workspace cleaning
    Clean(CleanArgs),
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Skip installing or updating optional cargo development tools
    #[arg(long)]
    skip_cargo_tools: bool,
}

#[derive(Args, Debug)]
struct BuildArgs {
    /// Build in release mode with optimizations
    #[arg(short, long)]
    release: bool,

    /// Build WebAssembly package (shucked-wasm)
    #[arg(long)]
    wasm: bool,

    /// Build CLI binary (shucked-cli)
    #[arg(long)]
    cli: bool,

    /// Build language server binaries (shucked-server, shucked-lsp)
    #[arg(long)]
    server: bool,

    /// Build all targets including wasm
    #[arg(short, long)]
    all: bool,

    /// Additional arguments passed to cargo build
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Args, Debug)]
struct LintArgs {
    /// Automatically fix supported linter and formatter issues
    #[arg(long)]
    fix: bool,

    /// Run clippy with --all-features
    #[arg(long)]
    all_features: bool,

    /// Skip cargo-shear unused dependency check
    #[arg(long)]
    skip_shear: bool,

    /// Skip release workflow security verification
    #[arg(long)]
    skip_security: bool,
}

#[derive(Args, Debug)]
struct FormatArgs {
    /// Check formatting without modifying files
    #[arg(long)]
    check: bool,

    /// Also format VS Code extension files
    #[arg(long)]
    vscode: bool,
}

#[derive(Args, Debug)]
struct TestArgs {
    /// Run Rust workspace unit tests
    #[arg(long)]
    unit: bool,

    /// Run shucked-lsp tests
    #[arg(long)]
    lsp: bool,

    /// Run shucked-linter tests
    #[arg(long)]
    linter: bool,

    /// Run Python tests via uv pytest
    #[arg(long)]
    python: bool,

    /// Run WebAssembly tests
    #[arg(long)]
    wasm: bool,

    /// Run all test suites
    #[arg(short, long)]
    all: bool,

    /// Run tests in release mode
    #[arg(short, long)]
    release: bool,

    /// Package to test
    #[arg(short, long)]
    package: Option<String>,

    /// Optional test name filter
    #[arg(value_name = "FILTER")]
    filter: Option<String>,

    /// Additional arguments passed to cargo test
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Args, Debug)]
struct BenchArgs {
    /// Specific benchmark suite to execute
    #[arg(value_enum)]
    target: Option<bench::BenchTarget>,

    /// Run memory profiling benchmarks
    #[arg(long)]
    memory: bool,

    /// Memory target (all, parser, linter, semantic)
    #[arg(long, value_enum, default_value = "all")]
    memory_target: bench::MemoryTarget,

    /// Save baseline results under given name
    #[arg(long)]
    save_baseline: Option<String>,

    /// Compare against saved baseline name
    #[arg(long)]
    baseline: Option<String>,

    /// Filter benchmark benchmarks
    #[arg(short, long)]
    filter: Option<String>,

    /// Run benchmarks in release mode
    #[arg(long)]
    release: bool,

    /// Additional arguments passed to cargo bench
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Args, Debug)]
struct ProfileArgs {
    /// Profiling target
    #[arg(value_enum)]
    target: profile::ProfileTarget,

    /// Case or fixture name (default: nvm)
    #[arg(short, long)]
    case: Option<String>,

    /// Target script path for cli profiling
    #[arg(short, long)]
    file: Option<String>,

    /// Output directory for profiles
    #[arg(short, long)]
    output_dir: Option<String>,

    /// Sampling rate in Hz
    #[arg(short, long, default_value = "1000")]
    rate: u32,

    /// Number of iterations
    #[arg(short, long, default_value = "1")]
    iterations: u32,

    /// Automatically open profile in samply viewer
    #[arg(long)]
    view: bool,
}

#[derive(Args, Debug)]
struct FlameArgs {
    /// Flamegraph target
    #[arg(value_enum)]
    target: flame::FlameTarget,

    /// Case or fixture name (default: nvm)
    #[arg(short, long)]
    case: Option<String>,

    /// Target script path for cli flamegraph
    #[arg(short, long)]
    file: Option<String>,

    /// Output SVG path
    #[arg(short, long)]
    output: Option<String>,

    /// Automatically open generated SVG in viewer
    #[arg(long)]
    open: bool,
}

#[derive(Args, Debug)]
struct CorpusArgs {
    #[command(subcommand)]
    command: CorpusCommands,
}

#[derive(Subcommand, Debug)]
enum CorpusCommands {
    /// Download large corpus test archives
    Download(CorpusDownloadArgs),

    /// Native Rust log compactor for large corpus logs
    CompactLog(CorpusCompactLogArgs),

    /// Run large corpus conformance test suite
    Test(CorpusTestArgs),

    /// Generate HTML compatibility report from large corpus log
    Report(CorpusReportArgs),
}

#[derive(Args, Debug)]
struct CorpusDownloadArgs {
    /// Shallow-clone repos directly from GitHub instead of downloading pre-built archive
    #[arg(short, long)]
    clone: bool,

    /// Dry run listing of repositories without cloning
    #[arg(short = 'l', long)]
    dry_run: bool,

    /// Custom target corpus directory
    #[arg(long)]
    corpus_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct CorpusCompactLogArgs {
    /// Input log path (reads from stdin if omitted)
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Output path (writes to stdout if omitted)
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct CorpusTestArgs {
    /// Timeout in seconds per fixture for shellcheck
    #[arg(long, default_value = "300")]
    timeout_secs: u64,

    /// Timeout in seconds per fixture for shuck
    #[arg(long)]
    shuck_timeout_secs: Option<u64>,

    /// Sampling percentage of corpus [1, 100]
    #[arg(long, default_value = "100")]
    sample_percent: u8,

    /// Limit shellcheck diagnostics to mapped codes
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    mapped_only: bool,

    /// Collect all fixture failures instead of failing fast
    #[arg(long, default_value = "true", action = clap::ArgAction::Set)]
    keep_going: bool,

    /// Run in timing-only pass
    #[arg(long)]
    timing: bool,

    /// Comma-separated rule selectors (e.g. C001,C006)
    #[arg(long)]
    rules: Option<String>,

    /// Run zsh fixture parse test instead
    #[arg(long)]
    zsh: bool,

    /// Automatically pipe output through native compact-log
    #[arg(long)]
    compact: bool,
}

#[derive(Args, Debug)]
struct CorpusReportArgs {
    /// Path to large corpus test log
    #[arg(short, long)]
    log: Option<PathBuf>,

    /// Destination HTML report path
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Open report in browser
    #[arg(long)]
    open: bool,
}

#[derive(Args, Debug)]
struct FuzzArgs {
    #[command(subcommand)]
    command: FuzzCommands,
}

#[derive(Subcommand, Debug)]
enum FuzzCommands {
    /// Initialize fuzzing toolchain and seed corpus
    Init(FuzzInitArgs),

    /// List registered fuzzing targets
    List,

    /// Run quick 1-iteration smoke pass across all fuzzers
    Smoke(FuzzSmokeArgs),

    /// Run a specific fuzz target
    Run(FuzzRunArgs),

    /// Run differential CLI fuzzing harness
    Cli(FuzzCliArgs),
}

#[derive(Args, Debug)]
struct FuzzInitArgs {
    /// Non-interactive setup for CI
    #[arg(long)]
    ci: bool,

    /// Run corpus minimization after seeding
    #[arg(long)]
    cmin: bool,

    /// Include large corpus fixtures in seed
    #[arg(long)]
    large_corpus: bool,
}

#[derive(Args, Debug)]
struct FuzzSmokeArgs {
    /// Sanitizer to pass to cargo-fuzz (e.g. none)
    #[arg(short, long)]
    sanitizer: Option<String>,
}

#[derive(Args, Debug)]
struct FuzzRunArgs {
    /// Fuzz target name (e.g. parser_fuzz)
    target: String,

    /// Max runtime in seconds
    #[arg(short, long, default_value = "60")]
    max_total_time: u32,

    /// Sanitizer override
    #[arg(short, long)]
    sanitizer: Option<String>,

    /// Extra flags passed to fuzz target
    #[arg(last = true)]
    extra_args: Vec<String>,
}

#[derive(Args, Debug)]
struct FuzzCliArgs {
    /// Dialect: sh or bash
    #[arg(short, long, default_value = "sh")]
    dialect: String,

    /// Profile: smoke or full
    #[arg(short, long, default_value = "smoke")]
    profile: String,

    /// Number of test scripts to generate and run
    #[arg(short, long, default_value = "1")]
    count: u32,

    /// Random seed
    #[arg(short, long, default_value = "0")]
    seed: u64,

    /// Parallel worker threads
    #[arg(short, long, default_value = "1")]
    workers: u32,
}

#[derive(Args, Debug)]
struct VscodeArgs {
    #[command(subcommand)]
    command: VscodeCommands,
}

#[derive(Subcommand, Debug)]
enum VscodeCommands {
    /// Compile the VS Code extension
    Compile,

    /// Package the extension into a VSIX file
    Package,

    /// Run extension tests
    Test,

    /// Run extension linter
    Lint,
}

#[derive(Args, Debug)]
struct DeployArgs {
    /// Reinstall the VS Code extension only
    #[arg(long)]
    vscode: bool,

    /// Install the shucked binaries only
    #[arg(long)]
    shucked: bool,
}

#[derive(Args, Debug)]
struct ReleaseArgs {
    #[command(subcommand)]
    command: ReleaseCommands,
}

#[derive(Subcommand, Debug)]
enum ReleaseCommands {
    /// Audit and harden .github/workflows/release.yml
    CheckSecurity(ReleaseCheckSecurityArgs),

    /// Verify .release-please-config.json crate and python mappings
    CheckConfig,

    /// Generate CycloneDX SBOM for release packaging
    GenerateSbom,
}

#[derive(Args, Debug)]
struct ReleaseCheckSecurityArgs {
    /// Automatically apply hardening fixes to the release workflow
    #[arg(long)]
    fix: bool,

    /// Path to release workflow file
    #[arg(long)]
    workflow: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct TagArgs {
    /// Component to show or bump (defaults to both)
    #[arg(value_enum)]
    target: Option<TagTarget>,

    /// Increment the patch version
    #[arg(long, group = "action")]
    up: bool,

    /// Decrement the patch version
    #[arg(long, group = "action")]
    down: bool,

    /// Print the current version(s) (default)
    #[arg(long, group = "action")]
    get: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TagTarget {
    Shucked,
    Vscode,
}

impl From<TagTarget> for tag::Target {
    fn from(target: TagTarget) -> Self {
        match target {
            TagTarget::Shucked => tag::Target::Shucked,
            TagTarget::Vscode => tag::Target::Vscode,
        }
    }
}

#[derive(Args, Debug)]
struct CleanArgs {
    /// Deep clean including fuzz corpus, artifacts, and profiles
    #[arg(short, long)]
    all: bool,

    /// Print what would be removed without deleting
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init(args) => init::run_init(args.skip_cargo_tools),
        Commands::Build(args) => build::run_build(
            args.release,
            args.wasm,
            args.cli,
            args.server,
            args.all,
            &args.extra_args,
        ),
        Commands::Lint(args) => lint::run_lint(
            args.fix,
            args.all_features,
            args.skip_shear,
            args.skip_security,
        ),
        Commands::Format(args) => format::run_format(args.check, args.vscode),
        Commands::Test(args) => test_cmd::run_test(
            args.unit,
            args.lsp,
            args.linter,
            args.python,
            args.wasm,
            args.all,
            args.release,
            args.package.as_deref(),
            args.filter.as_deref(),
            &args.extra_args,
        ),
        Commands::Check => check::run_check(),
        Commands::Bench(args) => {
            if args.memory {
                bench::run_bench_memory(
                    args.memory_target,
                    args.save_baseline.as_deref(),
                    args.baseline.as_deref(),
                    args.release,
                    args.filter.as_deref(),
                )
            } else {
                bench::run_bench(
                    args.target,
                    args.save_baseline.as_deref(),
                    args.baseline.as_deref(),
                    args.filter.as_deref(),
                    &args.extra_args,
                )
            }
        }
        Commands::Profile(args) => profile::run_profile(
            args.target,
            args.case.as_deref(),
            args.file.as_deref(),
            args.output_dir.as_deref(),
            args.rate,
            args.iterations,
            args.view,
        ),
        Commands::Flame(args) => flame::run_flame(
            args.target,
            args.case.as_deref(),
            args.file.as_deref(),
            args.output.as_deref(),
            args.open,
        ),
        Commands::Corpus(args) => match args.command {
            CorpusCommands::Download(d) => corpus::run_download(d.clone, d.dry_run, d.corpus_dir),
            CorpusCommands::CompactLog(c) => {
                corpus::run_compact_log(c.input.as_deref(), c.output.as_deref())
            }
            CorpusCommands::Test(t) => corpus::run_test(
                t.timeout_secs,
                t.shuck_timeout_secs,
                t.sample_percent,
                t.mapped_only,
                t.keep_going,
                t.timing,
                t.rules,
                t.zsh,
                t.compact,
            ),
            CorpusCommands::Report(r) => {
                corpus::run_report(r.log.as_deref(), r.output.as_deref(), r.open)
            }
        },
        Commands::Fuzz(args) => match args.command {
            FuzzCommands::Init(i) => fuzz::run_fuzz_init(i.ci, i.cmin, i.large_corpus),
            FuzzCommands::List => fuzz::run_fuzz_list(),
            FuzzCommands::Smoke(s) => fuzz::run_fuzz_smoke(s.sanitizer.as_deref()),
            FuzzCommands::Run(r) => fuzz::run_fuzz_target(
                &r.target,
                r.max_total_time,
                r.sanitizer.as_deref(),
                &r.extra_args,
            ),
            FuzzCommands::Cli(c) => {
                fuzz::run_fuzz_cli(&c.dialect, &c.profile, c.count, c.seed, c.workers)
            }
        },
        Commands::Vscode(args) => match args.command {
            VscodeCommands::Compile => vscode::run_vscode_compile(),
            VscodeCommands::Package => vscode::run_vscode_package(),
            VscodeCommands::Test => vscode::run_vscode_test(),
            VscodeCommands::Lint => vscode::run_vscode_lint(),
        },
        Commands::Deploy(args) => deploy::run_deploy(args.vscode, args.shucked),
        Commands::Release(args) => match args.command {
            ReleaseCommands::CheckSecurity(s) => {
                release::run_check_security(s.fix, s.workflow.as_deref())
            }
            ReleaseCommands::CheckConfig => release::run_check_config(),
            ReleaseCommands::GenerateSbom => release::run_generate_sbom(),
        },
        Commands::Tag(args) => {
            let action = if args.up {
                tag::Action::Up
            } else if args.down {
                tag::Action::Down
            } else {
                tag::Action::Get
            };
            tag::run_tag(args.target.map(Into::into), action)
        }
        Commands::Clean(args) => clean::run_clean(args.all, args.dry_run),
    }
}
