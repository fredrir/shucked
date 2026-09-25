"""Static contracts between the extension manifest, its sources, and its packaging rules.

These run without an editor. Each one guards a mistake that would otherwise
only show up as a dead command, an ignored setting, or a file shipped by accident.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Any

import pytest

Manifest = dict[str, Any]


@pytest.fixture(scope="module")
def sources(extension_root: Path) -> str:
    return "\n".join(path.read_text() for path in sorted((extension_root / "src").glob("*.ts")))


@pytest.fixture(scope="module")
def settings(extension_manifest: Manifest) -> dict[str, Any]:
    return extension_manifest["contributes"]["configuration"]["properties"]


@pytest.fixture(scope="module")
def server_commands(extension_root: Path) -> set[str]:
    """Commands the language server advertises; the language client registers these itself."""
    capabilities = extension_root.parents[1] / "crates" / "shucked-lsp" / "src" / "capabilities.rs"
    return set(re.findall(r'=> "(shucked\.[A-Za-z]+)"', capabilities.read_text()))


def _registered(sources: str, server_commands: set[str]) -> set[str]:
    return set(re.findall(r'registerCommand\(\s*"([^"]+)"', sources)) | server_commands


def test_contributed_commands_have_handlers(extension_manifest: Manifest, sources: str, server_commands: set[str]) -> None:
    contributed = {command["command"] for command in extension_manifest["contributes"]["commands"]}
    assert contributed - _registered(sources, server_commands) == set()


def test_key_bindings_target_registered_commands(extension_manifest: Manifest, sources: str, server_commands: set[str]) -> None:
    bound = {binding["command"] for binding in extension_manifest["contributes"]["keybindings"]}
    assert bound - _registered(sources, server_commands) == set()


def test_navigation_bindings_only_pass_supported_arguments(extension_manifest: Manifest, sources: str) -> None:
    supported = set(re.findall(r'"(select(?:Next|Prev)(?:Page)?Suggestion)"', sources))
    for binding in extension_manifest["contributes"]["keybindings"]:
        if binding["command"] == "shucked.navigateCompletion":
            assert binding["args"] in supported, binding


def test_intercepted_keys_are_scoped_to_pending_completion(extension_manifest: Manifest) -> None:
    for binding in extension_manifest["contributes"]["keybindings"]:
        assert "shucked.completionPending" in binding["when"], binding
        assert "editorTextFocus" in binding["when"], binding


def test_settings_read_by_the_client_are_declared(settings: dict[str, Any], sources: str) -> None:
    read = set(re.findall(r'\.get(?:<[^>]*>)?\(\s*"([^"]+)"', sources))
    read |= set(re.findall(r'serverSetting(?:<[^>]*>)?\(\s*\w+,\s*"([^"]+)"', sources))
    read |= {key.removeprefix("shucked.") for key in re.findall(r'affectsConfiguration\(\s*"(shucked\.[^"]+)"', sources)}
    declared = {key.removeprefix("shucked.") for key in settings}
    undeclared = {key for key in read if key not in declared and not any(name.startswith(f"{key}.") for name in declared)}
    assert undeclared == set()


def test_every_setting_reaches_the_client_or_the_server(settings: dict[str, Any], sources: str) -> None:
    # The client forwards whole setting groups to the server at start-up and on change.
    forwarded = set(re.findall(r'(\w+): config\.get\("\1"\)', sources))
    read = set(re.findall(r'\.get(?:<[^>]*>)?\(\s*"([^"]+)"', sources))
    handled_by_language_client = {"trace.server"}
    for key in settings:
        name = key.removeprefix("shucked.")
        assert name.split(".")[0] in forwarded or name in read or name in handled_by_language_client, key


def test_every_setting_is_documented_and_typed(settings: dict[str, Any]) -> None:
    for key, schema in settings.items():
        assert schema.get("description") or schema.get("markdownDescription"), key
        assert "type" in schema, key


def test_every_served_language_activates_the_extension(extension_manifest: Manifest, sources: str) -> None:
    selector = re.search(r'\[((?:"\w+",?\s*)+)\]\.flatMap\(language', sources)
    assert selector, "document selector languages not found in client.ts"
    languages = {"shellscript", *re.findall(r'"(\w+)"', selector.group(1))}
    activation = {event.removeprefix("onLanguage:") for event in extension_manifest["activationEvents"] if event.startswith("onLanguage:")}
    assert languages - activation == set()


def test_shell_language_defaults_cover_served_languages(extension_manifest: Manifest) -> None:
    defaults = extension_manifest["contributes"]["configurationDefaults"]
    for language in ("shellscript", "bash", "zsh", "sh", "ksh", "fish"):
        assert defaults[f"[{language}]"]["editor.wordBasedSuggestions"] == "off", language
    assert "editor.wordBasedSuggestions" not in defaults, "only shell languages change word suggestions"


def test_untrusted_workspaces_cannot_choose_programs(extension_manifest: Manifest, settings: dict[str, Any]) -> None:
    trust = extension_manifest["capabilities"]["untrustedWorkspaces"]
    assert trust["supported"] == "limited"
    for key in ("shucked.server.path", "shucked.server.extraArgs"):
        assert key in trust["restrictedConfigurations"]
        assert settings[key]["scope"] == "machine-overridable"
    for key in ("shucked.history.session", "shucked.history.files"):
        assert settings[key]["scope"] == "machine", "history is chosen per machine"
        assert settings[key]["default"] is False, "history is opt-in"


# Token types and modifiers every client knows; only additions need a manifest entry.
STANDARD_SEMANTIC_TOKEN_TYPES = {
    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter",
    "variable", "property", "enumMember", "event", "function", "method", "macro", "keyword",
    "modifier", "comment", "string", "number", "regexp", "operator", "decorator",
}
STANDARD_SEMANTIC_TOKEN_MODIFIERS = {
    "declaration", "definition", "readonly", "static", "deprecated", "abstract", "async",
    "modification", "documentation", "defaultLibrary",
}


@pytest.fixture(scope="module")
def server_legend(extension_root: Path) -> tuple[set[str], set[str]]:
    """Token types and modifiers the language server advertises in its legend."""
    legend = (extension_root.parents[1] / "crates" / "shucked-lsp" / "src" / "handlers" / "semantic_tokens.rs").read_text()
    types_block = re.search(r"SUPPORTED_TOKEN_TYPES[^=]*=\s*&\[(.*?)\];", legend, re.S).group(1)
    modifiers_block = re.search(r"SUPPORTED_TOKEN_MODIFIERS[^=]*=\s*&\[(.*?)\];", legend, re.S).group(1)

    def names(block: str, standard: set[str]) -> set[str]:
        custom = set(re.findall(r'new\("([^"]+)"\)', block))
        constants = re.findall(r"::([A-Z_]+)\b", block)
        camel = {"".join(part.capitalize() if index else part.lower() for index, part in enumerate(name.split("_"))) for name in constants}
        return custom | (camel & standard)

    return names(types_block, STANDARD_SEMANTIC_TOKEN_TYPES), names(modifiers_block, STANDARD_SEMANTIC_TOKEN_MODIFIERS)


def test_custom_semantic_tokens_match_the_server_legend(extension_manifest: Manifest, server_legend: tuple[set[str], set[str]]) -> None:
    contributes = extension_manifest["contributes"]
    server_types, server_modifiers = server_legend
    declared_types = {item["id"] for item in contributes["semanticTokenTypes"]}
    declared_modifiers = {item["id"] for item in contributes["semanticTokenModifiers"]}
    assert declared_types == server_types - STANDARD_SEMANTIC_TOKEN_TYPES
    assert declared_modifiers == server_modifiers - STANDARD_SEMANTIC_TOKEN_MODIFIERS
    for item in contributes["semanticTokenTypes"]:
        assert item["superType"] in STANDARD_SEMANTIC_TOKEN_TYPES, item


def test_semantic_token_scopes_use_declared_types(extension_manifest: Manifest) -> None:
    contributes = extension_manifest["contributes"]
    types = {item["id"] for item in contributes["semanticTokenTypes"]} | STANDARD_SEMANTIC_TOKEN_TYPES
    modifiers = {item["id"] for item in contributes["semanticTokenModifiers"]} | STANDARD_SEMANTIC_TOKEN_MODIFIERS
    for scope in contributes["semanticTokenScopes"]:
        for selector in scope["scopes"]:
            kind, *applied = selector.split(".")
            assert kind in types, selector
            assert set(applied) <= modifiers, selector


def test_shell_languages_enable_semantic_highlighting(extension_manifest: Manifest) -> None:
    defaults = extension_manifest["contributes"]["configurationDefaults"]
    for language in ("shellscript", "bash", "zsh", "sh", "ksh", "fish"):
        assert defaults[f"[{language}]"]["editor.semanticHighlighting.enabled"] is True, language


def test_type_definitions_do_not_exceed_the_engine(extension_manifest: Manifest) -> None:
    def minimum(version: str) -> tuple[int, ...]:
        return tuple(int(part) for part in re.sub(r"^[^\d]*", "", version).split("."))

    assert minimum(extension_manifest["devDependencies"]["@types/vscode"]) <= minimum(extension_manifest["engines"]["vscode"])


def test_packaging_excludes_sources_and_tests(extension_root: Path) -> None:
    ignored = set((extension_root / ".vscodeignore").read_text().split())
    for pattern in ("src/**", "tests/**", "node_modules/**", "**/*.map", "bun.lock", "*.vsix"):
        assert pattern in ignored, pattern
