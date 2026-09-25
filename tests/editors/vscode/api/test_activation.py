"""The extension activates in an isolated editor with the expected defaults."""

from __future__ import annotations

from pathlib import Path

import pytest

from ..harness.instance import EXTENSION_ID
from ..harness.session import EditorSession


def test_extension_activates_in_isolated_profile(editor: EditorSession) -> None:
    extension = editor.bridge.extension(EXTENSION_ID)
    assert extension is not None
    assert extension["isActive"]
    state = editor.bridge.ping()
    assert state["isTrusted"] is True
    assert state["environment"]["HOME"] == str(editor.home)
    assert Path(state["workspaceFolders"][0]["path"]) == editor.workspace


def test_extension_is_loaded_from_the_expected_location(
    editor: EditorSession, request: pytest.FixtureRequest, extension_root: Path
) -> None:
    extension = editor.bridge.extension(EXTENSION_ID)
    assert extension is not None
    location = Path(extension["extensionPath"]).resolve()
    if request.config.getoption("--vsix"):
        assert editor.instance.extensions.resolve() in location.parents, "installed-package mode must use the VSIX"
    else:
        assert location == extension_root.resolve()


def test_contributed_commands_are_registered(editor: EditorSession, extension_manifest: dict) -> None:
    registered = set(editor.bridge.commands())
    contributed = {command["command"] for command in extension_manifest["contributes"]["commands"]}
    bound = {binding["command"] for binding in extension_manifest["contributes"]["keybindings"]}
    assert contributed | bound <= registered, sorted((contributed | bound) - registered)


def test_shell_languages_turn_off_word_suggestions(editor: EditorSession) -> None:
    for language in ("shellscript", "fish"):
        uri = editor.bridge.open_untitled(language, "", show=False)["uri"]
        assert editor.bridge.setting("editor", "wordBasedSuggestions", uri) == "off", language
    plain = editor.bridge.open_untitled("plaintext", "", show=False)["uri"]
    assert editor.bridge.setting("editor", "wordBasedSuggestions", plain) != "off", "other languages keep their defaults"
