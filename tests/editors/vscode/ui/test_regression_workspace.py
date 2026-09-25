"""Opt-in check against an existing workspace: ``--regression-workspace PATH``.

The workspace is opened read-only in spirit: every probe types into one
untitled buffer and nothing is written into the folder.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from pathlib import Path

import pytest

from ..harness.session import EditorSession
from ..harness.waiting import wait_until

HEADER = "#!/bin/zsh\n# aaa_word_guess_from_comment\n"


@pytest.fixture
def existing(request: pytest.FixtureRequest, launch_editor: Callable[..., EditorSession]) -> EditorSession:
    workspace = request.config.getoption("--regression-workspace")
    if not workspace:
        pytest.skip("pass --regression-workspace PATH to check an existing workspace")
    return launch_editor("regression", workspace=Path(workspace).resolve())


@pytest.mark.parametrize(("before", "typed", "pattern"), [("brew", " ", r"^brew (?:--[\w-]+|[\w-]+)$"), ("ls ", "-", r"^ls -\S+$")])
def test_ordinary_defaults_give_contextual_suggestions(existing: EditorSession, before: str, typed: str, pattern: str) -> None:
    files = {path.name for path in existing.workspace.iterdir() if path.is_file()}
    uri = existing.bridge.open_untitled("shellscript", HEADER + before)["uri"]
    existing.bridge.set_cursor(uri, 2, len(before))
    existing.workbench.focus_editor()
    existing.workbench.type(typed)
    existing.workbench.wait_for_suggestions(f"suggestions after {before + typed!r}")
    existing.workbench.press("Enter")
    inserted = wait_until("accepted suggestion", lambda: (line := existing.bridge.text(uri).split("\n")[2]) != before + typed and line)
    assert re.match(pattern, inserted), inserted
    assert not re.search(r"aaa_word_guess|\.sh\b|/$", inserted), "a shell definition must win over document words and files"
    assert inserted[len(before + typed):] not in files


def test_cd_offers_only_directories(existing: EditorSession) -> None:
    uri = existing.bridge.open_untitled("shellscript", HEADER + "cd")["uri"]
    existing.bridge.set_cursor(uri, 2, 2)
    existing.workbench.focus_editor()
    existing.workbench.type(" ")
    labels = existing.workbench.wait_for_suggestions("cd suggestions")
    assert all(label.endswith("/") for label in labels), labels
    assert not any("aaa_word_guess" in label for label in labels)
