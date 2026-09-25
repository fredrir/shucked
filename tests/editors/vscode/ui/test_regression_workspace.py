"""Opt-in check against an existing workspace: ``--regression-workspace PATH``.

The workspace is opened read-only in spirit: every probe types into one
untitled buffer and nothing is written into the folder.
"""

from __future__ import annotations

import re
from collections.abc import Callable
from pathlib import Path

import pytest

from ..harness.bridge import label
from ..harness.session import EditorSession
from ..harness.waiting import wait_until

HEADER = "#!/bin/zsh\n# aaa_word_guess_from_comment\n"
TEXT, FILE = 0, 16  # vscode.CompletionItemKind


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
    inserted = existing.wait_for_line_change(uri, 2, before + typed)
    assert re.match(pattern, inserted), inserted
    assert not re.search(r"aaa_word_guess|\.sh\b|/$", inserted), "a shell definition must win over document words and files"
    assert inserted[len(before + typed) :] not in files


def test_word_guesses_are_off_through_extension_defaults_only(existing: EditorSession) -> None:
    uri = existing.bridge.open_untitled("shellscript", HEADER, show=False)["uri"]
    assert existing.bridge.setting("editor", "wordBasedSuggestions", uri) == "off"
    inspected = existing.bridge.inspect_setting("editor", "wordBasedSuggestions", uri)
    assert inspected.get("workspaceValue") is None, "no workspace override was needed or written"
    assert inspected.get("globalValue") is None, "no personal setting was needed or written"
    plain = existing.bridge.open_untitled("plaintext", "", show=False)["uri"]
    assert existing.bridge.setting("editor", "wordBasedSuggestions", plain) != "off", "other languages keep their defaults"


def test_cd_offers_only_directories_at_every_step(existing: EditorSession) -> None:
    files = {path.name for path in existing.workspace.iterdir() if path.is_file()}
    uri = existing.bridge.open_untitled("shellscript", HEADER + "cd")["uri"]
    existing.bridge.set_cursor(uri, 2, 2)
    existing.workbench.focus_editor()
    existing.workbench.type(" ")
    labels = existing.workbench.wait_for_suggestions("cd suggestions")
    assert all(label.endswith("/") for label in labels), labels
    # Every response the provider gives while it settles must already be contextual.
    samples: list[list[dict]] = []

    def settled() -> bool:
        result = existing.bridge.completions(uri, 2, 3)
        samples.append(result["items"])
        return not result["isIncomplete"]

    wait_until("cd completion settles", settled, timeout=15)
    for items in samples:
        names = [label(item) for item in items]
        assert not [item for item in items if item.get("kind") in (FILE, TEXT)], f"file or word items offered after cd: {names}"
        assert not [name for name in names if "aaa_word_guess" in name or name in files], names
