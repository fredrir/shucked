"""Process ownership helpers: every editor, shell, and server a test starts is reaped."""

from __future__ import annotations

import contextlib
import ctypes
import os
import sys
from collections.abc import Iterable

import psutil

_PR_SET_CHILD_SUBREAPER = 36


def adopt_orphans() -> None:
    """Make this process the reaper for descendants whose parents exit first.

    Electron and the language client detach helpers; in a container without an
    init process those would otherwise outlive the test run as zombies.
    """
    if sys.platform != "linux":
        return
    with contextlib.suppress(OSError, AttributeError):
        ctypes.CDLL(None).prctl(_PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0)


def reap_zombies() -> None:
    if sys.platform == "win32":
        return
    with contextlib.suppress(ChildProcessError):
        while os.waitpid(-1, os.WNOHANG)[0]:
            pass


def descendants(pid: int) -> list[psutil.Process]:
    with contextlib.suppress(psutil.Error):
        return psutil.Process(pid).children(recursive=True)
    return []


def processes_mentioning(marker: str) -> list[psutil.Process]:
    """Processes whose command line contains ``marker`` (e.g. a private profile path)."""
    found = []
    for process in psutil.process_iter(["cmdline"]):
        with contextlib.suppress(psutil.Error):
            if any(marker in argument for argument in process.info["cmdline"] or []):
                found.append(process)
    return found


def terminate(processes: Iterable[psutil.Process], timeout: float = 5.0) -> None:
    """Terminate processes, escalate to SIGKILL, and reap anything we adopted."""
    alive = []
    for process in processes:
        with contextlib.suppress(psutil.Error):
            if process.pid != os.getpid():
                process.terminate()
                alive.append(process)
    _, remaining = psutil.wait_procs(alive, timeout=timeout)
    for process in remaining:
        with contextlib.suppress(psutil.Error):
            process.kill()
    psutil.wait_procs(remaining, timeout=timeout)
    reap_zombies()


def is_running(pid: int) -> bool:
    try:
        return psutil.Process(pid).status() != psutil.STATUS_ZOMBIE
    except psutil.Error:
        return False


def find_language_servers(root_pid: int) -> list[psutil.Process]:
    """Shucked language server processes started beneath an editor process."""
    servers = []
    for process in descendants(root_pid):
        with contextlib.suppress(psutil.Error):
            command = process.cmdline()
            if not command:
                continue
            name = os.path.basename(command[0])
            if name.startswith("shucked-server") or (name.startswith("shucked") and "server" in command[1:2]):
                servers.append(process)
    return servers
