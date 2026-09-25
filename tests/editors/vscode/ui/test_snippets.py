"""Keyword snippets expand into editable blocks with working placeholders."""

from __future__ import annotations

import re

import pytest

from ..harness.session import EditorSession
from ..harness.waiting import wait_until


@pytest.mark.parametrize(
    ("keyword", "structure", "first", "second"),
    [
        ("if", r"^if condition; then\n\s+:\nfi\n$", "condition", ":"),
        ("for", r"^for item in items; do\n\s+:\ndone\n$", "item", "items"),
    ],
)
def test_keyword_snippet_placeholders(editor: EditorSession, keyword: str, structure: str, first: str, second: str) -> None:
    uri = editor.open(f"snippet-{keyword}.zsh", "#!/bin/zsh\n")
    editor.bridge.set_cursor(uri, 1, 0)
    workbench = editor.workbench
    workbench.focus_editor()
    workbench.type(keyword)
    workbench.wait_for_suggestions(f"{keyword} snippet", lambda labels: keyword in labels)
    workbench.press("Enter")
    block = wait_until(f"{keyword} block", lambda: (text := editor.bridge.text(uri)).count("\n") > 2 and text.removeprefix("#!/bin/zsh\n"))
    assert re.match(structure, block), block
    assert editor.bridge.active_editor()["selectedText"] == first, "the first placeholder is selected"
    workbench.type("edited")
    workbench.press("Tab")
    wait_until("next placeholder", lambda: editor.bridge.active_editor()["selectedText"] == second)
    assert "edited" in editor.bridge.text(uri)
