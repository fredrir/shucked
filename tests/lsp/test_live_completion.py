"""Live completer responses remain bound to the attached document/session."""
import asyncio
import os
import pytest
from tests.lsp.client import LspClient

pytestmark = pytest.mark.skipif(os.name != "posix", reason="POSIX fixture paths")

class LiveClient(LspClient):
    def __init__(self, binary, root):
        super().__init__(str(binary), dict(os.environ, PATH=str(root), HOME=str(root)))
        self.live_requests = asyncio.Queue()
        self.respond_live = True

    def _dispatch_message(self, message):
        if message.get("method") == "shucked/liveCompletion":
            self._all_notifications.append(message)
            self.live_requests.put_nowait(message)
            if self.respond_live:
                asyncio.create_task(self.finish_live(message))
            return
        super()._dispatch_message(message)

    async def finish_live(self, message):
        await self.send_message({"jsonrpc": "2.0", "id": message["id"], "result": {
            "candidates": [{"text": "alpha choice", "description": "Live fixture description"}], "partial": False,
        }})

async def setup(binary, root, filename="script.zsh", source="private_tool al", live=True):
    client = LiveClient(binary, root)
    await client.start()
    await client.initialize(root_uri=root.as_uri(), initialization_options={"nativeExecutionAllowed": True})
    await client.initialized()
    state = {"id": "fixture", "generation": 1, "cwd": str(root), "path": [str(root)], "functions": ["private_tool"], "aliases": {}, "shell": "zsh", "connected": True, "liveCompletion": live}
    await client.send_notification("shucked/shellSession", state)
    uri = (root / filename).as_uri()
    await client.open_document(uri, text=source, language_id="zsh")
    await client.send_notification("shucked/selectEnvironment", {"uri": uri, "options": {"sessionId": "fixture"}})
    return client, uri, state

async def complete(client, uri, column=15):
    response = await client.send_request("textDocument/completion", {"textDocument": {"uri": uri}, "position": {"line": 0, "character": column}}, timeout=5)
    return response.get("items", []) if isinstance(response, dict) else response or []

async def test_live_function_completer_returns_quoted_edits(shucked_binary, tmp_path):
    client, uri, _ = await setup(shucked_binary, tmp_path)
    try:
        # Initial filesystem watch registration may invalidate the first snapshot.
        # A client retries an incomplete request against the newly current context.
        async with asyncio.timeout(3):
            while True:
                items = await complete(client, uri)
                item = next((item for item in items if item["label"] == "alpha choice"), None)
                if item is not None:
                    break
                await asyncio.sleep(0.05)
        assert item["detail"] == "Live fixture description"
        assert item["textEdit"]["newText"] == "alpha\\ choice"
        request = client.live_requests.get_nowait()["params"]
        assert request["words"] == ["private_tool"] and request["prefix"] == "al"
        assert request["generation"] == 1 and request["version"] == 1
    finally:
        await client.shutdown_and_exit()

async def test_changed_session_rejects_inflight_live_completion(shucked_binary, tmp_path):
    client, uri, state = await setup(shucked_binary, tmp_path)
    client.respond_live = False
    try:
        task = asyncio.create_task(complete(client, uri))
        request = await asyncio.wait_for(client.live_requests.get(), 2)
        await client.send_notification("shucked/shellSession", dict(state, generation=2))
        await client.finish_live(request)
        assert not any(item["label"] == "alpha choice" for item in await task)
    finally:
        await client.shutdown_and_exit()

async def test_live_query_timeout_cancels_client_work_and_server_stays_responsive(shucked_binary, tmp_path):
    client, uri, _ = await setup(shucked_binary, tmp_path)
    client.respond_live = False
    try:
        assert not any(item["label"] == "alpha choice" for item in await complete(client, uri))
        request = client.live_requests.get_nowait()
        assert any(message.get("method") == "$/cancelRequest" and message["params"]["id"] == request["id"] for message in client._all_notifications)
        assert await client.send_request("textDocument/hover", {"textDocument": {"uri": uri}, "position": {"line": 0, "character": 2}})
    finally:
        await client.shutdown_and_exit()

@pytest.mark.parametrize("filename,source,column,live", [
    (".zshrc", "private_tool al", 15, True),
    ("script.zsh", "private_tool al", 15, False),
    ("script.zsh", "private_tool $(touch marker) al", 30, True),
])
async def test_startup_dynamic_and_unavailable_contexts_skip_live_queries(shucked_binary, tmp_path, filename, source, column, live):
    client, uri, _ = await setup(shucked_binary, tmp_path, filename, source, live)
    try:
        await complete(client, uri, column)
        assert client.live_requests.empty()
        assert not (tmp_path / "marker").exists()
    finally:
        await client.shutdown_and_exit()

@pytest.mark.parametrize("candidate,count", [
    ({"text": "alpha\0choice", "description": ""}, 1),
    ({"text": "alpha", "description": "x" * 16385}, 1),
    ({"text": "alpha", "description": ""}, 2001),
    ({"text": "alpha", "description": "x" * 1024}, 1024),
])
async def test_invalid_or_oversized_live_responses_do_not_reach_completion_items(shucked_binary, tmp_path, candidate, count):
    client, uri, _ = await setup(shucked_binary, tmp_path)
    async def respond(message):
        await client.send_message({"jsonrpc": "2.0", "id": message["id"], "result": {"candidates": [candidate] * count}})
    client.finish_live = respond
    try:
        assert not any(item["label"].startswith("alpha") for item in await complete(client, uri))
        assert not client.live_requests.empty()
    finally:
        await client.shutdown_and_exit()

@pytest.mark.parametrize("change", ["edit", "close", "portable"])
async def test_document_changes_discard_inflight_live_responses(shucked_binary, tmp_path, change):
    client, uri, _ = await setup(shucked_binary, tmp_path)
    client.respond_live = False
    try:
        task = asyncio.create_task(complete(client, uri))
        request = await asyncio.wait_for(client.live_requests.get(), 2)
        if change == "edit":
            await client.change_document(uri, "private_tool different", version=2)
        elif change == "close":
            await client.send_notification("textDocument/didClose", {"textDocument": {"uri": uri}})
        else:
            await client.send_notification("shucked/selectEnvironment", {"uri": uri, "options": {"policy": "portable"}})
        await client.finish_live(request)
        assert not any(item["label"] == "alpha choice" for item in await task)
    finally:
        await client.shutdown_and_exit()

@pytest.mark.parametrize("candidate,label,inserted", [
    ({"text": "alpha ", "encoding": "bashWord"}, "alpha", "alpha"),
    ({"text": "'alpha '", "encoding": "bashWord"}, "alpha ", "alpha\\ "),
    ({"text": "alpha "}, "alpha ", "alpha\\ "),
])
async def test_live_bash_word_encoding_preserves_literal_filename_spaces(shucked_binary, tmp_path, candidate, label, inserted):
    client, uri, _ = await setup(shucked_binary, tmp_path)
    client.respond_live = False
    try:
        task = asyncio.create_task(complete(client, uri))
        request = await asyncio.wait_for(client.live_requests.get(), 2)
        await client.send_message({"jsonrpc": "2.0", "id": request["id"], "result": {"candidates": [candidate], "partial": False}})
        item = next(item for item in await task if item["label"] == label)
        assert item["textEdit"]["newText"] == inserted
    finally:
        await client.shutdown_and_exit()

@pytest.mark.parametrize("candidate", [
    {"text": "alpha\nother"},
    {"text": '"alpha\nother"', "encoding": "bashWord"},
    {"text": "alpha$(touch marker)", "encoding": "bashWord"},
])
async def test_live_completion_drops_control_characters_and_dynamic_words(shucked_binary, tmp_path, candidate):
    client, uri, _ = await setup(shucked_binary, tmp_path)
    client.respond_live = False
    try:
        task = asyncio.create_task(complete(client, uri))
        request = await asyncio.wait_for(client.live_requests.get(), 2)
        await client.send_message({"jsonrpc": "2.0", "id": request["id"], "result": {"candidates": [candidate], "partial": False}})
        assert not any(item.get("detail") == "Live shell completion" for item in await task)
        assert not (tmp_path / "marker").exists()
    finally:
        await client.shutdown_and_exit()
