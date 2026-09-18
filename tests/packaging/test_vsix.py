"""Tests for the packaged VS Code extension (VSIX).

Validates:
- Building the extension package via `bun run package && bun run vsix`
- Inspection of .vsix archive using zipfile
- Manifest correctness: name, displayName, workspace trust, extensionKind, language activation
- Bundled assets: extension/dist/extension.js
- Exclusions: .vscode-extension-samples/ and other development artifacts
- Binary bundling paths and executable permissions
"""

import json
import os
import shutil
import stat
import subprocess
import tempfile
import zipfile
from pathlib import Path
import pytest


@pytest.fixture(scope="module")
def vsix_path(repo_root: Path) -> Path:
    """Build and locate the packaged .vsix file in editors/vscode."""
    vscode_dir = repo_root / "editors" / "vscode"

    # Run packaging commands
    build_result = subprocess.run(
        ["bun", "run", "package"],
        cwd=vscode_dir,
        capture_output=True,
        text=True,
        check=False,
    )
    assert (
        build_result.returncode == 0
    ), f"bun run package failed:\n{build_result.stderr}\n{build_result.stdout}"

    vsix_result = subprocess.run(
        ["bun", "run", "vsix"],
        cwd=vscode_dir,
        capture_output=True,
        text=True,
        check=False,
    )
    assert (
        vsix_result.returncode == 0
    ), f"bun run vsix failed:\n{vsix_result.stderr}\n{vsix_result.stdout}"

    vsix_files = list(vscode_dir.glob("*.vsix"))
    assert len(vsix_files) > 0, f"No .vsix file found in {vscode_dir}"
    return max(vsix_files, key=lambda p: p.stat().st_mtime)


def test_vsix_archive_structure(vsix_path: Path):
    """Verify standard VSIX archive files exist."""
    with zipfile.ZipFile(vsix_path, "r") as archive:
        namelist = archive.namelist()
        assert "[Content_Types].xml" in namelist
        assert "extension.vsixmanifest" in namelist
        assert "extension/package.json" in namelist
        assert "extension/dist/extension.js" in namelist


def test_vsix_manifest_metadata(vsix_path: Path):
    """Verify package.json manifest attributes inside .vsix."""
    with zipfile.ZipFile(vsix_path, "r") as archive:
        package_json_data = archive.read("extension/package.json")
        manifest = json.loads(package_json_data.decode("utf-8"))

        assert manifest.get("name") == "shucked", f"Expected name 'shucked', got: {manifest.get('name')}"
        assert manifest.get("displayName") == "Shucked", (
            f"Expected displayName 'Shucked', got: {manifest.get('displayName')}"
        )
        assert manifest.get("extensionKind") == ["workspace"], (
            f"Expected extensionKind ['workspace'], got: {manifest.get('extensionKind')}"
        )

        # Workspace trust capability
        capabilities = manifest.get("capabilities", {})
        workspace_trust = capabilities.get("untrustedWorkspaces", {})
        assert workspace_trust.get("supported") is True, (
            f"Workspace trust should be supported: {workspace_trust}"
        )

        # Activation events for shellscript
        activation_events = manifest.get("activationEvents", [])
        assert "onLanguage:shellscript" in activation_events, (
            f"Expected 'onLanguage:shellscript' in activationEvents: {activation_events}"
        )


def test_vsix_bundled_distribution_file(vsix_path: Path):
    """Verify extension/dist/extension.js is present and non-empty."""
    with zipfile.ZipFile(vsix_path, "r") as archive:
        info = archive.getinfo("extension/dist/extension.js")
        assert info.file_size > 1000, f"extension.js too small: {info.file_size} bytes"


def test_vsix_excludes_development_files(vsix_path: Path):
    """Verify .vscodeignore rules exclude development samples and node_modules."""
    with zipfile.ZipFile(vsix_path, "r") as archive:
        namelist = archive.namelist()
        for name in namelist:
            assert not name.startswith("extension/.vscode-extension-samples"), (
                f"Development sample file leaked into VSIX: {name}"
            )
            assert not name.startswith("extension/vscode-extension-samples"), (
                f"Development sample file leaked into VSIX: {name}"
            )
            assert not name.startswith("extension/node_modules"), (
                f"node_modules leaked into VSIX: {name}"
            )
            assert not name.startswith("extension/src/"), (
                f"TypeScript source directory leaked into VSIX: {name}"
            )


def test_vsix_binary_bundling_and_permissions(repo_root: Path):
    """Verify bundled binary path and preserved executable permissions in VSIX."""
    vscode_dir = repo_root / "editors" / "vscode"
    bin_dir = vscode_dir / "bin"
    bin_dir.mkdir(exist_ok=True)
    dummy_bin = bin_dir / "shucked"

    try:
        # Create an executable script simulating the target binary
        dummy_bin.write_text("#!/bin/sh\necho 'shucked binary'\n")
        dummy_bin.chmod(stat.S_IRUSR | stat.S_IWUSR | stat.S_IXUSR | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)

        # Repackage to test binary inclusion
        res = subprocess.run(
            ["bun", "run", "vsix"],
            cwd=vscode_dir,
            capture_output=True,
            text=True,
            check=False,
        )
        assert res.returncode == 0, f"vsix packaging with bin failed: {res.stderr}"

        vsix_file = max(vscode_dir.glob("*.vsix"), key=lambda p: p.stat().st_mtime)
        with zipfile.ZipFile(vsix_file, "r") as archive:
            assert "extension/bin/shucked" in archive.namelist(), (
                "extension/bin/shucked should be bundled in the archive"
            )
            info = archive.getinfo("extension/bin/shucked")
            mode = (info.external_attr >> 16) & 0o777
            assert mode & 0o111 != 0, f"Bundled binary must have executable permissions, got: {oct(mode)}"

    finally:
        # Clean up the test binary and re-package cleanly
        if dummy_bin.exists():
            dummy_bin.unlink()
        if bin_dir.exists():
            shutil.rmtree(bin_dir, ignore_errors=True)
        # Restore clean vsix package
        subprocess.run(
            ["bun", "run", "vsix"],
            cwd=vscode_dir,
            capture_output=True,
            text=True,
            check=False,
        )
