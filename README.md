# shucked

A fast shell script linter, formatter, and language server, written in Rust. **With Top IDE / Editor Integration**

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

WiP

## Usage

```sh
-- **Format** `shucked format`
-- **Check** `shucked check`
- **Clean caches** `shucked clean`
- ** Editor integration** ´shucked server`
```

## Rules

Shucked ships with rules organized into five categories:

| Category    | Prefix | Description                                                                                         |
| ----------- | ------ | --------------------------------------------------------------------------------------------------- |
| Correctness | C      | Bugs, errors, and likely mistakes. Enabled by default.                                              |
| Style       | S      | Code quality and best-practice suggestions.                                                         |
| Performance | P      | Inefficient patterns that have simpler or faster alternatives.                                      |
| Portability | X      | Bash-isms and shell-specific constructs that break under POSIX or other shells.                     |
| Security    | K      | Potentially dangerous shell patterns such as risky deletion, unsafe evaluation, or local expansion. |


## Configuration & Defaults

```toml
[check]
embedded = true

[per-file-shell]
"scripts/bash/**" = "bash"
"vendor/**/*.sh" = "sh"
"dot_z*" = "zsh"

[format]
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
## Development

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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for full development workflows and guidelines.

## Rust API

Shucked's parser and analysis crates are published for embedders. Their supported pre-1.0
integration path and selective compatibility guarantees are documented in
See [Rust API compatibility](docs/rust-api-compatibility.md).

## Acknowledgements

Shucked builds on ideas and inspiration from several excellent open-source projects. This section is a thank-you to those communities — it does not imply endorsement, affiliation, or any formal relationship between shucked and these projects.

- **[bashkit](https://github.com/everruns/shuck)**

- **[bashkit](https://github.com/everruns/bashkit)** — shucked-parser was originally forked from bashkit's bash lexer and parser; it has since evolved substantially to meet the needs of a linter (comment and trivia preservation, error recovery, multi-dialect parse views, extended AST coverage).
- **[Ruff](https://github.com/astral-sh/ruff)** — Linter architecture inspiration, particularly around caching, rule organization, and diagnostic output.
- **[ShellCheck](https://github.com/koalaman/shellcheck)** — An amazing project and the original source of inspiration for shucked. ShellCheck set the standard for shell script analysis.
- **[gbash](https://github.com/fredrir/gbash)** — A lot of lessons learned from this earlier project carried forward into shucked.

## License

MIT
