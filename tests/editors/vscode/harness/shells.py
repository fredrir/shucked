"""Real interactive shells with the Shucked hooks loaded, driven without an editor.

The prompt hook reports metadata by running a small Node helper that connects
to a Unix socket once; the live completion hook starts one persistent helper
per shell that keeps its connection open and exchanges newline-delimited JSON
frames. :class:`HookListener` plays the extension's part of both exchanges, and
:class:`ShellSession` runs the shell on a pseudo-terminal so it behaves exactly
as it would inside a terminal panel.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import shlex
import shutil
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
    """Collects the JSON frames hook helpers send over the session socket.

    One-shot senders (the prompt hook) close after a frame; the live helper
    keeps its connection, which stays available as :attr:`helper` for frames
    the extension would send.
    """

    def __init__(self, path: Path) -> None:
        self.path = path
        self._server = socket.socket(socket.AF_UNIX)
        self._server.bind(str(path))
        self._server.listen()
        self._server.settimeout(0.05)
        self._messages: list[Message] = []
        self._lock = threading.Lock()
        self._closed = threading.Event()
        self._connections: list[socket.socket] = []
        self.helper: socket.socket | None = None
        self.helper_closed = threading.Event()
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self) -> None:
        while not self._closed.is_set():
            try:
                connection, _ = self._server.accept()
            except TimeoutError:
                continue
            except OSError:
                return
            self._connections.append(connection)
            threading.Thread(target=self._read, args=(connection,), daemon=True).start()

    def _read(self, connection: socket.socket) -> None:
        data = bytearray()
        is_helper = False
        with contextlib.suppress(OSError):
            while chunk := connection.recv(65536):
                data.extend(chunk)
                while (newline := data.find(b"\n")) >= 0:
                    line, data = bytes(data[:newline]), data[newline + 1 :]
                    with contextlib.suppress(ValueError):
                        message = json.loads(line)
                        if message.get("kind") == "liveHelper" and message.get("phase") == "hello":
                            is_helper = True
                            self.helper = connection
                        with self._lock:
                            self._messages.append(message)
        if is_helper and self.helper is connection:
            self.helper_closed.set()
        with contextlib.suppress(OSError):
            connection.close()

    def send(self, message: Message) -> None:
        """Send a frame to the live helper, as the extension does."""
        assert self.helper is not None, "the live helper has not connected"
        self.helper.sendall(json.dumps(message).encode() + b"\n")

    @property
    def messages(self) -> list[Message]:
        with self._lock:
            return list(self._messages)

    def latest(self, predicate: Callable[[Message], bool]) -> Message | None:
        return next((message for message in reversed(self.messages) if predicate(message)), None)

    def close(self) -> None:
        self._closed.set()
        for connection in self._connections:
            with contextlib.suppress(OSError):
                connection.close()
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
        assert self.child.pid is not None
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
                phases = [message.get("phase", message.get("kind", "metadata")) for message in self.listener.messages]
                raise WaitTimeout(f"{self.shell}: {description} timed out; terminal={bytes(self.output[-2000:])!r}; messages={phases}")
            time.sleep(0.05)

    def wait_message(self, description: str, predicate: Callable[[Message], bool], timeout: float = 4.0) -> Message:
        return self.wait(description, lambda: self.listener.latest(predicate), timeout)

    def metadata(self, after: int = 0, timeout: float = 4.0) -> Message:
        """The newest prompt metadata with live completion, newer than ``after``."""
        return self.wait_message(
            "prompt metadata",
            lambda message: (
                "cwd" in message
                and message.get("shell") == self.shell
                and message.get("liveCompletion") is True
                and message.get("generation", 0) > after
            ),
            timeout,
        )

    def prompts(self) -> list[Message]:
        """Every prompt metadata frame received so far."""
        return [message for message in self.listener.messages if "cwd" in message]

    def helper(self, timeout: float = 6.0) -> Message:
        """The live helper's greeting; it starts with the shell and connects on its own."""
        return self.wait_message(
            "live helper greeting", lambda message: message.get("kind") == "liveHelper" and message.get("phase") == "hello", timeout
        )

    def at_prompt(self, prompt: bytes = b"READY>") -> bool:
        """True when nothing but terminal control sequences follows the last prompt."""
        index = self.output.rfind(prompt)
        return index >= 0 and not _CONTROL.sub(b"", bytes(self.output[index + len(prompt) :])).strip()

    def wait_idle(self, quiet: float = 0.3) -> None:
        """Wait until the shell shows its prompt and waits for input, as a user's terminal does."""
        while True:
            self.wait("idle prompt", self.at_prompt, timeout=5)
            size = len(self.output)
            time.sleep(quiet)
            self._drain()
            if len(self.output) == size and self.at_prompt():
                return

    def request(self, query: str, generation: int, prefix: str, words: list[str]) -> None:
        """Ask the live helper for a completion, as the extension does; it signals the shell itself."""
        self.listener.send({"kind": "request", "query": query, "generation": generation, "prefix": prefix, "words": words})

    def cancel(self, query: str) -> None:
        self.listener.send({"kind": "cancel", "query": query})

    def reply(self, query: str, timeout: float = 3.0) -> Message:
        return self.wait_message(
            f"reply to {query}", lambda message: message.get("query") == query and message.get("phase") == "result", timeout
        )

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
        "HOME": str(directory),
        "ZDOTDIR": str(directory),
        "XDG_CONFIG_HOME": str(directory),
        "TERM": "dumb",
        "SHUCKED_SESSION_SOCKET": str(listener.path),
        "SHUCKED_SESSION_ID": SESSION_ID,
        "SHUCKED_SESSION_TOKEN": SESSION_TOKEN,
        "SHUCKED_LIVE_ALLOWED": "1",
        "SHUCKED_LIVE_DIRECTORY": str(directory),
        "SHUCKED_LIVE_HELPER": str(integration / "live-helper.cjs"),
        "SHUCKED_CAPTURE": str(integration / "capture.cjs"),
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
