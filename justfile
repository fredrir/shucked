set shell := ["bash", "-uc"]

# Print available commands
default:
    @just --list

# ==============================================================================
# Setup & Initialization
# ==============================================================================

# Initialize project tooling, toolchains, components, and hooks
init *args="":
    @cargo run -q -p shucked-tooling -- init {{ args }}

# Install repository pre-commit hooks
setup-hooks:
    git config core.hooksPath .githooks

# Audit unreachable public APIs with cargo-hawk (requires cargo-hawk 0.1.8 and Rust 1.97.0)
hawk:
    cargo +1.97.0 hawk --all-targets

# ==============================================================================
# Development & Build
# ==============================================================================

# Build workspace crates, CLI, or WASM targets (fast dev defaults)
build *args="":
    cargo run -q -p shucked-tooling -- build {{ args }}

# Run the Shucked CLI with arguments (e.g. just run check .)
run *args="":
    cargo run -p shucked-cli -- {{ args }}

# Build WebAssembly package (shucked-wasm)
wasm-build:
    cargo run -q -p shucked-tooling -- build --wasm

# Test WebAssembly package
wasm-test:
    cargo run -q -p shucked-tooling -- test --wasm

# WebAssembly workflow (build, test)
wasm action="build":
    cargo run -q -p shucked-tooling -- {{ if action == "test" { "test --wasm" } else { "build --wasm" } }}

# ==============================================================================
# Testing & Verification
# ==============================================================================

# Run test suites: workspace tests, specific targets, or python test suite
test *args="":
    cargo run -q -p shucked-tooling -- test {{ args }}

# Run fast parallel pre-push sanity checks (formatting, clippy, dependencies, workflow security)
check:
    cargo run -q -p shucked-tooling -- check

# Run clippy linting and static analysis (--fix to auto-fix)
lint *args="":
    cargo run -q -p shucked-tooling -- lint {{ args }}

# Format Rust and workspace files (--check to verify without editing)
fmt *args="":
    cargo run -q -p shucked-tooling -- format {{ args }}

# Format alias for fmt
format *args="":
    cargo run -q -p shucked-tooling -- format {{ args }}

# ==============================================================================
# Benchmarking & Profiling
# ==============================================================================

# Run benchmarks (criterion, parser, linter)
bench *args="":
    cargo run -q -p shucked-tooling -- bench {{ args }}

# Benchmark parser Criterion suite
bench-parser *args="":
    cargo run -q -p shucked-tooling -- bench parser {{ args }}

# Benchmark linter Criterion suite
bench-linter *args="":
    cargo run -q -p shucked-tooling -- bench linter {{ args }}

# Run CPU profiling with samply (targets: parser, arithmetic, formatter, linter, cli, large-corpus)
profile *args="":
    cargo run -q -p shucked-tooling -- profile {{ args }}

# Profile parser benchmark with samply
profile-parser *args="":
    cargo run -q -p shucked-tooling -- profile parser {{ args }}

# Profile CLI against a script file with samply
profile-cli file:
    cargo run -q -p shucked-tooling -- profile cli --file {{ file }}

# Generate SVG flamegraphs with cargo-flamegraph (targets: parser, arithmetic, formatter, linter, cli)
flame *args="":
    cargo run -q -p shucked-tooling -- flame {{ args }}

# Generate parser flamegraph
flame-parser *args="":
    cargo run -q -p shucked-tooling -- flame parser {{ args }}

# Generate CLI flamegraph against a script file
flame-cli file:
    cargo run -q -p shucked-tooling -- flame cli --file {{ file }}

# ==============================================================================
# Large Corpus Conformance
# ==============================================================================

# Large corpus management: download, test, compact-log, report
corpus *args="":
    cargo run -q -p shucked-tooling -- corpus {{ args }}

# Download and extract large corpus test fixtures
setup-large-corpus:
    cargo run -q -p shucked-tooling -- corpus download

# Run large corpus comparison test suite against ShellCheck
test-large-corpus *args="":
    cargo run -q -p shucked-tooling -- corpus test {{ args }}

# Generate HTML compatibility report from large corpus log
large-corpus-report *args="":
    cargo run -q -p shucked-tooling -- corpus report {{ args }}

# ==============================================================================
# Fuzzing
# ==============================================================================

# Fuzzing workflow (init, list, smoke, run, cli)
fuzz *args="":
    cargo run -q -p shucked-tooling -- fuzz {{ args }}

# Initialize fuzzing toolchain, seeds, and directories
fuzz-init *args="":
    cargo run -q -p shucked-tooling -- fuzz init {{ args }}

# List available fuzz targets
fuzz-list:
    cargo run -q -p shucked-tooling -- fuzz list

# Run quick deterministic smoke pass across all fuzz targets
fuzz-smoke *args="":
    cargo run -q -p shucked-tooling -- fuzz smoke {{ args }}

# Run a specific cargo-fuzz target (e.g. just fuzz-run parser_fuzz)
fuzz-run target *args="":
    cargo run -q -p shucked-tooling -- fuzz run {{ target }} {{ args }}

# Run differential CLI fuzz generator
fuzz-cli *args="":
    cargo run -q -p shucked-tooling -- fuzz cli {{ args }}

# ==============================================================================
# VS Code Extension
# ==============================================================================

# VS Code extension tasks (compile, package, test, lint)
vscode *args="":
    cargo run -q -p shucked-tooling -- vscode {{ args }}

# ==============================================================================
# Release & Maintenance
# ==============================================================================

# Release checks and workflow audits (check-security, check-config)
release *args="":
    cargo run -q -p shucked-tooling -- release {{ args }}

# Audit and verify release workflow security
check-release-security:
    cargo run -q -p shucked-tooling -- release check-security

# Verify .release-please-config.json crate mappings
check-release-config:
    cargo run -q -p shucked-tooling -- release check-config

# Clean build artifacts, temporary caches, profiles, and logs
clean *args="":
    cargo run -q -p shucked-tooling -- clean {{ args }}
