"""Shucked terminals share live shell state with the editor without running editor text."""

from __future__ import annotations

import time
from collections.abc import Callable

import pytest

from ..harness import processes
from ..harness.session import EditorSession
from ..harness.waiting import wait_until


@pytest.fixture
def zsh_document(editor: EditorSession, require_shell: Callable[[str], str]) -> tuple[EditorSession, str]:
    require_shell("zsh")
    uri = editor.open("session.zsh", "#!/bin/zsh\nshucked_smoke_alias hello\n")
    return editor, uri


def test_terminal_startup_alias_resolves_in_the_editor(zsh_document: tuple[EditorSession, str]) -> None:
    editor, uri = zsh_document
    terminal = editor.create_terminal("zsh")
    assert terminal["name"].startswith("Shucked")
    hover = editor.wait_for_hover(uri, 1, 4, r"InteractiveSession", timeout=30)
    assert "Resolution: Builtin" in hover


def test_live_completer_reads_current_shell_state(zsh_document: tuple[EditorSession, str]) -> None:
    editor, uri = zsh_document
    terminal = editor.create_terminal("zsh")
    editor.wait_for_hover(uri, 1, 4, r"InteractiveSession", timeout=30)
    editor.edit(uri, "custom_fixture live_")
    editor.wait_for_completion(uri, 0, len("custom_fixture live_"), "live_fixture_value")
    # Authored input simulates the user changing shell state; the extension never sends text.
    editor.bridge.terminal_send_text(terminal["name"], "my_completion_value=live_fixture_changed")
    editor.wait_for_completion(uri, 0, len("custom_fixture live_"), "live_fixture_changed")


def test_slow_live_completer_is_bounded_and_its_worker_stopped(zsh_document: tuple[EditorSession, str]) -> None:
    editor, uri = zsh_document
    editor.create_terminal("zsh")
    editor.wait_for_hover(uri, 1, 4, r"InteractiveSession", timeout=30)
    marker = editor.home / "live_worker_pid"
    marker.unlink(missing_ok=True)
    editor.edit(uri, "slow_fixture live_")
    started = time.monotonic()
    editor.bridge.completions(uri, 0, len("slow_fixture live_"))
    assert time.monotonic() - started < 2.5, "a slow custom completer has a bounded deadline"
    worker = int(wait_until("live worker pid", lambda: marker.exists() and marker.read_text().strip(), timeout=5))
    wait_until("live worker stopped", lambda: not processes.is_running(worker), timeout=5)


def test_closing_the_terminal_revokes_session_evidence(zsh_document: tuple[EditorSession, str]) -> None:
    editor, uri = zsh_document
    terminal = editor.create_terminal("zsh")
    editor.wait_for_hover(uri, 1, 4, r"InteractiveSession", timeout=30)
    editor.bridge.dispose_terminals([terminal["name"]])
    editor.wait_for_hover(uri, 1, 4, r"(?i)stale|refresh|unavailable|detached")


def test_terminal_status_reflects_the_attached_shell(zsh_document: tuple[EditorSession, str]) -> None:
    editor, _ = zsh_document
    editor.create_terminal("zsh")
    wait_until("terminal status item", lambda: any("Terminal (zsh)" in text for text in editor.workbench.status_items()), timeout=30)
