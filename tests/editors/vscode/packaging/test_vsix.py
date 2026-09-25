"""The packaged extension (VSIX): contents, manifest, bundled binaries, and exclusions."""

from __future__ import annotations

import json
import re
import stat
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path
from typing import Any

import pytest

BINARIES = ("shucked", "shucked-server")
SHELL_HOOKS = (
    "bash.sh",
    "zsh.zsh",
    "fish.fish",
    "capture.cjs",
    "live-bash.sh",
    "live-zsh.zsh",
    "live-fish.fish",
    "live-fish.cjs",
    "live-read.cjs",
    "live-result.cjs",
    "live-watchdog.cjs",
)


def _manifest(archive: zipfile.ZipFile) -> dict[str, Any]:
    return json.loads(archive.read("extension/package.json"))


def _binary(archive: zipfile.ZipFile, name: str) -> str:
    names = set(archive.namelist())
    return next(entry for entry in (f"extension/bin/{name}", f"extension/bin/{name}.exe") if entry in names)


def test_archive_structure(archive: zipfile.ZipFile) -> None:
    names = set(archive.namelist())
    for required in (
        "[Content_Types].xml",
        "extension.vsixmanifest",
        "extension/package.json",
        "extension/dist/extension.js",
        "extension/bin/platform.json",
    ):
        assert required in names, required
    for name in BINARIES:
        assert _binary(archive, name)


def test_packaged_manifest_matches_the_sources(archive: zipfile.ZipFile, extension_manifest: dict[str, Any]) -> None:
    packaged = _manifest(archive)
    for key in ("name", "publisher", "version", "engines", "activationEvents", "capabilities", "contributes", "main", "extensionKind"):
        assert packaged[key] == extension_manifest[key], key
    assert packaged["capabilities"]["untrustedWorkspaces"]["supported"] == "limited"
    assert packaged["extensionKind"] == ["workspace"]


def test_bundle_is_a_production_build(archive: zipfile.ZipFile) -> None:
    info = archive.getinfo("extension/dist/extension.js")
    assert info.file_size > 1000
    assert b"sourceMappingURL" not in archive.read(info), "production bundles ship without source maps"
    assert not [name for name in archive.namelist() if name.endswith(".map")]


def test_development_files_are_excluded(archive: zipfile.ZipFile) -> None:
    forbidden = re.compile(
        r"^extension/(?:src/|tests/|node_modules/|\.vscode|vscode-extension-samples|.*\.vsix$|bun\.lock$|tsconfig\.json$|eslint\.config\.mts$|esbuild\.mjs$|bundle-bins\.mjs$|vsix\.mjs$)"
    )
    assert [name for name in archive.namelist() if forbidden.match(name)] == []


def test_shell_integration_hooks_are_shipped(archive: zipfile.ZipFile) -> None:
    names = set(archive.namelist())
    assert {f"extension/shell-integration/{hook}" for hook in SHELL_HOOKS} <= names


def test_provider_packs_and_runtime_match_the_target(archive: zipfile.ZipFile) -> None:
    names = set(archive.namelist())
    assert "extension/bin/providers/packs/manifest.json" in names
    runtime = json.loads(archive.read("extension/bin/providers/runtime/manifest.json"))
    assert runtime["target"] == json.loads(archive.read("extension/bin/platform.json"))["target"]


def test_binaries_are_executable_and_report_the_workspace_version(archive: zipfile.ZipFile, extension_root: Path) -> None:
    cargo = (extension_root.parents[1] / "Cargo.toml").read_text()
    found = re.search(r'\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', cargo, re.MULTILINE | re.DOTALL)
    assert found, "workspace version not found in Cargo.toml"
    version = found.group(1)
    with tempfile.TemporaryDirectory() as temporary:
        for name in BINARIES:
            entry = _binary(archive, name)
            info = archive.getinfo(entry)
            assert info.file_size > 1_000_000, f"{entry} is too small"
            mode = info.external_attr >> 16
            assert mode & stat.S_IXUSR, f"{entry} must keep its executable bit, got {oct(mode)}"
            path = Path(archive.extract(entry, temporary))
            path.chmod(0o755)
            if name == "shucked":
                output = subprocess.run([str(path), "--version"], capture_output=True, text=True, check=True, timeout=30).stdout
                assert output.strip() == f"shucked {version}"


def test_target_platform_matches_the_bundled_binaries(archive: zipfile.ZipFile) -> None:
    bundled = json.loads(archive.read("extension/bin/platform.json"))
    identity = ET.fromstring(archive.read("extension.vsixmanifest")).find("{*}Metadata/{*}Identity")
    assert identity is not None
    assert identity.attrib.get("TargetPlatform") == bundled["target"] != "web"


def test_file_inventory(archive: zipfile.ZipFile, snapshot) -> None:
    """Every shipped file is deliberate: new or missing files change this snapshot.

    Provider packs and runtimes are pinned by their own manifests, and binary
    names differ only by ``.exe`` between platforms, so both are normalised.
    """
    inventory = set()
    for name in archive.namelist():
        if name.endswith("/"):
            continue
        if name.startswith("extension/bin/providers/"):
            name = "/".join(name.split("/")[:4]) + "/**"
        inventory.add(re.sub(r"\.exe$", "", name))
    assert sorted(inventory) == snapshot


@pytest.mark.parametrize("name", BINARIES)
def test_binaries_match_the_platform_target(archive: zipfile.ZipFile, name: str) -> None:
    target = json.loads(archive.read("extension/bin/platform.json"))["target"]
    header = archive.read(_binary(archive, name))[:64]
    expected_format = {"linux": b"\x7fELF", "alpine": b"\x7fELF", "darwin": (b"\xcf\xfa\xed\xfe", b"\xca\xfe\xba\xbe"), "win32": b"MZ"}[
        target.split("-")[0]
    ]
    assert header.startswith(expected_format), f"{name} is not a {target} executable"
