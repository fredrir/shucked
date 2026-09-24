"""The server keeps answering requests after unusual client traffic."""

import pytest
from tests.lsp.client import LspClient


@pytest.mark.asyncio
async def test_unexpected_response_does_not_block_the_server(initialized_lsp_client: LspClient):
    """A reply to a request the server never sent (or already cancelled) is logged, not fatal.

    Logging used to go to stdout, whose lock the protocol writer holds for the
    whole session, so the log line blocked the main loop forever.
    """
    await initialized_lsp_client.send_message({"jsonrpc": "2.0", "id": "never-sent", "result": None})
    uri = "file:///tmp/robustness.sh"
    await initialized_lsp_client.open_document(uri, "shellscript", "#!/bin/bash\necho ok\n")
    result = await initialized_lsp_client.send_request(
        "textDocument/hover",
        {"textDocument": {"uri": uri}, "position": {"line": 1, "character": 1}},
        timeout=10.0,
    )
    assert result is None or "contents" in result
