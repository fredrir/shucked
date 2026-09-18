"""Tests for LSP document and range formatting."""

import pytest
from tests.lsp.client import LspClient


@pytest.mark.asyncio
async def test_document_formatting_unindented_block(initialized_lsp_client: LspClient):
    """Verify document formatting produces indentation edits for unindented blocks."""
    uri = "file:///tmp/unformatted_test.sh"
    # Unindented if block
    text = """#!/bin/bash
if true; then
echo "unindented body"
fi
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    await initialized_lsp_client.wait_for_diagnostics(uri)

    edits = await initialized_lsp_client.formatting(uri, tab_size=2, insert_spaces=True)
    assert edits is not None, "Formatting should return text edits"
    assert len(edits) > 0, "Expected at least one formatting edit for unindented body"

    # Verify that the edit targets line 2 (the echo statement)
    first_edit = edits[0]
    assert "range" in first_edit
    assert first_edit["range"]["start"]["line"] == 2


@pytest.mark.asyncio
async def test_document_formatting_already_formatted(
    initialized_lsp_client: LspClient,
):
    """Verify formatting an already formatted document returns empty edits or None."""
    uri = "file:///tmp/formatted_test.sh"
    # A single-line script or already clean script
    text = """#!/bin/bash
echo "clean"
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    await initialized_lsp_client.wait_for_diagnostics(uri)

    edits = await initialized_lsp_client.formatting(uri, tab_size=2, insert_spaces=True)
    assert edits is None or len(edits) == 0, (
        f"Expected no formatting edits for clean document, got: {edits}"
    )


@pytest.mark.asyncio
async def test_document_range_formatting(initialized_lsp_client: LspClient):
    """Verify range formatting targets the requested lines."""
    uri = "file:///tmp/range_format_test.sh"
    text = """#!/bin/bash
if true; then
echo "inside"
fi
echo "done"
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    await initialized_lsp_client.wait_for_diagnostics(uri)

    # Format just line 2
    edits = await initialized_lsp_client.range_formatting(
        uri, start_line=1, start_char=0, end_line=3, end_char=2
    )
    assert edits is not None, "Range formatting should return edits"
    assert len(edits) > 0, "Expected range formatting edits"
