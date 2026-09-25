"""Live completion hooks in real interactive shells.

The editor asks an attached shell to run its own completer by writing a private
request file and sending a reserved signal while the shell sits at its prompt.
These tests play the editor's part and check that the current shell state is
used, that editor text is never evaluated, and that runaway completers are
stopped without harming the shell.
"""

from __future__ import annotations

import os
import signal

import pytest

from ..harness import processes
from ..harness.shells import ShellSession, interactive

BASH_IDLE_PROMPT = pytest.mark.xfail(
    strict=True,
    reason="bash defers signal traps while readline waits at an idle prompt, so a live request is only served after the next keystroke",
)


def _shells(*names: str) -> list:
    return [pytest.param(name, marks=BASH_IDLE_PROMPT) if name == "bash" else name for name in names]


STATE = {
    "bash": (
        'my_completion_value=live_first\ncustom() { :; }\n_custom() { COMPREPLY=("$my_completion_value"); }\ncomplete -F _custom custom\n',
        "my_completion_value=live_second\n",
    ),
    "zsh": (
        "autoload -Uz compinit; compinit -D\n"
        'my_completion_value=live_first\ncustom() { :; }\n_custom() { compadd -- "$my_completion_value"; }\ncompdef _custom custom\n',
        "my_completion_value=live_second\n",
    ),
    "fish": (
        "set -g my_completion_value live_first\nfunction custom; end\nfunction _custom; printf '%s\\n' $my_completion_value; end\n"
        "complete -c custom -f -a '(_custom)'\n"
        'function _state_changed --on-variable my_completion_value; builtin printf event > "$HOME/must-not-restore-events"; end\n',
        "set -g my_completion_value live_second\n",
    ),
}

# A callback that never finishes and leaves a background child behind.
HANGING = (
    '_custom() { "$SHUCKED_NODE" -e \'require("fs").writeFileSync(process.env.HOME+"/worker-pid",String(process.ppid))\'; '
    '/bin/sleep 30 & echo $! > "$HOME/child-pid"; wait; }\nPATH=/nonexistent\n'
)

OCCUPIED = {
    "bash": "trap 'builtin printf occupied > \"$HOME/trap-called\"' USR1; trap ':' USR2\n",
    "zsh": 'TRAPUSR1() { builtin printf occupied > "$HOME/trap-called"; }; TRAPUSR2() { :; }\n',
    "fish": 'function existing_handler --on-signal USR1; builtin printf occupied > "$HOME/trap-called"; end\n',
}


def _ask(session: ShellSession, metadata: dict, query: str, words: list[str]) -> dict:
    session.wait_idle()
    session.request(query, metadata["generation"], "live", words, metadata["liveSignal"])
    return session.reply(query)


@pytest.mark.parametrize("shell", _shells("bash", "zsh", "fish"))
def test_completer_sees_current_state_and_never_runs_editor_words(shell: str, shell_path: str, integration, node: str) -> None:
    script, update = STATE[shell]
    with interactive(shell, script, integration, node) as session:
        metadata = session.metadata()
        assert metadata["pid"] == session.pid
        for index, expected in enumerate(["live_first", "live_second"]):
            if index:
                # Authored input simulates the user changing state; the extension never types.
                session.send(update)
                metadata = session.metadata(after=metadata["generation"])
            marker = session.directory / "must-not-exist"
            event_marker = session.directory / "must-not-restore-events"
            event_marker.unlink(missing_ok=True)
            reply = _ask(session, metadata, str(index + 1) * 32, ["custom", f"$(touch {marker})"])
            assert expected in [item["text"] for item in reply["candidates"]], reply
            assert reply["generation"] == metadata["generation"]
            assert not marker.exists(), "editor words were executed"
            assert not event_marker.exists(), "restoring private state fired a user event handler"
            prompts = [message for message in session.listener.messages if "shell" in message]
            assert all(message["pid"] == session.pid for message in prompts), "a private worker reported prompt metadata"
            assert all(message["generation"] <= metadata["generation"] for message in prompts), "a private worker advanced the session"


@pytest.mark.parametrize("shell", _shells("bash"))
def test_bash_distinguishes_filename_spaces_from_shell_quoting(shell: str, shell_path: str, integration, node: str) -> None:
    script = STATE["bash"][0] + "_custom() { compopt -o filenames; COMPREPLY=('file '); }\n"
    with interactive(shell, script, integration, node) as session:
        metadata = session.metadata()
        first = _ask(session, metadata, "1" * 32, ["custom"])["candidates"][0]
        assert (first["text"], first.get("encoding")) == ("file ", None), "readline quotes filename candidates itself"
        session.send("_custom() { compopt +o filenames; COMPREPLY=('file\\ '); }\n")
        metadata = session.metadata(after=metadata["generation"])
        second = _ask(session, metadata, "2" * 32, ["custom"])["candidates"][0]
        assert (second["text"], second.get("encoding")) == ("file\\ ", "bashWord"), "other callbacks supply shell-word text"


@pytest.mark.parametrize("shell", _shells("bash", "zsh"))
@pytest.mark.parametrize("returns_candidates", [False, True], ids=["hanging", "background-child"])
def test_watchdog_stops_callbacks_and_their_children(shell: str, returns_candidates: bool, shell_path: str, integration, node: str) -> None:
    callback = HANGING
    if returns_candidates:
        callback = callback.replace("wait; }", ("COMPREPLY=(live)" if shell == "bash" else "compadd -- live") + "; }")
    with interactive(shell, STATE[shell][0] + callback, integration, node) as session:
        metadata = session.metadata()
        query = "e" * 32
        session.wait_idle()
        session.request(query, metadata["generation"], "live", ["custom"], metadata["liveSignal"])
        files = [session.directory / "worker-pid", session.directory / "child-pid"]
        session.wait("worker and child started", lambda: all(path.exists() and path.read_text().strip() for path in files), timeout=3)
        if returns_candidates:
            assert "live" in [item["text"] for item in session.reply(query)["candidates"]]
        worker, child = (int(path.read_text()) for path in files)
        # The hook cleans up by itself: no editor, no ps, and PATH points nowhere.
        session.wait("worker and child stopped", lambda: not processes.is_running(worker) and not processes.is_running(child), timeout=4)
        assert session.alive(), "the interactive shell itself was terminated"


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
def test_existing_signal_handlers_are_kept(shell: str, shell_path: str, integration, node: str) -> None:
    with interactive(shell, OCCUPIED[shell] + STATE[shell][0], integration, node) as session:
        metadata = session.wait_message("prompt metadata", lambda message: message.get("shell") == shell)
        assert metadata.get("liveCompletion") is False, "live completion is declined when both signals are taken"
        session.wait_idle()
        os.kill(session.pid, signal.SIGUSR1)
        # Bash runs traps after the next input; the handler is the user's own, so typing is fair here.
        session.send("\n")
        session.wait("user handler ran", lambda: (session.directory / "trap-called").exists(), timeout=3)
        assert session.alive()
