"""Execution target evidence is shared across editor surfaces."""
import os
import pytest
from tests.lsp.client import LspClient

pytestmark = pytest.mark.skipif(os.name != "posix", reason="POSIX executable fixtures")

async def start_host(binary, root, capabilities=None, options=None):
    client = LspClient(str(binary), environment=dict(os.environ, PATH=str(root), HOME=str(root)))
    await client.start()
    await client.initialize(root_uri=root.as_uri(), capabilities=capabilities, initialization_options=options)
    await client.initialized()
    return client

async def wait_for_code(client, uri, code):
    for _ in range(10):
        diagnostics = await client.wait_for_diagnostics(uri)
        if any(d.get("code") == code for d in diagnostics):
            return diagnostics
    raise AssertionError(f"No {code} diagnostic arrived")

async def test_missing_command_warning_fix_and_portable_context(shucked_binary, tmp_path):
    client = await start_host(shucked_binary, tmp_path, capabilities={"workspace": {"applyEdit": True, "workspaceEdit": {"documentChanges": True}}})
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text="prinf hello\n", language_id="bash")
        diagnostics = await wait_for_code(client, uri, "ENV001")
        missing = next(d for d in diagnostics if d.get("code") == "ENV001")
        assert "Command not found" in missing["message"]
        assert missing["range"]["end"]["character"] == 5
        actions = await client.send_request("textDocument/codeAction", {
            "textDocument": {"uri": uri}, "range": missing["range"], "context": {"diagnostics": [missing], "only": ["quickfix"]},
        })
        correction = next(a for a in actions if a["title"] == "Replace with `printf`")
        assert "edit" not in correction
        await client.send_request("workspace/executeCommand", correction["command"])
        applied = next(n for n in client._all_notifications if n["method"] == "workspace/applyEdit")
        edit = applied["params"]["edit"]["documentChanges"][0]
        assert edit["textDocument"]["version"] == 1
        assert edit["edits"][0]["newText"] == "printf"
        await client.send_notification("shucked/selectEnvironment", {"uri": uri, "options": {"policy": "portable"}})
        for _ in range(10):
            if not any(d.get("code") == "ENV001" for d in await client.wait_for_diagnostics(uri)):
                break
        else:
            raise AssertionError("Portable context retained host-absence diagnostics")
        await client.change_document(uri, "if true\n", version=2)
        for _ in range(10):
            if any("parse error" in d["message"] for d in await client.wait_for_diagnostics(uri)):
                break
        else:
            raise AssertionError("Portable context lost parser diagnostics")
    finally:
        await client.shutdown_and_exit()

async def test_install_refresh_updates_diagnostics_and_tokens_without_edit(shucked_binary, tmp_path):
    client = await start_host(shucked_binary, tmp_path, capabilities={"workspace": {"semanticTokens": {"refreshSupport": True}}})
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text="newly-installed\n", language_id="bash")
        assert any(d.get("code") == "ENV001" for d in await wait_for_code(client, uri, "ENV001"))
        before = await client.send_request("textDocument/semanticTokens/full", {"textDocument": {"uri": uri}})
        assert before["data"][4] & 16
        executable = tmp_path / "newly-installed"
        executable.write_text("#!/bin/sh\nexit 99\n")
        executable.chmod(0o755)
        await client.send_request("workspace/executeCommand", {"command": "shucked.refreshEnvironment"})
        assert not any(d.get("code") == "ENV001" for d in await client.wait_for_diagnostics(uri))
        after = await client.send_request("textDocument/semanticTokens/full", {"textDocument": {"uri": uri}})
        assert not after["data"][4] & 16
        assert any(n["method"] == "workspace/semanticTokens/refresh" for n in client._all_notifications)
    finally:
        await client.shutdown_and_exit()

async def test_pull_reports_unchanged_results(shucked_binary, tmp_path):
    client = await start_host(shucked_binary, tmp_path, capabilities={"textDocument": {"diagnostic": {}}})
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text="shucked-unknown-command\n", language_id="bash")
        first = await client.send_request("textDocument/diagnostic", {"textDocument": {"uri": uri}})
        assert first["kind"] == "full"
        second = await client.send_request("textDocument/diagnostic", {"textDocument": {"uri": uri}, "previousResultId": first["resultId"]})
        assert second == {"kind": "unchanged", "resultId": first["resultId"]}
    finally:
        await client.shutdown_and_exit()

async def test_dynamic_commands_and_guarded_dependencies_do_not_warn(shucked_binary, tmp_path):
    client = await start_host(shucked_binary, tmp_path)
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text='"$COMMAND"\nif command -v optional-tool; then\n optional-tool\nfi\noptional-tool\n', language_id="bash")
        missing = [d for d in await wait_for_code(client, uri, "ENV001") if d.get("code") == "ENV001"]
        assert [d["range"]["start"]["line"] for d in missing] == [4]
    finally:
        await client.shutdown_and_exit()

async def test_startup_document_does_not_inherit_post_startup_aliases(shucked_binary, tmp_path):
    client = await start_host(shucked_binary, tmp_path, options={"nativeExecutionAllowed": True})
    uri = (tmp_path / ".zshrc").as_uri()
    try:
        await client.send_notification("shucked/shellSession", {"id": "terminal", "generation": 1, "cwd": str(tmp_path), "path": [str(tmp_path)], "aliases": {"earlier": ["printf"]}, "functions": [], "connected": True})
        await client.open_document(uri, text="earlier\nalias earlier=printf\nearlier\n", language_id="zsh")
        await client.send_notification("shucked/selectEnvironment", {"uri": uri, "options": {"sessionId": "terminal"}})
        report = await client.send_request("textDocument/diagnostic", {"textDocument": {"uri": uri}})
        missing = [d for d in report["items"] if d.get("code") == "ENV001"]
        assert not missing  # Post-startup PATH cannot establish entry-state absence.
        before = await client.send_request("textDocument/hover", {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 2}})
        after = await client.send_request("textDocument/hover", {"textDocument": {"uri": uri}, "position": {"line": 2, "character": 2}})
        assert "Unknown" in str(before)
        assert "Builtin" in str(after)
    finally:
        await client.shutdown_and_exit()

async def test_project_command_declarations_are_file_scoped(shucked_binary, tmp_path):
    (tmp_path / ".shucked.toml").write_text('[environment.commands.codegen]\nkind = "generated"\nfiles = ["generated.sh"]\n')
    client = await start_host(shucked_binary, tmp_path)
    try:
        for name, expected_missing in [("generated.sh", False), ("standalone.sh", True)]:
            uri = (tmp_path / name).as_uri()
            await client.open_document(uri, text="codegen\n", language_id="bash")
            if expected_missing:
                await wait_for_code(client, uri, "ENV001")
            else:
                # Generated declarations produce a distinct dependency diagnostic.
                await wait_for_code(client, uri, "ENV004")
            report = await client.send_request("textDocument/diagnostic", {"textDocument": {"uri": uri}})
            assert any(d.get("code") == "ENV001" for d in report["items"]) is expected_missing
    finally:
        await client.shutdown_and_exit()

async def test_sourced_functions_resolve_with_shared_hover(shucked_binary, tmp_path):
    (tmp_path / "library.sh").write_text("from_library() { printf ok; }\n")
    client = await start_host(shucked_binary, tmp_path)
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text='. ./library.sh\nfrom_library\n', language_id="bash")
        hover = await client.send_request("textDocument/hover", {"textDocument": {"uri": uri}, "position": {"line": 1, "character": 4}})
        assert "Function" in str(hover) and "Target:" in str(hover)
    finally:
        await client.shutdown_and_exit()

async def test_standard_pull_refresh_capability_delivers_environment_results(shucked_binary, tmp_path):
    import asyncio
    client = await start_host(shucked_binary, tmp_path, capabilities={
        "textDocument": {"diagnostic": {}},
        "workspace": {"diagnostics": {"refreshSupport": True}},
    }, options={"environment": {"cwd": "", "targetInventory": ""}})
    uri = (tmp_path / "script.sh").as_uri()
    try:
        await client.open_document(uri, text="shucked_missing_fixture\n", language_id="bash")
        await client.send_request("textDocument/diagnostic", {"textDocument": {"uri": uri}})
        for _ in range(100):
            if any(n["method"] == "workspace/diagnostic/refresh" for n in client._all_notifications):
                report = await client.send_request("textDocument/diagnostic", {"textDocument": {"uri": uri}})
                if any(d.get("code") == "ENV001" for d in report["items"]):
                    break
            await asyncio.sleep(0.05)
        else:
            raise AssertionError("Standard plural workspace.diagnostics did not refresh environment warnings")
    finally:
        await client.shutdown_and_exit()
