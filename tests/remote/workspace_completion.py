#!/usr/bin/env python3
"""Completion latency and candidate authority across real workspaces and transports."""
import argparse
import asyncio
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tests.lsp.client import LspClient
from tests.remote.acceptance import Transport


class WorkspaceClient(LspClient):
    def __init__(self, transport, binary, providers):
        super().__init__(binary)
        self.transport, self.providers = transport, providers

    async def start(self):
        arguments = [self.binary_path, "server"]
        if self.providers:
            arguments = ["/usr/bin/env", f"SHUCKED_PROVIDER_ROOT={self.providers}", *arguments]
        self.proc = await asyncio.create_subprocess_exec(
            *self.transport.command(arguments), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
        )
        self._read_task = asyncio.create_task(self._read_loop())


def items(result):
    return result.get("items", []) if isinstance(result, dict) else result or []


async def probe(client, uri, line, version, expected=None, directories=False):
    try:
        return await _probe(client, uri, line, version, expected, directories)
    except TimeoutError as error:
        raise AssertionError((line, "completion readiness timed out", expected)) from error


async def _probe(client, uri, line, version, expected, directories):
    await client.change_document(uri, text=line, version=version)
    started = time.monotonic()
    mark = len(client._all_notifications)
    first_ms = None
    responses = 0
    async with asyncio.timeout(15):
        while True:
            result = await client.completion(uri, 0, len(line))
            responses += 1
            if first_ms is None:
                first_ms = round((time.monotonic() - started) * 1000, 2)
                assert first_ms < 500, (line, "foreground response stalled", first_ms)
            candidates = items(result)
            if directories:
                assert all(item.get("kind") == 19 for item in candidates), (line, "non-directory offered to cd")
            elif not line.endswith("-"):
                assert not any(item.get("kind") in (17, 19) for item in candidates), (line, "workspace path fallback at subcommand position")
            if expected and any(item["label"] == expected for item in candidates):
                break
            if directories and (candidates or not result.get("isIncomplete", False)):
                break
            while True:
                notices = client._all_notifications[mark:]
                mark = len(client._all_notifications)
                if any(n.get("method") == "shucked/completionReady"
                       and n.get("params", {}).get("uri") == uri
                       and n["params"].get("version") == version
                       and n["params"].get("position") == {"line": 0, "character": len(line)}
                       for n in notices):
                    break
                await asyncio.sleep(.005)
    return {"line": line, "firstResponseMs": first_ms,
            "usefulMs": round((time.monotonic() - started) * 1000, 2),
            "candidates": len(candidates), "responses": responses}


async def run(options):
    transport = Transport(json.loads(options.command), options.ssh)
    installed = json.loads(transport.run(["/usr/bin/python3", "-c",
        "import json,shutil; print(json.dumps({n:bool(shutil.which(n)) for n in ['ls','brew','docker','podman','pacman','paru','xcb']}))"]))
    checks = []
    cases = [("ls", "ls -", "-a"), ("brew", "brew ", "install"),
             ("docker", "docker ", "container"), ("docker", "docker container ", "ls"),
             ("podman", "podman ", "container"), ("pacman", "pacman -", "-Q"),
             ("paru", "paru -", "-Q")]
    for workspace in options.workspace:
        client = WorkspaceClient(transport, options.binary, options.provider_root)
        await client.start()
        try:
            await client.initialize(root_uri=Path(workspace).as_uri(), initialization_options={
                "nativeExecutionAllowed": True, "environment": {"cwd": workspace},
            })
            await client.initialized()
            for dialect in options.dialects.split(","):
                uri = Path(workspace, "__shucked_completion_acceptance__." + dialect).as_uri()
                await client.open_document(uri, language_id=dialect, text="")
                version = 1
                for line in ["cd ", "cd ~/"]:
                    version += 1
                    check = await probe(client, uri, line, version, directories=True)
                    checks.append({"workspace": workspace, "dialect": dialect, **check})
                    print(json.dumps(check), file=sys.stderr, flush=True)
                for command, line, expected in cases:
                    if installed[command]:
                        version += 1
                        check = await probe(client, uri, line, version, expected)
                        checks.append({"workspace": workspace, "dialect": dialect, **check})
                        print(json.dumps(check), file=sys.stderr, flush=True)
                await client.send_notification("textDocument/didClose", {"textDocument": {"uri": uri}})
        finally:
            await client.shutdown_and_exit()
    print(json.dumps({"installed": installed, "checks": checks}, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", default='["/usr/bin/env"]', help="JSON transport argv")
    parser.add_argument("--ssh", action="store_true")
    parser.add_argument("--binary", required=True)
    parser.add_argument("--provider-root")
    parser.add_argument("--workspace", action="append", required=True)
    parser.add_argument("--dialects", default="zsh")
    asyncio.run(run(parser.parse_args()))
