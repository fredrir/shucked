"""Tests for Zsh language intelligence in Shucked LSP server.

Covers:
- Dialect detection via shebangs (#!/usr/bin/env zsh, #!/bin/zsh)
- Dialect detection via filename (script.zsh, .zshrc)
- Dialect detection via tags (#autoload, #compdef)
- Parameter expansions (${(q)var}, ${(f)lines}) without crash / false diagnostics
- Special parameters ($pipestatus, $path, $ZSH_VERSION) hover
- Builtins hover and completions (zstyle, compinit, autoload, typeset)
"""

import pytest
from tests.lsp.client import LspClient


@pytest.mark.asyncio
async def test_dialect_detection_shebang_env_zsh(initialized_lsp_client: LspClient):
    """Verify dialect detection with #!/usr/bin/env zsh shebang."""
    uri = "file:///tmp/shebang_env_test"
    text = """#!/usr/bin/env zsh
var="test value"
print -r -- ${(q)var}
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected 0 diagnostics for Zsh shebang script, got: {diags}"


@pytest.mark.asyncio
async def test_dialect_detection_shebang_bin_zsh(initialized_lsp_client: LspClient):
    """Verify dialect detection with #!/bin/zsh shebang."""
    uri = "file:///tmp/shebang_bin_test"
    text = """#!/bin/zsh
lines="one\\ntwo\\nthree"
print -r -- ${(f)lines}
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected 0 diagnostics for Zsh shebang script, got: {diags}"


@pytest.mark.asyncio
async def test_dialect_detection_filename_zsh_extension(
    initialized_lsp_client: LspClient,
):
    """Verify dialect detection via .zsh file extension without shebang."""
    uri = "file:///tmp/custom_script.zsh"
    text = """var="hello"
print -r -- ${(q)var}
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected 0 diagnostics for .zsh filename, got: {diags}"


@pytest.mark.asyncio
async def test_dialect_detection_filename_zshrc(initialized_lsp_client: LspClient):
    """Verify dialect detection for .zshrc without shebang."""
    uri = "file:///tmp/.zshrc"
    text = """entries="a\\nb\\nc"
print -r -- ${(f)entries}
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected 0 diagnostics for .zshrc file, got: {diags}"


@pytest.mark.asyncio
async def test_dialect_detection_tag_autoload(initialized_lsp_client: LspClient):
    """Verify dialect detection via #autoload tag at the beginning of file."""
    uri = "file:///tmp/autoload_func"
    text = """#autoload
target="world"
print -r -- ${(q)target}
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected 0 diagnostics for #autoload tag, got: {diags}"


@pytest.mark.asyncio
async def test_dialect_detection_tag_compdef(initialized_lsp_client: LspClient):
    """Verify dialect detection via #compdef tag at the beginning of file."""
    uri = "file:///tmp/_completion_func"
    text = """#compdef mycmd
items="first\\nsecond"
print -r -- ${(f)items}
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected 0 diagnostics for #compdef tag, got: {diags}"


@pytest.mark.asyncio
async def test_zsh_parameter_expansions_q_and_f(initialized_lsp_client: LspClient):
    """Verify ${(q)var} and ${(f)lines} parameter expansion flags parse correctly."""
    uri = "file:///tmp/parameter_expansions.zsh"
    text = """#!/usr/bin/env zsh
payload="echo 'hello world'"
quoted=${(q)payload}
print -r -- "$quoted"

multiline="alpha\\nbeta\\ngamma"
for line in ${(f)multiline}; do
    print -r -- "$line"
done
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Parameter expansion flags should not produce diagnostics: {diags}"


@pytest.mark.asyncio
async def test_zsh_special_parameters_hover(initialized_lsp_client: LspClient):
    """Verify hovering on Zsh special parameters ($pipestatus, $path, $ZSH_VERSION)."""
    uri = "file:///tmp/special_params.zsh"
    # Line 0: #!/usr/bin/env zsh
    # Line 1: print -r -- "$pipestatus"
    # Line 2: print -r -- "$path"
    # Line 3: print -r -- "$ZSH_VERSION"
    text = """#!/usr/bin/env zsh
print -r -- "$pipestatus"
print -r -- "$path"
print -r -- "$ZSH_VERSION"
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    await initialized_lsp_client.wait_for_diagnostics(uri)

    # 1. Hover on pipestatus (line 1, character 15)
    hover_pipestatus = await initialized_lsp_client.hover(uri, 1, 15)
    assert hover_pipestatus is not None, "Hover on $pipestatus should return result"
    content = hover_pipestatus["contents"]["value"]
    assert "pipestatus" in content
    assert "Provided by the active shell runtime" in content or "Array variable" in content

    # 2. Hover on path (line 2, character 15)
    hover_path = await initialized_lsp_client.hover(uri, 2, 15)
    assert hover_path is not None, "Hover on $path should return result"
    content = hover_path["contents"]["value"]
    assert "path" in content
    assert "Provided by the active shell runtime" in content or "Array variable" in content

    # 3. Hover on ZSH_VERSION (line 3, character 15)
    hover_zsh_version = await initialized_lsp_client.hover(uri, 3, 15)
    assert hover_zsh_version is not None, "Hover on $ZSH_VERSION should return result"
    content = hover_zsh_version["contents"]["value"]
    assert "ZSH_VERSION" in content
    assert "Provided by the active shell runtime" in content


@pytest.mark.asyncio
async def test_zsh_builtin_completions(initialized_lsp_client: LspClient):
    """Verify completions for Zsh builtins (e.g. zstyle, compinit)."""
    uri = "file:///tmp/completion_test.zsh"
    base_text = "#!/usr/bin/env zsh\n"
    await initialized_lsp_client.open_document(uri, "shellscript", base_text)
    await initialized_lsp_client.wait_for_diagnostics(uri)

    # Trigger completion for 'zst'
    await initialized_lsp_client.change_document(uri, base_text + "zst", version=2)
    await initialized_lsp_client.wait_for_diagnostics(uri)
    res_zst = await initialized_lsp_client.completion(uri, line=1, character=3)
    items_zst = res_zst.get("items", []) if isinstance(res_zst, dict) else res_zst
    labels_zst = [item["label"] for item in items_zst]
    assert "zstyle" in labels_zst, f"Expected 'zstyle' in completions, got: {labels_zst}"

    # Trigger completion for 'comp'
    await initialized_lsp_client.change_document(uri, base_text + "comp", version=3)
    await initialized_lsp_client.wait_for_diagnostics(uri)
    res_comp = await initialized_lsp_client.completion(uri, line=1, character=4)
    items_comp = res_comp.get("items", []) if isinstance(res_comp, dict) else res_comp
    labels_comp = [item["label"] for item in items_comp]
    assert "compinit" in labels_comp, f"Expected 'compinit' in completions, got: {labels_comp}"


@pytest.mark.asyncio
async def test_zsh_typeset_and_builtins_declarations(initialized_lsp_client: LspClient):
    """Verify typeset and zstyle declarations produce valid semantic symbols with hover."""
    uri = "file:///tmp/typeset_test.zsh"
    # Line 0: #!/usr/bin/env zsh
    # Line 1: typeset custom_var="hello"
    # Line 2: print -r -- "$custom_var"
    # Line 3: zstyle -s ':completion:*' format my_format
    # Line 4: print -r -- "$my_format"
    text = """#!/usr/bin/env zsh
typeset custom_var="hello"
print -r -- "$custom_var"
zstyle -s ':completion:*' format my_format
print -r -- "$my_format"
"""
    await initialized_lsp_client.open_document(uri, "shellscript", text)
    diags = await initialized_lsp_client.wait_for_diagnostics(uri)
    assert diags == [], f"Expected no unused variable diagnostics, got: {diags}"

    # Hover on custom_var reference (line 2, character 16)
    hover_custom = await initialized_lsp_client.hover(uri, 2, 16)
    assert hover_custom is not None, "Hover on custom_var reference should succeed"
    content = hover_custom["contents"]["value"]
    assert "custom_var" in content
    assert "Declaration" in content or "Variable" in content

    # Hover on my_format reference (line 4, character 16)
    hover_format = await initialized_lsp_client.hover(uri, 4, 16)
    assert hover_format is not None, "Hover on my_format reference should succeed"
    content = hover_format["contents"]["value"]
    assert "my_format" in content
    assert "Variable" in content

