"""Tests for LSP code actions (quickfix and source.fixAll.shucked)."""

import pytest
from tests.lsp.client import LspClient


@pytest.mark.asyncio
async def test_code_action_quickfix_for_diagnostic(
    initialized_lsp_client: LspClient,
):
    """Verify codeAction returns quickfix for lint diagnostics."""
    uri = "file:///tmp/code_action_test.sh"
    # Unused variable on line 1
    text = """#!/bin/bash
unused_val=10
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert len(diags) > 0, "Expected diagnostics for unused variable"

    actions = await initialized_lsp_client.code_action(
        uri,
        range={"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 10}},
        diagnostics=diags,
        only=["quickfix"],
    )

    assert actions is not None, "Code actions request should return a list"
    assert len(actions) > 0, "Expected at least one quickfix action"

    # Find a quickfix action
    quickfixes = [a for a in actions if a.get("kind") == "quickfix"]
    assert len(quickfixes) > 0, f"Expected quickfix action in: {actions}"

    action = quickfixes[0]
    assert "title" in action
    assert "edit" in action
    assert uri in action["edit"].get("changes", {})


@pytest.mark.asyncio
async def test_code_action_fix_all_shucked(lsp_client_factory):
    """Verify codeAction returns source.fixAll.shucked when enabled."""
    client = await lsp_client_factory(initialization_options={"unsafeFixes": True})
    uri = "file:///tmp/fix_all_test.sh"
    text = """#!/bin/bash
dummy_var=99
"""
    await client.open_document(uri, "shellscript", text)
    diags = await client.wait_for_diagnostics(uri)
    assert len(diags) > 0, "Expected diagnostics for unused variable"

    actions = await client.code_action(
        uri,
        range={"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 10}},
        diagnostics=diags,
        only=["source.fixAll.shucked"],
    )

    assert actions is not None, "Code action response should not be None"
    assert len(actions) > 0, "Expected fix-all action"

    fix_all_actions = [a for a in actions if a.get("kind") == "source.fixAll.shucked"]
    assert len(fix_all_actions) > 0, (
        f"Expected 'source.fixAll.shucked' action, got: {actions}"
    )

    action = fix_all_actions[0]
    assert action.get("isPreferred") is True
    assert "edit" in action
    changes = action["edit"].get("changes", {})
    assert uri in changes
    assert len(changes[uri]) > 0
