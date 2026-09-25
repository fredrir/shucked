"""Status bar items: server state and the per-document execution context picker."""

from __future__ import annotations

from ..harness.session import EditorSession
from ..harness.waiting import wait_until


def test_server_status_reports_ready(editor: EditorSession) -> None:
    editor.open_workspace_file("smoke.zsh")
    wait_until("ready status item", lambda: "Shucked" in editor.workbench.status_items())


def test_execution_context_item_follows_the_active_shell_document(editor: EditorSession) -> None:
    editor.open_workspace_file("smoke.zsh")
    wait_until("workspace context item", lambda: "Workspace (local)" in editor.workbench.status_items())
    editor.open("notes.txt", "plain text\n")
    wait_until("context item hidden for other languages", lambda: "Workspace (local)" not in editor.workbench.status_items())


def test_choosing_portable_for_one_document(editor: EditorSession) -> None:
    uri = editor.open("portable.sh", "#!/bin/bash\nshucked_portable_missing\n")
    editor.wait_for_diagnostic(uri, "ENV001", line=1)
    workbench = editor.workbench
    wait_until("workspace context item", lambda: "Workspace (local)" in workbench.status_items())
    workbench.status_item(r"Workspace \(local\)").click()
    workbench.pick("Portable")
    wait_until("portable context item", lambda: "Portable" in workbench.status_items())
    editor.wait_without_diagnostic(uri, "ENV001")
    workbench.status_item("Portable").click()
    workbench.pick("Use workspace settings")
    wait_until("workspace context restored", lambda: "Workspace (local)" in workbench.status_items())
    editor.wait_for_diagnostic(uri, "ENV001", line=1)


def test_portable_choice_is_scoped_to_the_document(editor: EditorSession) -> None:
    first = editor.open("scoped-a.sh", "#!/bin/bash\nshucked_scoped_missing\n")
    second = editor.open("scoped-b.sh", "#!/bin/bash\nshucked_scoped_missing\n", show=False)
    editor.bridge.show(first)
    editor.workbench.status_item(r"Workspace \(local\)").click()
    editor.workbench.pick("Portable")
    editor.wait_without_diagnostic(first, "ENV001")
    editor.bridge.show(second)
    editor.wait_for_diagnostic(second, "ENV001", line=1)
    wait_until("workspace item for the other document", lambda: "Workspace (local)" in editor.workbench.status_items())
    editor.bridge.show(first)
    editor.workbench.status_item("Portable").click()
    editor.workbench.pick("Use workspace settings")
