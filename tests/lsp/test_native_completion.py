"""Installed upstream definitions work without command-specific server logic."""

import asyncio
import os
import time

import pytest

from tests.lsp.client import LspClient

pytestmark = pytest.mark.skipif(os.name != "posix", reason="Unix native providers")


def install_tool(root, name, body):
    path = root / name
    path.write_text(f"#!/bin/sh\n{body}\n")
    path.chmod(0o755)


async def wait_for_completion_ready(client, uri, line, character, version=1, start=0):
    async with asyncio.timeout(5):
        while True:
            for notification in client._all_notifications[start:]:
                params = notification.get("params", {})
                if (notification.get("method") == "shucked/completionReady"
                        and params.get("uri") == uri
                        and params.get("version") == version
                        and params.get("position") == {"line": line, "character": character}):
                    return params
            await asyncio.sleep(0.01)


async def complete_when_ready(client, uri, line, character, expected, version=1):
    start = len(client._all_notifications)
    async with asyncio.timeout(5):
        while True:
            result = await client.completion(uri, line, character)
            if expected in {item["label"] for item in result["items"]}:
                return result
            await wait_for_completion_ready(client, uri, line, character, version=version, start=start)
            start = len(client._all_notifications)


def fixture_host(tmp_path):
    host = tmp_path / "remote"
    bin_dir = host / "bin"
    bin_dir.mkdir(parents=True)
    definitions = host / "share/zsh/site-functions"
    definitions.mkdir(parents=True)
    install_tool(bin_dir, "shucked-fixture", "printf executed > native-ran; exit 9")
    (definitions / "_shucked_fixture").write_text("""#compdef shucked-fixture
case $words[2] in
  install) _arguments '--kind=[Choose kind]:kind:(binary source)' '*:package:(remote-package)' ;;
  *) _arguments '--absolute[Show full entry paths]' '1:action:(install inspect)' ;;
esac
""")
    (host / ".zshrc").write_text("exit 1\n")
    environment = dict(os.environ, PATH=str(bin_dir), HOME=str(host), ZDOTDIR=str(host))
    return host, definitions, environment


async def test_arbitrary_installed_definitions_complete_without_shell_configuration(shucked_binary, tmp_path):
    host, _, environment = fixture_host(tmp_path)
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri(), initialization_options={"nativeExecutionAllowed": True})
        await client.initialized()
        uri = (host / "script.sh").as_uri()
        lines = ["shucked-fixture ", "shucked-fixture install remote-", "shucked-fixture --absuffix", "shucked-fixture -- --abs"]
        await client.open_document(uri, text="\n".join(lines))
        await complete_when_ready(client, uri, 0, len(lines[0]), "install")
        await complete_when_ready(client, uri, 1, len(lines[1]), "remote-package")
        flags = await complete_when_ready(client, uri, 2, len("shucked-fixture --abs"), "--absolute")
        flag = next(item for item in flags["items"] if item["label"] == "--absolute")
        assert "Show full entry paths" in flag["detail"]
        assert flag["textEdit"]["newText"] == "--absolute"
        assert flag["textEdit"]["range"]["end"]["character"] == len(lines[2])
        after_separator = await client.completion(uri, 3, len(lines[3]))
        assert "--absolute" not in {item["label"] for item in after_separator["items"]}
        assert not (host / "native-ran").exists()
    finally:
        await client.shutdown_and_exit()


async def test_slow_provider_returns_immediately_and_notifies_without_another_keystroke(shucked_binary, tmp_path):
    host, definitions, environment = fixture_host(tmp_path)
    (definitions / "_shucked_fixture").write_text("#compdef shucked-fixture\n/bin/sleep 0.5\n_arguments '--delayed[Provider result]'\n")
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri(), initialization_options={"nativeExecutionAllowed": True})
        await client.initialized()
        uri = (host / "script.sh").as_uri()
        line = "shucked-fixture --del"
        await client.open_document(uri, text=line)
        start = len(client._all_notifications)
        before = time.monotonic()
        initial = await client.completion(uri, 0, len(line))
        assert time.monotonic() - before < 0.35
        result = initial
        async with asyncio.timeout(5):
            while "--delayed" not in {item["label"] for item in result["items"]}:
                ready = await wait_for_completion_ready(client, uri, 0, len(line), start=start)
                start = len(client._all_notifications)
                if ready.get("reason") not in ("environmentChanged", "analysisReady"):
                    assert ready["candidateCount"] > 0
                assert ready["generation"] >= 0
                result = await client.completion(uri, 0, len(line))
        assert "--delayed" in {item["label"] for item in result["items"]}
    finally:
        await client.shutdown_and_exit()


async def test_workspace_configuration_cannot_grant_native_execution(shucked_binary, tmp_path):
    host, definitions, environment = fixture_host(tmp_path)
    (definitions / "_shucked_fixture").write_text("#compdef shucked-fixture\nprintf executed > native-ran\ncompadd remote-package\n")
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri())
        await client.initialized()
        uri = (host / "script.sh").as_uri()
        line = "shucked-fixture remote-"
        await client.open_document(uri, text=line)
        for promote in [False, True]:
            if promote:
                await client.send_notification("workspace/didChangeConfiguration", {"settings": {"shucked": {"nativeExecutionAllowed": True}}})
            result = await client.completion(uri, 0, len(line))
            assert "remote-package" not in {item["label"] for item in result["items"]}
            await asyncio.sleep(0.05)
            assert not (host / "native-ran").exists()
    finally:
        await client.shutdown_and_exit()
