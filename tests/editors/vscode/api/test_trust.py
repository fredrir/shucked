"""An untrusted workspace can read shell scripts but cannot make the editor run programs."""

from __future__ import annotations

import json
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

from ..harness.instance import VSCodeInstance
from ..harness.session import EditorFactory, EditorSession, artifact_name, keep_artifacts, scratch_directory
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
    workspace_settings = {"shucked.server.path": str(hostile), "shucked.server.extraArgs": ["--hostile-argument"]}
    # History is a machine setting, so it is enabled for the user; trust alone must still keep it off.
    instance = editor_factory.launch(
        "untrusted",
        trusted=False,
        settings={"shucked.history.files": True},
        files={".vscode/settings.json": json.dumps(workspace_settings)},
    )
    yield instance
    editor_factory.stop(instance)


@pytest.fixture
def untrusted(request: pytest.FixtureRequest, untrusted_editor: VSCodeInstance, artifacts_dir: Path) -> Iterator[EditorSession]:
    session = EditorSession(untrusted_editor, scratch_directory(untrusted_editor, artifact_name(request.node)))
    yield session
    keep_artifacts(request.node, untrusted_editor, artifacts_dir)
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
    # Same history and prefix as test_history.test_default_history_file_is_read_without_a_terminal.
    assert untrusted.bridge.setting("shucked", "history.files") is True
    (untrusted.home / ".zsh_history").write_text(": 1700000000:0;printf shucked_default_history\n")
    uri = untrusted.open("history.zsh", "printf shucked_d")
    assert untrusted.no_inline_suggestion(uri, "printf shucked_d")
