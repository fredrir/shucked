# Shucked for VS Code

VS Code extension for **Shucked**: Industry-grade Shellscript & ZSH Language Server, Linter, and Formatter.

## Features

- **Rich Language Features**: Diagnostics, code actions, completions, formatting, hovers, navigation, and symbol indexing powered by the Shucked language server.
- **Platform Binary Discovery**: Automatically detects bundled platform binaries, workspace build targets, or system PATH binaries.
- **Resilient Client Management**: Automatic crash-loop protection with exponential backoff and interactive error recovery.
- **Multi-Root Workspace & Remote Ready**: Full support for Remote SSH, WSL, Dev Containers, and multi-root workspaces.

## Settings

| Setting                    | Default | Description                                                                                                                                             |
| -------------------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `shucked.server.path`      | `""`    | Path to custom `shucked` executable. Bundled platform binaries are the standard supported mode; custom binary paths are unsupported and provided as-is. |
| `shucked.server.extraArgs` | `[]`    | Extra CLI arguments passed to `shucked server`.                                                                                                         |
| `shucked.trace.server`     | `"off"` | Traces communication between VS Code and the Shucked language server (`"off"`, `"messages"`, `"verbose"`).                                              |

> `shucked.server.path` expands `~`, `$VAR`, and `${VAR}`.

| Completion setting                                  | Default                     |
| --------------------------------------------------- | --------------------------- |
| `shucked.server.completion.includeEnvironment`      | `true`                      |
| `shucked.server.completion.includePaths`            | `true`                      |
| `shucked.server.completion.includeCommandArguments` | `true`                      |
| `shucked.server.completion.includeNative`           | `true` (trusted workspaces) |
| `shucked.server.completion.useShellConfig`          | `false` (machine setting)   |
| `shucked.server.completion.includeRuntimeNames`     | `true`                      |
| `shucked.server.completion.includeKeywords`         | `true`                      |
| `shucked.server.completion.maxItems`                | `200` (1–2000)              |

## Completion behavior

| Source              | Behavior                                                                                                                      |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Symbols             | Visible variables, functions, sourced functions, builtins, shell keywords, Zsh options                                        |
| Commands            | Server host's `PATH`, then standard system and Homebrew directories; first match wins                                         |
| Variables           | Inherited environment names; values are not included in suggestions                                                           |
| Paths               | Script directory; server working directory for untitled buffers                                                               |
| Path syntax         | Absolute/relative paths, `~/`, `$HOME/`, `${HOME}/`, quoted and escaped names                                                 |
| Directories         | Directory-only suggestions for `cd`, `pushd`, and `rmdir`                                                                     |
| Arguments           | Bundled flags and subcommands for Git, Homebrew, Pacman, curl, SSH, Docker, kubectl, common file tools, and selected builtins |
| Native flags        | Installed `ls`, `gls`, `eza`, `exa`, `rg`, `fd`, and `bat` help output; managed Zsh completion fallback                       |
| Packages            | `pacman -S` uses repository package names; `pacman -R` / `-Q` use installed names                                             |
| Homebrew            | `brew install` suggests formulae and casks; `--formula` / `--cask` narrow the list; removal and upgrade use installed names   |
| Descriptions        | Native help/completion descriptions appear beside flags                                                                       |
| Personal aliases    | Optional `useShellConfig` loads Zsh login/interactive startup files for flag completion                                       |
| Edits               | Replace the word suffix; negotiate insert/replace edits with the editor                                                       |
| Limits              | Bounded directory caches and result counts; partial lists request further completion                                          |
| Refresh             | Directories: 2 seconds; native queries: 60 seconds; Zsh: 30 seconds; watched-file changes invalidate caches                   |
| Execution           | Trusted workspaces only; fixed help/package queries and Zsh completion scripts, with deadlines and cancellation               |
| Default shell setup | Zsh starts with `-f`; Shucked initializes completion without `.zshrc`, plugins, or a completion dump                          |
| Native platforms    | Unix workspace hosts; other platforms retain bundled metadata, paths, and symbols                                             |

The providers ship inside the server binary bundled with the extension. Homebrew and Pacman completion requires the corresponding tool and its package metadata on the workspace host; Zsh is not required for those providers. Missing tools, unavailable metadata, failed queries, and timeouts fall back to bundled metadata and paths.

Flags reflect the installed executable: macOS `ls`, GNU `ls`, and `eza` have different options. Personal aliases such as `ls=eza` are used only with `useShellConfig` enabled. The default needs no personal shell configuration.

Completion never runs the edited command, installs packages, or explicitly refreshes package databases. Package names and help are queried through fixed arguments. Personal shell configuration, when enabled, runs its normal startup code.

## Remote workspaces

| Environment                              | Server and completion source   |
| ---------------------------------------- | ------------------------------ |
| Local                                    | Local workspace extension host |
| Remote SSH                               | SSH host                       |
| WSL                                      | WSL distribution               |
| Dev Container                            | Container                      |
| Other LSP editors                        | Host running `shucked-server`  |
| Virtual filesystem without a native host | Unsupported                    |

Install the extension on the workspace host. VSIX packages target that host's OS, architecture, and Linux/Alpine family. Incompatible bundled binaries are skipped; discovery continues through build artifacts and `PATH`.

`PATH`, home, and environment names are captured when the server starts. Restart the language server after changing its inherited environment. An editor launched from a desktop session can inherit a different environment from an interactive terminal.

```json
{
  "nativeExecutionAllowed": true,
  "server": {
    "completion": {
      "includeEnvironment": true,
      "includePaths": true,
      "includeCommandArguments": true,
      "includeNative": true,
      "useShellConfig": false,
      "maxItems": 200
    }
  }
}
```

Other editors pass this object as LSP initialization options for trusted workspaces. VS Code sets `nativeExecutionAllowed` from workspace trust automatically. Later workspace configuration cannot grant this permission; granting VS Code trust restarts the server.

## Commands

- **Shucked: Restart Language Server** (`shucked.restartServer`)
- **Shucked: Show Language Server Logs** (`shucked.showOutputChannel`)
- **Shucked: Show Version** (`shucked.showVersion`)

## Status Bar

The status bar displays the current state of Shucked:
- `$(sync~spin) Shucked: Starting` — Language server is starting.
- `$(check) Shucked` — Ready and idle.
- `$(sync~spin) Shucked: Indexing` — Indexing files or analyzing workspace.
- `$(error) Shucked: Error` — An error occurred or the server failed to start (click to restart or view logs).

## Development

| Task                 | Command               |
| -------------------- | --------------------- |
| Install dependencies | `bun install`         |
| Typecheck            | `bun run check-types` |
| Lint                 | `bun run lint`        |
| Build                | `bun run build`       |
| Watch                | `bun run watch`       |
| Platform tests       | `bun run test`        |
| Package host VSIX    | `bun run vsix`        |

## Architecture

| File               | Responsibility                                                                        |
| ------------------ | ------------------------------------------------------------------------------------- |
| `src/extension.ts` | Extension activation, log channel creation, workspace event dispatching.              |
| `src/binary.ts`    | Resolves binary through custom path, bundled binary, workspace artifacts, or PATH.    |
| `src/client.ts`    | LanguageClient lifecycle, error handler, crash-loop detection, and progress handling. |
| `src/status.ts`    | Status bar item state management and interactions.                                    |
| `src/commands.ts`  | Command registrations (`restartServer`, `showOutputChannel`, `showVersion`).          |
| `src/config.ts`    | Configuration watcher and live server reload.                                         |
| `platform.mjs`     | Native executable and host-platform detection                                         |
| `vsix.mjs`         | Package/publish a VSIX for the current host                                           |
