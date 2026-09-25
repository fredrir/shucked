"""Suggestions open by themselves while typing, and accepting one inserts the right text.

These tests press real keys. Invoking the completion provider directly would
hide a missing trigger character or a broken refresh of incomplete results.
"""

from __future__ import annotations

import re

import pytest

from ..harness.session import EditorSession


@pytest.fixture
def typing(editor: EditorSession) -> EditorSession:
    (editor.workspace / "aaa_completion_fixture").mkdir(exist_ok=True)
    return editor


def _type_and_accept(editor: EditorSession, uri: str, line: int, before: str, typed: str) -> str:
    editor.edit(uri, f"#!/bin/zsh\n{before}", cursor=(line, len(before)))
    workbench = editor.workbench
    workbench.focus_editor()
    workbench.type(typed)
    workbench.wait_for_suggestions(f"automatic suggestions after {before + typed!r}")
    workbench.press("Enter")
    return editor.wait_for_line_change(uri, line, before + typed)


@pytest.mark.parametrize(
    ("before", "typed", "pattern"),
    [
        ("cd", " ", r"^cd \S+/$"),
        ("ls ", "-", r"^ls -\S+$"),
    ],
    ids=["directories-after-cd", "flags-after-dash"],
)
def test_typing_opens_contextual_suggestions(typing: EditorSession, before: str, typed: str, pattern: str) -> None:
    uri = typing.open("automatic.zsh", "#!/bin/zsh\n")
    # A second round checks the warm path, which reuses cached provider state.
    for _ in range(2):
        assert re.match(pattern, _type_and_accept(typing, uri, 1, before, typed))


def test_subcommands_come_from_the_command_definition(typing: EditorSession) -> None:
    # An empty launch directory keeps file names from satisfying this check.
    empty = typing.path("empty-launch-directory")
    empty.mkdir(parents=True, exist_ok=True)
    typing.bridge.update_setting("shucked", "environment.cwd", str(empty), target="workspace")
    typing.bridge.execute("shucked.restartServer")
    uri = typing.open("subcommands.zsh", "#!/bin/zsh\n")
    for _ in range(2):
        inserted = _type_and_accept(typing, uri, 1, "docker", " ")
        assert re.match(r"^docker (?:attach|build|builder|--config|--debug)$", inserted), inserted


def test_directory_suggestion_is_the_first_entry(typing: EditorSession) -> None:
    uri = typing.open("first.zsh", "#!/bin/zsh\ncd")
    typing.bridge.set_cursor(uri, 1, 2)
    typing.workbench.focus_editor()
    typing.workbench.type(" ")
    labels = typing.workbench.wait_for_suggestions("directory suggestions")
    assert labels[0] == "aaa_completion_fixture/"
    assert all(label.endswith("/") for label in labels), "cd only offers directories"


def test_shell_documents_do_not_offer_word_guesses(editor: EditorSession) -> None:
    uri = editor.open("words.zsh", "#!/bin/zsh\n# aaa_word_guess_from_comment\nbrew")
    assert editor.bridge.setting("editor", "wordBasedSuggestions", uri) == "off"
    editor.bridge.set_cursor(uri, 2, 4)
    editor.workbench.focus_editor()
    editor.workbench.type(" ")
    labels = editor.workbench.wait_for_suggestions("brew suggestions")
    assert not any("aaa_word_guess" in label for label in labels)
