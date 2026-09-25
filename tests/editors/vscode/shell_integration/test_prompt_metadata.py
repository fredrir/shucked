"""What a prompt hook reports about the shell: names and settings, never code or history."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path

import pytest

from ..harness.shells import SESSION_TOKEN, HookListener
from ..harness.waiting import wait_until

pytestmark = pytest.mark.skipif(sys.platform == "win32", reason="the hooks use Unix sockets")

HOOKS = {"bash": "bash.sh", "zsh": "zsh.zsh", "fish": "fish.fish"}
POSIX_SCRIPT = (
    "source \"$1\"; alias ls='eza --icons'; alias dangerous='touch should-never-be-executed'; "
    "function demo { echo PRIVATE_FUNCTION_BODY; }; __shucked_capture"
)
FISH_SCRIPT = (
    "source \"$argv[1]\"; alias ls 'eza --icons'; alias dangerous 'touch should-never-be-executed'; "
    "function demo; echo PRIVATE_FUNCTION_BODY; end; __shucked_capture"
)


@pytest.fixture
def private_directory() -> Iterator[Path]:
    directory = Path(tempfile.mkdtemp(prefix="shucked-hook-"))
    directory.chmod(0o700)
    yield directory
    shutil.rmtree(directory, ignore_errors=True)


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
@pytest.mark.parametrize("files_enabled", [False, True], ids=["history-files-off", "history-files-on"])
def test_prompt_metadata_reports_names_but_never_bodies(
    shell: str, files_enabled: bool, shell_path: str, integration: Path, node: str, private_directory: Path
) -> None:
    policy = private_directory / "policy"
    policy.write_text(f"0\n{1 if files_enabled else 0}\n")
    history = private_directory / "custom-history"
    listener = HookListener(private_directory / "state.sock")
    hook = integration / HOOKS[shell]
    arguments = ["--no-config", "-c", FISH_SCRIPT, str(hook)] if shell == "fish" else ["-c", POSIX_SCRIPT, shell, str(hook)]
    environment = {
        **os.environ,
        # A private HOME keeps the developer's own startup files (e.g. ~/.zshenv) out of the result.
        "HOME": str(private_directory),
        "ZDOTDIR": str(private_directory),
        "XDG_CONFIG_HOME": str(private_directory),
        "HISTFILE": str(history),
        "fish_history": "custom",
        "XDG_DATA_HOME": str(private_directory),
        "SHUCKED_HISTORY_POLICY": str(policy),
        "SHUCKED_SESSION_ID": "a" * 32,
        "SHUCKED_SESSION_TOKEN": SESSION_TOKEN,
        "SHUCKED_SESSION_SOCKET": str(listener.path),
        "SHUCKED_CAPTURE": str(integration / "capture.cjs"),
        "SHUCKED_NODE": node,
    }
    try:
        subprocess.run([shell_path, *arguments], env=environment, cwd=private_directory, check=True, capture_output=True, timeout=15)
        message = wait_until("prompt metadata", lambda: listener.latest(lambda item: item.get("shell") == shell), timeout=5)
    finally:
        listener.close()
    if shell == "fish":
        assert "ls" in message["functions"], "fish aliases are functions"
    else:
        assert message["aliases"]["ls"] == ["eza", "--icons"]
    assert "dangerous" not in message["aliases"], "aliases that run other commands are reported by name only"
    assert {"demo", "dangerous"} <= set(message["functions"])
    assert "PRIVATE_FUNCTION_BODY" not in str(message)
    assert not (private_directory / "should-never-be-executed").exists()
    assert isinstance(message["private"], bool)
    assert message.get("acceptedHistoryHash") is None, "session history collection is off"
    expected_history = (str(private_directory / "fish" / "custom_history") if shell == "fish" else str(history)) if files_enabled else None
    assert message.get("historyFile") == expected_history
