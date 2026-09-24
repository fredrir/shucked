"""Argument providers must not be replaced by unrelated file guesses while loading."""

import asyncio
import os

import pytest

from tests.lsp.client import LspClient
from tests.lsp.test_native_completion import complete_when_ready, fixture_host, install_tool, wait_for_completion_ready


pytestmark = pytest.mark.skipif(os.name != "posix", reason="Unix native providers")


@pytest.mark.parametrize("dialect", ["bash", "zsh", "fish"])
async def test_argument_results_remain_relevant_through_background_refresh(shucked_binary, tmp_path, dialect):
    host, definitions, environment = fixture_host(tmp_path)
    (host / "workspace-noise").write_text("unrelated document")
    (host / "workspace-folder").mkdir()
    (definitions / "_shucked_fixture").write_text("""#compdef shucked-fixture
/bin/sleep 0.25
case $words[2] in
  empty) _message 'No values in this context' ;;
  *) _arguments '1:action:(install inspect empty)' ;;
esac
""")
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri(), initialization_options={"nativeExecutionAllowed": True})
        await client.initialized()
        uri = (host / f"script.{dialect}").as_uri()
        await client.open_document(uri, language_id="fish" if dialect == "fish" else "shellscript", text="")
        for version, (line, expected) in enumerate([
            ("shucked-fixture ", "install"),
            ("shucked-fixture in", "install"),
            ("shucked-fixture empty ", None),
        ], 2):
            await client.change_document(uri, text=line, version=version)
            start = len(client._all_notifications)
            async with asyncio.timeout(5):
                while True:
                    result = await client.completion(uri, 0, len(line))
                    labels = {item["label"] for item in result["items"]}
                    assert "workspace-noise" not in labels
                    assert "workspace-folder/" not in labels
                    if expected is not None and expected in labels:
                        break
                    if expected is None and not result["isIncomplete"]:
                        assert not labels
                        break
                    await wait_for_completion_ready(client, uri, 0, len(line), version=version, start=start)
                    start = len(client._all_notifications)
    finally:
        await client.shutdown_and_exit()


async def test_unavailable_provider_releases_explicitly_typed_file_fallback(shucked_binary, tmp_path):
    host, _, environment = fixture_host(tmp_path)
    install_tool(host / "bin", "unregistered-fixture", "exit 9")
    (host / "chosen-file").write_text("")
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri(), initialization_options={"nativeExecutionAllowed": True})
        await client.initialized()
        uri = (host / "script.sh").as_uri()
        line = "unregistered-fixture chosen"
        await client.open_document(uri, text=line)
        result = await complete_when_ready(client, uri, 0, len(line), "chosen-file")
        assert any(item["label"] == "chosen-file" and item["kind"] == 17 for item in result["items"])
    finally:
        await client.shutdown_and_exit()
