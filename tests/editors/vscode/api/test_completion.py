"""Completion through the VS Code client: document symbols, languages, and bundled providers."""

from __future__ import annotations

import shutil

import pytest

from ..harness.bridge import inserted_text, label
from ..harness.session import EditorSession
from ..harness.waiting import wait_until


def test_document_function_is_offered(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.zsh")
    item = editor.wait_for_completion(uri, 3, len("shucked_smoke_f"), "shucked_smoke_function")
    assert inserted_text(item).startswith("shucked_smoke_function")


def test_fish_documents_are_registered_and_completed(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.fish")
    assert editor.bridge.document(uri)["languageId"] == "fish"
    editor.wait_for_completion(uri, 3, len("fish_fi"), "fish_fixture")


def test_fish_shebang_selects_the_fish_language(editor: EditorSession) -> None:
    uri = editor.open("script-without-extension", "#!/usr/bin/env fish\necho hi\n")
    assert editor.bridge.document(uri)["languageId"] == "fish"


@pytest.mark.skipif(shutil.which("git") is None, reason="git is not installed")
@pytest.mark.parametrize(("dialect", "flag"), [("bash", "--detach"), ("zsh", "--detach"), ("fish", "--track")])
def test_bundled_provider_completes_subcommand_flags(editor: EditorSession, dialect: str, flag: str) -> None:
    # No terminal is attached and no personal completer is configured, so the
    # flag can only come from the bundled provider definitions.
    command = "git checkout --"
    uri = editor.open(f"native.{dialect}", f"#!/usr/bin/env {dialect}\n{command}")
    item = editor.wait_for_completion(uri, 1, len(command), flag)
    assert inserted_text(item) == flag, "a completion separator must not become a literal space"


def test_keywords_follow_their_setting(editor: EditorSession) -> None:
    uri = editor.open("keywords.sh", "#!/bin/bash\nfun")
    editor.wait_for_completion(uri, 1, 3, "function")
    editor.bridge.update_setting("shucked", "server.completion.includeKeywords", False)
    wait_until("keywords removed", lambda: "function" not in editor.bridge.completion_labels(uri, 1, 3))


def test_max_items_limits_candidates(editor: EditorSession) -> None:
    uri = editor.open("limit.sh", "#!/bin/bash\n")
    editor.wait_for_completion(uri, 1, 0, lambda _: True)
    editor.bridge.update_setting("shucked", "server.completion.maxItems", 5)

    def bounded() -> list[dict] | None:
        items = editor.bridge.completions(uri, 1, 0)["items"]
        return items if 0 < len(items) <= 5 else None

    assert all(label(item) for item in wait_until("completion limited to five items", bounded))
