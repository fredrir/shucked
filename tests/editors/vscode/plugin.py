"""Command-line options and test selection for the VS Code extension suite.

Registered from ``tests/conftest.py`` so the options exist however pytest is
invoked. Fast suites (contract, shell integration) always run. Editor suites
(``api``, ``ui``) need a pinned VS Code download and a display, so they run only
with ``--e2e`` (or ``--vsix``), or when a path inside them is named explicitly.
Packaging checks need a VSIX: ``--vsix PATH`` inspects one, ``--build-vsix``
builds one first.
"""

from __future__ import annotations

from pathlib import Path

import pytest

SUITE = Path(__file__).resolve().parent
EDITOR_SUITES = ("api", "ui")


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("vscode", "Shucked VS Code extension")
    group.addoption("--e2e", action="store_true", help="run the editor suites (api/, ui/) in an isolated VS Code")
    group.addoption("--vsix", type=Path, help="installed-package mode: run editor suites against this VSIX and inspect it")
    group.addoption("--build-vsix", action="store_true", help="build a VSIX for the packaging checks (release build; slow)")
    group.addoption(
        "--vscode-version",
        default=None,
        help="VS Code version to download (default: the manifest's minimum engine; 'stable' for the latest)",
    )
    group.addoption("--vscode-executable", type=Path, help="use an existing VS Code executable instead of a pinned download")
    group.addoption("--headed", action="store_true", help="use the current DISPLAY instead of a private Xvfb server")
    group.addoption("--vscode-trace", action="store_true", help="record verbose language server traces in the editor logs")
    group.addoption("--vscode-artifacts", type=Path, help="where screenshots and logs of failed editor tests are kept")
    group.addoption("--require-shells", action="store_true", help="fail instead of skipping when bash, zsh, or fish is missing")
    group.addoption("--regression-workspace", type=Path, help="read-only completion regression check against an existing workspace")


def pytest_configure(config: pytest.Config) -> None:
    for marker in (
        "vscode: runs inside an isolated VS Code instance",
        "ui: drives the workbench with Playwright",
        "packaging: inspects a packaged VSIX",
        "contract: static checks of the extension manifest and sources",
    ):
        config.addinivalue_line("markers", marker)


def _suite(item: pytest.Item) -> str | None:
    try:
        relative = Path(str(item.path)).resolve().relative_to(SUITE)
    except ValueError:
        return None
    return relative.parts[0] if len(relative.parts) > 1 else None


def _explicitly_requested(config: pytest.Config, suite: str) -> bool:
    for argument in config.args:
        path = Path(argument.split("::", 1)[0]).resolve()
        with_suite = SUITE / suite
        if path == with_suite or with_suite in path.parents:
            return True
    return False


def editor_suites_enabled(config: pytest.Config) -> bool:
    return bool(config.getoption("--e2e") or config.getoption("--vsix"))


def pytest_collection_modifyitems(config: pytest.Config, items: list[pytest.Item]) -> None:
    selected, deselected = [], []
    packaging = bool(config.getoption("--vsix") or config.getoption("--build-vsix"))
    for item in items:
        suite = _suite(item)
        if suite in EDITOR_SUITES:
            item.add_marker(pytest.mark.vscode)
            if suite == "ui":
                item.add_marker(pytest.mark.ui)
            if not (editor_suites_enabled(config) or _explicitly_requested(config, suite)):
                deselected.append(item)
                continue
        elif suite == "packaging":
            item.add_marker(pytest.mark.packaging)
            if not (packaging or _explicitly_requested(config, suite)):
                deselected.append(item)
                continue
        elif suite == "contract":
            item.add_marker(pytest.mark.contract)
        selected.append(item)
    if deselected:
        config.hook.pytest_deselected(items=deselected)
        items[:] = selected


def pytest_report_header(config: pytest.Config) -> list[str]:
    editor = "enabled" if editor_suites_enabled(config) else "off (pass --e2e)"
    packaging = "enabled" if config.getoption("--vsix") or config.getoption("--build-vsix") else "off (pass --vsix PATH or --build-vsix)"
    return [f"vscode editor suites: {editor}; packaging: {packaging}"]


@pytest.hookimpl(wrapper=True, tryfirst=True)
def pytest_runtest_makereport(item: pytest.Item, call: pytest.CallInfo[None]):
    report = yield
    # Fixtures read this to keep screenshots and logs for failed tests.
    setattr(item, f"report_{report.when}", report)
    return report
