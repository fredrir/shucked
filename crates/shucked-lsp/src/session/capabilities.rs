use lsp_types::ClientCapabilities;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ResolvedClientCapabilities {
    pub(crate) completion_insert_replace: bool,
    pub(crate) completion_snippets: bool,
    pub(crate) completion_adjust_indentation: bool,
    pub(crate) code_action_deferred_edit_resolution: bool,
    pub(crate) apply_edit: bool,
    pub(crate) document_changes: bool,
    pub(crate) pull_diagnostics: bool,
    pub(crate) diagnostic_refresh: bool,
    pub(crate) semantic_token_refresh: bool,
    pub(crate) hierarchical_document_symbols: bool,
    pub(crate) folding_range_limit: Option<u32>,
    pub(crate) line_folding_only: bool,
}

impl ResolvedClientCapabilities {
    pub(super) fn new(client_capabilities: &ClientCapabilities) -> Self {
        let code_action_settings = client_capabilities
            .text_document
            .as_ref()
            .and_then(|doc_settings| doc_settings.code_action.as_ref());
        let code_action_data_support = code_action_settings
            .and_then(|settings| settings.data_support)
            .unwrap_or_default();
        let code_action_edit_resolution = code_action_settings
            .and_then(|settings| settings.resolve_support.as_ref())
            .is_some_and(|resolve_support| resolve_support.properties.contains(&"edit".into()));

        let apply_edit = client_capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.apply_edit)
            .unwrap_or_default();

        let document_changes = client_capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref())
            .and_then(|workspace_edit| workspace_edit.document_changes)
            .unwrap_or_default();

        let pull_diagnostics = client_capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.diagnostic.as_ref())
            .is_some();

        let hierarchical_document_symbols = client_capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.document_symbol.as_ref())
            .and_then(|document_symbol| document_symbol.hierarchical_document_symbol_support)
            .unwrap_or_default();

        let folding_ranges = client_capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.folding_range.as_ref());
        let folding_range_limit = folding_ranges.and_then(|folding| folding.range_limit);
        let line_folding_only = folding_ranges
            .and_then(|folding| folding.line_folding_only)
            .unwrap_or_default();

        Self {
            completion_snippets: client_capabilities
                .text_document
                .as_ref()
                .and_then(|document| document.completion.as_ref())
                .and_then(|completion| completion.completion_item.as_ref())
                .and_then(|item| item.snippet_support)
                .unwrap_or_default(),
            completion_adjust_indentation: client_capabilities
                .text_document
                .as_ref()
                .and_then(|document| document.completion.as_ref())
                .and_then(|completion| completion.completion_item.as_ref())
                .and_then(|item| item.insert_text_mode_support.as_ref())
                .is_some_and(|support| {
                    support
                        .value_set
                        .contains(&lsp_types::InsertTextMode::ADJUST_INDENTATION)
                }),
            completion_insert_replace: client_capabilities
                .text_document
                .as_ref()
                .and_then(|document| document.completion.as_ref())
                .and_then(|completion| completion.completion_item.as_ref())
                .and_then(|item| item.insert_replace_support)
                .unwrap_or_default(),
            code_action_deferred_edit_resolution: code_action_data_support
                && code_action_edit_resolution,
            apply_edit,
            document_changes,
            pull_diagnostics,
            diagnostic_refresh: client_capabilities
                .workspace
                .as_ref()
                .and_then(|w| w.diagnostic.as_ref())
                .and_then(|d| d.refresh_support)
                .unwrap_or(false),
            semantic_token_refresh: client_capabilities
                .workspace
                .as_ref()
                .and_then(|w| w.semantic_tokens.as_ref())
                .and_then(|d| d.refresh_support)
                .unwrap_or(false),
            hierarchical_document_symbols,
            folding_range_limit,
            line_folding_only,
        }
    }
}
