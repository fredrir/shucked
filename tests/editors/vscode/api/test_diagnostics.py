"""Diagnostics published through the VS Code client, and the settings that shape them."""

from __future__ import annotations

import pytest

from ..harness.session import EditorSession
from ..harness.waiting import stays_false, wait_until

ERROR = 0  # vscode.DiagnosticSeverity.Error


def test_missing_command_is_reported_for_the_workspace_host(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.zsh")
    diagnostic = editor.wait_for_diagnostic(uri, "ENV001", line=2)
    assert "shucked_missing_smoke" in diagnostic["message"]
    assert diagnostic["source"] == "shucked"


def test_portable_policy_suppresses_host_checks_without_restart(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.zsh")
    editor.wait_for_diagnostic(uri, "ENV001", line=2)
    editor.bridge.update_setting("shucked", "environment.policy", "portable", target="workspace")
    editor.wait_without_diagnostic(uri, "ENV001")
    editor.bridge.update_setting("shucked", "environment.policy", "workspace", target="workspace")
    editor.wait_for_diagnostic(uri, "ENV001", line=2)


def test_declared_dependency_is_reported_as_a_declaration(editor: EditorSession) -> None:
    uri = editor.open("declared.sh", "#!/bin/bash\nshucked_declared_tool --version\n")
    editor.wait_for_diagnostic(uri, "ENV001", line=1)
    editor.bridge.update_setting("shucked", "environment.declarations", {"shucked_declared_tool": "required"}, target="workspace")
    diagnostic = editor.wait_for_diagnostic(uri, "ENV004", line=1)
    assert "shucked_declared_tool" in diagnostic["message"]
    assert "ENV001" not in editor.bridge.diagnostic_codes(uri)


def test_lint_rules_are_published(editor: EditorSession) -> None:
    uri = editor.open("unused.sh", "#!/bin/bash\nunused_variable=42\n")
    diagnostic = editor.wait_for_diagnostic(uri, "C001", line=1)
    assert "unused_variable" in diagnostic["message"]


@pytest.mark.xfail(strict=True, reason="the server parses lint.enable but never reads it (RawLintOptions.enable)")
def test_disabling_lint_clears_rule_diagnostics(editor: EditorSession) -> None:
    uri = editor.open("unused.sh", "#!/bin/bash\nunused_variable=42\n")
    editor.wait_for_diagnostic(uri, "C001", line=1)
    editor.bridge.update_setting("shucked", "lint.enable", False)
    editor.wait_without_diagnostic(uri, "C001", timeout=10)


def test_syntax_errors_follow_their_setting(editor: EditorSession) -> None:
    uri = editor.open("broken.sh", "#!/bin/bash\nif true; then\n  echo missing fi\n")

    def syntax_errors() -> list[dict]:
        return [item for item in editor.bridge.diagnostics(uri) if item.get("severity") == ERROR and item.get("code") is None]

    wait_until("syntax error diagnostic", syntax_errors)
    editor.bridge.update_setting("shucked", "lint.showSyntaxErrors", False)
    wait_until("syntax error hidden", lambda: not syntax_errors())


def test_diagnostics_follow_edits(editor: EditorSession) -> None:
    uri = editor.open("edited.sh", "#!/bin/bash\necho ok\n")
    assert stays_false(lambda: "C001" in editor.bridge.diagnostic_codes(uri), duration=2)
    editor.edit(uri, "#!/bin/bash\nnow_unused=1\n")
    editor.wait_for_diagnostic(uri, "C001", line=1)
    editor.edit(uri, "#!/bin/bash\nnow_used=1\necho \"$now_used\"\n")
    editor.wait_without_diagnostic(uri, "C001")
