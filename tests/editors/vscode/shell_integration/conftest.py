"""Fixtures for exercising the shipped shell hooks in real shells."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest

from ..harness import processes

pytestmark = pytest.mark.skipif(__import__("sys").platform == "win32", reason="Unix signal transport")


@pytest.fixture(scope="session", autouse=True)
def _reap_hook_workers() -> None:
    # Workers and their children are reparented here when their shell exits first.
    processes.adopt_orphans()


@pytest.fixture(scope="session")
def integration(extension_root: Path) -> Path:
    return extension_root / "shell-integration"


@pytest.fixture
def shell_path(request: pytest.FixtureRequest, require_shell: Callable[[str], str]) -> str:
    """The shell named by the test's ``shell`` parameter, or a skip/failure when missing."""
    return require_shell(request.node.callspec.params["shell"])
