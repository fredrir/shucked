# Shucked for VS Code

Shell diagnostics, completion, formatting, navigation, and execution-context awareness.

## Execution context

| Context | Behavior |
|---|---|
| Workspace (default) | Resolve commands against the workspace host's exact inherited `PATH` |
| Portable | Keep syntax and document analysis; suppress host-dependent absence warnings |
| Captured target | Use an explicitly selected inventory offline; do not run local providers for it |
| Attached terminal | Use that session's aliases, functions, options, `PATH`, and working directory |
| Startup file | Analyze definitions in source order; do not treat post-startup state as entry state |
| Launch directory | Workspace directory is an assumption; select an explicit directory when needed |
| Untitled document | One workspace is selected automatically; use Associate workspace in multi-root windows |

Use **Shucked: Select Execution Context** or the context status item. Per-document selections override workspace settings. Session attachment is explicit and temporary.

| Setting | Default |
|---|---|
| `shucked.environment.policy` | `workspace` (`portable` opt-in) |
| `shucked.environment.cwd` | Empty; assumed workspace directory |
| `shucked.environment.targetInventory` | Empty |
| `shucked.environment.declarations` | Empty command-to-dependency-kind map |
| `shucked.history.session` | `false` |
| `shucked.history.files` | `false` |
| `shucked.server.path` | Empty; bundled platform server |
| `shucked.server.extraArgs` | `[]` |
| `shucked.trace.server` | `off` |

`shucked.server.path` expands `~`, `$VAR`, and `${VAR}`. Custom server binaries must support the extension's protocol.

## Completion and diagnostics

| Source | Behavior |
|---|---|
| Commands | Exact target `PATH`, applicable aliases/functions, and shell builtins |
| Symbols | Document and sourced functions, variables, keywords, and shell options |
| Paths | Selected launch directory; quoted/escaped names and supported home expansions |
| Arguments | Bundled definitions plus bounded installed-tool queries |
| Native engines | Managed Zsh, Bash, and Fish adapters; personal dotfiles are unnecessary |
| Live completers | Explicitly attached Unix Bash/Zsh/Fish sessions; current custom functions and variables, bounded and cancellable |
| Packages | Installed package tools and their available metadata; no package installation or database refresh |
| Aliases | Preserve injected arguments and source ranges; standalone scripts do not inherit interactive aliases |
| Missing commands | Debounced warning and invalid semantic classification when absence is established |
| Uncertain context | Unknown; incomplete inventories and dynamic commands do not prove absence |
| Subcommands/flags | Validate only with complete applicable tool evidence; missing suggestions are not errors |
| Typo corrections | Explicit quick fixes, checked again against document/target state; excluded from fix-all |
| Syntax | Parser diagnostics enabled by default; Fish uses its own frontend |
| Refresh | Watched changes, terminal prompts, explicit refresh, and a 30-second host refresh fallback |

| Completion setting | Default |
|---|---|
| `shucked.server.completion.includeEnvironment` | `true` |
| `shucked.server.completion.includePaths` | `true` |
| `shucked.server.completion.includeCommandArguments` | `true` |
| `shucked.server.completion.includeNative` | `true` in trusted workspaces |
| `shucked.server.completion.includeRuntimeNames` | `true` |
| `shucked.server.completion.includeKeywords` | `true` |
| `shucked.server.completion.maxItems` | `200` (1–2000) |
| `shucked.server.completion.useShellConfig` | Deprecated; attach a terminal instead |

Managed completion workers bypass personal startup files. Creating a Shucked terminal starts a real shell with its normal trusted startup configuration. Attaching an existing terminal copies an attachment command for you to run at an idle prompt; it does not inject commands or modify dotfiles. Analysis never executes editor-buffer contents or follows sources by executing them.

History suggestions are separately opt-in for accepted session commands and history files. Attached sessions report their actual history path only when file reads are enabled, including custom `HISTFILE` and Fish namespaces. Entries stay in bounded memory caches; disabling history or disconnecting clears cached and displayed suggestions. Session collection requires an authenticated prompt confirmation.

## Workspace navigation

| Action | Result |
|---|---|
| Hover a variable | Assignment locations and consuming files, including unsaved edits |
| Go to References | Reads of the selected assignment; includes possible reads through known conditional imports and called loaders |
| Hover a source path or `source=` directive | Resolved file, searched paths, or reason resolution stopped |
| Incomplete analysis | Hover labels partial results; References reports incomplete results when requested |
| Analysis limits | Workspace index defaults to `5000` files; source analysis also bounds depth, file count, bytes, and work |

## Project dependencies

Declare expected commands in `.shucked.toml`:

```toml
[environment.commands.codegen]
kind = "generated"
files = ["scripts/**"]
targets = ["deployment"]
```

| Field | Values |
|---|---|
| `kind` | `required`, `optional`, `generated`, `deployment` |
| `files` | Optional workspace-relative glob list |
| `targets` | Optional target-ID list |

Declarations distinguish expected dependencies from spelling mistakes; they do not fabricate an installed executable. Supported availability guards also suppress warnings within the guarded branch.

## Remote workspaces and targets

| Environment | Server and provider host |
|---|---|
| Local | Local workspace extension host |
| Remote SSH | SSH host |
| WSL | WSL distribution |
| Dev Container | Container |
| Other LSP editors | Host running the server |

Install the extension on the workspace host. The VSIX must match that host's platform. Inherited environment changes outside a linked terminal require a server restart; installing/removing files on the existing `PATH` does not.

**Capture Target Inventory** exports a bounded, checksummed inventory. Trusted workspaces also capture audited tool capabilities; untrusted capture reads the executable inventory only. **Compare Target Inventories** compares the active document against selected inventories, preserving Unknown where authoritative grammar is unavailable. CLI capture is filesystem-only unless `--capabilities` is explicit.

```sh
shucked target capture --label deployment --shell bash --capabilities --output deployment.json
shucked target inspect deployment.json
shucked target compare --target deployment.json --target workstation.json script.sh
```

Other editors initialize trusted native execution with `{"nativeExecutionAllowed": true}`. VS Code derives this permission from workspace trust; later workspace settings cannot grant it.

## Commands

| Command | ID |
|---|---|
| Select Execution Context | `shucked.selectEnvironment` |
| Refresh Environment | `shucked.refreshEnvironment` |
| Create Terminal | `shucked.createTerminal` |
| Attach Terminal | `shucked.attachTerminal` |
| Capture Target Inventory | `shucked.captureTarget` |
| Compare Target Inventories | `shucked.compareTargets` |
| Clear History Suggestions | `shucked.clearHistorySuggestions` |
| Restart Language Server | `shucked.restartServer` |
| Show Language Server Logs | `shucked.showOutputChannel` |
| Show Version | `shucked.showVersion` |

## Coverage and distribution

| Area | Current boundary |
|---|---|
| Bundled engines | macOS arm64 tested; other runtime distributions require their platform gates |
| Linux | GNU/musl source-build recipes; target-host execution required before release |
| Windows | Process-tree cancellation implemented; compatible private shell runtime distribution remains outstanding |
| Unix helpers | Some definitions require standard POSIX utilities from the host |
| Tool validation | Brew/Git inventories and selected exact-version flag grammars; unsupported versions remain Unknown |
| Fish | Dedicated syntax/command frontend; not full Bash/Zsh lint-rule or formatting parity |
| Themes | Standard warning diagnostics plus semantic classifications; token color depends on theme |
| Clients without refresh support | Completed results are served on the next diagnostic/token request; immediate no-edit updates require negotiated refresh support |
| Remote validation | Workspace-host architecture; real SSH/WSL/container acceptance runs remain required |

See [provider builds and licenses](../../tooling/providers/README.md) and the [implementation ledger](../../plans/shell-intelligence.md). No full platform-parity claim is implied by the available build recipes.

## Development

| Task | Command |
|---|---|
| Install | `npm ci` |
| Typecheck | `npm run check-types` |
| Lint | `npm run lint` |
| Build | `npm run build` |
| Unit and hook tests | `npm test` |
| Package host VSIX | `npm run vsix` |
| Real editor smoke | `npm run test:extension-host` |
| Installed package smoke | `SHUCKED_TEST_VSIX=/absolute/path/package.vsix npm run test:extension-host` |
