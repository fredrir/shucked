# Semantic navigation, highlighting and shell awareness: findings and roadmap

Status: findings from a code-level review of the language server and the VS Code
extension, the changes made on this branch, and a prioritised roadmap for the
rest. Companion to [shell-intelligence.md](shell-intelligence.md), which tracks
the environment-aware command intelligence programme.

## Symptoms that triggered the review

| Symptom                                                                                     | Root cause                                                                                                                                                                                             |
| ------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| CMD+click on `SAVEHIST=100000` shows "Workspace references are incomplete: some source effects could not be followed" | `WorkspaceVariableIndex::explain` flagged the query incomplete when *any* persistent `source` in the file family was unresolved, regardless of the variable, its position, or whether the source ran before the assignment |
| Go to Definition returns nothing for variables in most startup files                        | `WorkspaceVariableIndex::definitions` fails closed (`None`) when a conditional or unresolved top-level `source` precedes the cutoff, and the definition handler mapped `None` to "no definition" instead of the document-local binding |
| `source ~/.fzf.zsh`, `source $ZSH/oh-my-zsh.sh` count as "could not be followed"           | `~` is never expanded; unquoted `$VAR/tail` operands are classified `Dynamic` even for zsh, which does not word-split them; `HOME` is only seeded for files literally named `.zshrc`                     |
| No Go to Implementation / Go to Declaration                                                 | Neither capability was advertised or handled                                                                                                                                                             |
| `[[ -`, `[ -`, `test -` show no completions                                                 | No completion context for test operators, no static table, and no provider registration for `[[`/`[`; the zsh worker even receives `'[['` quoted                                                       |
| `brew `, `docker `, `ls -`, `eza -` show nothing after the first `source`/`cd`/`autoload` line, or inside functions | `CommandSiteFacts::environment_uncertain` (set by any earlier `source`, `eval`, `cd`, `autoload`, PATH edit, or function body) makes `grammar_allowed` false, which switches native and live completion off for the rest of the file |
| Completion popup closes after the first slow completer                                      | A timeout or cancellation kills the persistent zsh worker; the next request pays a cold `compinit -D`; a failure yields an empty *complete* list which closes the popup                                 |
| Flags stop completing when PATH has a relative/empty entry or under Portable policy        | Such entries make resolution `Unknown`, and any `Unknown` disables argument completion                                                                                                                  |
| Highlighting misses `[[ ]]`, redirects, `select`/`time`/`coproc` bodies; strings swallow `$(...)` | `AstCollector` skips those nodes; unclassified text inside `"..."` is painted `string` on top of the TextMate grammar; overlap resolution had no priority tiebreak                                     |
| "Local shell" awareness feels broken on macOS                                               | The server only ever sees the PATH inherited from VS Code; the user's zsh is never consulted unless a Shucked terminal is attached, and that terminal is not a login shell (`~/.zprofile` skipped)      |

## Changes on this branch

### Navigation and references

- `crates/shucked-semantic/src/workspace_variables.rs`: `explain` now marks a
  query incomplete only when an unfollowed `source` could actually observe the
  selected binding (it runs after the assignment, before a later definite write,
  and is not shadowed by its loader). The offending sites are returned as
  `unfollowed_sources`.
- `crates/shucked-lsp/src/handlers/workspace_functions.rs`: navigation no longer
  fails closed on a partial index or an ambiguous source graph; exact reaching
  definitions are preferred, possible definitions are the fallback, and the
  request handler falls back to the document's own binding after that.
- References show one precise notice per situation ("References to `SAVEHIST`
  may be incomplete: 1 source operation could not be followed while the value is
  visible (.zshrc:12 `source "$ZSH/oh-my-zsh.sh"` (runtime expression))"), once
  per session; repeats go to the log. Hover lists the same sites under
  "Possible hidden reads".
- New `textDocument/declaration` (declaration builtins, function definitions,
  `source` targets) and `textDocument/implementation` (every candidate function
  body including redefinitions, every file loaded by a `source` operand, the
  script behind an external command, every possible assignment of a variable).
- Go to Definition on a `source` operand opens the loaded file(s); on an external
  command implemented by a shebang script it opens the script.
- Hover on zsh configuration parameters (`SAVEHIST`, `HISTSIZE`, `HISTFILE`,
  `ZDOTDIR`, `WORDCHARS`, `KEYTIMEOUT`, prompts, hook arrays, ...) explains what
  the runtime does with the value.

### Completion

- Test operators: `[[ -`, `[ -`, `test -`, `[[ ! -`, `[[ $a -`, `[[ -f x && -`
  and `[ -f x -a -` complete from a static table with a description per
  operator, dialect-aware (`=~`, `<`, `>`, `&&`, `||` only in `[[`; `-a`/`-o`
  connectors only in `[`/`test`; POSIX flagging for `sh`). The result is
  complete immediately; no shell is consulted.
- Native completion gating, worker lifetime, `compinit` caching, Homebrew
  environment, `fpath` discovery and PATH-entry handling: see the worker report
  recorded in the commit message and the tests under
  `crates/shucked-lsp/tests/completion/`.

### Source resolution

- `~/` expansion, unquoted `$VAR/tail` for zsh, `HOME`/`XDG_*`/`ZDOTDIR` seeding
  for every zsh and bash file, and `${ZDOTDIR:-$HOME}`-style defaults.

### Highlighting

- Semantic tokens cover every compound command, redirects, test operators and
  brackets, expansion operators and delimiters, numbers, special parameters,
  options (`shellOption`), zsh modifiers, wrapper words; strings no longer paint
  over expansions; overlap resolution is deterministic; fish gets comments,
  keywords and numbers. Scopes for builtins and options are mapped in the
  extension, and semantic highlighting is enabled for the `bash`, `zsh`, `sh` and
  `ksh` language ids.

### Environment

- `login-shell` environment policy (opt-in, trusted workspaces only): the server
  captures PATH, aliases, functions and options from the user's login shell once
  per rc-file fingerprint and treats them like an attached terminal. Shucked
  terminals start as login shells. `tracing.logLevel` is forwarded from the
  extension, and `shucked.showEnvironmentDetails` explains trust, policy,
  provider root, engines and the resolution of the command under the cursor.

## Roadmap

Priorities: P0 is done on this branch, P1 is the next increment, P2 is
worthwhile but can wait.

### Semantic model and navigation

| Pri | Proposal                                                                                                                                                                                                                                                                  | Where                                                                                     |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| P1  | Index files that *source the current file* from outside the workspace roots (e.g. `~/.zshrc` sourcing a workspace file). Today only transitive targets are pulled in; incoming loaders outside the roots are invisible, so inherited definitions are missed.               | `workspace_functions.rs::build_projections`, `discover_closed_shell_files`               |
| P1  | Use the oh-my-zsh / zinit / prezto plugin-manager resolvers that the linter already has for the LSP index, so `source $ZSH/oh-my-zsh.sh` and plugin loads resolve to real files.                                                                                           | `source_closure/plugin_managers/`, `WorkspacePathProvider`                                |
| P1  | Rename across files for variables when the family is proven; today only functions have cross-file rename.                                                                                                                                                                 | `cross_file_rename.rs`, `WorkspaceVariableIndex::references`                              |
| P1  | Model `autoload`ed functions: resolve `autoload -Uz name` to the file on `$fpath` and offer it for definition/implementation; also `zstyle`/`bindkey` widget names.                                                                                                        | `builder/special_builtins.rs`, `handlers/zsh.rs`, workspace index                          |
| P2  | Treat `builtin source x` / `command . x` as source effects (currently ignored).                                                                                                                                                                                             | `builder/special_builtins.rs`                                                             |
| P2  | Contract-aware LSP models: apply the linter's ambient contracts (`contracts/zsh/config.yaml`) so runtime-consumed names get a "consumed by zsh" reference instead of "no references".                                                                                      | `crates/shucked-lsp/src/editor.rs` (`resolve_source_closure`, contracts)                  |
| P2  | Type definition request mapping array/assoc/integer declarations to their `typeset` site.                                                                                                                                                                                  | new `type_definition.rs`                                                                  |

### Completion and shell awareness

| Pri | Proposal                                                                                                                                                                                                                                | Where                                                                                     |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| P1  | Recognise more completion contexts statically: `setopt`/`unsetopt` options already exist; add `case` patterns, `for … in` word lists, function names after `unfunction`/`autoload`, `zstyle` contexts, `bindkey` widgets, `trap` signals, `kill -`, `ulimit -`, `read -`, `printf` formats. | `completion/context.rs`, semantic `editor.rs::completion_context`                        |
| P1  | Argument descriptions for the validator grammars (ls, eza, docker, rg, fd, bat, curl, ssh, kubectl) so flag completion works offline and instantly, with the native completer as enrichment rather than the only source.                   | `crates/shucked-command/data/validators/*.json` (add `description`), `completion/native.rs` |
| P1  | Subcommand completion for brew/git/docker/kubectl from a cached one-shot inventory (`brew commands`, `git --list-cmds`, `docker --help`) refreshed by the environment watcher; the resolver already runs these for validation.             | `crates/shucked-command/src/metadata.rs`, `completion/native.rs`                          |
| P1  | Keep the live-completion channel off the Electron-as-node path: a persistent helper (one process per terminal) instead of spawning node three times per request inside a 1.3 s budget.                                                   | `editors/vscode/shell-integration/live-*.{zsh,sh,fish}`, `capture.cjs`, `live-completion.ts` |
| P1  | Accept aliases with option words (`--icons=auto`) as aliases instead of opaque functions in the terminal capture, so `eza -` completes with the user's alias.                                                                            | `capture.cjs::simpleAlias`, `commands.rs::update_session`                                 |
| P2  | Ship a darwin-x64 runtime lock (or a universal build) so Intel Macs and Rosetta VS Code get the provider bundle; surface "no provider bundle" in the status bar instead of a debug log.                                                    | `tooling/providers/bundle-runtime.py`, `native_assets.rs::root`, `status.ts`              |
| P2  | Remove the dead `useShellConfig`/`SHUCKED_NATIVE_PERSONAL` path or wire it to the login-shell policy.                                                                                                                                    | `options.rs`, `background.rs`, `zsh_supervisor.zsh`                                        |

### Highlighting

| Pri | Proposal                                                                                                                                                              | Where                                                                  |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| P1  | Range and delta semantic tokens with a per-document `result_id` cache; large startup files re-tokenise on every keystroke today.                                        | `capabilities.rs`, `requests/semantic_tokens.rs`                        |
| P1  | Add `elif`/`else`/`do`/`then` spans to the AST so keyword tokens never rely on text search.                                                                             | `shucked-ast/src/ast.rs`, parser                                        |
| P1  | Ship a fish TextMate grammar and `language-configuration.json` (comment toggling, brackets) so fish is usable without a third-party extension.                         | `editors/vscode/package.json`, new `syntaxes/fish.tmLanguage.json`     |
| P2  | Move the formatter's `AstVisitor` into `shucked-ast` and use it for tokens, inlay hints and folding so traversal holes cannot reappear.                                | `shucked-formatter/src/visit`, `shucked-ast`                            |
| P2  | Escape-sequence tokens inside `$'...'` and `printf` formats; brace-expansion and glob tokens.                                                                          | `semantic_tokens.rs::AstCollector`                                      |

### Environment model

| Pri | Proposal                                                                                                                                                                               | Where                                                                       |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| P1  | Make the login-shell capture the default for startup files (`.zshrc`, `.zshenv`, `.bashrc`, ...) in trusted workspaces, since their execution context *is* the interactive shell.       | `commands.rs` startup-file detection, `environment_options.rs`              |
| P1  | Re-resolve PATH on `refreshEnvironment` and on rc-file watcher events instead of freezing it at launch.                                                                                | `session.rs::refresh_environment`, `environment_watcher.rs`                 |
| P2  | Per-document execution context inference from the shebang and `#!/usr/bin/env -S` lines, including `zsh -f` (no rc files) versus interactive.                                          | `commands.rs::ExecutionContext`                                              |
| P2  | Windows: a custom-completer transport that does not depend on Unix signals.                                                                                                             | `live_completion.rs`, `live-completion.ts`                                   |

## Validation gaps

- No macOS acceptance run exists in CI; the completion fixes above were validated
  with the Linux toolchain and unit tests. A macOS arm64 and an Intel run of
  `tests/editors/vscode` and `tests/lsp/test_native_completion.py` should gate
  the next release.
- The "no provider bundle" state cannot be forced in tests because the debug
  build always finds `tooling/providers` at compile time; add an environment
  override (`SHUCKED_PROVIDER_ROOT=/nonexistent`) case.
- Real oh-my-zsh, prezto and zinit startup files should be added as fixtures
  for the navigation and source-resolution tests.
