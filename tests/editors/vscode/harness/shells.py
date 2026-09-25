"""Real interactive shells with the Shucked hooks loaded, driven without an editor.

The hooks report prompt metadata and live completion results by running small
Node helpers that connect to a Unix socket. :class:`HookListener` plays the
extension's part of that exchange, and :class:`ShellSession` runs the shell on a
pseudo-terminal so it behaves exactly as it would inside a terminal panel.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import shlex
import shutil
import signal
import socket
import tempfile
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pexpect

from .waiting import WaitTimeout

_CONTROL = re.compile(rb"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07|[\r\n\x08]")
SESSION_ID = "b" * 32
SESSION_TOKEN = "c" * 64
Message = dict[str, Any]


class HookListener:
    """Collects the JSON messages hook helpers send over the session socket."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._server = socket.socket(socket.AF_UNIX)
        self._server.bind(str(path))
        self._server.listen()
        self._server.settimeout(0.05)
        self._messages: list[Message] = []
        self._lock = threading.Lock()
        self._closed = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self) -> None:
        while not self._closed.is_set():
            try:
                connection, _ = self._server.accept()
            except (TimeoutError, socket.timeout):
                continue
            except OSError:
                return
            with connection:
                connection.settimeout(2)
                data = bytearray()
                with contextlib.suppress(OSError):
                    while chunk := connection.recv(65536):
                        data.extend(chunk)
            with contextlib.suppress(ValueError):
                with self._lock:
                    self._messages.append(json.loads(data))

    @property
    def messages(self) -> list[Message]:
        with self._lock:
            return list(self._messages)

    def latest(self, predicate: Callable[[Message], bool]) -> Message | None:
        return next((message for message in reversed(self.messages) if predicate(message)), None)

    def close(self) -> None:
        self._closed.set()
        self._server.close()
        self._thread.join(timeout=2)


class ShellSession:
    def __init__(self, shell: str, directory: Path, child: pexpect.spawn, listener: HookListener) -> None:
        self.shell = shell
        self.directory = directory
        self.child = child
        self.listener = listener
        self.output = bytearray()

    @property
    def pid(self) -> int:
        return self.child.pid

    def _drain(self) -> None:
        # Keep the terminal flowing so a full pty buffer never blocks the shell.
        with contextlib.suppress(pexpect.TIMEOUT, pexpect.EOF):
            while True:
                self.output.extend(self.child.read_nonblocking(65536, timeout=0))

    def wait(self, description: str, condition: Callable[[], Any], timeout: float = 4.0) -> Any:
        deadline = time.monotonic() + timeout
        while True:
            self._drain()
            value = condition()
            if value:
                return value
            if time.monotonic() >= deadline:
                phases = [message.get("phase", "metadata") for message in self.listener.messages]
                raise WaitTimeout(f"{self.shell}: {description} timed out; terminal={bytes(self.output[-2000:])!r}; messages={phases}")
            time.sleep(0.05)

    def wait_message(self, description: str, predicate: Callable[[Message], bool], timeout: float = 4.0) -> Message:
        return self.wait(description, lambda: self.listener.latest(predicate), timeout)

    def metadata(self, after: int = 0, timeout: float = 4.0) -> Message:
        """The newest prompt metadata with live completion, newer than ``after``."""
        return self.wait_message(
            "prompt metadata",
            lambda message: message.get("shell") == self.shell and message.get("liveCompletion") is True and message.get("generation", 0) > after,
            timeout,
        )

    def at_prompt(self, prompt: bytes = b"READY>") -> bool:
        """True when nothing but terminal control sequences follows the last prompt."""
        index = self.output.rfind(prompt)
        return index >= 0 and not _CONTROL.sub(b"", bytes(self.output[index + len(prompt):])).strip()

    def wait_idle(self, quiet: float = 0.3) -> None:
        """Wait until the shell shows its prompt and waits for input, as a user's terminal does."""
        while True:
            self.wait("idle prompt", self.at_prompt, timeout=5)
            size = len(self.output)
            time.sleep(quiet)
            self._drain()
            if len(self.output) == size and self.at_prompt():
                return

    def request(self, query: str, generation: int, prefix: str, words: list[str], signal_name: str) -> None:
        """Write a private live completion request and signal the shell, as the extension does."""
        fields = [query, str(generation), prefix, str(len(words)), *words]
        (self.directory / f"request-{query}").write_bytes(("\0".join(fields) + "\0").encode())
        os.kill(self.pid, getattr(signal, signal_name))

    def reply(self, query: str, timeout: float = 3.0) -> Message:
        return self.wait_message(f"reply to {query}", lambda message: message.get("query") == query and message.get("phase") == "result", timeout)

    def send(self, text: str) -> None:
        self.child.send(text)

    def alive(self) -> bool:
        return self.child.isalive()

    def close(self) -> None:
        with contextlib.suppress(Exception):
            self.child.terminate(force=True)
        self.listener.close()


def _command(shell: str, directory: Path, script: str, integration: Path) -> list[str]:
    hook = integration / {"bash": "bash.sh", "zsh": "zsh.zsh", "fish": "fish.fish"}[shell]
    if shell == "bash":
        rc = directory / "bashrc"
        rc.write_text(f"{script}source {shlex.quote(str(hook))}\nPS1='READY> '\n")
        return ["bash", "--noprofile", "--rcfile", str(rc), "-i"]
    if shell == "zsh":
        (directory / ".zshrc").write_text(f"{script}source {shlex.quote(str(hook))}\nPS1='READY> '\n")
        return ["zsh", "-i"]
    rc = directory / "init.fish"
    rc.write_text(f"{script}source {shlex.quote(str(hook))}\nfunction fish_prompt; printf 'READY> '; end\n")
    return ["fish", "--no-config", "-i", "--init-command", f"source {shlex.quote(str(rc))}"]


@contextlib.contextmanager
def interactive(shell: str, script: str, integration: Path, node: str):
    """Start ``shell`` interactively with ``script`` sourced before the Shucked hook."""
    # Unix socket paths are short-lived and must stay under the platform length limit.
    directory = Path(tempfile.mkdtemp(prefix="shucked-live-"))
    directory.chmod(0o700)
    listener = HookListener(directory / "state.sock")
    environment = {
        **{key: value for key, value in os.environ.items() if not key.startswith(("SHUCKED_", "VSCODE_"))},
        "HOME": str(directory), "ZDOTDIR": str(directory), "XDG_CONFIG_HOME": str(directory), "TERM": "dumb",
        "SHUCKED_SESSION_SOCKET": str(listener.path), "SHUCKED_SESSION_ID": SESSION_ID, "SHUCKED_SESSION_TOKEN": SESSION_TOKEN,
        "SHUCKED_LIVE_ALLOWED": "1", "SHUCKED_LIVE_DIRECTORY": str(directory),
        "SHUCKED_LIVE_READ": str(integration / "live-read.cjs"), "SHUCKED_LIVE_RESULT": str(integration / "live-result.cjs"),
        "SHUCKED_LIVE_FISH": str(integration / "live-fish.cjs"), "SHUCKED_CAPTURE": str(integration / "capture.cjs"),
        "SHUCKED_NODE": node,
    }
    command = _command(shell, directory, script, integration)
    child = pexpect.spawn(command[0], command[1:], env=environment, cwd=str(directory), timeout=10)
    session = ShellSession(shell, directory, child, listener)
    try:
        yield session
    finally:
        session.close()
        shutil.rmtree(directory, ignore_errors=True)
