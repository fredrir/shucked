"""Pytest configuration and fixtures for Shucked test suite."""

import sys
from collections.abc import AsyncGenerator, Callable
from pathlib import Path
from typing import Optional

import pytest
import pytest_asyncio

# Ensure repo root is on sys.path
_repo_root = Path(__file__).resolve().parent.parent
if str(_repo_root) not in sys.path:
    sys.path.insert(0, str(_repo_root))

from tests.lsp.client import LspClient

# Options and test selection for the VS Code extension suites.
pytest_plugins = ("editors.vscode.plugin",)


@pytest.fixture(scope="session")
def repo_root() -> Path:
    """Return the repository root directory."""
    return Path(__file__).resolve().parent.parent


@pytest.fixture(scope="session")
def shucked_binary(repo_root: Path) -> Path:
    """Return the path to target/debug/shucked binary."""
    binary = repo_root / "target" / "debug" / "shucked"
    if not binary.exists():
        pytest.fail(f"shucked binary not found at {binary}. Run `cargo build` first.")
    return binary


@pytest_asyncio.fixture
async def lsp_client(shucked_binary: Path) -> AsyncGenerator[LspClient, None]:
    """Provide a started but uninitialized LspClient."""
    client = LspClient(binary_path=str(shucked_binary))
    await client.start()
    try:
        yield client
    finally:
        await client.shutdown_and_exit()


@pytest_asyncio.fixture
async def initialized_lsp_client(
    lsp_client: LspClient,
) -> AsyncGenerator[LspClient, None]:
    """Provide an initialized LspClient ready for document operations."""
    await lsp_client.initialize()
    await lsp_client.initialized()
    yield lsp_client


@pytest_asyncio.fixture
async def lsp_client_factory(
    shucked_binary: Path,
) -> AsyncGenerator[Callable[..., AsyncGenerator[LspClient, None]], None]:
    """Factory fixture to create LspClient with custom options."""
    clients = []

    async def _create(
        capabilities: dict | None = None,
        initialization_options: dict | None = None,
        initialize: bool = True,
    ) -> LspClient:
        client = LspClient(binary_path=str(shucked_binary))
        await client.start()
        clients.append(client)
        if initialize:
            await client.initialize(
                capabilities=capabilities,
                initialization_options=initialization_options,
            )
            await client.initialized()
        return client

    try:
        yield _create
    finally:
        for c in clients:
            await c.shutdown_and_exit()
