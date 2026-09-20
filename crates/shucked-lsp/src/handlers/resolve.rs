use lsp_types as types;
use shucked_linter::{
    ShellCheckCodeMap, SuppressionAction, SuppressionSource, rule_metadata_by_code,
};
use shucked_semantic::{
    BindingAttributes, EditorHover, EditorSymbolKind, ScopeKind, SemanticModel,
};

use crate::edit::PositionExt;
use crate::handlers::zsh;
use crate::session::{Client, DocumentSnapshot};

pub(crate) fn hover(
    snapshot: DocumentSnapshot,
    _client: &Client,
    params: types::HoverParams,
) -> crate::server::Result<Option<types::Hover>> {
    let Some(analysis) = snapshot.analysis() else {
        return Ok(None);
    };

    let query = snapshot.query();
    let source = analysis.source();
    let shellcheck_map = ShellCheckCodeMap::default();
    let position = params.text_document_position_params.position;
    let offset = position.to_offset(source, query.document().index(), snapshot.encoding());

    if let Some(hover) = directive_hover(
        &snapshot,
        source,
        analysis.line_index(),
        analysis.indexer().comment_index(),
        &shellcheck_map,
        params.text_document_position_params.position,
        offset,
    ) {
        return Ok(Some(hover));
    }

    let semantic = analysis.semantic();
    if let Some(semantic_hover) = semantic.editor_query().hover_at_offset(offset) {
        return Ok(Some(render_semantic_hover(
            &snapshot,
            source,
            analysis.line_index(),
            semantic,
            &semantic_hover,
        )));
    }

    if let Some(fallback) = fallback_hover(&snapshot, analysis.as_ref(), source, offset) {
        return Ok(Some(fallback));
    }

    Ok(None)
}

fn directive_hover(
    snapshot: &DocumentSnapshot,
    source: &str,
    line_index: &shucked_indexer::LineIndex,
    comment_index: &shucked_indexer::CommentIndex,
    shellcheck_map: &ShellCheckCodeMap,
    position: types::Position,
    offset: usize,
) -> Option<types::Hover> {
    let directives = shucked_linter::parse_directives(source, comment_index, shellcheck_map);
    let line = usize::try_from(position.line).unwrap_or_default() + 1;
    let directive = directives.iter().find(|directive| {
        usize::try_from(directive.line).ok() == Some(line)
            && offset >= usize::from(directive.range.start())
            && offset <= usize::from(directive.range.end())
            && matches!(
                (directive.source, directive.action),
                (
                    SuppressionSource::Shuck,
                    SuppressionAction::Ignore
                        | SuppressionAction::Disable
                        | SuppressionAction::DisableFile
                ) | (SuppressionSource::ShellCheck, SuppressionAction::Disable)
            )
    })?;

    let (display_code, canonical_code, start_offset, end_offset) = code_at_offset(
        directive.range.slice(source),
        usize::from(directive.range.start()),
        offset,
    )?;
    let metadata = rule_metadata_by_code(&canonical_code)?;

    let rule_name = humanize_rule_name(&canonical_code);
    let fix_marker = if metadata.fix_description.is_some() {
        "Fix available"
    } else {
        "No auto-fix"
    };
    let mut markdown = format!(
        "# {} ({})\n\n{}\n\n{}\n\n{}",
        rule_name, display_code, metadata.description, fix_marker, metadata.rationale
    );
    if display_code != canonical_code {
        markdown.push_str(&format!("\n\nSee also: {}", canonical_code));
    } else if let Some(rule) = shucked_linter::code_to_rule(&canonical_code)
        && let Some(shellcheck_code) = shellcheck_map.code_for_rule(rule)
    {
        markdown.push_str(&format!("\n\nSee also: SC{shellcheck_code:04}"));
    }

    Some(types::Hover {
        contents: types::HoverContents::Markup(types::MarkupContent {
            kind: types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(crate::edit::to_lsp_range(
            shucked_ast::TextRange::new(
                shucked_ast::TextSize::new(start_offset as u32),
                shucked_ast::TextSize::new(end_offset as u32),
            ),
            source,
            line_index,
            snapshot.encoding(),
        )),
    })
}

fn render_semantic_hover(
    snapshot: &DocumentSnapshot,
    source: &str,
    line_index: &shucked_indexer::LineIndex,
    semantic: &SemanticModel,
    hover: &EditorHover,
) -> types::Hover {
    let mut markdown = format!(
        "### {}\n\n{}",
        markdown_code(hover.symbol.name.as_str()),
        symbol_kind_label(hover.symbol.kind)
    );

    if hover.runtime {
        markdown.push_str("\n\nProvided by the active shell runtime.");
        if let Some(doc) = zsh::special_parameter_doc(hover.symbol.name.as_str()) {
            markdown.push_str(&format!(
                "\n\n**Type:** {}\n\n{}",
                doc.param_type, doc.markdown
            ));
        }
    } else {
        markdown.push_str(&format!(
            "\n\nDefined at line {}, column {}.",
            hover.symbol.definition_span.start.line(),
            hover.symbol.definition_span.start.column()
        ));
    }

    markdown.push_str(&format!(
        "\n\nScope: {}.",
        scope_summary(semantic, hover.symbol.scope)
    ));

    let attributes = attribute_labels(hover.attributes);
    if !attributes.is_empty() {
        markdown.push_str(&format!("\n\nAttributes: {}.", attributes.join(", ")));
    }
    if hover.imported {
        markdown.push_str("\n\nImported into this analysis.");
    }
    if let Some(count) = hover.function_call_count {
        let noun = if count == 1 { "site" } else { "sites" };
        markdown.push_str(&format!("\n\nFile-local call sites: {count} {noun}."));
    }

    types::Hover {
        contents: types::HoverContents::Markup(types::MarkupContent {
            kind: types::MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(crate::edit::to_lsp_range(
            hover.target_span.to_range(),
            source,
            line_index,
            snapshot.encoding(),
        )),
    }
}

fn fallback_hover(
    snapshot: &DocumentSnapshot,
    analysis: &crate::handlers::analysis::DocumentAnalysis,
    source: &str,
    offset: usize,
) -> Option<types::Hover> {
    let (start, end, word) = find_word_at_offset(source, offset)?;
    let text_range = shucked_ast::TextRange::new(
        shucked_ast::TextSize::new(start as u32),
        shucked_ast::TextSize::new(end as u32),
    );
    let lsp_range = Some(crate::edit::to_lsp_range(
        text_range,
        source,
        analysis.line_index(),
        snapshot.encoding(),
    ));

    // 1. Is this word an option inside `setopt` or `unsetopt`?
    if is_option_context(source, analysis.indexer(), start)
        && let Some((opt_doc, is_inverted)) = zsh::option_doc(word)
    {
        let markdown = format!(
            "### `{}` (Zsh Option)\n\n{}\n\n**Default:** {}",
            if is_inverted {
                format!("NO_{}", opt_doc.canonical_name)
            } else {
                opt_doc.canonical_name.to_string()
            },
            opt_doc.description,
            if opt_doc.default_on {
                "enabled"
            } else {
                "disabled"
            },
        );
        return Some(types::Hover {
            contents: types::HoverContents::Markup(types::MarkupContent {
                kind: types::MarkupKind::Markdown,
                value: markdown,
            }),
            range: lsp_range,
        });
    }

    // 2. Is this word in command position? Check builtins!
    if is_command_position(source, start)
        && let Some(builtin) = zsh::builtin_doc(word)
    {
        return Some(types::Hover {
            contents: types::HoverContents::Markup(types::MarkupContent {
                kind: types::MarkupKind::Markdown,
                value: builtin.markdown.to_string(),
            }),
            range: lsp_range,
        });
    }

    // 3. Special parameter fallback (e.g. `$pipestatus` or `pipestatus` or `$match`)
    let bare_param = word.strip_prefix('$').unwrap_or(word);
    let bare_param = bare_param
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(bare_param);
    let bare_param = bare_param.split('[').next().unwrap_or(bare_param);
    if let Some(doc) = zsh::special_parameter_doc(bare_param) {
        let markdown = format!(
            "### `${}` (Zsh Special Parameter)\n\n**Type:** {}\n\n{}",
            bare_param, doc.param_type, doc.markdown
        );
        return Some(types::Hover {
            contents: types::HoverContents::Markup(types::MarkupContent {
                kind: types::MarkupKind::Markdown,
                value: markdown,
            }),
            range: lsp_range,
        });
    }

    None
}

fn find_word_at_offset(source: &str, offset: usize) -> Option<(usize, usize, &str)> {
    if offset > source.len() || !source.is_char_boundary(offset) {
        return None;
    }
    let is_delim = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                ';' | '|' | '&' | '(' | ')' | '<' | '>' | '`' | '"' | '\''
            )
    };

    let probe =
        if offset == source.len() || is_delim(source[offset..].chars().next().unwrap_or(' ')) {
            if offset > 0 && !is_delim(source[..offset].chars().next_back().unwrap_or(' ')) {
                offset - 1
            } else {
                return None;
            }
        } else {
            offset
        };

    let start = source[..probe]
        .char_indices()
        .rev()
        .find(|&(_, c)| is_delim(c))
        .map(|(idx, c)| idx + c.len_utf8())
        .unwrap_or(0);

    let end = source[probe..]
        .char_indices()
        .find(|&(_, c)| is_delim(c))
        .map(|(idx, _)| probe + idx)
        .unwrap_or(source.len());

    if start < end {
        Some((start, end, &source[start..end]))
    } else {
        None
    }
}

fn is_option_context(source: &str, indexer: &shucked_indexer::Indexer, word_start: usize) -> bool {
    let line = indexer
        .line_index()
        .line_number(shucked_ast::TextSize::new(word_start as u32));
    let Some(line_range) = indexer.line_index().line_range(line, source) else {
        return false;
    };
    let line_start = usize::from(line_range.start());
    let before = &source[line_start..word_start];
    let command_start = before
        .char_indices()
        .rev()
        .find_map(|(index, ch)| {
            matches!(ch, ';' | '|' | '&' | '(' | '{').then_some(index + ch.len_utf8())
        })
        .unwrap_or(0);
    let mut words = before[command_start..].split_whitespace();
    let first = match words.next() {
        Some("then" | "do" | "else") => words.next(),
        first => first,
    };
    let Some(first) = first else {
        return false;
    };
    matches!(first, "setopt" | "unsetopt")
}

fn is_command_position(source: &str, word_start: usize) -> bool {
    let raw_before = &source[..word_start];
    if raw_before.ends_with('\n')
        || raw_before
            .rsplit_once('\n')
            .is_some_and(|(_, line_prefix)| line_prefix.trim().is_empty())
    {
        return true;
    }
    let before = raw_before.trim_end();
    if before.is_empty() {
        return true;
    }
    if before.ends_with('\n')
        || before.ends_with(';')
        || before.ends_with('|')
        || before.ends_with('&')
        || before.ends_with('(')
        || before.ends_with('{')
    {
        return true;
    }
    let previous = before
        .rsplit(|ch: char| ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '(' | '{'))
        .find(|word| !word.is_empty());
    matches!(previous, Some("then" | "do" | "else" | "elif"))
}

fn markdown_code(text: &str) -> String {
    format!("`{}`", text.replace('`', "\\`"))
}

fn symbol_kind_label(kind: EditorSymbolKind) -> &'static str {
    match kind {
        EditorSymbolKind::Function => "Function",
        EditorSymbolKind::Variable => "Variable",
        EditorSymbolKind::Array => "Array variable",
        EditorSymbolKind::AssociativeArray => "Associative array variable",
        EditorSymbolKind::Declaration => "Declaration",
        EditorSymbolKind::RuntimeName => "Runtime name",
    }
}

fn scope_summary(semantic: &SemanticModel, scope: shucked_semantic::ScopeId) -> String {
    match semantic.scope_kind(scope) {
        ScopeKind::File => "top-level".to_owned(),
        ScopeKind::Function(function) => function
            .static_names()
            .first()
            .map(|name| format!("function {}", markdown_code(name.as_str())))
            .unwrap_or_else(|| "function-local".to_owned()),
        ScopeKind::Subshell => "subshell".to_owned(),
        ScopeKind::CommandSubstitution => "command substitution".to_owned(),
        ScopeKind::Pipeline => "pipeline".to_owned(),
    }
}

fn attribute_labels(attributes: BindingAttributes) -> Vec<&'static str> {
    [
        (BindingAttributes::EXPORTED, "exported"),
        (BindingAttributes::READONLY, "readonly"),
        (BindingAttributes::LOCAL, "local"),
        (BindingAttributes::INTEGER, "integer"),
        (BindingAttributes::ARRAY, "array"),
        (BindingAttributes::ASSOC, "associative array"),
        (BindingAttributes::NAMEREF, "nameref"),
        (BindingAttributes::LOWERCASE, "lowercase"),
        (BindingAttributes::UPPERCASE, "uppercase"),
        (
            BindingAttributes::DECLARATION_INITIALIZED,
            "initialized by declaration",
        ),
    ]
    .into_iter()
    .filter_map(|(flag, label)| attributes.contains(flag).then_some(label))
    .collect()
}

fn code_at_offset(
    text: &str,
    base_offset: usize,
    offset: usize,
) -> Option<(String, String, usize, usize)> {
    let mut search_from = 0usize;
    for token in text.split(|ch: char| !matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '-')) {
        if token.is_empty() {
            continue;
        }
        let start = text[search_from..].find(token)? + search_from;
        search_from = start + token.len();
        let start_offset = base_offset + start;
        let end_offset = start_offset + token.len();
        if offset < start_offset || offset > end_offset {
            continue;
        }

        if let Some(rule) = shucked_linter::code_to_rule(token) {
            return Some((
                token.to_owned(),
                rule.code().to_owned(),
                start_offset,
                end_offset,
            ));
        }
        if let Some(rule) = ShellCheckCodeMap::default().resolve(token) {
            return Some((
                token.to_owned(),
                rule.code().to_owned(),
                start_offset,
                end_offset,
            ));
        }
    }

    None
}

fn humanize_rule_name(code: &str) -> String {
    let Some(rule) = shucked_linter::code_to_rule(code) else {
        return code.to_owned();
    };
    let raw = format!("{rule:?}");
    let mut output = String::new();
    for (index, ch) in raw.chars().enumerate() {
        if index > 0 && ch.is_uppercase() {
            output.push(' ');
        }
        output.push(ch);
    }
    output
}

#[cfg(test)]
mod tests {
    use crossbeam::channel;
    use lsp_types::{
        ClientCapabilities, HoverParams, Position, TextDocumentIdentifier,
        TextDocumentPositionParams, Url, WorkDoneProgressParams,
    };
    use shucked_ast::{TextRange, TextSize};
    use shucked_indexer::LineIndex;

    use super::*;
    use crate::{
        Client, GlobalOptions, PositionEncoding, Session, TextDocument, Workspace, Workspaces,
    };

    fn make_snapshot(source: &str) -> (DocumentSnapshot, Client) {
        make_snapshot_with_encoding(source, PositionEncoding::UTF16)
    }

    fn make_snapshot_with_encoding(
        source: &str,
        encoding: PositionEncoding,
    ) -> (DocumentSnapshot, Client) {
        let (main_loop_sender, _main_loop_receiver) = channel::unbounded();
        let (client_sender, _client_receiver) = channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let workspace_root = std::env::temp_dir().join("shuck-server-hover-tests");
        let workspace_uri =
            Url::from_file_path(&workspace_root).expect("workspace path should convert to a URL");
        let workspaces = Workspaces::new(vec![Workspace::default(workspace_uri)]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &ClientCapabilities::default(),
            encoding,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");

        let uri = script_uri();
        session.open_text_document(
            uri.clone(),
            TextDocument::new(source.to_owned(), 1).with_language_id("shellscript"),
        );

        (
            session
                .take_snapshot(uri)
                .expect("test document should produce a snapshot"),
            client,
        )
    }

    fn script_uri() -> Url {
        Url::from_file_path(std::env::temp_dir().join("shuck-server-hover-tests/script.sh"))
            .expect("script path should convert to a URL")
    }

    fn hover_params(source: &str, needle: &str, encoding: PositionEncoding) -> HoverParams {
        let offset = source.find(needle).expect("needle should exist") + needle.len() / 2;
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: script_uri() },
                position: position_for_offset(source, offset, encoding),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        }
    }

    fn position_for_offset(source: &str, offset: usize, encoding: PositionEncoding) -> Position {
        let index = LineIndex::new(source);
        crate::edit::to_lsp_range(
            TextRange::new(TextSize::new(offset as u32), TextSize::new(offset as u32)),
            source,
            &index,
            encoding,
        )
        .start
    }

    fn hover_markdown(hover: types::Hover) -> String {
        let types::HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover content");
        };
        markup.value
    }

    #[test]
    fn hover_resolves_shuck_ignore_codes() {
        let (snapshot, client) = make_snapshot("#!/bin/bash\necho $foo  # shucked: ignore=C006\n");
        let hover = hover(
            snapshot,
            &client,
            HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: Url::from_file_path(
                            std::env::temp_dir()
                                .join("shuck-server-hover-tests")
                                .join("script.sh"),
                        )
                        .expect("script path should convert to a URL"),
                    },
                    position: Position::new(1, 30),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
        .expect("hover request should succeed")
        .expect("directive hover should be present");

        let types::HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover content");
        };
        assert!(markup.value.contains("Undefined Variable"));
        assert!(markup.value.contains("C006"));
        assert!(markup.value.contains("Fix available") || markup.value.contains("No auto-fix"));
    }

    #[test]
    fn hover_resolves_shellcheck_disable_codes() {
        let (snapshot, client) =
            make_snapshot("#!/bin/bash\necho $foo  # shellcheck disable=SC2154\n");
        let hover = hover(
            snapshot,
            &client,
            HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: Url::from_file_path(
                            std::env::temp_dir()
                                .join("shuck-server-hover-tests")
                                .join("script.sh"),
                        )
                        .expect("script path should convert to a URL"),
                    },
                    position: Position::new(1, 37),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
        .expect("hover request should succeed")
        .expect("directive hover should be present");

        let types::HoverContents::Markup(markup) = hover.contents else {
            panic!("expected markdown hover content");
        };
        assert!(markup.value.contains("Undefined Variable"));
        assert!(markup.value.contains("SC2154"));
        assert!(markup.value.contains("See also: C006"));
        assert!(markup.value.contains("Fix available") || markup.value.contains("No auto-fix"));
    }

    #[test]
    fn hover_falls_back_to_semantic_symbols() {
        let source = "#!/bin/bash\nname=world\nprintf '%s\\n' \"$name\"\n";
        let (snapshot, client) = make_snapshot(source);
        let hover = hover(
            snapshot,
            &client,
            hover_params(source, "name\"", PositionEncoding::UTF16),
        )
        .expect("hover request should succeed")
        .expect("semantic hover should be present");

        let markdown = hover_markdown(hover.clone());
        assert!(markdown.contains("`name`"));
        assert!(markdown.contains("Variable"));
        assert!(markdown.contains("Defined at line 2, column 1"));
        assert!(markdown.contains("Scope: top-level"));
        let range = hover.range.expect("semantic hover should have a range");
        assert_eq!(range.start.line, 2);
    }

    #[test]
    fn hover_reports_semantic_function_call_details() {
        let source = "#!/bin/bash\nbuild() { :; }\nbuild\n";
        let (snapshot, client) = make_snapshot(source);
        let hover = hover(
            snapshot,
            &client,
            hover_params(source, "build\n", PositionEncoding::UTF16),
        )
        .expect("hover request should succeed")
        .expect("function hover should be present");

        let markdown = hover_markdown(hover);
        assert!(markdown.contains("`build`"));
        assert!(markdown.contains("Function"));
        assert!(markdown.contains("File-local call sites: 1 site"));
    }

    #[test]
    fn hover_reports_runtime_names() {
        let source = "#!/bin/bash\nprintf '%s\\n' \"$HOME\"\n";
        let (snapshot, client) = make_snapshot(source);
        let hover = hover(
            snapshot,
            &client,
            hover_params(source, "HOME", PositionEncoding::UTF16),
        )
        .expect("hover request should succeed")
        .expect("runtime hover should be present");

        let markdown = hover_markdown(hover);
        assert!(markdown.contains("`HOME`"));
        assert!(markdown.contains("Runtime name"));
        assert!(markdown.contains("Provided by the active shell runtime"));
    }

    #[test]
    fn semantic_hover_ranges_use_negotiated_position_encoding() {
        let source = "#!/bin/bash\nname=world\nprintf 'é' \"$name\"\n";
        let (utf16_snapshot, utf16_client) =
            make_snapshot_with_encoding(source, PositionEncoding::UTF16);
        let utf16_hover = hover(
            utf16_snapshot,
            &utf16_client,
            hover_params(source, "name\"", PositionEncoding::UTF16),
        )
        .expect("hover request should succeed")
        .expect("utf16 hover should be present");

        let (utf8_snapshot, utf8_client) =
            make_snapshot_with_encoding(source, PositionEncoding::UTF8);
        let utf8_hover = hover(
            utf8_snapshot,
            &utf8_client,
            hover_params(source, "name\"", PositionEncoding::UTF8),
        )
        .expect("hover request should succeed")
        .expect("utf8 hover should be present");

        let utf16_range = utf16_hover.range.expect("utf16 hover range");
        let utf8_range = utf8_hover.range.expect("utf8 hover range");
        assert_eq!(utf16_range.start.line, utf8_range.start.line);
        assert!(utf8_range.start.character > utf16_range.start.character);
    }

    #[test]
    fn hover_returns_none_for_non_shell_documents() {
        let (main_loop_sender, _main_loop_receiver) = channel::unbounded();
        let (client_sender, _client_receiver) = channel::unbounded();
        let client = Client::new(main_loop_sender, client_sender);
        let workspace_root = std::env::temp_dir().join("shuck-server-hover-tests");
        let workspace_uri =
            Url::from_file_path(&workspace_root).expect("workspace path should convert to a URL");
        let workspaces = Workspaces::new(vec![Workspace::default(workspace_uri)]);
        let global = GlobalOptions::default().into_settings(client.clone());
        let mut session = Session::new(
            &ClientCapabilities::default(),
            PositionEncoding::UTF16,
            global,
            &workspaces,
            &client,
        )
        .expect("test session should initialize");

        let uri = Url::from_file_path(workspace_root.join("README.md"))
            .expect("document path should convert to a URL");
        session.open_text_document(
            uri.clone(),
            TextDocument::new("# shellcheck disable=SC2154\n".to_owned(), 1)
                .with_language_id("markdown"),
        );
        let snapshot = session
            .take_snapshot(uri.clone())
            .expect("test document should produce a snapshot");

        let hover = hover(
            snapshot,
            &client,
            HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri },
                    position: Position::new(0, 22),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            },
        )
        .expect("hover request should succeed");

        assert!(hover.is_none());
    }

    #[test]
    fn hover_zsh_builtin() {
        let source = "#!/bin/zsh\nautoload -Uz compinit\nzstyle ':completion:*' verbose yes\n";
        let (snapshot, client) = make_snapshot(source);
        let params = hover_params(source, "autoload", PositionEncoding::UTF16);
        let h1 = hover(snapshot.clone(), &client, params)
            .expect("hover should succeed")
            .expect("hover should be present for autoload");
        let markdown = hover_markdown(h1);
        assert!(markdown.contains("`autoload` (Zsh Builtin)"));
        assert!(markdown.contains("$fpath"));

        let params2 = hover_params(source, "zstyle", PositionEncoding::UTF16);
        let h2 = hover(snapshot, &client, params2)
            .expect("hover should succeed")
            .expect("hover should be present for zstyle");
        let markdown2 = hover_markdown(h2);
        assert!(markdown2.contains("`zstyle` (Zsh Builtin)"));
    }

    #[test]
    fn hover_zsh_special_parameter() {
        let source = "#!/bin/zsh\necho $pipestatus[1]\necho $prompt\n";
        let (snapshot, client) = make_snapshot(source);
        let params = hover_params(source, "pipestatus", PositionEncoding::UTF16);
        let h1 = hover(snapshot.clone(), &client, params)
            .expect("hover should succeed")
            .expect("hover should be present for pipestatus");
        let markdown = hover_markdown(h1);
        assert!(markdown.contains("pipestatus"));
        assert!(markdown.contains("Array of integers"));

        let params2 = hover_params(source, "prompt", PositionEncoding::UTF16);
        let h2 = hover(snapshot, &client, params2)
            .expect("hover should succeed")
            .expect("hover should be present for prompt");
        let markdown2 = hover_markdown(h2);
        assert!(markdown2.contains("prompt"));
    }

    #[test]
    fn hover_zsh_option() {
        let source = "#!/bin/zsh\nsetopt NULL_GLOB\nunsetopt extended_glob\n";
        let (snapshot, client) = make_snapshot(source);
        let params = hover_params(source, "NULL_GLOB", PositionEncoding::UTF16);
        let h1 = hover(snapshot.clone(), &client, params)
            .expect("hover should succeed")
            .expect("hover should be present for NULL_GLOB");
        let markdown1 = hover_markdown(h1);
        assert!(markdown1.contains("`NULL_GLOB` (Zsh Option)"));
        assert!(markdown1.contains("filename generation"));

        let params2 = hover_params(source, "extended_glob", PositionEncoding::UTF16);
        let h2 = hover(snapshot, &client, params2)
            .expect("hover should succeed")
            .expect("hover should be present for extended_glob");
        let markdown2 = hover_markdown(h2);
        assert!(markdown2.contains("`EXTENDED_GLOB` (Zsh Option)"));
    }

    #[test]
    fn hover_zsh_syntax_constructs() {
        let source = "#!/bin/zsh\nmsg=\"hello world\"\necho ${(q)msg}\narr=(alpha beta gamma)\necho $arr[1]\n";
        let (snapshot, client) = make_snapshot(source);
        let params = hover_params(source, "msg", PositionEncoding::UTF16);
        let h1 = hover(snapshot.clone(), &client, params)
            .expect("hover should succeed")
            .expect("hover should be present for msg");
        let markdown = hover_markdown(h1);
        assert!(markdown.contains("msg"));

        let params2 = hover_params(source, "arr", PositionEncoding::UTF16);
        let h2 = hover(snapshot, &client, params2)
            .expect("hover should succeed")
            .expect("hover should be present for arr");
        let markdown2 = hover_markdown(h2);
        assert!(markdown2.contains("arr"));
    }

    #[test]
    fn zsh_syntax_constructs_produce_no_false_errors() {
        let source =
            "#!/bin/zsh\nfiles=(*(.))\ndirs=(*(/))\nfor f in $files; do\n  echo ${(f)f}\ndone\n";
        let (snapshot, _client) = make_snapshot(source);
        let analysis = snapshot.analysis().expect("analysis should exist");
        let raw = analysis.raw_diagnostics(&snapshot);
        assert!(
            raw.parse_error.is_none(),
            "unexpected parse error: {:?}",
            raw.parse_error
        );
    }
}
