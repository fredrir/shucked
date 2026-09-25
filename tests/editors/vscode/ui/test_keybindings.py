"""Escape and arrow keys are intercepted only while a Shucked completion is pending."""

from __future__ import annotations

from ..harness.session import EditorSession
from ..harness.waiting import stays_false, wait_until


def _open_flags(editor: EditorSession, name: str) -> str:
    uri = editor.open(name, "#!/bin/zsh\nls ")
    editor.bridge.set_cursor(uri, 1, 3)
    editor.workbench.focus_editor()
    editor.workbench.type("-")
    editor.workbench.wait_for_suggestions("flag suggestions", lambda labels: len(labels) > 1)
    return uri


def test_escape_dismisses_and_late_results_do_not_reopen(editor: EditorSession) -> None:
    uri = _open_flags(editor, "escape.zsh")
    editor.workbench.press("Escape")
    wait_until("suggestions hidden", lambda: not editor.workbench.suggest_visible(), timeout=5)
    assert stays_false(editor.workbench.suggest_visible, duration=3), "a late refresh reopened dismissed suggestions"
    assert editor.bridge.text(uri).endswith("ls -")


def test_arrow_keys_move_the_selection_and_keep_it(editor: EditorSession) -> None:
    _open_flags(editor, "arrows.zsh")
    first = editor.workbench.focused_suggestion()
    editor.workbench.press("ArrowDown")
    second = wait_until("selection moved", lambda: (label := editor.workbench.focused_suggestion()) != first and label)
    assert stays_false(lambda: editor.workbench.focused_suggestion() != second, duration=2), "a refresh reset the selection"
    editor.workbench.press("ArrowUp")
    wait_until("selection moved back", lambda: editor.workbench.focused_suggestion() == first)


def test_escape_keeps_its_default_meaning_without_pending_completion(editor: EditorSession) -> None:
    uri = editor.open("cursors.zsh", "#!/bin/zsh\necho one\necho two\n")
    editor.bridge.show(uri)
    editor.bridge.evaluate("""
        const editor = vscode.window.activeTextEditor;
        editor.selections = [new vscode.Selection(1, 0, 1, 0), new vscode.Selection(2, 0, 2, 0)];
    """)
    editor.workbench.focus_editor()
    editor.workbench.press("Escape")
    count = lambda: editor.bridge.evaluate("return vscode.window.activeTextEditor.selections.length")  # noqa: E731
    wait_until("secondary cursor removed", lambda: count() == 1, timeout=5)
