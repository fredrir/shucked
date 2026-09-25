"""Per-test view of a running editor: a private scratch folder plus cleanup."""

from __future__ import annotations

import contextlib
import re
import shutil
import subprocess
from collections.abc import Callable
from pathlib import Path
from typing import Any

from .bridge import Bridge, diagnostic_code, file_uri
from .instance import LaunchSpec, VSCodeInstance
from .waiting import wait_until
from .workbench import Workbench


def prepare_home(source: Path, destination: Path) -> Path:
    shutil.copytree(source, destination, dirs_exist_ok=True)
    destination.chmod(0o700)
    return destination


def prepare_workspace(source: Path, destination: Path) -> Path:
    shutil.copytree(source, destination, dirs_exist_ok=True)
    if shutil.which("git"):
        subprocess.run(["git", "init", "--quiet", str(destination)], check=True, capture_output=True, timeout=30)
    return destination


class EditorFactory:
    """Creates isolated editors; each one gets its own root, HOME, and workspace copy."""

    def __init__(self, make_root: Callable[[str], Path], start: Callable[[LaunchSpec, Path], VSCodeInstance], defaults: dict[str, Any]) -> None:
        self._make_root = make_root
        self._start = start
        self._defaults = defaults
        self._instances: list[VSCodeInstance] = []

    def launch(self, name: str = "editor", workspace: Path | None = None, files: dict[str, str] | None = None, **overrides: Any) -> VSCodeInstance:
        """Start an editor; ``files`` are written into the workspace copy before launch."""
        root = self._make_root(name)
        options = {**self._defaults, **overrides}
        home = prepare_home(options.pop("home_source"), root / "home")
        workspace_source = options.pop("workspace_source")
        if workspace is None:
            workspace = prepare_workspace(workspace_source, root / "workspace")
        for relative, content in (files or {}).items():
            target = (workspace if workspace.is_dir() else workspace.parent) / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
        instance = self._start(LaunchSpec(workspace=workspace, home=home, **options), root)
        self._instances.append(instance)
        return instance

    def stop(self, instance: VSCodeInstance) -> None:
        if instance in self._instances:
            self._instances.remove(instance)
        instance.stop()

    def stop_all(self) -> None:
        while self._instances:
            self._instances.pop().stop()


class EditorSession:
    """What a test sees: the bridge, a workbench page, and a scratch folder."""

    def __init__(self, instance: VSCodeInstance, directory: Path) -> None:
        self.instance = instance
        self.bridge: Bridge = instance.bridge
        self.directory = directory
        self.directory.mkdir(parents=True, exist_ok=True)
        self._workbench: Workbench | None = None

    @property
    def workspace(self) -> Path:
        return self.instance.spec.workspace

    @property
    def home(self) -> Path:
        return self.instance.spec.home

    @property
    def workbench(self) -> Workbench:
        if self._workbench is None:
            self._workbench = Workbench(self.instance.page, self.bridge)
        return self._workbench

    def path(self, name: str) -> Path:
        return self.directory / name

    def write(self, name: str, content: str) -> Path:
        path = self.path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return path

    def open(self, name: str, content: str | None = None, show: bool = True) -> str:
        """Write (when content is given) and open a file; returns its URI."""
        path = self.write(name, content) if content is not None else self.path(name)
        return self.bridge.open(path, show=show)["uri"]

    def open_workspace_file(self, relative: str, show: bool = True) -> str:
        return self.bridge.open(self.workspace / relative, show=show)["uri"]

    def uri(self, name: str) -> str:
        return file_uri(self.path(name))

    def edit(self, uri: str, text: str, cursor: tuple[int, int] | None = None) -> None:
        """Replace a document's text and optionally place the cursor."""
        assert self.bridge.replace_text(uri, text)
        if cursor is not None:
            self.bridge.show(uri)
            self.bridge.set_cursor(uri, *cursor)

    def cursor_at_end(self, uri: str) -> tuple[int, int]:
        lines = self.bridge.text(uri).split("\n")
        return len(lines) - 1, len(lines[-1])

    def wait_for_diagnostic(self, uri: str, code: str, line: int | None = None, timeout: float = 30.0) -> dict[str, Any]:
        def find() -> dict[str, Any] | None:
            for item in self.bridge.diagnostics(uri):
                if diagnostic_code(item) == code and (line is None or item["range"]["start"]["line"] == line):
                    return item
            return None

        return wait_until(f"{code} diagnostic in {uri}", find, timeout=timeout)

    def wait_without_diagnostic(self, uri: str, code: str, timeout: float = 30.0) -> None:
        wait_until(f"{code} cleared in {uri}", lambda: code not in self.bridge.diagnostic_codes(uri), timeout=timeout)

    def wait_for_completion(self, uri: str, line: int, character: int, wanted: str | Callable[[str], bool], timeout: float = 20.0) -> dict[str, Any]:
        from .bridge import label

        matches = wanted if callable(wanted) else (lambda text: text == wanted)
        return wait_until(
            f"completion {wanted!r} at {line}:{character}",
            lambda: next((item for item in self.bridge.completions(uri, line, character)["items"] if matches(label(item))), None),
            timeout=timeout,
        )

    def wait_for_hover(self, uri: str, line: int, character: int, pattern: str, timeout: float = 20.0) -> str:
        expression = re.compile(pattern)
        return wait_until(
            f"hover matching {pattern!r}",
            lambda: next((text for text in self.bridge.hover_text(uri, line, character) if expression.search(text)), None),
            timeout=timeout,
        )

    def create_terminal(self, shell: str, timeout: float = 30.0) -> dict[str, Any]:
        """Create a Shucked terminal attached to the active document."""
        before = {item["name"] for item in self.bridge.terminals()}
        self.bridge.execute("shucked.createTerminal", shell)
        return wait_until(
            f"Shucked {shell} terminal",
            lambda: next((item for item in self.bridge.terminals() if item["name"] not in before and item["processId"]), None),
            timeout=timeout,
        )

    def reset(self) -> None:
        """Return the shared editor to a neutral state for the next test."""
        with contextlib.suppress(Exception):
            self.bridge.execute("shucked.dismissCompletion")
        with contextlib.suppress(Exception):
            self.bridge.execute("hideSuggestWidget")
        with contextlib.suppress(Exception):
            self.bridge.restore_settings()
        with contextlib.suppress(Exception):
            self.bridge.dispose_terminals()
        with contextlib.suppress(Exception):
            self.bridge.close_all_editors()
        with contextlib.suppress(Exception):
            self.bridge.execute("notifications.clearAll")
        if self._workbench is not None:
            with contextlib.suppress(Exception):
                self._workbench.press("Escape")
