"""Download and cache a pinned VS Code build for tests.

Tests never use the developer's own ``code`` installation unless explicitly
asked to with ``--vscode-executable``; a pinned build keeps editor behaviour
reproducible. The default version is the manifest's minimum ``engines.vscode``.
"""

from __future__ import annotations

import contextlib
import json
import os
import platform
import shutil
import sys
import tarfile
import tempfile
import time
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

RELEASES = "https://update.code.visualstudio.com"


@dataclass(frozen=True)
class Installation:
    version: str
    root: Path
    executable: Path
    cli: Path


def minimum_engine(manifest: Path) -> str:
    engine = json.loads(manifest.read_text())["engines"]["vscode"]
    return engine.lstrip("^~>=")


def resolve_version(requested: str) -> str:
    if requested != "stable":
        return requested
    with urllib.request.urlopen(f"{RELEASES}/api/releases/stable", timeout=60) as response:
        return json.loads(response.read())[0]


def _platform() -> tuple[str, str]:
    """Download platform id and archive suffix for this host."""
    machine = platform.machine().lower()
    arm = machine in ("arm64", "aarch64")
    if sys.platform.startswith("linux"):
        return ("linux-arm64" if arm else "linux-x64"), ".tar.gz"
    if sys.platform == "darwin":
        return ("darwin-arm64" if arm else "darwin"), ".zip"
    if sys.platform == "win32":
        return ("win32-arm64-archive" if arm else "win32-x64-archive"), ".zip"
    raise RuntimeError(f"No VS Code build for {sys.platform}/{machine}")


def _locate(root: Path) -> tuple[Path, Path]:
    if sys.platform.startswith("linux"):
        base = next(root.glob("VSCode-linux-*"))
        return base / "code", base / "bin" / "code"
    if sys.platform == "darwin":
        app = next(root.glob("*.app"))
        executable = next(path for path in (app / "Contents/MacOS/Code", app / "Contents/MacOS/Electron") if path.exists())
        return executable, app / "Contents/Resources/app/bin/code"
    return root / "Code.exe", root / "bin" / "code.cmd"


@contextlib.contextmanager
def _lock(path: Path, timeout: float = 900.0):
    """Cross-process lock so parallel workers download a build only once."""
    deadline = time.monotonic() + timeout
    while True:
        try:
            descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            break
        except FileExistsError:
            with contextlib.suppress(FileNotFoundError):
                if time.time() - path.stat().st_mtime > timeout:
                    path.unlink()
                    continue
            if time.monotonic() > deadline:
                raise TimeoutError(f"Timed out waiting for {path}") from None
            time.sleep(0.5)
    try:
        yield
    finally:
        os.close(descriptor)
        path.unlink(missing_ok=True)


def install(version: str, cache: Path) -> Installation:
    resolved = resolve_version(version)
    target, suffix = _platform()
    root = cache / f"{resolved}-{target}"
    cache.mkdir(parents=True, exist_ok=True)
    with _lock(cache / f".{resolved}-{target}.lock"):
        if not (root / ".complete").exists():
            shutil.rmtree(root, ignore_errors=True)
            with tempfile.TemporaryDirectory(dir=cache) as temporary:
                archive = Path(temporary) / f"vscode{suffix}"
                with (
                    urllib.request.urlopen(f"{RELEASES}/{resolved}/{target}/stable", timeout=600) as response,
                    archive.open("wb") as output,
                ):
                    shutil.copyfileobj(response, output)
                staging = Path(temporary) / "staging"
                if suffix == ".tar.gz":
                    with tarfile.open(archive) as bundle:
                        # The archive comes from the official update service over HTTPS.
                        if hasattr(tarfile, "fully_trusted_filter"):
                            bundle.extractall(staging, filter="fully_trusted")
                        else:
                            bundle.extractall(staging)
                else:
                    with zipfile.ZipFile(archive) as bundle:
                        bundle.extractall(staging)
                    if sys.platform == "darwin":
                        # zipfile drops permission bits; restore executables inside the app bundle.
                        for path in staging.rglob("*"):
                            if path.is_file() and ("MacOS" in path.parts or path.parent.name == "bin"):
                                path.chmod(0o755)
                staging.rename(root)
            (root / ".complete").write_text(resolved)
    executable, cli = _locate(root)
    return Installation(resolved, root, executable, cli)


def from_executable(executable: Path) -> Installation:
    """Use an existing installation (``--vscode-executable``) as-is."""
    executable = executable.resolve()
    candidates = [
        executable.parent / "bin" / "code",
        executable.parent / "bin" / "code.cmd",
        executable.parents[1] / "Resources/app/bin/code",
    ]
    cli = next((path for path in candidates if path.exists()), executable)
    return Installation("custom", executable.parent, executable, cli)
