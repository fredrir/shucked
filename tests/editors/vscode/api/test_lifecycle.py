"""Server start, restart, crash recovery, and crash-loop protection."""

from __future__ import annotations

from collections.abc import Callable

import psutil

from ..harness import processes
from ..harness.session import EditorSession
from ..harness.waiting import stays_false, wait_until


def _server(session: EditorSession) -> psutil.Process:
    return wait_until(
        "running language server", lambda: next(iter(processes.find_language_servers(session.instance.pid)), None), timeout=30
    )


def _lint_works(session: EditorSession, name: str) -> None:
    uri = session.open(name, "#!/bin/bash\nlifecycle_unused=1\n")
    session.wait_for_diagnostic(uri, "C001", line=1)


def test_restart_replaces_the_server_process(launch_editor: Callable[..., EditorSession]) -> None:
    session = launch_editor("restart")
    _lint_works(session, "before.sh")
    before = _server(session).pid
    session.bridge.execute("shucked.restartServer")
    wait_until("new server process", lambda: _server(session).pid != before)
    wait_until("old server exited", lambda: not processes.is_running(before), timeout=10)
    _lint_works(session, "after.sh")


def test_a_crashed_server_is_restarted(launch_editor: Callable[..., EditorSession]) -> None:
    session = launch_editor("crash")
    _lint_works(session, "before.sh")
    crashed = _server(session)
    crashed.kill()
    wait_until("replacement server", lambda: _server(session).pid != crashed.pid, timeout=30)
    _lint_works(session, "after.sh")


def test_repeated_crashes_stop_automatic_restarts(launch_editor: Callable[..., EditorSession]) -> None:
    session = launch_editor("crash-loop")
    _lint_works(session, "before.sh")
    killed: set[int] = set()
    # Five crashes inside the rolling window count as a crash loop.
    for _ in range(5):
        server = wait_until(
            "server to kill",
            lambda: next((item for item in processes.find_language_servers(session.instance.pid) if item.pid not in killed), None),
            timeout=30,
        )
        killed.add(server.pid)
        server.kill()
    assert stays_false(lambda: any(item.pid not in killed for item in processes.find_language_servers(session.instance.pid)), duration=12)
    assert any("crashed repeatedly" in text for text in session.workbench.notifications())
    session.bridge.execute("shucked.restartServer")
    _server(session)
    _lint_works(session, "recovered.sh")


def test_an_invalid_server_path_reports_a_start_failure(launch_editor: Callable[..., EditorSession], tmp_path) -> None:
    missing = tmp_path / "no-such-server"
    session = launch_editor("invalid-server", settings={"shucked.server.path": str(missing)})
    wait_until("start failure notification", lambda: any("failed to start" in text for text in session.workbench.notifications()))
    wait_until("error status", lambda: any("Shucked: Error" in text for text in session.workbench.status_items()))
    assert not processes.find_language_servers(session.instance.pid)
