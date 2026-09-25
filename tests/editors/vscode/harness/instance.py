"""Launch an isolated VS Code window driven through the bridge and Playwright.

Each instance gets a private temporary root holding its HOME, profile,
extensions directory, and logs, so tests never read or modify the developer's
own editor state, shell startup files, or history.
"""

from __future__ import annotations

import contextlib
import json
import os
import secrets
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any

from . import processes
from .bridge import Bridge
from .display import Display
from .install import Installation
from .waiting import WaitAborted, wait_until

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page, Playwright

EXTENSION_ID = "fredrir.shucked"

BASE_SETTINGS: dict[str, Any] = {
    "telemetry.telemetryLevel": "off",
    "update.mode": "none",
    "extensions.autoUpdate": False,
    "extensions.autoCheckUpdates": False,
    "extensions.ignoreRecommendations": True,
    "workbench.startupEditor": "none",
    "workbench.tips.enabled": False,
    "workbench.secondarySideBar.defaultVisibility": "hidden",
    "window.restoreWindows": "none",
    "window.dialogStyle": "custom",
    "files.simpleDialog.enable": True,
    "chat.disableAIFeatures": True,
    "git.openRepositoryInParentFolders": "never",
    "shucked.lint.showSyntaxErrors": True,
}

TRUST_SETTINGS: dict[str, Any] = {
    "security.workspace.trust.enabled": True,
    "security.workspace.trust.startupPrompt": "never",
    "security.workspace.trust.banner": "never",
    "security.workspace.trust.untrustedFiles": "open",
    "security.workspace.trust.emptyWindow": False,
}


@dataclass
class LaunchSpec:
    """Everything that distinguishes one editor launch from another."""

    workspace: Path
    home: Path
    extension: Path
    server: Path | None = None
    vsix: Path | None = None
    trusted: bool = True
    settings: dict[str, Any] = field(default_factory=dict)
    environment: dict[str, str] = field(default_factory=dict)
    trace: bool = False
    # False when the workspace was supplied by the caller and must not be written to.
    owns_workspace: bool = True


class PlaywrightDriver:
    """One Playwright driver per test session, started on first use."""

    def __init__(self) -> None:
        self._manager: Any = None
        self._playwright: Playwright | None = None

    def get(self) -> Playwright:
        if self._playwright is not None:
            return self._playwright
        from playwright.sync_api import sync_playwright

        self._manager = sync_playwright()
        playwright: Playwright = self._manager.start()
        self._playwright = playwright
        return playwright

    def stop(self) -> None:
        if self._manager is not None:
            with contextlib.suppress(Exception):
                self._manager.__exit__(None, None, None)
        self._manager = self._playwright = None


def _clean_environment() -> dict[str, str]:
    # A test run started from a VS Code terminal must not reach that editor's IPC.
    return {key: value for key, value in os.environ.items() if not key.startswith(("VSCODE_", "ELECTRON_"))}


class VSCodeInstance:
    def __init__(
        self, installation: Installation, spec: LaunchSpec, root: Path, display: Display, bridge_script: Path, driver: PlaywrightDriver
    ) -> None:
        self.installation = installation
        self.spec = spec
        self.root = root
        self.display = display
        self.bridge_script = bridge_script
        self.driver = driver
        self.user_data = root / "user"
        self.extensions = root / "extensions"
        self.log_file = root / "editor.log"
        self.process: subprocess.Popen[bytes] | None = None
        self._bridge: Bridge | None = None
        self._browser: Browser | None = None
        self._page: Page | None = None

    # -- Lifecycle -------------------------------------------------------

    def environment(self) -> dict[str, str]:
        home = self.spec.home
        return {
            **_clean_environment(),
            **self.display.environment(),
            "HOME": str(home),
            "USERPROFILE": str(home),
            "ZDOTDIR": str(home),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "XDG_DATA_HOME": str(home / ".local" / "share"),
            "XDG_CACHE_HOME": str(home / ".cache"),
            **self.spec.environment,
        }

    def _write_settings(self) -> None:
        settings = {**BASE_SETTINGS, **(TRUST_SETTINGS if not self.spec.trusted else {"security.workspace.trust.enabled": False})}
        if self.spec.server and not self.spec.vsix:
            settings["shucked.server.path"] = str(self.spec.server)
        if self.spec.trace:
            settings["shucked.trace.server"] = "verbose"
        settings.update(self.spec.settings)
        (self.user_data / "User").mkdir(parents=True, exist_ok=True)
        (self.user_data / "User" / "settings.json").write_text(json.dumps(settings, indent=2))

    def _development_path(self) -> Path:
        if not self.spec.vsix:
            return self.spec.extension
        # Test mode needs a development extension; a stub hosts the bridge while
        # Shucked itself comes from the installed package.
        runner = self.root / "runner"
        runner.mkdir(exist_ok=True)
        (runner / "package.json").write_text(
            json.dumps(
                {
                    "name": "shucked-test-runner",
                    "publisher": "shucked-tests",
                    "version": "0.0.0",
                    "engines": {"vscode": "*"},
                    "main": "./index.cjs",
                    "activationEvents": [],
                }
            )
        )
        (runner / "index.cjs").write_text("exports.activate = () => undefined;\n")
        return runner

    def _install_vsix(self, environment: dict[str, str]) -> None:
        assert self.spec.vsix
        subprocess.run(
            [
                str(self.installation.cli),
                "--user-data-dir",
                str(self.user_data),
                "--extensions-dir",
                str(self.extensions),
                "--install-extension",
                str(self.spec.vsix),
                "--force",
            ],
            env=environment,
            check=True,
            capture_output=True,
            timeout=180,
        )

    def start(self, timeout: float = 120.0) -> None:
        self.extensions.mkdir(parents=True, exist_ok=True)
        self._write_settings()
        environment = self.environment()
        if self.spec.vsix:
            self._install_vsix(environment)
        token = secrets.token_hex(32)
        port_file = self.root / "bridge.port"
        # The secret goes through a private file the bridge deletes after reading it: the
        # environment would pass it on to every terminal and server the editor starts.
        token_file = self.root / "bridge.token"
        token_file.touch(mode=0o600)
        token_file.write_text(token)
        environment.update({"SHUCKED_BRIDGE_TOKEN_FILE": str(token_file), "SHUCKED_BRIDGE_PORT_FILE": str(port_file)})
        arguments = [
            str(self.installation.executable),
            *(["--no-sandbox", "--disable-dev-shm-usage"] if sys.platform.startswith("linux") else []),
            "--disable-gpu",
            "--remote-debugging-port=0",
            f"--user-data-dir={self.user_data}",
            f"--extensions-dir={self.extensions}",
            "--use-inmemory-secretstorage",
            "--skip-welcome",
            "--skip-release-notes",
            "--disable-telemetry",
            *(["--log", "trace"] if self.spec.trace else []),
            "--new-window",
            *([] if self.spec.vsix else ["--disable-extensions"]),
            *(["--disable-workspace-trust"] if self.spec.trusted else []),
            f"--extensionDevelopmentPath={self._development_path()}",
            f"--extensionTestsPath={self.bridge_script}",
            str(self.spec.workspace),
        ]
        with self.log_file.open("wb") as log:
            self.process = subprocess.Popen(arguments, env=environment, stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)

        def bridge_port() -> int | None:
            if self.process and self.process.poll() is not None:
                raise WaitAborted(f"VS Code exited with {self.process.returncode} before the bridge started; see {self.log_file}")
            text = port_file.read_text().strip() if port_file.exists() else ""
            return int(text) if text else None

        try:
            port = wait_until("editor bridge", bridge_port, timeout=timeout, interval=0.2)
        except Exception:
            self.stop()
            raise
        self._bridge = Bridge(port, token)

    def stop(self) -> None:
        owned = processes.descendants(self.process.pid) if self.process else []
        if self._browser is not None:
            with contextlib.suppress(Exception):
                self._browser.close()
            self._browser = self._page = None
        if self._bridge is not None:
            self._bridge.shutdown()
            self._bridge = None
        if self.process is not None:
            with contextlib.suppress(subprocess.TimeoutExpired):
                self.process.wait(timeout=20)
            processes.terminate([*owned, *processes.processes_mentioning(str(self.user_data))])
            with contextlib.suppress(Exception):
                self.process.kill()
                self.process.wait(timeout=5)
            self.process = None

    # -- Access ----------------------------------------------------------

    @property
    def bridge(self) -> Bridge:
        if self._bridge is None:
            raise RuntimeError("The editor is not running")
        return self._bridge

    @property
    def pid(self) -> int:
        assert self.process
        return self.process.pid

    @property
    def page(self) -> Page:
        """The workbench window, attached over the Chrome DevTools Protocol."""
        if self._page is not None:
            return self._page
        port_file = self.user_data / "DevToolsActivePort"
        port = wait_until("DevTools port", lambda: port_file.exists() and port_file.read_text().split()[0], timeout=30)
        self._browser = self.driver.get().chromium.connect_over_cdp(f"http://127.0.0.1:{port}")

        def workbench() -> Page | None:
            assert self._browser
            for context in self._browser.contexts:
                for page in context.pages:
                    if "workbench" in page.url:
                        return page
            return None

        page = wait_until("workbench page", workbench, timeout=30)
        page.wait_for_selector(".monaco-workbench", timeout=30000)
        self._page = page
        return page

    def logs(self) -> Path:
        return self.user_data / "logs"

    def capture(self, destination: Path) -> None:
        """Keep a screenshot and logs for a failed test."""
        shutil.rmtree(destination, ignore_errors=True)
        destination.mkdir(parents=True, exist_ok=True)
        with contextlib.suppress(Exception):
            self.page.screenshot(path=str(destination / "workbench.png"))
        with contextlib.suppress(Exception):
            shutil.copy2(self.log_file, destination / "editor.log")
        with contextlib.suppress(Exception):
            shutil.copytree(self.logs(), destination / "logs", dirs_exist_ok=True)
