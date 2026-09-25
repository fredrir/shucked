# Shucked Integration Test Suite

Python integration tests managed as one uv project (`tests/pyproject.toml`).

| Directory | Covers |
|---|---|
| `lsp/` | The `shucked` language server over stdio: diagnostics, completion, formatting, code actions, command intelligence |
| `editors/vscode/` | The VS Code extension: contracts, shell hooks, real-editor behaviour, and VSIX packaging ([details](editors/vscode/README.md)) |
| `remote/` | Manual acceptance and benchmark scripts for remote hosts |

```bash
uv run --project tests pytest tests/                      # everything that needs no editor download
uv run --project tests pytest tests/editors/vscode --e2e  # add the real-editor suites
```

The language server tests need `target/debug/shucked` (`cargo build -p shucked-cli`).
