"""An untrusted workspace can read shell scripts but cannot make the editor run programs."""

from __future__ import annotations

import json
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

from ..harness.instance import VSCodeInstance
from ..harness.session import EditorFactory, EditorSession
from ..harness.waiting import stays_false, wait_until

MARKER = "workspace-server-ran"


@pytest.fixture(scope="module")
def marker_root(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return tmp_path_factory.mktemp("untrusted-markers")


@pytest.fixture(scope="module")
def untrusted_editor(editor_factory: EditorFactory, marker_root: Path) -> Iterator[VSCodeInstance]:
    # A workspace that tries to choose the server program and its arguments.
    hostile = marker_root / "hostile-server"
    hostile.write_text(f"#!/bin/sh\ntouch '{marker_root / MARKER}'\nexec sleep 30\n")
    hostile.chmod(0o755)
    settings = {"shucked.server.path": str(hostile), "shucked.server.extraArgs": ["--hostile-argument"], "shucked.history.files": True}
    instance = editor_factory.launch("untrusted", trusted=False, files={".vscode/settings.json": json.dumps(settings)})
    yield instance
    editor_factory.stop(instance)


@pytest.fixture
def untrusted(request: pytest.FixtureRequest, untrusted_editor: VSCodeInstance) -> Iterator[EditorSession]:
    session = EditorSession(untrusted_editor, untrusted_editor.spec.workspace / "tests" / request.node.name)
    yield session
    session.reset()


def test_workspace_is_restricted(untrusted: EditorSession) -> None:
    assert untrusted.bridge.ping()["isTrusted"] is False


def test_workspace_settings_cannot_choose_the_server(untrusted: EditorSession, marker_root: Path) -> None:
    uri = untrusted.open("untrusted.sh", "#!/bin/bash\nuntrusted_unused=1\n")
    # Diagnostics prove the user-level server runs; the workspace program never does.
    untrusted.wait_for_diagnostic(uri, "C001", line=1)
    assert not (marker_root / MARKER).exists()


def test_terminals_require_trust(untrusted: EditorSession, require_shell: Callable[[str], str]) -> None:
    require_shell("bash")
    untrusted.open("terminal.sh", "#!/bin/bash\necho hi\n")
    untrusted.bridge.start("shucked.createTerminal", "bash")
    wait_until("trust notice", lambda: any("Trust this workspace" in text for text in untrusted.workbench.notifications()))
    assert stays_false(lambda: any(item["name"].startswith("Shucked") for item in untrusted.bridge.terminals()), duration=2)


def test_history_suggestions_require_trust(untrusted: EditorSession) -> None:
    (untrusted.home / ".bash_history").write_text("printf shucked_untrusted_history\n")
    uri = untrusted.open("history.sh", "printf shucked_u")

    def accepted() -> bool:
        untrusted.bridge.set_cursor(uri, 0, len("printf shucked_u"))
        untrusted.bridge.execute("editor.action.inlineSuggest.trigger")
        untrusted.bridge.execute("editor.action.inlineSuggest.commit")
        return untrusted.bridge.text(uri) != "printf shucked_u"

    assert stays_false(accepted, duration=3)
