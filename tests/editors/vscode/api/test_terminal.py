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


@pytest.mark.parametrize(
    "shell",
    [
        "zsh",
        pytest.param(
            "bash", marks=pytest.mark.xfail(strict=True, reason="bash defers signal traps while readline waits at an idle prompt")
        ),
    ],
)
def test_live_completer_reads_current_shell_state(editor: EditorSession, require_shell: Callable[[str], str], shell: str) -> None:
    require_shell(shell)
    uri = editor.open(f"live.{'zsh' if shell == 'zsh' else 'sh'}", f"#!/bin/{shell}\nshucked_smoke_alias hello\n")
    terminal = editor.create_terminal(shell)
    editor.wait_for_hover(uri, 1, 4, r"InteractiveSession", timeout=30)
    editor.edit(uri, "custom_fixture live_")
    editor.wait_for_completion(uri, 0, len("custom_fixture live_"), "live_fixture_value", timeout=10)
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
    durations: list[float] = []

    def worker_started() -> str | None:
        # On a loaded machine the deadline can pass before the shell starts a
        # worker; that request is simply cancelled, so ask again.
        started = time.monotonic()
        editor.bridge.completions(uri, 0, len("slow_fixture live_"))
        durations.append(time.monotonic() - started)
        return marker.read_text().strip() if marker.exists() else None

    worker = int(wait_until("live worker started", worker_started, timeout=30, interval=0.5))
    assert max(durations) < 2.5, f"a slow custom completer has a bounded deadline: {durations}"
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
