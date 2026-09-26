"""Offline argument completion: bundled option grammars bound to the resolved
executable, and cached subcommand inventories for brew, git, docker and
kubectl. No completion engine is involved, so these pass where zsh, bash and
fish are absent."""
import asyncio
import os

from tests.lsp.client import LspClient
from tests.lsp.test_native_completion import complete_when_ready, install_tool, wait_for_completion_ready


def fixture_host(tmp_path):
    """A workspace with fixture tools on PATH. Run markers, the cache directory
    and HOME live outside the workspace so the environment watcher does not see
    the files the server and the fixtures write during a test. Only `ls`
    answers `--version`: the diagnostics validator identifies tools that way and
    would otherwise list brew and git itself, blurring the run counts."""
    host = tmp_path / "offline-host"
    bin_dir = host / "bin"
    bin_dir.mkdir(parents=True)
    state = tmp_path / "state"
    runs = state / "runs"
    runs.mkdir(parents=True)
    (state / "home").mkdir()
    install_tool(bin_dir, "ls", f"""case "$1" in
  --version) printf x >> '{runs / "ls"}'; printf 'ls (GNU coreutils) 9.7\\n' ;;
  *) exit 9 ;;
esac""")
    install_tool(bin_dir, "brew", f"""case "$1" in
  commands) printf x >> '{runs / "brew"}'; printf 'install\\nlist\\nsearch\\n' ;;
  *) exit 1 ;;
esac""")
    install_tool(bin_dir, "git", f"""case "$1" in
  --list-cmds=main,others,alias,nohelpers) printf x >> '{runs / "git"}'; printf 'add\\ncommit\\npush\\nfixture-alias\\n' ;;
  *) exit 1 ;;
esac""")
    install_tool(bin_dir, "docker", f"""case "$1" in
  --help) printf x >> '{runs / "docker"}'; printf 'Usage:  docker [OPTIONS] COMMAND\\n\\nCommon Commands:\\n  run         Start a fixture container\\n  exec        Enter a fixture container\\n\\nManagement Commands:\\n  builder     Manage fixture builds\\n\\nGlobal Options:\\n      --config string   Location of client config files\\n' ;;
  *) exit 1 ;;
esac""")
    environment = dict(
        os.environ,
        PATH=str(bin_dir),
        HOME=str(state / "home"),
        SHUCKED_CACHE_DIR=str(state / "cache"),
    )
    environment.pop("ZDOTDIR", None)
    return host, runs, environment


def run_count(runs, tool):
    path = runs / tool
    return len(path.read_text()) if path.exists() else 0


async def start(shucked_binary, host, environment, trusted):
    client = LspClient(str(shucked_binary), environment=environment)
    await client.start()
    options = {"nativeExecutionAllowed": True} if trusted else None
    await client.initialize(root_uri=host.as_uri(), initialization_options=options)
    await client.initialized()
    return client


def detail(result, label):
    return next(item["detail"] for item in result["items"] if item["label"] == label)


async def complete_until(client, uri, line, character, accept):
    """Re-request after each readiness notice until the list satisfies `accept`."""
    start = len(client._all_notifications)
    async with asyncio.timeout(5):
        while True:
            result = await client.completion(uri, line, character)
            if accept(result):
                return result
            await wait_for_completion_ready(client, uri, line, character, start=start)
            start = len(client._all_notifications)


def described(label, text):
    return lambda result: any(
        item["label"] == label and text in item["detail"] for item in result["items"]
    )


async def test_flags_complete_from_the_grammar_bound_to_the_resolved_executable(shucked_binary, tmp_path):
    host, runs, environment = fixture_host(tmp_path)
    client = await start(shucked_binary, host, environment, trusted=True)
    try:
        uri = (host / "script.sh").as_uri()
        lines = ["ls -", "ls --col", "ls -la", "ls -- -"]
        await client.open_document(uri, text="\n".join(lines))
        result = await complete_when_ready(client, uri, 0, len(lines[0]), "-l")
        assert detail(result, "-l") == "List in long format · gnu-ls 9.7"
        assert detail(result, "--all") == "Include entries whose names begin with a dot · gnu-ls 9.7"
        long = next(item for item in result["items"] if item["label"] == "-l")
        assert long["kind"] == 5  # field
        assert long["textEdit"]["newText"] == "-l"
        # The binding runs one version query; diagnostics validation may add
        # one of its own, but neither the grammar nor later requests add more.
        assert run_count(runs, "ls") in (1, 2), "one version query binds the grammar"
        result = await complete_when_ready(client, uri, 1, len(lines[1]), "--color")
        assert detail(result, "--color").startswith("Colorize output")
        result = await complete_when_ready(client, uri, 2, len(lines[2]), "-lah")
        assert detail(result, "-lah").startswith("Show sizes in human-readable units")
        after_separator = await client.completion(uri, 3, len(lines[3]))
        assert not any(item["label"].startswith("-") for item in after_separator["items"])
        assert run_count(runs, "ls") in (1, 2), "later requests use the cached binding"
    finally:
        await client.shutdown_and_exit()


async def test_untrusted_workspaces_get_unverified_grammar_flags_without_running_anything(shucked_binary, tmp_path):
    host, runs, environment = fixture_host(tmp_path)
    client = await start(shucked_binary, host, environment, trusted=False)
    try:
        uri = (host / "script.sh").as_uri()
        line = "ls -"
        await client.open_document(uri, text=line)
        result = await complete_when_ready(client, uri, 0, len(line), "-l")
        assert detail(result, "-l") == "List in long format · gnu-ls 9.7 (unverified)"
        await asyncio.sleep(0.05)
        assert run_count(runs, "ls") == 0
    finally:
        await client.shutdown_and_exit()


async def test_subcommand_inventories_complete_brew_git_and_docker_once_per_installation(shucked_binary, tmp_path):
    host, runs, environment = fixture_host(tmp_path)
    client = await start(shucked_binary, host, environment, trusted=True)
    try:
        uri = (host / "script.sh").as_uri()
        lines = ["brew ", "git ", "docker ", "git co"]
        await client.open_document(uri, text="\n".join(lines))
        result = await complete_when_ready(client, uri, 0, len(lines[0]), "install")
        assert detail(result, "install") == "Install a formula or cask · brew"
        assert {"install", "list", "search"} <= {item["label"] for item in result["items"]}
        result = await complete_when_ready(client, uri, 1, len(lines[1]), "commit")
        assert detail(result, "commit") == "Record staged changes to the repository · git"
        assert detail(result, "fixture-alias") == "git"
        # The bundled docker grammar lists `run` at once; the inventory's
        # description arrives with the readiness notice.
        result = await complete_until(client, uri, 2, len(lines[2]), described("run", "fixture"))
        assert detail(result, "run") == "Start a fixture container · docker"
        assert detail(result, "builder") == "Manage fixture builds · docker"
        assert "exec" in {item["label"] for item in result["items"]}
        result = await complete_when_ready(client, uri, 3, len(lines[3]), "commit")
        assert {item["label"] for item in result["items"]} == {"commit"}
        assert (run_count(runs, "brew"), run_count(runs, "git"), run_count(runs, "docker")) == (1, 1, 1)
        cached = sorted(path.name for path in (tmp_path / "state" / "cache" / "subcommands").iterdir())
        assert [name.split("-")[0] for name in cached] == ["brew", "docker", "git"]
    finally:
        await client.shutdown_and_exit()
    # A new server answers from the cache directory without running the tools.
    client = await start(shucked_binary, host, environment, trusted=True)
    try:
        uri = (host / "script.sh").as_uri()
        await client.open_document(uri, text="brew ins")
        result = await complete_when_ready(client, uri, 0, len("brew ins"), "install")
        assert detail(result, "install") == "Install a formula or cask · brew"
        assert run_count(runs, "brew") == 1
    finally:
        await client.shutdown_and_exit()


async def test_changed_tool_is_inventoried_again_after_environment_refresh(shucked_binary, tmp_path):
    host, runs, environment = fixture_host(tmp_path)
    client = await start(shucked_binary, host, environment, trusted=True)
    try:
        uri = (host / "script.sh").as_uri()
        line = "brew "
        await client.open_document(uri, text=line)
        result = await complete_when_ready(client, uri, 0, len(line), "install")
        assert "upgrade" not in {item["label"] for item in result["items"]}
        install_tool(host / "bin", "brew", f"""case "$1" in
  commands) printf x >> '{runs / "brew"}'; printf 'install\\nlist\\nsearch\\nupgrade\\n' ;;
  *) exit 1 ;;
esac""")
        # The watcher notices the rewritten executable on its own (a 250 ms
        # debounce and a 2 s fingerprint pass); let those refreshes settle so
        # they cannot cancel the query the explicit refresh below triggers.
        await asyncio.sleep(2.5)
        await client.send_request("workspace/executeCommand", {"command": "shucked.refreshEnvironment"})
        result = await complete_when_ready(client, uri, 0, len(line), "upgrade")
        assert detail(result, "upgrade") == "Upgrade outdated formulae and casks · brew"
        assert run_count(runs, "brew") == 2
    finally:
        await client.shutdown_and_exit()


async def test_inventories_require_native_execution_trust(shucked_binary, tmp_path):
    host, runs, environment = fixture_host(tmp_path)
    client = await start(shucked_binary, host, environment, trusted=False)
    try:
        uri = (host / "script.sh").as_uri()
        lines = ["brew ", "git "]
        await client.open_document(uri, text="\n".join(lines))
        for index, line in enumerate(lines):
            result = await client.completion(uri, index, len(line))
            assert not {"install", "commit"} & {item["label"] for item in result["items"]}
        await client.send_notification("workspace/didChangeConfiguration", {"settings": {"shucked": {"nativeExecutionAllowed": True}}})
        await asyncio.sleep(0.1)
        for index, line in enumerate(lines):
            result = await client.completion(uri, index, len(line))
            assert not {"install", "commit"} & {item["label"] for item in result["items"]}
        await asyncio.sleep(0.05)
        assert run_count(runs, "brew") == 0 and run_count(runs, "git") == 0
        assert not (tmp_path / "state" / "cache" / "subcommands").exists()
    finally:
        await client.shutdown_and_exit()
