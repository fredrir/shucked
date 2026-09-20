import json
import os

from tests.lsp.client import LspClient
from tests.lsp.test_native_completion import complete_when_ready


async def test_completion_uses_server_environment_and_never_executes_candidates(
    shucked_binary, tmp_path
):
    host = tmp_path / "server-host"
    host.mkdir()
    binary_directory = host / "bin"
    binary_directory.mkdir()
    marker = host / "must-not-exist"
    command = binary_directory / "shucked-test-remote.exe"
    command.write_text(f"#!/bin/sh\nprintf executed > '{marker}'\n")
    command.chmod(0o755)
    (binary_directory / "shucked-test-nonexecutable").write_text("plain file")
    (host / "remote file.txt").write_text("")
    environment = dict(os.environ)
    environment.update(
        PATH=str(binary_directory),
        HOME=str(host),
        SHUCKED_REMOTE_TEST_VARIABLE="value-that-must-not-appear-in-completions",
    )
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri())
        await client.initialized()
        uri = (host / "script.sh").as_uri()
        lines = [
            "shucked-test-r",
            "echo $SHUCKED_REMOTE_TEST_",
            "cat ~/remote",
        ]
        await client.open_document(uri, text="\n".join(lines))
        expected = ["shucked-test-remote.exe", "SHUCKED_REMOTE_TEST_VARIABLE", "remote file.txt"]
        results = [await complete_when_ready(client, uri, index, len(line), expected[index])
                   for index, line in enumerate(lines)]
        assert "shucked-test-remote.exe" in {
            item["label"] for item in results[0]["items"]
        }
        assert "shucked-test-nonexecutable" not in {
            item["label"] for item in results[0]["items"]
        }
        assert "SHUCKED_REMOTE_TEST_VARIABLE" in {
            item["label"] for item in results[1]["items"]
        }
        assert "remote file.txt" in {
            item["label"] for item in results[2]["items"]
        }
        assert environment["SHUCKED_REMOTE_TEST_VARIABLE"] not in json.dumps(results)
        assert not marker.exists()
        await client.send_notification(
            "workspace/didChangeConfiguration",
            {"settings": {"shucked": {"server": {"completion": {
                "includeEnvironment": False,
                "includePaths": False,
                "includeCommandArguments": False,
            }}}}},
        )
        for index, line in enumerate(lines):
            result = await client.completion(uri, index, len(line))
            assert result["items"] == []
    finally:
        await client.shutdown_and_exit()
