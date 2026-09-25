"""Hover, semantic tokens, symbols, formatting, and code actions through the client."""

from __future__ import annotations

import pytest

from ..harness.bridge import BridgeError
from ..harness.session import EditorSession
from ..harness.waiting import wait_until

UNFORMATTED = "#!/bin/bash\nif true; then\necho \"body\"\nfi\n"


def test_missing_commands_carry_the_invalid_semantic_modifier(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.zsh")
    legend = wait_until("semantic token legend", lambda: editor.bridge.semantic_legend(uri))
    invalid = 1 << legend["tokenModifiers"].index("invalid")
    tokens = wait_until("semantic tokens", lambda: editor.bridge.semantic_tokens(uri))
    # Tokens are encoded as five integers; the last one is the modifier bit set.
    modifiers = tokens[4::5]
    assert any(value & invalid for value in modifiers)


def test_hover_explains_a_missing_command(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.zsh")
    editor.wait_for_diagnostic(uri, "ENV001", line=2)
    hover = editor.wait_for_hover(uri, 2, 4, r"Command: shucked_missing_smoke")
    assert "Resolution: Command not found" in hover
    assert "Target: workspace" in hover


def test_document_symbols_include_functions(editor: EditorSession) -> None:
    uri = editor.open_workspace_file("smoke.zsh")
    symbols = wait_until("document symbols", lambda: editor.bridge.document_symbols(uri))
    assert "shucked_smoke_function" in [symbol["name"] for symbol in symbols]


def test_definition_of_a_function_call(editor: EditorSession) -> None:
    uri = editor.open("definition.sh", "#!/bin/bash\ngreet() { echo hi; }\ngreet\n")
    locations = wait_until("definition", lambda: editor.bridge.definitions(uri, 2, 1))
    target = locations[0].get("targetRange") or locations[0]["range"]
    assert target["start"]["line"] == 1


def test_format_document_indents_blocks(editor: EditorSession) -> None:
    uri = editor.open("format.sh", UNFORMATTED)
    edits = wait_until("format edits", lambda: editor.bridge.format_edits(uri))
    assert any(edit["range"]["start"]["line"] <= 2 <= edit["range"]["end"]["line"] for edit in edits)
    editor.bridge.execute("editor.action.formatDocument")
    formatted = wait_until("formatted text", lambda: (text := editor.bridge.text(uri)) != UNFORMATTED and text)
    body = formatted.split("\n")[2]
    assert body.strip() == 'echo "body"' and body[0].isspace()


def test_formatting_a_clean_document_changes_nothing(editor: EditorSession) -> None:
    uri = editor.open("clean.sh", "#!/bin/bash\necho \"clean\"\n")
    editor.wait_for_completion(uri, 1, 0, lambda _: True)
    assert editor.bridge.format_edits(uri) == []


@pytest.mark.xfail(strict=True, reason="the server parses format.enable but never reads it (RawFormatOptions.enable)")
def test_disabling_formatting_stops_format_edits(editor: EditorSession) -> None:
    uri = editor.open("format-disabled.sh", UNFORMATTED)
    wait_until("format edits", lambda: editor.bridge.format_edits(uri))
    editor.bridge.update_setting("shucked", "format.enable", False)
    wait_until("formatting disabled", lambda: not editor.bridge.format_edits(uri), timeout=10)


def _titles(actions: list[dict]) -> list[str]:
    return [action["title"] for action in actions]


def test_code_actions_offer_fixes_and_suppressions(editor: EditorSession) -> None:
    uri = editor.open("actions.sh", "#!/bin/bash\nunused_value=10\n")
    editor.wait_for_diagnostic(uri, "C001", line=1)
    actions = wait_until("code actions", lambda: editor.bridge.code_actions(uri, 1, 0, 1, 12))
    kinds = {action.get("kind") for action in actions}
    assert any(kind and kind.startswith("quickfix") for kind in kinds)
    assert any("C001" in title for title in _titles(actions))


def test_suppression_actions_follow_their_setting(editor: EditorSession) -> None:
    uri = editor.open("suppress.sh", "#!/bin/bash\nunused_value=10\n")
    editor.wait_for_diagnostic(uri, "C001", line=1)
    before = wait_until("code actions", lambda: editor.bridge.code_actions(uri, 1, 0, 1, 12))
    editor.bridge.update_setting("shucked", "codeAction.disableRuleComment.enable", False)
    after = wait_until("fewer code actions", lambda: (actions := editor.bridge.code_actions(uri, 1, 0, 1, 12)) is not None and len(actions) < len(before) and actions)
    removed = set(_titles(before)) - set(_titles(after))
    assert removed, "turning off suppression comments removes their actions"


def test_applying_a_suppression_action_silences_the_rule(editor: EditorSession) -> None:
    uri = editor.open("apply.sh", "#!/bin/bash\nunused_value=10\n")
    editor.wait_for_diagnostic(uri, "C001", line=1)
    actions = wait_until("code actions", lambda: editor.bridge.code_actions(uri, 1, 0, 1, 12))
    suppressions = [action for action in actions if "line" in action["title"].lower() and action.get("edit")]
    assert suppressions, _titles(actions)
    edit = suppressions[0]["edit"][0]["edits"][0]
    editor.bridge.call("evaluate", code="""
        const edit = new vscode.WorkspaceEdit();
        edit.replace(vscode.Uri.parse(args.uri), new vscode.Range(args.start.line, args.start.character, args.end.line, args.end.character), args.text);
        return vscode.workspace.applyEdit(edit);
    """, args={"uri": uri, "start": edit["range"]["start"], "end": edit["range"]["end"], "text": edit["newText"]})
    editor.wait_without_diagnostic(uri, "C001")


def test_unknown_command_arguments_are_reported_by_the_bridge(editor: EditorSession) -> None:
    with pytest.raises(BridgeError):
        editor.bridge.call("noSuchMethod")
