"""Contributed commands are reachable from the command palette and do what they say."""

from __future__ import annotations

import re
import subprocess
from typing import Any

from ..harness.session import EditorSession
from ..harness.waiting import wait_until


def test_every_contributed_command_is_in_the_palette(editor: EditorSession, extension_manifest: dict[str, Any]) -> None:
    editor.open_workspace_file("smoke.zsh")
    titles = {command["title"] for command in extension_manifest["contributes"]["commands"]}
    listed = set(editor.workbench.palette_commands("Shucked:"))
    assert titles <= listed, f"missing from the palette: {sorted(titles - listed)}"


def test_show_version_reports_the_server_version(editor: EditorSession, language_server) -> None:
    editor.open_workspace_file("smoke.zsh")
    editor.workbench.run_command("Shucked: Show Version")
    message = wait_until(
        "version notification", lambda: next((text for text in editor.workbench.notifications() if text.startswith("Shucked:")), None)
    )
    if language_server is not None:
        expected = subprocess.run([str(language_server), "--version"], capture_output=True, text=True, check=True).stdout.strip()
        assert message == f"Shucked: {expected}"
    else:
        assert re.match(r"^Shucked: shucked \d+\.\d+\.\d+", message)


def test_show_logs_opens_the_output_channel(editor: EditorSession) -> None:
    editor.open_workspace_file("smoke.zsh")
    editor.workbench.run_command("Shucked: Show Language Server Logs")
    output = editor.workbench.page.locator("#workbench\\.parts\\.panel .monaco-editor .view-lines")
    # The editor renders spaces as non-breaking spaces.
    wait_until("Shucked output channel", lambda: "Shucked extension activation complete" in output.first.inner_text().replace("\xa0", " "))


def test_restart_from_the_palette_keeps_diagnostics_working(editor: EditorSession) -> None:
    uri = editor.open("restart.sh", "#!/bin/bash\nrestart_unused=1\n")
    editor.wait_for_diagnostic(uri, "C001", line=1)
    editor.workbench.run_command("Shucked: Restart Language Server")
    wait_until("ready after restart", lambda: "Shucked" in editor.workbench.status_items(), timeout=30)
    editor.edit(uri, "#!/bin/bash\nrestart_unused_again=1\n")
    diagnostic = editor.wait_for_diagnostic(uri, "C001", line=1)
    assert "restart_unused_again" in diagnostic["message"]


def test_clear_history_suggestions_command_runs(editor: EditorSession) -> None:
    editor.open_workspace_file("smoke.zsh")
    editor.workbench.run_command("Shucked: Clear History Suggestions")
    assert not [text for text in editor.workbench.notifications() if "error" in text.lower()]
