"""Tests for LSP initialization handshake, capabilities, and server info."""

import pytest
from tests.lsp.client import LspClient


@pytest.mark.asyncio
async def test_lsp_initialize_handshake_and_server_info(lsp_client: LspClient):
    """Verify initialize request returns valid serverInfo and name 'shucked'."""
    result = await lsp_client.initialize()
    assert result is not None, "initialize response must have a result"

    # Verify serverInfo
    server_info = result.get("serverInfo")
    assert server_info is not None, "serverInfo must be present in initialize result"
    assert server_info.get("name") == "shucked", "serverInfo.name must be 'shucked'"
    assert "version" in server_info, "serverInfo must include version"

    # Complete handshake
    await lsp_client.initialized()


@pytest.mark.asyncio
async def test_lsp_capabilities_advertisement(lsp_client: LspClient):
    """Verify server advertises expected LSP capabilities: hover, completion, definition, formatting, code actions."""
    result = await lsp_client.initialize()
    capabilities = result.get("capabilities", {})

    # 1. Hover
    assert capabilities.get("hoverProvider") is True, "hoverProvider must be True"

    # 2. Completion
    completion_provider = capabilities.get("completionProvider")
    assert completion_provider is not None, "completionProvider must be advertised"
    assert "triggerCharacters" in completion_provider

    # 3. Definitions
    assert capabilities.get("definitionProvider") is True, "definitionProvider must be True"

    # 4. Document Formatting & Range Formatting
    assert capabilities.get("documentFormattingProvider") is True, (
        "documentFormattingProvider must be True"
    )
    assert capabilities.get("documentRangeFormattingProvider") is True, (
        "documentRangeFormattingProvider must be True"
    )

    # 5. Code Actions
    code_action_provider = capabilities.get("codeActionProvider")
    assert code_action_provider is not None, "codeActionProvider must be advertised"
    kinds = code_action_provider.get("codeActionKinds", [])
    assert "quickfix" in kinds, "quickfix must be in codeActionKinds"
    assert "source.fixAll.shucked" in kinds, "source.fixAll.shucked must be in codeActionKinds"


@pytest.mark.asyncio
async def test_shutdown_and_exit(lsp_client: LspClient):
    """Verify server cleanly shuts down and exits with 0."""
    await lsp_client.initialize()
    await lsp_client.initialized()

    shutdown_res = await lsp_client.send_request("shutdown", None)
    assert shutdown_res is None, "shutdown request should return None"

    await lsp_client.send_notification("exit", None)
    await lsp_client.proc.wait()
    assert lsp_client.proc.returncode == 0, "server should exit with code 0"
