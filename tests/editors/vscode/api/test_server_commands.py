"""Commands the language server advertises run through the client's command registry."""

from __future__ import annotations

import os
from collections.abc import Callable

from ..harness.session import EditorSession


def test_debug_information_does_not_block_the_server(editor: EditorSession) -> None:
    # The handler logs at info level; logging once shared the protocol stream.
    editor.bridge.execute("shucked.printDebugInformation")
    uri = editor.open("after-debug.sh", "#!/bin/bash\nafter_debug_unused=1\n")
    editor.wait_for_diagnostic(uri, "C001", line=1, timeout=15)


def test_refresh_environment_finds_a_command_installed_later(launch_editor: Callable[..., EditorSession], tmp_path) -> None:
    tools = tmp_path / "late-tools"
    tools.mkdir()
    session = launch_editor("refresh", environment={"PATH": f"{tools}{os.pathsep}{os.environ['PATH']}"})
    uri = session.open("late.sh", "#!/bin/bash\nshucked_late_tool --version\n")
    session.wait_for_diagnostic(uri, "ENV001", line=1)
    tool = tools / "shucked_late_tool"
    tool.write_text("#!/bin/sh\n")
    tool.chmod(0o755)
    session.bridge.execute("shucked.refreshEnvironment")
    session.wait_without_diagnostic(uri, "ENV001")
