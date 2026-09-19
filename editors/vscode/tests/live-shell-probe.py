"""Exercise shipped query hooks in isolated real interactive shell processes."""
import json
import fcntl
import ctypes
import termios
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

# Minimal containers may have no init reaper. Adopt and reap our fixture orphans.
if sys.platform == "linux":
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("Could not enable fixture child reaping")
shell = sys.argv[1]
mode = sys.argv[2] if len(sys.argv) > 2 else "state"
integration = Path(os.environ.get("SHUCKED_TEST_INTEGRATION", Path(__file__).resolve().parents[1] / "shell-integration"))
with tempfile.TemporaryDirectory(prefix="shucked-live-test-") as temporary:
    directory = Path(temporary)
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(directory / "state.sock"))
    listener.listen()
    listener.settimeout(0.05)
    env = dict(os.environ, HOME=temporary, ZDOTDIR=temporary, XDG_CONFIG_HOME=temporary,
               TERM="dumb", SHUCKED_SESSION_SOCKET=str(directory / "state.sock"),
               SHUCKED_SESSION_ID="b" * 32, SHUCKED_SESSION_TOKEN="c" * 64,
               SHUCKED_LIVE_ALLOWED="1", SHUCKED_LIVE_DIRECTORY=temporary,
               SHUCKED_LIVE_READ=str(integration / "live-read.cjs"),
               SHUCKED_LIVE_RESULT=str(integration / "live-result.cjs"),
               SHUCKED_LIVE_FISH=str(integration / "live-fish.cjs"),
               SHUCKED_NODE=shutil.which("node"), SHUCKED_CAPTURE=str(integration / "capture.cjs"))
    if shell == "bash":
        script = "my_completion_value=live_first\ncustom() { :; }\n_custom() { COMPREPLY=(\"$my_completion_value\"); }\ncomplete -F _custom custom\n"
        script += f"source {shlex.quote(str(integration / 'bash.sh'))}\nPS1='READY> '\n"
        filename = directory / "bashrc"
        command = [shell, "--noprofile", "--rcfile", str(filename), "-i"]
        update = "my_completion_value=live_second\n"
    elif shell == "zsh":
        script = "autoload -Uz compinit; compinit -D\nmy_completion_value=live_first\ncustom() { :; }\n_custom() { compadd -- \"$my_completion_value\"; }\ncompdef _custom custom\n"
        script += f"source {shlex.quote(str(integration / 'zsh.zsh'))}\nPS1='READY> '\n"
        filename = directory / ".zshrc"
        command = [shell, "-i"]
        update = "my_completion_value=live_second\n"
    else:
        script = "set -g my_completion_value live_first\nfunction custom; end\nfunction _custom; printf '%s\\n' $my_completion_value; end\ncomplete -c custom -f -a '(_custom)'\n"
        script += 'function _state_changed --on-variable my_completion_value; builtin printf event > "$HOME/must-not-restore-events"; end\n'
        script += f"source {shlex.quote(str(integration / 'fish.fish'))}\nfunction fish_prompt; printf 'READY> '; end\n"
        filename = directory / "init.fish"
        command = [shell, "--no-config", "-i", "--init-command", f"source {shlex.quote(str(filename))}"]
        update = "set -g my_completion_value live_second\n"
    if mode in ("timeout", "background"):
        # The hook must clean up independently: no editor manager or PATH ps exists.
        callback = "_custom() { \"$SHUCKED_NODE\" -e 'require(\"fs\").writeFileSync(process.env.HOME+\"/worker-pid\",String(process.ppid))'; /bin/sleep 30 & echo $! > \"$HOME/child-pid\"; wait; }\nPATH=/nonexistent\n"
        if mode == "background":
            completion = 'COMPREPLY=(live)' if shell == 'bash' else 'compadd -- live'
            callback = callback.replace('wait; }', completion + '; }')
        script += callback
    if mode == "occupied":
        if shell == "fish":
            reservation = "function existing_handler --on-signal USR1; builtin printf occupied > \"$HOME/trap-called\"; end\n"
        elif shell == "zsh":
            reservation = "TRAPUSR1() { builtin printf occupied > \"$HOME/trap-called\"; }; TRAPUSR2() { :; }\n"
        else:
            reservation = "trap 'builtin printf occupied > \"$HOME/trap-called\"' USR1; trap ':' USR2\n"
        script = reservation + script
    filename.write_text(script)
    master, slave = pty.openpty()
    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    command[0] = os.environ.get("SHUCKED_TEST_SHELL", shell)
    process = subprocess.Popen(command, env=env, stdin=slave, stdout=slave, stderr=slave, preexec_fn=controlling_terminal)
    os.close(slave)
    output = bytearray()
    messages = []

    def wait(predicate, timeout=4):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if sys.platform == "linux":
                try:
                    while os.waitpid(-1, os.WNOHANG)[0]:
                        pass
                except ChildProcessError:
                    pass
            ready, _, _ = select.select([master], [], [], 0)
            if ready:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    pass
            try:
                connection, _ = listener.accept()
                connection.settimeout(1)
                data = bytearray()
                while chunk := connection.recv(65536):
                    data.extend(chunk)
                connection.close()
                messages.append(json.loads(data))
            except socket.timeout:
                pass
            match = next((item for item in reversed(messages) if predicate(item)), None)
            if match:
                return match
        raise AssertionError(f"{shell}: missing response; terminal={output[-2000:]!r}; phases={[m.get('phase', 'metadata') for m in messages]}")

    try:
        if mode == "occupied":
            metadata = wait(lambda message: message.get("shell") == shell)
            assert metadata.get("liveCompletion") is False, metadata
            os.kill(process.pid, signal.SIGUSR1)
            wait(lambda _: (directory / "trap-called").exists(), timeout=2)
            assert process.poll() is None
            print(json.dumps({"shell": shell, "passed": True, "preservedTrap": True}))
            sys.exit(0)
        metadata = wait(lambda message: message.get("liveCompletion") is True)
        assert metadata["pid"] == process.pid
        if mode in ("timeout", "background"):
            query = "e" * 32
            request = [query, str(metadata["generation"]), "live", "1", "custom"]
            (directory / f"request-{query}").write_bytes(("\0".join(request) + "\0").encode())
            os.kill(process.pid, getattr(signal, metadata["liveSignal"]))
            wait(lambda _: (directory / "worker-pid").exists() and (directory / "child-pid").exists(), timeout=2)
            if mode == "background":
                reply = wait(lambda message: message.get("query") == query and message.get("phase") == "result", timeout=2)
                assert any(item["text"] == "live" for item in reply["candidates"]), reply
            worker = int((directory / "worker-pid").read_text())
            child = int((directory / "child-pid").read_text())
            def exited(pid):
                try:
                    os.kill(pid, 0)
                    return False
                except ProcessLookupError:
                    return True
            try:
                wait(lambda _: exited(worker) and exited(child), timeout=3)
            except AssertionError:
                print(subprocess.check_output(["/bin/ps", "-o", "pid,ppid,pgid,stat,comm", "-p", f"{worker},{child}"]).decode(), file=sys.stderr)
                raise
            assert process.poll() is None, "The interactive parent was terminated"
            print(json.dumps({"shell": shell, "passed": True, "watchdog": True}))
            sys.exit(0)
        for index, expected in enumerate(["live_first", "live_second"]):
            if index:
                # Authored fixture input simulates a user's state change; the extension never sends this.
                os.write(master, update.encode())
                metadata = wait(lambda message: message.get("generation", 0) > metadata["generation"] and message.get("liveCompletion") is True)
            query = str(index + 1) * 32
            marker = directory / "must-not-exist"
            event_marker = directory / "must-not-restore-events"
            event_marker.unlink(missing_ok=True)
            request = [query, str(metadata["generation"]), "live", "2", "custom", f"$(touch {marker})"]
            (directory / f"request-{query}").write_bytes(("\0".join(request) + "\0").encode())
            os.kill(process.pid, getattr(signal, metadata["liveSignal"]))
            reply = wait(lambda message: message.get("query") == query and message.get("phase") == "result", timeout=2)
            assert any(item["text"] == expected for item in reply["candidates"]), reply
            assert not marker.exists(), "Editor arguments were executed"
            assert not event_marker.exists(), "Private state restoration fired a user event handler"
            assert all(message.get("pid") == process.pid for message in messages if "shell" in message), "Private worker emitted parent prompt metadata"
            assert all(message.get("generation", 0) <= metadata["generation"] for message in messages if "shell" in message), "Private worker advanced the parent session generation"
            assert reply["generation"] == metadata["generation"]
        print(json.dumps({"shell": shell, "passed": True, "queries": 2, "currentState": "live_second"}))
    finally:
        process.kill()
        os.close(master)
        process.wait(timeout=5)
        listener.close()
