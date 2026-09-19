"""Tests for LSP push diagnostics handling on didOpen and didChange."""

import pytest

from tests.lsp.client import LspClient


@pytest.mark.asyncio
async def test_diagnostics_on_did_open(initialized_lsp_client: LspClient):
    """Verify push diagnostics are published on didOpen and source is 'shucked'."""
    uri = "file:///tmp/unused_var_test.sh"
    # Unused variable triggers rule C001
    text = """#!/bin/bash
unused_variable=42
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)

    assert len(diags) > 0, "Expected at least one diagnostic for unused variable"
    diag = diags[0]
    assert diag.get("source") == "shucked", (
        f"Diagnostic source must be 'shucked', got: {diag.get('source')}"
    )
    assert diag.get("code") == "C001"
    assert "unused_variable" in diag.get("message", "")
    assert "range" in diag
    assert diag["range"]["start"]["line"] == 1


@pytest.mark.asyncio
async def test_diagnostics_update_on_did_change(initialized_lsp_client: LspClient):
    """Verify push diagnostics are updated when document content changes."""
    uri = "file:///tmp/change_diag_test.sh"
    # Start with an unused variable
    text_v1 = """#!/bin/bash
var=100
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text_v1, version=1)
    diags_v1 = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert len(diags_v1) == 1
    assert diags_v1[0]["code"] == "C001"

    # Now use the variable, resolving the unused variable diagnostic
    text_v2 = """#!/bin/bash
var=100
echo "$var"
"""
    await initialized_lsp_client.change_document(uri, text_v2, version=2)
    diags_v2 = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert len(diags_v2) == 0, (
        f"Expected diagnostics to clear after using variable, got: {diags_v2}"
    )


@pytest.mark.asyncio
async def test_diagnostics_cleared_by_directive_comment(
    initialized_lsp_client: LspClient,
):
    """Verify inline suppression directive comments suppress diagnostics."""
    uri = "file:///tmp/suppress_diag_test.sh"
    text_with_suppression = """#!/bin/bash
unused=1  # shuck: ignore=C001
"""
    await initialized_lsp_client.open_document(
        uri, "shellscript", text_with_suppression
    )
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], (
        f"Expected diagnostic to be suppressed by directive, got: {diags}"
    )
