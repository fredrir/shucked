set shell := ["bash", "-uc"]
set default-list

# [ compact-log, download, report, test ]
mod corpus 'just/corpus.just'

# [ init, list, smoke, run, cli ]
mod fuzz 'just/fuzz.just'

# Release workflow audits and SBOM generation
mod release 'just/release.just'

# VS Code extension: compile, package, test, lint
mod vscode 'just/vscode.just'

# WebAssembly package: build, test
mod wasm 'just/wasm.just'

# [ --cli, --server, --wasm, --release, --all ] -- Build workspace
[group('dev')]
build *args="":
    cargo run -q -p shucked-tooling -- build {{ args }}

# Run the Shucked CLI (`just run check .`)
[group('dev')]
run *args="":
    cargo run -p shucked-cli -- {{ args }}

# [ --vscode, --shucked ] -- Build release and install locally
[group('dev')]
deploy *args="":
    cargo run -q -p shucked-tooling -- deploy {{ args }}

# Fast pre-push checks: formatting, clippy, dependencies, workflow security
[group('verify')]
check:
    cargo run -q -p shucked-tooling -- check

# [ --up, --down, --get ] target: shucked, vscode
[group('release')]
tag *args="":
    cargo run -q -p shucked-tooling -- tag {{ args }}

# Rust and workspace formatting (--check, --vscode)
[group('verify')]
fmt *args="":
    cargo run -q -p shucked-tooling -- format {{ args }}

# Clippy and static analysis (--fix, --all-features, --skip-shear, --skip-security)
[group('verify')]
lint *args="":
    cargo run -q -p shucked-tooling -- lint {{ args }}

# Test suites (--unit, --lsp, --linter, --python, --wasm, --all, -p PKG, FILTER)
[group('verify')]
test *args="":
    cargo run -q -p shucked-tooling -- test {{ args }}

# Criterion benchmarks (parser, arithmetic, lexer, semantic, linter, formatter, lsp, all)
[group('perf')]
bench target="all" *args="":
    cargo run -q -p shucked-tooling -- bench {{ target }} {{ args }}

# Flamegraph (parser, arithmetic, formatter, linter, cli)
[group('perf')]
flame target *args="":
    cargo run -q -p shucked-tooling -- flame {{ target }} {{ args }}

# samply profile (parser, arithmetic, formatter, linter, cli, large-corpus)
[group('perf')]
profile target *args="":
    cargo run -q -p shucked-tooling -- profile {{ target }} {{ args }}

# Install toolchains, cargo tools, and hooks (--skip-cargo-tools)
[group('setup')]
init *args="":
    cargo run -q -p shucked-tooling -- init {{ args }}

# Point git at .githooks
[group('setup')]
setup-hooks:
    git config core.hooksPath .githooks

# Reject unreachable public APIs (cargo-hawk 0.1.8, Rust 1.97.0)
[group('setup')]
hawk:
    cargo +1.97.0 hawk --all-targets

# Remove artifacts, caches, profiles, and logs (--all, --dry-run)
[group('setup')]
clean *args="":
    cargo run -q -p shucked-tooling -- clean {{ args }}
