"""The login-shell context captures startup state once and feeds it to the resolver like an attached terminal."""
import asyncio
import os
import stat

import pytest

from tests.lsp.client import LspClient

pytestmark = pytest.mark.skipif(os.name != "posix", reason="POSIX shell fixtures")

# A stand-in for the user's login shell: it ignores `-l -i -c <script>` and
# prints the same NUL-framed records the terminal hooks produce.
FAKE_SHELL = """#!/bin/sh
: > "$HOME/login-shell-ran"
printf 'cwd\\0%s\\0' "$HOME"
printf 'searchpath\\0%s\\0' "$HOME/login-bin"
printf 'alias\\0short=printf\\0'
printf 'function\\0greet\\0'
printf 'option\\0expand_aliases=1\\0'
printf 'option\\0capture=%s\\0' "${SHUCKED_CAPTURE-unset}"
printf 'end\\0shucked-login-shell\\0'
"""


def fake_shell(root):
    shell = root / "bash"
    shell.write_text(FAKE_SHELL)
    shell.chmod(shell.stat().st_mode | stat.S_IXUSR)
    return shell


async def start_host(binary, root, shell, options=None):
    environment = dict(os.environ, PATH=str(root), HOME=str(root), SHELL=str(shell))
    client = LspClient(str(binary), environment=environment)
    await client.start()
    await client.initialize(root_uri=root.as_uri(), initialization_options=options)
    await client.initialized()
    return client


async def wait_for_codes(client, uri, predicate):
    for _ in range(12):
        diagnostics = await client.wait_for_diagnostics(uri)
        codes = {d.get("code") for d in diagnostics}
        if predicate(codes):
            return diagnostics
    raise AssertionError("diagnostics never matched")


async def wait_until(description, predicate, timeout=10.0):
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        if await predicate():
            return
        if asyncio.get_running_loop().time() > deadline:
            raise AssertionError(f"timed out waiting for {description}")
        await asyncio.sleep(0.1)


async def hover_text(client, uri, line, character):
    hover = await client.send_request("textDocument/hover", {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
    return str(hover)


async def test_login_shell_context_resolves_aliases_functions_and_path(shucked_binary, tmp_path):
    shell = fake_shell(tmp_path)
    (tmp_path / "login-bin").mkdir()
    tool = tmp_path / "login-bin" / "login_only_tool"
    tool.write_text("#!/bin/sh\nexit 0\n")
    tool.chmod(0o755)
    client = await start_host(shucked_binary, tmp_path, shell, options={"nativeExecutionAllowed": True})
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text="short hello\nlogin_only_tool\ngreet\n", language_id="bash")
        await wait_for_codes(client, uri, lambda codes: "ENV001" in codes)
        assert not (tmp_path / "login-shell-ran").exists(), "the workspace context must not run the login shell"
        await client.send_notification("shucked/selectEnvironment", {"uri": uri, "options": {"policy": "login-shell"}})
        # Selecting the context starts one capture; its state reaches the resolver after the refresh tick.
        async def marker_written():
            return (tmp_path / "login-shell-ran").exists()
        await wait_until("the login shell to run", marker_written)

        async def hover_uses_login_shell():
            hover = await hover_text(client, uri, 1, 2)
            return "Environment: login shell (" in hover and "Command not found" not in hover
        await wait_until("hover to use the login shell capture", hover_uses_login_shell)
        assert "Resolution: Builtin · printf" in await hover_text(client, uri, 0, 1), "the captured alias resolves `short`"

        async def push_without_missing_commands():
            diagnostics = await client.wait_for_diagnostics(uri, timeout=1.0)
            return not any(d.get("code") == "ENV001" for d in diagnostics)
        await wait_until("published diagnostics without ENV001", push_without_missing_commands)
        details = await client.send_request("shucked/environmentDetails", {"textDocument": {"uri": uri}, "position": {"line": 1, "character": 2}})
        assert details["trusted"] is True
        assert details["source"].startswith("login shell (")
        markdown = details["markdown"]
        assert "Workspace trust (native execution): trusted" in markdown
        assert "- Login shell: `%s` (bash) · captured (1 PATH entries, 1 aliases, 1 functions)" % shell in markdown
        assert "login-bin" in markdown
        assert "- Name: `login_only_tool`" in markdown
        assert "- Resolution: Executable `login_only_tool`" in markdown
    finally:
        await client.shutdown_and_exit()


async def test_untrusted_host_never_runs_the_login_shell(shucked_binary, tmp_path):
    shell = fake_shell(tmp_path)
    client = await start_host(
        shucked_binary, tmp_path, shell, options={"nativeExecutionAllowed": False, "environment": {"policy": "login-shell"}}
    )
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text="short hello\n", language_id="bash")
        await wait_for_codes(client, uri, lambda codes: "ENV001" in codes)
        details = await client.send_request("shucked/environmentDetails", {"textDocument": {"uri": uri}})
        assert details["trusted"] is False
        assert "untrusted" in details["markdown"]
        assert "disabled (the workspace is not trusted)" in details["markdown"]
        assert "_The cursor is not on a command name._" in details["markdown"]
        await client.send_request("workspace/executeCommand", {"command": "shucked.refreshEnvironment"})
        await asyncio.sleep(0.3)
        assert not (tmp_path / "login-shell-ran").exists(), "an untrusted workspace ran the login shell"
    finally:
        await client.shutdown_and_exit()
