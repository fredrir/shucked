"""Native candidates come from the workspace host without shell setup."""

import os

import pytest

from tests.lsp.client import LspClient

pytestmark = pytest.mark.skipif(os.name != "posix", reason="Unix native providers")


def install_tool(root, name, body):
    path = root / name
    path.write_text(f"#!/bin/sh\n{body}\n")
    path.chmod(0o755)


async def test_native_completions_ship_without_shell_configuration(shucked_binary, tmp_path):
    host = tmp_path / "remote"
    host.mkdir()
    bin_dir = host / "bin"
    bin_dir.mkdir()
    install_tool(bin_dir, "pacman", "[ \"$*\" = '-Slq' ] || exit 9; printf 'remote-package\\n'")
    install_tool(bin_dir, "brew", "case \"$*\" in formulae) printf 'remote-formula\\n' ;; casks) printf 'remote-cask\\n' ;; *) exit 9 ;; esac")
    install_tool(bin_dir, "eza", "[ \"$*\" = '--help' ] || exit 9; printf '  --absolute  Show full entry paths\\n  -a, --all  Include hidden entries\\n'")
    (host / ".zshrc").write_text("exit 1\n")
    environment = dict(os.environ, PATH=str(bin_dir), HOME=str(host), ZDOTDIR=str(host))
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=host.as_uri(), initialization_options={"nativeExecutionAllowed": True})
        await client.initialized()
        uri = (host / "script.sh").as_uri()
        lines = ["pacman -S remote-p", "brew install remote-", "brew install --cask remote-", "eza --absuffix", "eza -- --abs"]
        await client.open_document(uri, text="\n".join(lines))
        packages = await client.completion(uri, 0, len(lines[0]))
        assert "remote-package" in {item["label"] for item in packages["items"]}
        assert packages["isIncomplete"]  # Refresh dynamic candidates as typing continues.
        brew = await client.completion(uri, 1, len(lines[1]))
        assert {"remote-formula", "remote-cask"} <= {item["label"] for item in brew["items"]}
        casks = await client.completion(uri, 2, len(lines[2]))
        assert {item["label"] for item in casks["items"]} == {"remote-cask"}
        flags = await client.completion(uri, 3, len("eza --abs"))
        flag = next(item for item in flags["items"] if item["label"] == "--absolute")
        assert flag["detail"] == "Show full entry paths"
        assert flag["textEdit"]["newText"] == "--absolute"
        assert flag["textEdit"]["range"]["end"]["character"] == len(lines[3])
        after_separator = await client.completion(uri, 4, len(lines[4]))
        assert "--absolute" not in {item["label"] for item in after_separator["items"]}
        await client.send_notification("workspace/didChangeConfiguration", {"settings": {"shucked": {"server": {"completion": {"includeNative": False}}}}})
        disabled = await client.completion(uri, 0, len(lines[0]))
        assert "remote-package" not in {item["label"] for item in disabled["items"]}
    finally:
        await client.shutdown_and_exit()


async def test_workspace_configuration_cannot_grant_native_execution(shucked_binary, tmp_path):
    install_tool(tmp_path, "pacman", "printf executed > native-ran; printf 'remote-package\\n'")
    environment = dict(os.environ, PATH=str(tmp_path))
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    try:
        await client.initialize(root_uri=tmp_path.as_uri())
        await client.initialized()
        uri = (tmp_path / "script.sh").as_uri()
        line = "pacman -S remote-"
        await client.open_document(uri, text=line)
        for promote in [False, True]:
            if promote:
                await client.send_notification("workspace/didChangeConfiguration", {"settings": {"shucked": {"nativeExecutionAllowed": True, "server": {"completion": {"includeNative": True}}}}})
            result = await client.completion(uri, 0, len(line))
            assert "remote-package" not in {item["label"] for item in result["items"]}
            assert not (tmp_path / "native-ran").exists()
    finally:
        await client.shutdown_and_exit()
