"""Folder settings in a multi-root workspace apply to the documents in that folder only."""

from __future__ import annotations

import json
from collections.abc import Callable

from ..harness.session import EditorSession
from ..harness.waiting import stays_false

SCRIPT = "#!/bin/bash\nshucked_multiroot_missing\n"


def test_folder_policy_is_scoped_to_its_folder(launch_editor: Callable[..., EditorSession], tmp_path) -> None:
    root = tmp_path / "multiroot"
    for folder, policy in (("host", "workspace"), ("portable", "portable")):
        (root / folder / ".vscode").mkdir(parents=True)
        (root / folder / ".vscode" / "settings.json").write_text(json.dumps({"shucked.environment.policy": policy}))
        (root / folder / "script.sh").write_text(SCRIPT)
    workspace = root / "shucked.code-workspace"
    workspace.write_text(json.dumps({"folders": [{"path": "host"}, {"path": "portable"}]}))
    session = launch_editor("multiroot", workspace=workspace)
    assert [folder["name"] for folder in session.bridge.ping()["workspaceFolders"]] == ["host", "portable"]
    host = session.bridge.open(root / "host" / "script.sh")["uri"]
    portable = session.bridge.open(root / "portable" / "script.sh")["uri"]
    session.wait_for_diagnostic(host, "ENV001", line=1)
    assert stays_false(lambda: "ENV001" in session.bridge.diagnostic_codes(portable), duration=3)
