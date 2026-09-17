# Shuck for VS Code

Vscode Extension for using Shuck for Linting, Formatting and LSP for shell scripts

> Requires the shuck binary installed and on `PATH`

## Settings

| Setting               | Default                 | Values                         |
| --------------------- | ----------------------- | ------------------------------ |
| `shuck.path`          | `shuck`                 | Path to the `shuck` executable |
| `shuck.enabledShells` | `["sh", "bash", "zsh"]` | `sh`, `bash`, `zsh`            |

`shuck.path` expands `~`, `$VAR` and `${VAR}`.

Settings changes restart the language server. A failed start surfaces a `Retry` action and logs to the `Shuck` output channel.

## Development

| Task                  | Command               |
| --------------------- | --------------------- |
| Install               | `bun install`         |
| Build                 | `bun run build`       |
| Watch (esbuild + tsc) | `bun run watch`       |
| Typecheck             | `bun run check-types` |
| Lint                  | `bun run lint`        |
| Package               | `bun run package`     |
| VSIX                  | `bun run vsix`        |
| Debug                 | F5 → `Run Extension`  |

## Layout

| Path                | Contents                            |
| ------------------- | ----------------------------------- |
| `src/extension.ts`  | Language client entry point         |
| `esbuild.mjs`       | Bundle config                       |
| `dist/extension.js` | Bundled entry (`main`), git-ignored |
