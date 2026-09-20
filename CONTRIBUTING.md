# Contributing to Shucked

Thanks for your interest in contributing to Shucked! This guide covers how to build, test, and add lint rules.

By participating in this project you agree to abide by its [Code of Conduct](CODE_OF_CONDUCT.md). Please also read [CLEAN_ROOM.md](CLEAN_ROOM.md) before looking at ShellCheck internals — Shucked is a clean-room reimplementation and contributions must preserve that.

## Prerequisites

- **Rust** stable toolchain (pinned in `rust-toolchain.toml`; includes `rustfmt` and `clippy`)

## Getting Started

```bash
git clone https://github.com/fredrir/shucked.git
cd shucked

# Set up pre-commit hooks (runs cargo fmt and clippy before each commit)
just setup-hooks

# Build
just build

# Run tests
just test


# Run code style, clippy linting, and dependency checks
just check

# Run the CLI
just run check .

# Download large corpus fixtures
just corpus download

# Run large corpus compatibility tests against ShellCheck
just corpus test

# Generate large corpus HTML report
just corpus report

# Run fuzz testing smoke suite
just fuzz smoke
```

## Shell completions

Shell completions live in `just/_just`: recipe names come from `just` itself, recipe arguments are
read from the `tooling` CLI help.

```bash
mkdir -p ~/.zsh/completions
ln -sf "$PWD/just/_just" ~/.zsh/completions/_just
```

Add to `~/.zshrc` and restart the shell:

```bash
fpath=(~/.zsh/completions $fpath)
autoload -U compinit && compinit
```

Argument completion uses the built `tooling` binary (`target/debug/tooling`, `target/release/tooling`,
or `tooling` on `PATH`). If the binary is missing but the workspace has been built before, it falls
back to `cargo run -p shucked-tooling`; recipe names always complete.

## Development Workflow

Before submitting changes, run the full check suite:

```bash
just check    # formatting, clippy, dependency, and security checks
just hawk     # reject public APIs that are unreachable from shipped targets
```

The Hawk check requires `cargo-hawk` 0.1.8 and the Rust 1.97.0 toolchain. CI
installs both pinned versions before running the check.

Or run the individual steps:

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
```

Pre-commit hooks enforce formatting and clippy automatically when you commit.

## Commit messages

Shucked uses [Conventional Commits](https://www.conventionalcommits.org/) so that [release-please](https://github.com/googleapis/release-please) can generate `CHANGELOG.md` and pick the next version automatically from commit history on `main`.

PRs are squash-merged, so **the PR title is what ends up on `main`** — please write PR titles in Conventional Commit form:

```
<type>(<optional scope>): <short summary>
```

Common types:

| Type       | Use for                                    | Appears in changelog |
| ---------- | ------------------------------------------ | -------------------- |
| `feat`     | New user-visible behavior                  | yes                  |
| `fix`      | Bug fix                                    | yes                  |
| `perf`     | Performance improvement                    | yes                  |
| `docs`     | Documentation-only change                  | yes                  |
| `refactor` | Internal restructuring, no behavior change | yes                  |
| `test`     | Tests only                                 | no                   |
| `chore`    | Tooling, deps, misc                        | no                   |
| `ci`       | Workflows under `.github/`                 | no                   |
| `build`    | Build system, packaging                    | no                   |

For a breaking change, append `!` to the type or add a `BREAKING CHANGE:` footer (e.g., `feat!: drop C005`).

Examples:

```
feat(linter): add C042 for unused function parameters
fix(parser): handle nested heredocs inside $()
perf(checker): cache fact lookups per rule
docs: clarify category code vs SCNNNN suppression syntax
chore(deps): bump clap to 4.5
```

### Releases

You do **not** bump `workspace.package.version` or edit `CHANGELOG.md` by hand. `release-please` watches `main` and maintains a release PR that bumps the version and updates `CHANGELOG.md` from the Conventional Commit history. Merging that PR creates the `vX.Y.Z` tag, which triggers `release.yml` (cargo-dist) to build and publish artifacts and the GitHub Release.

## Testing

**Run all tests:**

```bash
just test          # or: cargo test
```

**Run a single test:**

```bash
cargo test -p shucked-linter -- test_name
```

**Snapshot tests** — The linter uses [insta](https://insta.rs) for snapshot testing. When you add or change a rule, the test will fail with a diff. Review and accept with:

```bash
cargo insta accept --workspace
```


```bash
just corpus download          # download corpus (first time only)
just corpus test              # run full comparison against ShellCheck
```

You can target specific rules or sample a subset:

```bash
just corpus test --rules C001
just corpus test --sample-percent 10
just corpus test --timing
```

## Fuzzing

Shucked keeps fuzzing in the `tooling/fuzz/` workspace, managed via `just fuzz` and `tooling/`.

`cargo fuzz` resolves the fuzz directory relative to the nearest non-fuzz package, so it must run
from `tooling/fuzz` (or with `--fuzz-dir tooling/fuzz`).

Initialize the fuzz toolchain, generated corpora, and artifact directories with:

```bash
just fuzz init
```

For CI or non-interactive setup:

```bash
just fuzz init --ci
```

The setup command seeds repository-owned fixtures into one generated corpus:

- `tooling/fuzz/corpus/common` for parser, recovered-parser, arithmetic, glob, and linter targets

Seed sources:

- `crates/shucked-linter/resources/test/fixtures`
- `crates/shucked-formatter/tests/oracle-fixtures`
- `crates/shucked-benchmark/resources/files`

If `rustup` is not installed yet, the setup script bootstraps it so fuzzing can use nightly
without changing the repo's default stable toolchain.

List fuzz targets:

```bash
just fuzz list
```

Blocking smoke coverage:

```bash
just fuzz smoke
```

`just fuzz smoke` is intentionally deterministic. It runs each PR-blocking fuzz target with
`-runs=1` to verify toolchain setup, corpus wiring, and harness startup. Longer mutation-heavy
fuzzing belongs in the scheduled GitHub Actions workflow or in manual local runs.

Run one target with a longer budget:

```bash
just fuzz run parser_fuzz '-max_total_time=60'
```

Available `cargo-fuzz` targets:

- `parser_fuzz`
- `lexer_fuzz`
- `arithmetic_fuzz`
- `glob_fuzz`
- `recovered_parser_fuzz`
- `linter_no_panic_fuzz`
- `lsp_document_sync_fuzz`
- `lsp_request_surface_fuzz`
- `lsp_protocol_sequence_fuzz`

The LSP fuzz targets exercise document synchronization, editor request handlers, and bounded
in-memory protocol transcripts. Invalid generated LSP inputs may return normal LSP errors; fuzz
failures are panics, hangs, invalid response ranges/edits, malformed serializations, or server
sessions that do not shut down cleanly.

Run the CLI generator-driven fuzzer:

```bash
just fuzz cli '--dialect bash --profile full --count 50 --seed 100'
```

Useful CLI fuzzer flags:

- `--dialect {sh,bash}`
- `--profile {smoke,full}`
- `--count N`
- `--seed N`
- `--workers N`
- `--artifact-dir PATH`
- `--timeout SECONDS`

The blocking PR smoke job uses the conservative `smoke` profile. The scheduled fuzz workflow uses
the broader `full` profile.

To minimize a `cargo-fuzz` crash:

```bash
cd tooling/fuzz
cargo +nightly fuzz tmin parser_fuzz artifacts/parser_fuzz/crash-...
```

CLI fuzzer failures are minimized automatically and written under `tooling/fuzz/artifacts/cli/`.

## Project Structure

| Crate               | Purpose                                                                                        |
| ------------------- | ---------------------------------------------------------------------------------------------- |
| `shucked-cli`       | CLI binary `shucked` — command orchestration, discovery, config, caching, fixes, and reporting |
| `shucked-linter`    | Lint rule registry, checker dispatch, facts, suppressions, fixes, and diagnostics              |
| `shucked-semantic`  | Semantic model — bindings, scopes, CFG, dataflow                                               |
| `shucked-indexer`   | Positional and structural indexes over parsed scripts                                          |
| `shucked-parser`    | Recursive-descent Bash parser                                                                  |
| `shucked-ast`       | AST node types, tokens, spans                                                                  |
| `shucked-extract`   | Embedded shell extraction for supported host files such as GitHub Actions workflows            |
| `shucked-cache`     | SHA-256 keyed file-level result caching                                                        |
| `shucked-formatter` | Shell script formatter                                                                         |
| `shucked-benchmark` | Shared benchmark fixtures and benchmark harness helpers                                        |
| `shucked-tooling`   | High-performance developer tooling CLI (`tooling/`)                                            |
| `shucked-fuzz`      | Separate `cargo-fuzz` workspace under `tooling/fuzz/`                                          |

## Adding a Lint Rule

Rules are organized into five categories:

| Prefix | Category    | Example                       |
| ------ | ----------- | ----------------------------- |
| `C`    | Correctness | `C001` — unused assignment    |
| `S`    | Style       | `S001` — unquoted expansion   |
| `P`    | Performance | `P001` — useless cat          |
| `X`    | Portability | `X001` — bashism in sh script |
| `K`    | Security    | `K001` — unquoted glob in rm  |

### Step 1: Write the rule spec

Create `tooling/fixtures/{CODE}.yaml` with the rule definition:

```yaml
new_category: Correctness
new_code: C042
runtime_kind: ast          # ast, semantic, or flow
shellcheck_code: SC2034    # ShellCheck compatibility code, if applicable
shellcheck_level: warning  # Populate from the ShellCheck oracle when shellcheck_code is set
shells:
  - sh
  - bash
description: What the rule detects.
rationale: Why it matters and how to fix it.
examples:
  - kind: invalid
    code: |
      #!/bin/sh
      problematic_code
  - kind: valid
    code: |
      #!/bin/sh
      correct_code
```

### Step 2: Register the rule

In `crates/shucked-linter/src/registry.rs`, add an entry to the `declare_rules!` macro in code-sorted order:

```rust
declare_rules! {
    ("C001", Category::Correctness, Severity::Warning, UnusedAssignment),
    // ...
    ("C042", Category::Correctness, Severity::Warning, YourRuleName),
    // ...
}
```

### Step 3: Populate generated metadata

If the rule maps to a ShellCheck code, set `shellcheck_code` in `tooling/fixtures/{CODE}.yaml` and populate the matching ShellCheck log level.

`crates/shucked-linter/build.rs` generates the runtime rule metadata and ordinary
ShellCheck-code mappings from `tooling/fixtures/*.yaml`. Do not hand-edit
`crates/shucked-linter/src/suppression/shellcheck_map.rs` for normal rule
mappings; only update `SUPPRESSION_ALIAS_CODES` there when an old ShellCheck
code should suppress a rule without being the rule's canonical compatibility
code.

### Step 4: Implement the rule

Create `crates/shucked-linter/src/rules/{category}/{snake_case_name}.rs`:

```rust
use crate::{Checker, Rule, Violation};

pub struct YourRuleName {
    pub name: String,
}

impl Violation for YourRuleName {
    fn rule() -> Rule {
        Rule::YourRuleName
    }

    fn message(&self) -> String {
        format!("description of the problem for `{}`", self.name)
    }
}

pub fn your_rule_name(checker: &mut Checker) {
    // Query the semantic model or precomputed linter facts.
    // Report violations with checker.report(violation, span)
}
```

Key APIs available on `Checker`:
- `checker.semantic()` — bindings, references, scopes, call graph
- `checker.facts()` — normalized commands, pipelines, conditionals
- `checker.source()` — source text
- `checker.shell()` — detected shell dialect
- `checker.report(violation, span)` — emit a diagnostic
- `checker.report_dedup(violation, span)` — emit with deduplication

New rule files should be cheap filters over `checker.facts()` or
`checker.semantic()`. Do not directly walk the AST or rescan source text to
rediscover shell structure; if a rule needs structural data that facts do not
expose yet, add that data to `crates/shucked-linter/src/facts/` first.

Look at existing rules for patterns:
- Simple semantic rule: `rules/correctness/unused_assignment.rs`
- Facts-based rule: `rules/style/read_without_raw.rs`
- Complex rule: `rules/correctness/find_output_to_xargs.rs`

### Step 5: Register the module

In `crates/shucked-linter/src/rules/{category}/mod.rs`, add:

```rust
pub mod your_rule_name;
```

Then add a `#[test_case]` entry in the test function at the bottom of the same file:

```rust
#[test_case(Rule::YourRuleName, Path::new("C042.sh"))]
```

### Step 6: Wire into checker dispatch

In `crates/shucked-linter/src/checker.rs`, add the rule to the appropriate checker phase:

| Phase                | Use for                                |
| -------------------- | -------------------------------------- |
| `check_bindings`     | Variable assignments, unused variables |
| `check_references`   | Variable uses, undefined variables     |
| `check_declarations` | `declare`/`local`/`export` commands    |
| `check_call_sites`   | Function calls                         |
| `check_source_refs`  | `source`/`.` commands                  |
| `check_commands`     | Command structure, most rules go here  |
| `check_flow`         | Control flow, dead code                |

```rust
if self.is_rule_enabled(Rule::YourRuleName) {
    rules::category::your_rule_name::your_rule_name(self);
}
```

### Step 7: Create a test fixture

Create `crates/shucked-linter/resources/test/fixtures/{category}/C042.sh` with both triggering and non-triggering cases:

```bash
#!/bin/sh

# Should trigger
problematic_code

# Should not trigger
correct_code
```

### Step 8: Run tests and accept snapshots

```bash
cargo test -p shucked-linter -- your_rule_name    # run the new tests
cargo insta accept --workspace                   # accept snapshot output
cargo test                                       # verify no regressions
```

## Clean-Room Policy

Shucked is a clean-room reimplementation. All contributors must follow these rules:

- **Do not** read, reference, or import ShellCheck source code or wiki pages
- **Do not** reuse diagnostic wording from ShellCheck materials
- **Do** write all descriptions, rationales, and messages from scratch in your own words
- **Do** reference shell language manuals and specifications (POSIX, Bash reference manual)
- **Do** use the ShellCheck binary as a black-box oracle (run it, observe behavior, but do not copy its output text)

See `CLAUDE.md` for the full policy.

## Code Style

- **Rust edition 2024**, stable toolchain
- Repo-pinned `rustfmt` settings via `rustfmt.toml`, plus default `clippy` settings
- **Error handling**: `anyhow` for error propagation with `.context()`, `thiserror` for domain-specific error enums
- **Suppression codes**: Shucked uses category codes (e.g., `C001`, `S001`) or ShellCheck `SCNNNN` format in suppression directives

## Benchmarking

```bash
just bench                    # Criterion microbenchmarks (all targets)
just bench parser             # single benchmark target
just bench --memory           # memory profiling benchmarks
just flame parser             # flamegraph for one target
just profile parser           # samply profile for one target
```

Benchmark targets: `parser`, `arithmetic`, `lexer`, `semantic`, `linter`, `formatter`, `lsp`, `all`.
Flamegraphs and profiles support a subset of those targets.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
