"""The VSIX under inspection: ``--vsix PATH`` or a fresh ``--build-vsix`` package."""

from __future__ import annotations

import shutil
import subprocess
import zipfile
from collections.abc import Iterator
from pathlib import Path

import pytest


@pytest.fixture(scope="session")
def vsix(request: pytest.FixtureRequest, extension_root: Path, tmp_path_factory: pytest.TempPathFactory) -> Path:
    given = request.config.getoption("--vsix")
    if given:
        return Path(given).resolve()
    if not request.config.getoption("--build-vsix"):
        pytest.skip("pass --vsix PATH or --build-vsix")
    node = shutil.which("node")
    if not node or not (extension_root / "node_modules").is_dir():
        pytest.fail("Building a VSIX needs Node.js and the extension's dependencies: (cd editors/vscode && bun install)")
    output = tmp_path_factory.mktemp("vsix") / "shucked.vsix"
    # vsce runs the prepublish step: a release build, bundled binaries, and provider runtimes.
    result = subprocess.run([node, "vsix.mjs", "package", "--out", str(output)], cwd=extension_root, capture_output=True, text=True, timeout=3600)
    if result.returncode:
        pytest.fail(f"VSIX packaging failed:\n{result.stdout[-4000:]}\n{result.stderr[-4000:]}")
    return output


@pytest.fixture(scope="session")
def archive(vsix: Path) -> Iterator[zipfile.ZipFile]:
    with zipfile.ZipFile(vsix) as bundle:
        yield bundle
