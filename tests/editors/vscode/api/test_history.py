"""Opt-in history suggestions: inline only, scoped to the session, and revocable.

These tests watch the ghost text in the editor, so a suggestion is known to be
displayed (or known to be absent) before it is accepted or revoked.
"""

from __future__ import annotations

from collections.abc import Callable

from ..harness.session import EditorSession
from ..harness.waiting import wait_until

PREFIX = "printf shucked_h"
SESSION_ENTRY = "printf shucked_history_fixture"


def _attach_zsh(editor: EditorSession, uri: str) -> None:
    editor.create_terminal("zsh")
    editor.wait_for_hover(uri, 0, 2, r"InteractiveSession", timeout=30)


def _accept(editor: EditorSession) -> None:
    editor.bridge.execute("editor.action.inlineSuggest.commit")


def test_history_suggestions_are_off_by_default(editor: EditorSession) -> None:
    assert editor.bridge.setting("shucked", "history.session") is False
    assert editor.bridge.setting("shucked", "history.files") is False
    (editor.home / ".zsh_history").write_text(f": 1700000000:0;{SESSION_ENTRY}\n")
    uri = editor.open("default.zsh", PREFIX)
    assert editor.no_inline_suggestion(uri, PREFIX)
    _accept(editor)
    assert editor.bridge.text(uri) == PREFIX


def test_session_history_file_supplies_an_inline_suggestion(editor: EditorSession, require_shell: Callable[[str], str]) -> None:
    require_shell("zsh")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("history.zsh", PREFIX)
    _attach_zsh(editor, uri)
    assert editor.wait_for_inline_suggestion(uri, PREFIX) == SESSION_ENTRY.removeprefix(PREFIX)
    _accept(editor)
    wait_until("suggestion inserted", lambda: editor.bridge.text(uri) == SESSION_ENTRY)


def test_opting_out_revokes_a_displayed_suggestion(editor: EditorSession, require_shell: Callable[[str], str]) -> None:
    require_shell("zsh")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("revoke.zsh", PREFIX)
    _attach_zsh(editor, uri)
    editor.wait_for_inline_suggestion(uri, PREFIX)
    editor.bridge.update_setting("shucked", "history.files", False)
    wait_until("displayed suggestion withdrawn", lambda: not editor.workbench.ghost_text(), timeout=5)
    _accept(editor)
    assert editor.bridge.text(uri) == PREFIX


def test_default_history_file_is_read_without_a_terminal(editor: EditorSession) -> None:
    # Also the trusted counterpart of test_trust.test_history_suggestions_require_trust.
    (editor.home / ".zsh_history").write_text(": 1700000000:0;printf shucked_default_history\n")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("no-terminal.zsh", "printf shucked_d")
    assert editor.wait_for_inline_suggestion(uri, "printf shucked_d") == "efault_history"


def test_clearing_history_hides_the_displayed_suggestion(editor: EditorSession) -> None:
    (editor.home / ".zsh_history").write_text(": 1700000000:0;printf shucked_cleared_history\n")
    editor.bridge.update_setting("shucked", "history.files", True)
    uri = editor.open("clear.zsh", "printf shucked_c")
    editor.wait_for_inline_suggestion(uri, "printf shucked_c")
    editor.bridge.execute("shucked.clearHistorySuggestions")
    wait_until("suggestion hidden", lambda: not editor.workbench.ghost_text(), timeout=5)
