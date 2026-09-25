"""Opt-in history suggestions: inline only, scoped to the session, and revocable."""

from __future__ import annotations

import time
from collections.abc import Callable

from ..harness.session import EditorSession
from ..harness.waiting import stays_false, wait_until

PREFIX = "printf shucked_h"


def _accept_inline(editor: EditorSession, uri: str) -> str:
    editor.bridge.show(uri)
    editor.bridge.set_cursor(uri, 0, len(PREFIX))
    editor.bridge.execute("workbench.action.focusActiveEditorGroup")
    editor.bridge.execute("hideSuggestWidget")
    editor.bridge.execute("editor.action.inlineSuggest.trigger")
    # The provider reads history asynchronously; give the ghost text time to appear.
    time.sleep(0.3)
    editor.bridge.execute("editor.action.inlineSuggest.commit")
    return editor.bridge.text(uri)


def _attach_zsh(editor: EditorSession, uri: str) -> None:
    editor.create_terminal("zsh")
    editor.wait_for_hover(uri, 0, 2, r"InteractiveSession", timeout=30)


def test_history_suggestions_are_off_by_default(editor: EditorSession) -> None:
    assert editor.bridge.setting("shucked", "history.session") is False
    assert editor.bridge.setting("shucked", "history.files") is False
    uri = editor.open("default.zsh", PREFIX)
    assert stays_false(lambda: _accept_inline(editor, uri) != PREFIX, duration=2)


def test_session_history_file_supplies_an_inline_suggestion(editor: EditorSession, require_shell: Callable[[str], str]) -> None:
    require_shell("zsh")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("history.zsh", PREFIX)
    _attach_zsh(editor, uri)
    wait_until("history inline suggestion accepted", lambda: _accept_inline(editor, uri) == "printf shucked_history_fixture", timeout=30)


def test_opting_out_revokes_a_displayed_suggestion(editor: EditorSession, require_shell: Callable[[str], str]) -> None:
    require_shell("zsh")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("revoke.zsh", PREFIX)
    _attach_zsh(editor, uri)
    wait_until("history inline suggestion accepted", lambda: _accept_inline(editor, uri) == "printf shucked_history_fixture", timeout=30)
    editor.edit(uri, PREFIX, cursor=(0, len(PREFIX)))
    editor.bridge.execute("editor.action.inlineSuggest.trigger")
    time.sleep(0.3)
    editor.bridge.update_setting("shucked", "history.files", False)
    editor.bridge.execute("editor.action.inlineSuggest.commit")
    assert editor.bridge.text(uri) == PREFIX


def test_default_history_file_is_read_without_a_terminal(editor: EditorSession) -> None:
    (editor.home / ".zsh_history").write_text(": 1700000000:0;printf shucked_default_history\n")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("no-terminal.zsh", "printf shucked_d")
    editor.bridge.show(uri)

    def accepted() -> bool:
        editor.bridge.set_cursor(uri, 0, len("printf shucked_d"))
        editor.bridge.execute("editor.action.inlineSuggest.trigger")
        time.sleep(0.3)
        editor.bridge.execute("editor.action.inlineSuggest.commit")
        return editor.bridge.text(uri) == "printf shucked_default_history"

    wait_until("default history suggestion", accepted, timeout=30)
