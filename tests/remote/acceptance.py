#!/usr/bin/env python3
"""Run host-isolation/reconnect acceptance against a real SSH, container, or WSL host."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tests.lsp.client import LspClient


class Transport:
    def __init__(self, prefix, ssh):
        self.prefix, self.ssh = prefix, ssh

    def command(self, arguments):
        return self.prefix + ([shlex.join(arguments)] if self.ssh else arguments)

    def run(self, arguments):
        return subprocess.run(self.command(arguments), check=True, text=True, capture_output=True).stdout


class RemoteClient(LspClient):
    def __init__(self, transport, binary, root, local_bin, relative_path=False):
        super().__init__(binary)
        self.local_environment = dict(os.environ, PATH=str(local_bin) + os.pathsep + os.environ.get("PATH", ""))
        self.transport, self.root = transport, root
        self.path = "bin" if relative_path else f"{root}/bin"

    async def start(self):
        command = self.transport.command(["/usr/bin/env", f"PATH={self.path}", f"HOME={self.root}", self.binary_path, "server"])
        self.proc = await asyncio.create_subprocess_exec(*command, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, env=self.local_environment)
        self._read_task = asyncio.create_task(self._read_loop())


async def completion(client, uri, line, character):
    result = await client.send_request("textDocument/completion", {"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
    return {item["label"] for item in (result.get("items", []) if isinstance(result, dict) else result or [])}


async def run(args):
    transport = Transport(json.loads(args.command), args.ssh)
    root = f"/tmp/shucked-remote-{uuid.uuid4().hex}"
    transport.run(["/usr/bin/python3", "-c", "from pathlib import Path; import sys; p=Path(sys.argv[1]); (p/'bin').mkdir(parents=True); t=p/'bin/shucked_remote_only'; t.write_text('#!/bin/sh\\nexit 0\\n'); t.chmod(0o755); (p/'remote file.txt').touch()", root])
    uri = f"file://{root}/script.sh"
    local = tempfile.TemporaryDirectory(prefix="shucked-local-fixture-")
    local_bin = Path(local.name)
    local_tool = local_bin / "shucked_local_only"
    local_tool.write_text("#!/bin/sh\nexit 0\n")
    local_tool.chmod(0o755)
    checks = []
    started = time.monotonic()
    try:
        for cycle in range(2):
            client = RemoteClient(transport, args.binary, root, local_bin, args.relative_path)
            await client.start()
            try:
                await client.initialize(root_uri=f"file://{root}", initialization_options={"nativeExecutionAllowed": False, **({"environment": {"cwd": root}} if args.relative_path else {})})
                await client.initialized()
                await client.open_document(uri, text="shucked_\nprintf './remote'\nshucked_installed_later\n", language_id="shellscript")
                labels = await completion(client, uri, 0, 8)
                assert ("shucked_remote_only" in labels) == (cycle == 0), labels
                assert "shucked_local_only" not in labels
                assert "remote file.txt" in await completion(client, uri, 1, 16)
                checks.append("workspace host inventory and paths" if cycle == 0 else "reconnect discards previous inventory")
                if cycle == 0:
                    # Wait for the initial environment diagnostic before installing the command.
                    deadline = time.monotonic() + 12
                    while not any("shucked_installed_later" in item.get("message", "") for item in client._latest_diagnostics.get(uri, [])):
                        assert time.monotonic() < deadline, client._latest_diagnostics
                        await asyncio.sleep(.05)
                    changed = time.monotonic()
                    transport.run(["/usr/bin/python3", "-c", "from pathlib import Path; import sys; p=Path(sys.argv[1])/'bin/shucked_installed_later'; p.write_text('#!/bin/sh\\nexit 0\\n'); p.chmod(0o755)", root])
                    while any("shucked_installed_later" in item.get("message", "") for item in client._latest_diagnostics.get(uri, [])):
                        assert time.monotonic() - changed < 12, client._latest_diagnostics
                        await asyncio.sleep(.05)
                    assert "shucked_installed_later" in await completion(client, uri, 2, len("shucked_installed_later"))
                    checks.append("filesystem installation refresh without document edit")
                    transport.run(["/usr/bin/python3", "-c", "from pathlib import Path; import sys; (Path(sys.argv[1])/'bin/shucked_remote_only').unlink()", root])
            finally:
                await client.shutdown_and_exit()
        print(json.dumps({"transport": args.name, "relativePath": args.relative_path, "host": transport.run(["uname", "-sm"]).strip(), "checks": checks, "elapsedSeconds": round(time.monotonic()-started, 3)}, indent=2))
    finally:
        local.cleanup()
        transport.run(["/usr/bin/python3", "-c", "import shutil,sys; shutil.rmtree(sys.argv[1])", root])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", required=True, help='JSON argv prefix, e.g. ["ssh","-T","fixture"] or ["podman","exec","-i","fixture"]')
    parser.add_argument("--ssh", action="store_true", help="Quote remote argv for SSH's shell transport")
    parser.add_argument("--binary", required=True, help="Absolute server binary path on the target host")
    parser.add_argument("--name", required=True)
    parser.add_argument("--relative-path", action="store_true")
    asyncio.run(run(parser.parse_args()))
