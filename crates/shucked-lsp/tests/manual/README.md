# Manual Neovim Black-Box Tests

This directory holds the Neovim-backed LSP smoke harness for `shucked server`.

It exercises a real headless Neovim client talking to a real `shucked server`
process over stdio, with a feature-organized fixture workspace staged into a
temporary directory for each run.

The same harness powers the GitHub Actions job named `LSP Integration Tests`.

## Prerequisites

Enter the repo dev shell so the expected runtimes are on `PATH`:



## Run

Run the full smoke suite:

```bash
python3 crates/shucked-lsp/tests/manual/run_neovim_blackbox.py
```

Run a single scenario:

```bash
python3 crates/shucked-lsp/tests/manual/run_neovim_blackbox.py --case diagnostics/open_edit
python3 crates/shucked-lsp/tests/manual/run_neovim_blackbox.py --case code_actions/quick_fix
```

Available scenarios:

- `diagnostics/open_edit`
- `completion/semantic_completion`
- `hover/rule_directive`
- `hover/semantic_symbol`
- `navigation/definition_references_highlights`
- `symbols/document_symbols`
- `symbols/workspace_symbols`
- `rename/same_file`
- `code_actions/quick_fix`
- `code_actions/fix_all`
- `formatting/request_round_trip`
- `configuration/reload_workspace_config`

The runner builds `target/debug/shucked`, stages `fixtures/workspace/` into a
temporary workspace, launches `nvim --headless`, and exits non-zero if either
the Neovim-side assertions or the server transport fail.

The formatting scenario verifies that a real `textDocument/formatting` request
is routed through `shucked-formatter` and applies the returned text edit to the
buffer.
