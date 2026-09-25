"""Platform targets as the extension names them (see editors/vscode/platform.mjs)."""

from __future__ import annotations

import json
import platform
import sys
import zipfile
from pathlib import Path


def host_target() -> str:
    """This machine's VSIX target, e.g. ``linux-x64`` or ``darwin-arm64``."""
    cpu = {"x86_64": "x64", "amd64": "x64", "aarch64": "arm64", "arm64": "arm64", "armv7l": "armhf"}.get(platform.machine().lower(), "")
    family = "win32" if sys.platform == "win32" else sys.platform
    if family == "linux" and Path("/etc/alpine-release").exists():
        family = "alpine"
    return f"{family}-{cpu}"


def vsix_target(vsix: Path) -> str:
    """The target a packaged VSIX was built for."""
    with zipfile.ZipFile(vsix) as archive:
        return json.loads(archive.read("extension/bin/platform.json"))["target"]
