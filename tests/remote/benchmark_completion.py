#!/usr/bin/env python3
"""Non-gating cold/warm completion benchmark over SSH or container stdio."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import sys
import tempfile
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tests.lsp.benchmark_command_intelligence import items, positive, summary
from tests.lsp.client import LspClient
from tests.remote.acceptance import Transport


class BenchmarkClient(LspClient):
    def __init__(self, transport, binary, root, directories, local_bin):
        super().__init__(binary)
        self.transport, self.root = transport, root
        self.paths = [f"{root}/bin-{index}" for index in range(directories)]
        self.local_environment = dict(os.environ, PATH=str(local_bin) + os.pathsep + os.environ.get("PATH", ""))

    async def start(self):
        command = self.transport.command([
            "/usr/bin/env", f"PATH={':'.join(self.paths)}", f"HOME={self.root}",
            f"XDG_CONFIG_HOME={self.root}/.config", self.binary_path, "server",
        ])
        self.proc = await asyncio.create_subprocess_exec(
            *command, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE, env=self.local_environment,
        )
        self._read_task = asyncio.create_task(self._read_loop())


CREATE_FIXTURES = r"""
import hashlib,json,pathlib,platform,sys
root,binary,executables,directories,flags=sys.argv[1:]
executables,directories,flags=map(int,(executables,directories,flags))
root=pathlib.Path(root)
paths=[root/f'bin-{index}' for index in range(directories)]
for directory in paths: directory.mkdir(parents=True)
for index in range(executables):
    command=paths[index % directories]/f'fixture-tool-{index:05d}'
    command.write_text('#!/bin/sh\nexit 99\n'); command.chmod(0o755)
help_lines=[f'    --fixture-{index:04d}  Fixture option {index}' for index in range(flags)]
tool=paths[0]/'eza'
tool.write_text('#!/bin/sh\ncase "$1" in\n--help) printf \'%s\\n\' '
    +' '.join(repr(line) for line in help_lines)
    +';;\n--version) printf \'%s\\n\' \'benchmark fixture, no validator version\';;\n*) exit 99;;\nesac\n')
tool.chmod(0o755)
print(json.dumps({'platform':platform.platform(),'machine':platform.machine(),
    'binary_sha256':hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()}))
"""


async def benchmark(options):
    prefix = json.loads(options.command)
    if not isinstance(prefix, list) or not prefix or not all(isinstance(word, str) for word in prefix):
        raise ValueError("--command must be a nonempty JSON string array")
    transport = Transport(prefix, options.ssh)
    root = f"/tmp/shucked-completion-benchmark-{uuid.uuid4().hex}"
    host = json.loads(transport.run([
        "/usr/bin/python3", "-c", CREATE_FIXTURES, root, options.binary,
        str(options.executables), str(options.path_directories), str(options.flags),
    ]))
    lines = [f"bench_function_{index:04d}() {{ :; }}" for index in range(options.functions)]
    lines += ["fixture-tool-", "eza --fixture-", "# edit 0"]
    source = "\n".join(lines) + "\n"
    uri = f"file://{root}/benchmark.sh"
    measurements = {}
    samples, counts = {}, {}
    version = 1

    def record(kind, started, count=0):
        samples.setdefault(kind, []).append((time.perf_counter_ns() - started) / 1_000_000)
        counts.setdefault(kind, []).append(count)

    async def request(client, kind):
        nonlocal version
        if kind == "edit_to_completion":
            version += 1
            lines[-1] = f"# edit {version}"
            await client.change_document(uri, "\n".join(lines) + "\n", version)
        argument = "arguments" in kind
        line = options.functions + int(argument)
        result = items(await client.completion(uri, line, len(lines[line])))
        prefix = "--fixture-" if argument else "fixture-tool-"
        expected = options.flags if argument else options.executables
        if len(result) != expected or not all(item["label"].startswith(prefix) for item in result):
            raise RuntimeError(f"{kind}: expected {expected} host items; got {len(result)}, {result[:3]}")
        if argument and not any("Fixture option" in item.get("detail", "") for item in result):
            raise RuntimeError("Native help descriptions missing")
        return len(result)

    try:
        with tempfile.TemporaryDirectory(prefix="shucked-benchmark-client-") as local:
            # Same completion prefix; an accidental client-side PATH lookup must fail.
            local_tool = Path(local) / "fixture-tool-client-only"
            local_tool.write_text("#!/bin/sh\nexit 99\n")
            local_tool.chmod(0o755)
            for cycle in range(options.cold_iterations):
                client = BenchmarkClient(transport, options.binary, root, options.path_directories, local)
                started = time.perf_counter_ns()
                await client.start()
                try:
                    await client.initialize(root_uri=f"file://{root}", initialization_options={
                        "nativeExecutionAllowed": True, "environment": {"cwd": root},
                        "server": {"completion": {"maxItems": 2000}},
                    })
                    await client.initialized()
                    record("transport_start_and_initialize", started)
                    await client.open_document(uri, language_id="bash", text=source, version=version)
                    requested = time.perf_counter_ns()
                    count = await request(client, "cold_commands")
                    record("cold_commands", requested, count)
                    record("transport_start_to_first_completion", started, count)
                    requested = time.perf_counter_ns()
                    count = await request(client, "cold_native_arguments")
                    record("cold_native_arguments", requested, count)
                    if cycle == options.cold_iterations - 1:
                        for kind in ("warm_commands", "warm_native_arguments", "edit_to_completion"):
                            for _ in range(options.warmup):
                                await request(client, kind)
                            for _ in range(options.iterations):
                                requested = time.perf_counter_ns()
                                count = await request(client, kind)
                                record(kind, requested, count)
                finally:
                    await client.shutdown_and_exit()
        for kind in samples:
            measurements[kind] = summary(samples[kind], counts[kind])
        return {
            "transport": options.name, "binary": options.binary, "build_mode": options.build_mode,
            "host": host, "client": {"platform": platform.platform(), "python": platform.python_version()},
            "fixtures": {"path_directories": options.path_directories, "executable_files": options.executables + 1,
                "native_help_flags": options.flags, "source_functions": options.functions,
                "source_lines": len(lines), "source_bytes": len(source.encode()), "documents": 1,
                "max_completion_items": 2000, "warmup_per_scenario": options.warmup},
            "measurements": measurements,
            "scope": {
                "cold": "fresh server and transport process per sample; OS filesystem caches remain warm",
                "cold_arguments": "first fixture help query in each process, after its command completion",
                "transport": "sequential stdio requests include transport and Python client; no latency simulation",
                "edits": "full-text didChange through completion response, including reanalysis",
                "diagnostics": "background diagnostics enabled; not awaiting debounced publication",
                "timing_thresholds": "none; observations are not a performance gate",
            },
        }
    finally:
        transport.run(["/usr/bin/python3", "-c", "import shutil,sys; shutil.rmtree(sys.argv[1])", root])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--command", required=True, help="JSON transport argv prefix")
    parser.add_argument("--ssh", action="store_true", help="Quote remote argv for SSH")
    parser.add_argument("--name", required=True)
    parser.add_argument("--binary", required=True, help="Absolute target-host server path")
    parser.add_argument("--build-mode", choices=("debug", "release", "custom"), required=True)
    parser.add_argument("--cold-iterations", type=positive, default=10)
    parser.add_argument("--iterations", type=positive, default=100)
    parser.add_argument("--warmup", type=positive, default=10)
    parser.add_argument("--executables", type=positive, default=1000)
    parser.add_argument("--path-directories", type=positive, default=4)
    parser.add_argument("--functions", type=positive, default=100)
    parser.add_argument("--flags", type=positive, default=128)
    arguments = parser.parse_args()
    if max(arguments.executables, arguments.flags) > 2000:
        parser.error("fixture candidate counts must not exceed the 2000-item response limit")
    print(json.dumps(asyncio.run(benchmark(arguments)), indent=2))
