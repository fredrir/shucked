"""Fixtures for the VS Code extension suites.

``editor`` is the everyday fixture: a per-test scratch folder inside one shared,
isolated VS Code instance, reset after every test. ``launch_editor`` starts a
separate instance for tests that need different trust, workspaces, settings, or
that deliberately break the language server.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest

from .harness import display as display_server
from .harness import install, processes
from .harness.instance import EXTENSION_ID, LaunchSpec, PlaywrightDriver, VSCodeInstance
from .harness.session import EditorFactory, EditorSession, artifact_name, keep_artifacts, scratch_directory

SUITE = Path(__file__).resolve().parent
REPOSITORY = SUITE.parents[2]
EXTENSION = REPOSITORY / "editors" / "vscode"
FIXTURES = SUITE / "fixtures"
BRIDGE = SUITE / "bridge" / "runner.cjs"


@pytest.fixture(scope="session")
def extension_root() -> Path:
    return EXTENSION


@pytest.fixture(scope="session")
def extension_manifest() -> dict[str, Any]:
    return json.loads((EXTENSION / "package.json").read_text())


@pytest.fixture(scope="session")
def node() -> str:
    path = shutil.which("node")
    if not path:
        pytest.fail("Node.js is required to run the shell integration hooks and to build the extension")
    return path


@pytest.fixture(scope="session")
def built_extension(request: pytest.FixtureRequest, node: str) -> Path:
    """Bundle ``dist/extension.js`` from the current sources (development mode only)."""
    if request.config.getoption("--vsix"):
        return EXTENSION
    if not (EXTENSION / "node_modules").is_dir():
        pytest.fail("Install the extension's dependencies first: (cd editors/vscode && bun install)")
    result = subprocess.run([node, "esbuild.mjs"], cwd=EXTENSION, capture_output=True, text=True, timeout=300)
    if result.returncode:
        pytest.fail(f"Extension build failed:\n{result.stdout}\n{result.stderr}")
    return EXTENSION


@pytest.fixture(scope="session")
def language_server(request: pytest.FixtureRequest) -> Path | None:
    """The development server binary; an installed VSIX brings its own."""
    if request.config.getoption("--vsix"):
        return None
    override = os.environ.get("SHUCKED_TEST_SERVER")
    binary = Path(override) if override else REPOSITORY / "target" / "debug" / ("shucked.exe" if sys.platform == "win32" else "shucked")
    if not binary.exists():
        pytest.fail(f"Language server not found at {binary}. Run `cargo build -p shucked-cli -p shucked-server` first.")
    return binary.resolve()


@pytest.fixture(scope="session")
def vscode_installation(request: pytest.FixtureRequest) -> install.Installation:
    executable = request.config.getoption("--vscode-executable")
    if executable:
        return install.from_executable(executable)
    version = (
        request.config.getoption("--vscode-version")
        or os.environ.get("SHUCKED_VSCODE_VERSION")
        or install.minimum_engine(EXTENSION / "package.json")
    )
    cache = Path(os.environ.get("SHUCKED_VSCODE_CACHE", REPOSITORY / ".cache" / "vscode-test"))
    return install.install(version, cache)


@pytest.fixture(scope="session")
def display(request: pytest.FixtureRequest) -> Iterator[display_server.Display]:
    server = display_server.start(bool(request.config.getoption("--headed")))
    yield server
    server.stop()


@pytest.fixture(scope="session")
def playwright_driver() -> Iterator[PlaywrightDriver]:
    driver = PlaywrightDriver()
    yield driver
    driver.stop()


@pytest.fixture(scope="session")
def artifacts_dir(request: pytest.FixtureRequest) -> Path:
    configured = request.config.getoption("--vscode-artifacts")
    return Path(configured) if configured else REPOSITORY / "target" / "vscode-tests"


@pytest.fixture(scope="session")
def editor_factory(
    request: pytest.FixtureRequest,
    tmp_path_factory: pytest.TempPathFactory,
    vscode_installation: install.Installation,
    display: display_server.Display,
    playwright_driver: PlaywrightDriver,
    built_extension: Path,
    language_server: Path | None,
) -> Iterator[EditorFactory]:
    processes.adopt_orphans()
    vsix = request.config.getoption("--vsix")

    def make_root(name: str) -> Path:
        root = tmp_path_factory.mktemp(name)
        root.chmod(0o700)
        return root

    def start(spec: LaunchSpec, root: Path) -> VSCodeInstance:
        instance = VSCodeInstance(vscode_installation, spec, root, display, BRIDGE, playwright_driver)
        instance.start()
        try:
            instance.bridge.activate_extension(EXTENSION_ID)
        except Exception:
            instance.stop()
            raise
        return instance

    factory = EditorFactory(
        make_root,
        start,
        {
            "home_source": FIXTURES / "home",
            "workspace_source": FIXTURES / "workspaces" / "default",
            "extension": built_extension,
            "server": language_server,
            "vsix": vsix.resolve() if vsix else None,
            "trace": bool(request.config.getoption("--vscode-trace")),
        },
    )
    yield factory
    factory.stop_all()


@pytest.fixture(scope="session")
def shared_editor(editor_factory: EditorFactory) -> VSCodeInstance:
    return editor_factory.launch("shared")


@pytest.fixture
def editor(request: pytest.FixtureRequest, shared_editor: VSCodeInstance, artifacts_dir: Path) -> Iterator[EditorSession]:
    session = EditorSession(shared_editor, scratch_directory(shared_editor, artifact_name(request.node)))
    yield session
    keep_artifacts(request.node, shared_editor, artifacts_dir)
    session.reset()


@pytest.fixture
def launch_editor(
    request: pytest.FixtureRequest, editor_factory: EditorFactory, artifacts_dir: Path
) -> Iterator[Callable[..., EditorSession]]:
    """Start a dedicated editor: ``launch_editor(trusted=False, settings={...})``."""
    launched: list[VSCodeInstance] = []

    def launch(name: str = "isolated", **overrides: Any) -> EditorSession:
        instance = editor_factory.launch(name, **overrides)
        launched.append(instance)
        return EditorSession(instance, scratch_directory(instance, artifact_name(request.node)))

    yield launch
    for instance in launched:
        keep_artifacts(request.node, instance, artifacts_dir)
        editor_factory.stop(instance)


@pytest.fixture
def require_shell(request: pytest.FixtureRequest) -> Callable[[str], str]:
    """Return a shell's path, skipping (or failing with --require-shells) when it is missing."""

    def require(name: str) -> str:
        path = shutil.which(name)
        if path:
            return path
        message = f"{name} is not installed"
        if request.config.getoption("--require-shells"):
            pytest.fail(message)
        pytest.skip(message)

    return require
