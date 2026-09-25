# VS Code extension tests

Pytest suites for the Shucked VS Code extension. Unit tests for the extension's
TypeScript modules stay next to the sources in `editors/vscode/tests` and run
with `node --test`; everything that needs real shells, a real editor, or a
packaged VSIX lives here.

```bash
just vscode test                      # unit tests, then contract + shell hook suites
just vscode test --e2e                # also the editor suites (downloads a pinned VS Code once)
just vscode test --vsix /abs/pkg.vsix # editor suites against the installed package, plus packaging checks
just vscode test --build-vsix         # build a VSIX (release build, provider runtimes) and inspect it
just vscode test --e2e -- -k terminal # anything after -- goes to pytest
```

Or directly: `uv run --project tests pytest tests/editors/vscode --e2e`.

## Suites

| Directory | Needs | What it checks |
|---|---|---|
| `contract/` | nothing | Manifest against sources: commands have handlers, settings are declared and forwarded, activation covers every served language, trust restrictions, packaging rules |
| `shell_integration/` | bash, zsh, fish, Node | The shipped hooks in real interactive shells on a pseudo-terminal: prompt metadata, live completion, the watchdog, existing signal handlers |
| `api/` | VS Code, a display | Extension behaviour through the VS Code API: diagnostics, completion, hover, formatting, code actions, settings, terminals, history, trust, lifecycle, multi-root |
| `ui/` | VS Code, a display | The workbench with real key presses: suggestion popup, key bindings, snippets, status bar, command palette, file dialogs |
| `packaging/` | a VSIX | Archive contents, manifest, binaries, shell hooks, and a snapshot of every shipped file |

`contract/` and `shell_integration/` always run. `api/` and `ui/` run with
`--e2e` or `--vsix`, or when you name a path inside them. `packaging/` runs with
`--vsix` or `--build-vsix`. A missing shell skips its tests; `--require-shells`
turns that into a failure.

## How the editor suites work

- **Pinned editor.** `harness/install.py` downloads the manifest's minimum
  `engines.vscode` build into `.cache/vscode-test` (override with
  `--vscode-version`, `SHUCKED_VSCODE_VERSION`, or `--vscode-executable`).
- **Isolation.** Every editor gets a private temporary HOME, profile, extensions
  directory, and a copy of `fixtures/workspaces/default` with the startup files
  from `fixtures/home`. Nothing reads or writes the developer's own editor state.
- **Bridge.** `bridge/runner.cjs` is loaded with `--extensionTestsPath`. It keeps
  the test host alive and serves the extension API over an authenticated
  loopback socket; `harness/bridge.py` is the Python client. Arguments tagged
  with `uri()`, `position()`, `range_()` become API objects, and results come
  back as plain JSON. `evaluate()` exists as an escape hatch; prefer adding a
  bridge method.
- **Workbench.** `harness/workbench.py` attaches Playwright to the editor window
  over the Chrome DevTools Protocol for real key presses, the suggestion
  widget, quick picks, the status bar, and notifications. On Linux the harness
  starts its own Xvfb display; `--headed` uses your `DISPLAY` instead.
- **Development or installed package.** By default the extension loads from
  `editors/vscode` (rebuilt once per session) and runs `target/debug/shucked`
  (or `SHUCKED_TEST_SERVER`). With `--vsix` the package is installed into the
  isolated profile and uses its bundled binaries.
- **Cleanup.** Editors, servers, and shells are stopped as whole process trees;
  on Linux the test process adopts orphaned helpers so nothing outlives a run.
- **Local access.** The bridge requires a per-run secret, but the editor's
  DevTools port (used by Playwright) listens on loopback without one, so any
  local process could drive a test editor while it runs. Run the editor suites
  on single-user machines or CI runners.

## Writing tests

- Use the `editor` fixture for most tests: a scratch folder inside one shared
  editor, reset after every test (settings changed through
  `bridge.update_setting` are restored, editors closed, terminals disposed).
- Use `launch_editor(...)` for anything that changes trust, workspace folders,
  or start-up settings, or that deliberately breaks the server. A workspace you
  pass in is never written to; scratch files go to the editor's private root.
- Wait for state with `wait_until` and the `EditorSession.wait_for_*` helpers;
  never sleep for a fixed time to let the editor catch up. A probe passed to
  `wait_until` must not assert: failures inside it count as "not yet".
- Prefer the API (`api/`) when the extension API can observe the behaviour. Use
  `ui/` for what only the workbench shows: key bindings, popups, pickers.
- A known product bug gets `pytest.mark.xfail(strict=True, reason=...)`, so the
  test starts failing, and has to be updated, once the bug is fixed.

## Failures

Failed editor tests keep a screenshot, the editor's stdout, and VS Code's logs
under `target/vscode-tests/<test name>` (or `--vscode-artifacts DIR`). Add
`--vscode-trace` to record the full language server protocol trace.

## Lint

```bash
uv run --project tests ruff check tests/editors
uv run --project tests ruff format --check tests/editors
uv run --project tests basedpyright -p tests
```
