"""Non-gating benchmark: python3 tests/lsp/benchmark_command_intelligence.py."""

import argparse
import asyncio
import hashlib
import json
import math
import os
import platform
import statistics
import tempfile
import time
from pathlib import Path

if __package__:
    from .client import LspClient
else:
    from client import LspClient


def positive(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def summary(samples, counts):
    ordered = sorted(samples)
    return {
        "samples": len(samples),
        "p50_ms": round(statistics.median(ordered), 3),
        "p95_ms": round(ordered[math.ceil(len(ordered) * 0.95) - 1], 3),
        "min_ms": round(ordered[0], 3),
        "max_ms": round(ordered[-1], 3),
        "items_min": min(counts),
        "items_max": max(counts),
    }


def items(response):
    return response.get("items", []) if isinstance(response, dict) else response or []


async def benchmark(options):
    binary = options.binary.expanduser().resolve(strict=True)
    binary_digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    build_mode = options.build_mode or (
        binary.parent.name if binary.parent.name in ("debug", "release") else "unspecified"
    )
    with tempfile.TemporaryDirectory(prefix="shucked-completion-bench-") as temporary:
        root = Path(temporary)
        paths = [root / f"bin-{index}" for index in range(options.path_directories)]
        for directory in paths:
            directory.mkdir()
        for index in range(options.executables):
            executable = paths[index % len(paths)] / f"fixture-tool-{index:05d}"
            executable.write_text("#!/bin/sh\nexit 99\n")
            executable.chmod(0o755)
        # Native help is a fixed local fixture, independent of installed tools/packages.
        tool = paths[0] / "eza"
        help_lines = [
            f"    --fixture-{index:04d}  Fixture option {index}"
            for index in range(options.flags)
        ]
        tool.write_text(
            "#!/bin/sh\ncase \"$1\" in\n--help) printf '%s\\n' "
            + " ".join(f"'{line}'" for line in help_lines)
            + ";;\n--version) printf '%s\\n' 'benchmark fixture, no validator version';;\n"
            + "*) exit 99;;\nesac\n"
        )
        tool.chmod(0o755)
        functions = [
            f"bench_function_{index:04d}() {{ :; }}"
            for index in range(options.functions)
        ]
        lines = functions + ["fixture-tool-00", "eza --fixture-", "# edit 0"]
        source = "\n".join(lines) + "\n"
        uri = (root / "benchmark.sh").as_uri()
        environment = dict(
            os.environ,
            PATH=os.pathsep.join(str(directory) for directory in paths),
            HOME=str(root),
            XDG_CONFIG_HOME=str(root / ".config"),
        )
        client = LspClient(str(binary), environment=environment)
        measurements = {}
        version = 1
        await client.start()
        try:
            await client.initialize(
                root_uri=root.as_uri(),
                initialization_options={
                    "nativeExecutionAllowed": True,
                    "environment": {"cwd": str(root)},
                    "server": {"completion": {"maxItems": 2000}},
                },
            )
            await client.initialized()
            await client.open_document(uri, language_id="bash", text=source, version=version)

            async def request(kind):
                nonlocal version
                if kind == "edit_to_completion":
                    version += 1
                    lines[-1] = f"# edit {version}"
                    await client.change_document(uri, "\n".join(lines) + "\n", version)
                argument = kind == "warm_native_arguments"
                line = options.functions + int(argument)
                response = await client.completion(uri, line, len(lines[line]))
                result = items(response)
                prefix = "--fixture-" if argument else "fixture-tool-00"
                if not result or not all(item["label"].startswith(prefix) for item in result):
                    raise RuntimeError(f"unexpected {kind} completion results: {result[:3]}")
                if argument and not any("Fixture option" in item.get("detail", "") for item in result):
                    raise RuntimeError("native help descriptions missing")
                return len(result)

            for kind in ("warm_commands", "warm_native_arguments", "edit_to_completion"):
                for _ in range(options.warmup):
                    await request(kind)
                samples, counts = [], []
                for _ in range(options.iterations):
                    started = time.perf_counter_ns()
                    count = await request(kind)
                    samples.append((time.perf_counter_ns() - started) / 1_000_000)
                    counts.append(count)
                measurements[kind] = summary(samples, counts)
        finally:
            await client.shutdown_and_exit()

        return {
            "binary": str(binary),
            "binary_sha256": binary_digest,
            "build_mode": build_mode,
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "fixtures": {
                "path_directories": len(paths),
                "executable_files": options.executables + 1,
                "native_help_flags": options.flags,
                "source_functions": options.functions,
                "source_lines": len(lines),
                "initial_source_bytes": len(source.encode()),
                "documents": 1,
                "max_completion_items": 2000,
                "warmup_per_scenario": options.warmup,
            },
            "measurements": measurements,
            "scope": {
                "transport": "local stdio, sequential requests, Python client included",
                "native_arguments": "cached help from fixture eza; excludes first provider startup",
                "edits": "full-text didChange followed by completion response; includes reanalysis",
                "diagnostics": "normal background scheduling enabled; not waiting for debounced diagnostics",
                "timing_thresholds": "none; results are observations, not a performance gate",
            },
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[2] / "target/debug/shucked")
    parser.add_argument("--build-mode", choices=("debug", "release", "custom"))
    parser.add_argument("--iterations", type=positive, default=100)
    parser.add_argument("--warmup", type=positive, default=10)
    parser.add_argument("--executables", type=positive, default=1000)
    parser.add_argument("--path-directories", type=positive, default=4)
    parser.add_argument("--functions", type=positive, default=100)
    parser.add_argument("--flags", type=positive, default=128)
    options = parser.parse_args()
    if os.name != "posix":
        parser.error("POSIX executable fixtures required")
    print(json.dumps(asyncio.run(benchmark(options)), indent=2))


if __name__ == "__main__":
    main()
