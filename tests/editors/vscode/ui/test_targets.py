"""Capturing an execution target and comparing a script against it, through the real dialogs."""

from __future__ import annotations

import json

from ..harness.session import EditorSession
from ..harness.waiting import wait_until


def _capture(editor: EditorSession, name: str, label: str) -> None:
    destination = editor.path(name)
    workbench = editor.workbench
    workbench.run_command("Shucked: Capture Execution Target")
    workbench.enter_text(str(destination))
    workbench.pick("bash")
    workbench.enter_text(label)
    wait_until("capture notification", lambda: any("captured" in text for text in workbench.notifications()), timeout=60)
    inventory = json.loads(destination.read_text())
    assert label in json.dumps(inventory)


def test_capture_then_compare_a_script(editor: EditorSession) -> None:
    editor.open("compare.sh", "#!/bin/bash\ngit status\nshucked_absent_everywhere\n")
    _capture(editor, "host.json", "ci-host")
    editor.workbench.clear_notifications()
    editor.workbench.run_command("Shucked: Compare Execution Targets")
    editor.workbench.enter_text(str(editor.path("host.json")))
    report = wait_until("comparison report", lambda: (active := editor.bridge.active_editor()) and active["uri"].startswith("shucked-targets:") and active)
    text = editor.bridge.text(report["uri"])
    assert "| Command | Source line | ci-host |" in text
    assert "shucked_absent_everywhere" in text
    assert "| git |" in text
