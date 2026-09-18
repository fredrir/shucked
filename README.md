# shucked

A fast shell script linter, formatter, and language server, written in Rust.

Shucked parses, analyzes, formats, and powers editor feedback for shell scripts. It catches common bugs, style issues, security hazards, performance traps, and portability problems; formats shell sources with configurable layout rules; and ships a first-party Language Server Protocol (LSP) server for editor diagnostics, fixes, navigation, symbols, hover, completion, and formatting. It also lints shell embedded in supported non-shell files such as GitHub Actions workflows. A caching layer keeps incremental runs fast.

## Features

- High performance — ~20-100x faster than ShellCheck
- Linting with rules across correctness, security, performance, portability, and style categories
- Safe and unsafe fix support for selected diagnostics
- Shell formatting with write, check, diff, stdin, and configuration-file modes
- Multi-dialect support: bash, sh/POSIX, mksh, zsh
- Automatic file discovery via extensions and shebang detection
- Embedded shell extraction for GitHub Actions workflows and composite actions
- First-party Language Server Protocol server for editor diagnostics, code actions, navigation, symbols, hover, completion, and formatting
- WebAssembly npm package for bundled Node.js extensions and browser-hosted editors
- ShellCheck suppression compatibility (`# shellcheck disable=SC2086`)

## Installation

### Homebrew

```sh
brew install fredrir/tap/shucked-cli
```

### PyPI

```sh
pip install shucked-cli
```

The PyPI package is named `shucked-cli`, but it still installs the `shucked`
command.

### npm / WebAssembly

```sh
npm install shucked-wasm
```

`shucked-wasm` exposes source linting and formatting to wasm-aware bundlers, so
Node.js editor extensions and browser-hosted editors can use Shucked without a
separately installed executable. See the
[`shucked-wasm` package documentation](crates/shucked-wasm/README.md) for its
TypeScript API and runtime boundaries.

### From source

```sh
cargo install shucked-cli
```

### Pre-built binaries

Pre-built binaries are available for macOS (aarch64) and Linux (x86_64) from the [releases page](https://github.com/fredrir/shucked/releases).

### GitHub Actions

Use [`fredrir/shucked-action`](https://github.com/fredrir/shucked-action) to install Shucked and report lint findings as native GitHub annotations:

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: fredrir/shucked-action@v1
    with:
      mode: check
      paths: |
        scripts
        .github/workflows
```

See the action's documentation for setup-only and formatting modes, version pinning, and additional inputs.

## Usage

Shucked's main workflows are split across subcommands:

| Workflow                                              | Command          |
| ----------------------------------------------------- | ---------------- |
| Lint files, directories, and supported embedded shell | `shucked check`  |
| Format shell files                                    | `shucked format` |
| Run the editor language server over stdio             | `shucked server` |
| Remove project cache entries                          | `shucked clean`  |

### Lint

```sh
# Check files and directories
shucked check script.sh src/

# Check the current directory
shucked check .

# Check GitHub Actions workflow `run:` blocks
shucked check .github/workflows/ci.yml

# Check a composite action
shucked check action.yml

# Read from stdin
echo 'echo $foo' | shucked check -

# Read from stdin using a logical filename for dialect and project settings
echo 'echo $foo' | shucked check --stdin-filename script.bash

# Apply safe fixes automatically
shucked check --fix .

# Apply opt-in unsafe fixes too
shucked check --unsafe-fixes .

# Skip the cache
shucked check --no-cache .

# Override the cache location
shucked --cache-dir .tmp/shucked-cache check .
```

### Format

The formatter CLI is currently experimental, so set `SHUCK_EXPERIMENTAL=1` when invoking `shucked format`:

```sh
# Format files in place
SHUCK_EXPERIMENTAL=1 shucked format .

# Check formatting without modifying files
SHUCK_EXPERIMENTAL=1 shucked format --check .

# Show unified diffs for files that would change
SHUCK_EXPERIMENTAL=1 shucked format --diff .

# Format stdin and infer the dialect from a filename
printf 'foo(){\necho hi\n}\n' | SHUCK_EXPERIMENTAL=1 shucked format --stdin-filename script.bash -

# Override formatting options for a single run
SHUCK_EXPERIMENTAL=1 shucked format --indent-style space --indent-width 2 --function-next-line .
```

`shucked format` uses the same file discovery, exclusion, gitignore, project-root, and cache behavior as `shucked check`, but it only rewrites standalone shell files. Embedded GitHub Actions `run:` blocks are linted by `shucked check`; they are not rewritten by the formatter.

In `--check` and `--diff` modes, `shucked format` exits with status `1` when files would change and leaves the files untouched. Formatter parse errors exit with status `2`.

### Pre-commit

Use the binary-backed hook when you want pre-commit to install `shucked`
without requiring a local Rust toolchain:

```yaml
repos:
  - repo: https://github.com/fredrir/shucked
    rev: v0.0.38
    hooks:
      - id: shucked
```

Replace `v0.0.38` with the release you want to pin.

If you would rather run from the checked-out Rust sources, or you are on a
platform that does not have a published wheel yet, use the source hook instead:

```yaml
repos:
  - repo: https://github.com/fredrir/shucked
    rev: v0.0.38
    hooks:
      - id: shucked-src
```

The `shucked-src` hook requires a working Rust toolchain because it runs
`cargo run -p shucked-cli -- check ...` from the cloned hook repository.

### Clean caches

```sh
# Remove cache entries for the current project
shucked clean
```

### Editor integration

Shucked ships with a first-party Language Server Protocol (LSP) server in the main CLI. Editors and LSP clients should launch it over stdio:

```sh
shucked server
```

The server analyzes the editor's in-memory buffer, publishes diagnostics as you edit, and reuses the same parser, lint rules, formatter settings, configuration, and fix machinery as the CLI. It currently supports incremental document sync, real-time diagnostics, quick fixes, `source.fixAll.shucked`, disable-this-line actions, whole-document and range formatting, hover help for rule codes in `# shucked:` and `# shellcheck` directives, completion, go-to-definition, references, document highlights, call hierarchy (incoming and outgoing calls for functions, across files connected by `source` statements and `# shucked: source=` directives), document symbols, and workspace symbols.

Any editor that can launch a stdio LSP server can use Shucked by pointing shell buffers at `shucked server`. See the [editor integration guide](https://fredrir.github.io/shucked/docs/editors/) for setup examples.

## Output

`shucked check` prints rich code-frame diagnostics by default:

```
warning[C001]: variable `tmp` is assigned but never used
 --> deploy.sh:14:1
  |
14 | tmp=$(mktemp)
  | ^^^
  |
```

Use `--output-format concise` for a compact one-line format:

```
path:line:col: severity[CODE] message
```

```
deploy.sh:14:1: warning[C001] variable `tmp` is assigned but never used
deploy.sh:31:10: error[C006] undefined variable `DEPLY_ENV`
deploy.sh:45:3: warning[S005] prefer `$(...)` over backtick command substitution
.github/workflows/ci.yml:12:11: warning[C001] jobs.test.steps[0].run: variable `summary` is assigned but never used
```

### Exit codes

| Code | Meaning                                    |
| ---- | ------------------------------------------ |
| `0`  | No issues found                            |
| `1`  | Lint violations or parse errors detected   |
| `2`  | Runtime error (bad arguments, I/O failure) |

## Rules

Shucked ships with rules organized into five categories:

| Category    | Prefix | Description                                                                                         |
| ----------- | ------ | --------------------------------------------------------------------------------------------------- |
| Correctness | C      | Bugs, errors, and likely mistakes. Enabled by default.                                              |
| Style       | S      | Code quality and best-practice suggestions.                                                         |
| Performance | P      | Inefficient patterns that have simpler or faster alternatives.                                      |
| Portability | X      | Bash-isms and shell-specific constructs that break under POSIX or other shells.                     |
| Security    | K      | Potentially dangerous shell patterns such as risky deletion, unsafe evaluation, or local expansion. |

Each rule has a short code (e.g., `C006`, `S001`) that appears in diagnostics and can be used in suppression directives. Diagnostics are classified as error, warning, or hint depending on severity.

### ShellCheck compatibility

Where possible, shucked rules align with ShellCheck rules. Shucked supports ShellCheck suppression syntax (`# shellcheck disable=SC2086`) and maps ShellCheck codes to their shucked equivalents, so existing suppression comments continue to work without changes. Both suppression syntaxes accept either code namespace, and native `# shucked: disable=...` follows ShellCheck's scope rules: before the first statement it is file-wide, otherwise it applies to the next command.

That said, shucked is not a port of ShellCheck. It is a clean-room reimplementation built on its own parser and analysis engine, so results will sometimes differ:

- Shucked's parser and analysis logic were written from scratch. Edge cases may be handled differently, and some diagnostics may fire in slightly different locations or contexts.
- In cases where ShellCheck's behavior appears incorrect or inconsistent with shell semantics, shucked intentionally chooses correctness over compatibility.

Compatibility is continuously validated against a large corpus of shell scripts from popular open-source projects including [acme.sh](https://github.com/acmesh-official/acme.sh), [oh-my-zsh](https://github.com/ohmyzsh/ohmyzsh), [nvm](https://github.com/nvm-sh/nvm), [pyenv](https://github.com/pyenv/pyenv), [pi-hole](https://github.com/pi-hole/pi-hole), [bats-core](https://github.com/bats-core/bats-core), [powerlevel10k](https://github.com/romkatv/powerlevel10k), [dokku](https://github.com/dokku/dokku), [gentoo](https://github.com/gentoo/gentoo), and others. The latest conformance report is published at [fredrir.github.io/shucked/reports/corpus](https://fredrir.github.io/shucked/reports/corpus/).

## Suppression

Suppress diagnostics with inline comments. Both native and ShellCheck-style directives are supported.

```sh
# Suppress a specific rule for the next command
# shucked:disable=C001
unused_var="ok"

# Suppress multiple rules
# shucked:disable=C001,S001
code_here

# Suppress for the entire file (place anywhere)
# shucked:disable-file=S001,S002

# ShellCheck-compatible syntax (also works)
# shellcheck disable=SC2034,SC2086

# Code aliases are interchangeable in either style
# shucked: disable=SC2086
# shellcheck disable=S001

# Before the first statement, disable becomes file-wide
# shucked: disable=S001
```

For embedded GitHub Actions scripts, put suppression comments inside the `run:` block as shell comments:

```yaml
- run: |
    # shellcheck disable=SC2086
    echo $FOO
```

YAML comments outside the `run:` scalar are not visible to the shell parser and do not suppress shell diagnostics.

## Sourced files

When a `source`/`.` path is computed (for example `source "$DIR/lib.sh"`), shucked
usually cannot resolve it statically and reports it as an untracked source.
Current-file anchors such as `$(dirname "${BASH_SOURCE[0]}")/lib.sh` are
recognized conservatively. For other computed paths, point shucked at the real
file with a hint comment on (or just above) the `source` line:

```sh
# Import the file's definitions so references resolve, and stop the
# untracked-source warning. The file itself is not linted.
# shucked: source=lib/util.sh
source "$DIR/util.sh"

# Same, and also lint the target as an additional input.
# shucked: source=lib/util.sh lint=true
source "$DIR/util.sh"

# Nothing to include here; just silence the warning.
# shucked: source=/dev/null
source "$maybe_present"
```

The path is resolved relative to the annotating file's directory, then against
any configured `source-paths`; the nearest match wins. A `lint=true` target is
linted like a directly checked file: its own `source` statements are imported
for symbol resolution, and nested `lint=true` directives inside it are honored
transitively. The ShellCheck-compatible `# shellcheck source=<path>` directive
is recognized too.

Configure resolution and target linting in `.shucked.toml`:

```toml
[lint]
# Extra directories (relative to the project root) searched when resolving
# `# shucked: source=` directive targets.
source-paths = ["lib", "scripts"]

# When false, lint=true directives only import symbols; the targets are not
# linted. Default: true.
lint-sources = true
```

### Resolving a file that isn't next to the script

When the target lives somewhere the annotating file can't reach relatively — a
shared `lib/` while the script sits in `scripts/` — give the hint just the
**file name** and add its directory to `source-paths`:

```sh
# scripts/deploy.sh
# shucked: source=util.sh lint=true
source "$SHARED_DIR/util.sh"
greet   # defined in lib/util.sh
```

```toml
# .shucked.toml (at the project root)
[lint]
source-paths = ["lib"]
```

Resolution tries the hint against the annotating file's own directory first
(`scripts/util.sh`), then each `source-paths` root (`lib/util.sh` — found). The
hint may be a bare name (`util.sh`), a subpath (`net/http.sh`, joined onto each
root), or an absolute path (used as-is). `source-paths` roots are relative to
the project root; the token `SCRIPTDIR` means the annotating file's directory.
This works the same in `shucked check` and in the editor (LSP call hierarchy).

## Configuration

Project settings live in `.shucked.toml` or `shucked.toml`. Shucked walks up from each
input to find the nearest of these files and treats that directory as the project
root.

If no project config is found, Shucked falls back to a user-level global config at
`~/.config/shucked/shucked.toml` (or `.shucked.toml`), honoring `XDG_CONFIG_HOME` when
set. Set `SHUCK_CONFIG_HOME` to point at a different directory for the global
config. A project config always takes precedence over the global one, and
`--config <file>` or `--isolated` bypass global config entirely.

Use the `[check]` section to control embedded-script extraction:

```toml
[check]
# Lint supported embedded shell scripts in non-shell files such as
# GitHub Actions workflows and composite actions.
# Default: true
embedded = true

[per-file-shell]
# Override shell dialect inference for matching files in check, format, and LSP.
"scripts/bash/**" = "bash"
"vendor/**/*.sh" = "sh"
"dot_z*" = "zsh"

[format]
# Configure `shucked format` and editor formatting.
exclude = ["generated/**"] # project-relative globs that should never be formatted
indent-style = "tab"       # tab | space
indent-width = 4           # used when indent-style = "space"
binary-next-line = false   # put binary operators on continuation lines
switch-case-indent = false # indent case branch bodies
space-redirects = false    # add spaces around redirection operators
keep-padding = false       # preserve safe horizontal padding
function-next-line = false # put function opening braces on their own line
never-split = false        # prefer compact layouts
```

`format.exclude` applies to directory discovery, explicitly named files, and
whole-document or range formatting requested through the language server. This
makes it suitable for generated files that must remain untouched even when an
editor has format-on-save enabled.

`per-file-shell` patterns are resolved relative to the project root and apply to
`shucked check`, direct and stdin formatting, and editor analysis/formatting.
Supported values are `sh`, `bash`, `dash`, `ksh`, `mksh`, and `zsh`; generic
`ksh` mappings remain available to the linter but cannot be used by the
formatter. Overlapping patterns may select the same dialect, but selecting
different dialects for one file is an error. Existing
`[lint].per-file-shell` and `[lint].extend-per-file-shell` settings remain
supported as compatibility aliases.

An explicit `shucked format --dialect bash|posix|mksh|zsh` override takes
precedence over `per-file-shell`. Unmatched files continue to infer their
dialect from the file name, shebang, and source.

## File discovery

When given a directory, shucked recursively discovers standalone shell scripts by:

1. **Extension**: `.sh`, `.bash`, `.zsh`, `.ksh`, `.dash`, `.mksh`, `.bats`
2. **Shebang**: files starting with `#!/bin/bash`, `#!/usr/bin/env sh`, etc.

Shucked also discovers embedded shell in supported non-shell files:

1. **GitHub Actions workflows**: `.github/workflows/*.yml` and `.github/workflows/*.yaml`
2. **Composite actions**: `action.yml` and `action.yaml`

For GitHub Actions files, shucked lints `run:` blocks independently, remaps diagnostics back to the host YAML file, and includes the step path (for example `jobs.test.steps[0].run`) in the message. Steps that target unsupported shells such as PowerShell or `cmd` are skipped.

The following directories are skipped by default: `.git`, `.hg`, `.svn`, `.jj`, `.bzr`, `.cache`, `node_modules`, `vendor`, `.shucked_cache`.

Gitignore and `.ignore` files are respected by default. Use `--no-respect-gitignore` to disable.

## Caching

Shucked caches lint and format results per file in a shared cache root outside the project tree by default. The default location follows the OS cache directory convention, which is typically `~/Library/Caches/shucked` on macOS and `$XDG_CACHE_HOME/shucked` or `~/.cache/shucked` on Linux.

Override the cache root with `--cache-dir` or `SHUCK_CACHE_DIR`.

Disable caching with `--no-cache` or remove a project's cache entries with `shucked clean [PATH]`.

## Development

Shucked uses [just](https://github.com/casey/just) as its primary task runner and provides a high-performance development toolkit crate under `tooling/` (`shucked-tooling`).

Common commands:

```sh
# Build the project (fast iteration)
just build

# Run unit and integration tests
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

See [CONTRIBUTING.md](CONTRIBUTING.md) for full development workflows and guidelines.

## Rust API

Shucked's parser and analysis crates are published for embedders. Their supported pre-1.0
integration path and selective compatibility guarantees are documented in
[Rust API compatibility](docs/rust-api-compatibility.md).

## Acknowledgements

Shucked builds on ideas and inspiration from several excellent open-source projects. This section is a thank-you to those communities — it does not imply endorsement, affiliation, or any formal relationship between shucked and these projects.

- **[bashkit](https://github.com/everruns/bashkit)** — shucked-parser was originally forked from bashkit's bash lexer and parser; it has since evolved substantially to meet the needs of a linter (comment and trivia preservation, error recovery, multi-dialect parse views, extended AST coverage).
- **[Ruff](https://github.com/astral-sh/ruff)** — Linter architecture inspiration, particularly around caching, rule organization, and diagnostic output.
- **[ShellCheck](https://github.com/koalaman/shellcheck)** — An amazing project and the original source of inspiration for shucked. ShellCheck set the standard for shell script analysis.
- **[gbash](https://github.com/fredrir/gbash)** — A lot of lessons learned from this earlier project carried forward into shucked.

## License

MIT
