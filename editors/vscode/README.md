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
| Package VSIX         | `bun run vsix`        |

## Architecture

| File               | Responsibility                                                                        |
| ------------------ | ------------------------------------------------------------------------------------- |
| `src/extension.ts` | Extension activation, log channel creation, workspace event dispatching.              |
| `src/binary.ts`    | Resolves binary through custom path, bundled binary, workspace artifacts, or PATH.    |
| `src/client.ts`    | LanguageClient lifecycle, error handler, crash-loop detection, and progress handling. |
| `src/status.ts`    | Status bar item state management and interactions.                                    |
| `src/commands.ts`  | Command registrations (`restartServer`, `showOutputChannel`, `showVersion`).          |
| `src/config.ts`    | Configuration watcher and live server reload.                                         |
