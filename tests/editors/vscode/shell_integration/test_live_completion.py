"""Live completion hooks in real interactive shells.

Each attached shell starts one persistent helper that keeps a connection to the
editor's session socket. The editor sends a request frame; the helper writes a
private request file, signals the shell while it sits at its prompt, and
streams the shell worker's result back. These tests play the editor's part and
check that the current shell state is used, that editor text is never
evaluated, that runaway completers are stopped without harming the shell, and
that the helper's lifetime follows the shell's.
"""

from __future__ import annotations

import os
import signal
import sys

import pytest

from ..harness import processes
from ..harness.shells import ShellSession, interactive

pytestmark = pytest.mark.skipif(sys.platform == "win32", reason="the hooks use Unix signals and sockets")

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

# Bash requests travel on the window-size signal, the one readline dispatches
# promptly while idle; zsh and fish reserve USR1 (or USR2 for zsh).
RESERVED = {"bash": "SIGWINCH", "zsh": "SIGUSR1", "fish": "SIGUSR1"}
OCCUPIED = {
    "bash": "trap 'builtin printf occupied > \"$HOME/trap-called\"' WINCH\n",
    "zsh": 'TRAPUSR1() { builtin printf occupied > "$HOME/trap-called"; }; TRAPUSR2() { :; }\n',
    "fish": 'function existing_handler --on-signal USR1; builtin printf occupied > "$HOME/trap-called"; end\n',
}


def _deliver(session: ShellSession, metadata: dict, query: str, words: list[str]) -> None:
    """Send a request while the shell is idle at its prompt; no keystroke follows."""
    session.wait_idle()
    session.request(query, metadata["generation"], "live", words)


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
def test_helper_starts_with_the_shell_and_names_it(shell: str, shell_path: str, integration, node: str) -> None:
    with interactive(shell, STATE[shell][0], integration, node) as session:
        hello = session.helper()
        metadata = session.metadata()
        assert hello["shell"] == shell
        assert hello["shellPid"] == metadata["pid"] == session.pid
        assert hello["signal"] == metadata["liveSignal"] == RESERVED[shell]
        assert processes.is_running(hello["pid"])
        assert (session.directory / "live.fifo").is_fifo(), "the helper owns the record FIFO"
        greetings = [message for message in session.listener.messages if message.get("kind") == "liveHelper"]
        assert len(greetings) == 1, "one helper per shell"


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
def test_completer_sees_current_state_and_never_runs_editor_words(shell: str, shell_path: str, integration, node: str) -> None:
    script, update = STATE[shell]
    with interactive(shell, script, integration, node) as session:
        session.helper()
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
            query = str(index + 1) * 32
            _deliver(session, metadata, query, ["custom", f"$(touch {marker})"])
            reply = session.reply(query)
            assert expected in [item["text"] for item in reply["candidates"]], reply
            assert reply["generation"] == metadata["generation"]
            assert isinstance(reply.get("elapsedMs"), int), "the helper reports how long the shell took"
            assert not marker.exists(), "editor words were executed"
            assert not event_marker.exists(), "restoring private state fired a user event handler"
            prompts = session.prompts()
            assert all(message["pid"] == session.pid for message in prompts), "a private worker reported prompt metadata"
            assert all(message["generation"] <= metadata["generation"] for message in prompts), "a private worker advanced the session"


def test_bash_distinguishes_filename_spaces_from_shell_quoting(integration, node: str, require_shell) -> None:
    require_shell("bash")
    script = STATE["bash"][0] + "_custom() { compopt -o filenames; COMPREPLY=('file '); }\n"
    with interactive("bash", script, integration, node) as session:
        session.helper()
        metadata = session.metadata()
        _deliver(session, metadata, "1" * 32, ["custom"])
        first = session.reply("1" * 32)["candidates"][0]
        assert (first["text"], first.get("encoding")) == ("file ", None), "readline quotes filename candidates itself"
        session.send("_custom() { compopt +o filenames; COMPREPLY=('file\\ '); }\n")
        metadata = session.metadata(after=metadata["generation"])
        _deliver(session, metadata, "2" * 32, ["custom"])
        second = session.reply("2" * 32)["candidates"][0]
        assert (second["text"], second.get("encoding")) == ("file\\ ", "bashWord"), "other callbacks supply shell-word text"


@pytest.mark.parametrize("shell", ["bash", "zsh"])
@pytest.mark.parametrize("returns_candidates", [False, True], ids=["hanging", "background-child"])
def test_deadline_stops_callbacks_and_their_children(shell: str, returns_candidates: bool, shell_path: str, integration, node: str) -> None:
    callback = HANGING
    if returns_candidates:
        callback = callback.replace("wait; }", ("COMPREPLY=(live)" if shell == "bash" else "compadd -- live") + "; }")
    with interactive(shell, STATE[shell][0] + callback, integration, node) as session:
        session.helper()
        query = "e" * 32
        _deliver(session, session.metadata(), query, ["custom"])
        files = [session.directory / "worker-pid", session.directory / "child-pid"]
        session.wait("worker and child started", lambda: all(path.exists() and path.read_text().strip() for path in files), timeout=3)
        reply = session.reply(query)
        if returns_candidates:
            assert "live" in [item["text"] for item in reply["candidates"]]
        else:
            assert reply["partial"] is True
            assert "timed out" in reply["reason"]
        worker, child = (int(path.read_text()) for path in files)
        # The helper cleans up by itself: no editor, no ps, and PATH points nowhere.
        session.wait("worker and child stopped", lambda: not processes.is_running(worker) and not processes.is_running(child), timeout=4)
        assert session.alive(), "the interactive shell itself was terminated"


@pytest.mark.parametrize("shell", ["bash", "zsh"])
def test_cancellation_stops_the_worker_at_once(shell: str, shell_path: str, integration, node: str) -> None:
    with interactive(shell, STATE[shell][0] + HANGING, integration, node) as session:
        session.helper()
        query = "d" * 32
        _deliver(session, session.metadata(), query, ["custom"])
        files = [session.directory / "worker-pid", session.directory / "child-pid"]
        session.wait("worker and child started", lambda: all(path.exists() and path.read_text().strip() for path in files), timeout=3)
        worker, child = (int(path.read_text()) for path in files)
        session.cancel(query)
        session.wait("worker and child stopped", lambda: not processes.is_running(worker) and not processes.is_running(child), timeout=1.5)
        assert session.listener.latest(lambda message: message.get("query") == query) is None, "a cancelled request is not answered"
        assert session.alive()


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
def test_helper_exits_with_its_shell(shell: str, shell_path: str, integration, node: str) -> None:
    with interactive(shell, STATE[shell][0], integration, node) as session:
        hello = session.helper()
        session.metadata()
        session.wait_idle()
        session.send("exit\n")
        session.wait("shell exited", lambda: not session.alive(), timeout=5)
        session.wait("helper exited", lambda: not processes.is_running(hello["pid"]) and session.listener.helper_closed.is_set(), timeout=4)
        assert not (session.directory / "live.fifo").exists(), "the helper removes its FIFO"


@pytest.mark.parametrize("shell", ["bash", "zsh", "fish"])
def test_existing_signal_handlers_are_kept(shell: str, shell_path: str, integration, node: str) -> None:
    with interactive(shell, OCCUPIED[shell] + STATE[shell][0], integration, node) as session:
        metadata = session.wait_message("prompt metadata", lambda message: "cwd" in message and message.get("shell") == shell)
        assert metadata.get("liveCompletion") is False, "live completion is declined when the reserved signals are taken"
        session.wait_idle()
        assert session.listener.latest(lambda message: message.get("kind") == "liveHelper") is None, "no helper is started"
        os.kill(session.pid, getattr(signal, RESERVED[shell]))
        # The handler is the user's own; a keystroke is fair for shells that run traps after input.
        session.send("\n")
        session.wait("user handler ran", lambda: (session.directory / "trap-called").exists(), timeout=3)
        assert session.alive()
