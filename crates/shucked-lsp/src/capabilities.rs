//! Server capabilities calculation and supported capabilities for Shucked LSP.

use lsp_types as types;
use lsp_types::{
    CodeActionKind, CodeActionOptions, DiagnosticOptions, OneOf, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, WorkDoneProgressOptions,
    WorkspaceFoldersServerCapabilities,
};

use crate::PositionEncoding;

/// Calculate LSP server capabilities for the given options.
pub fn server_capabilities(
    position_encoding: PositionEncoding,
    workspace_diagnostics_enabled: bool,
) -> types::ServerCapabilities {
    types::ServerCapabilities {
        position_encoding: Some(position_encoding.into()),
        code_action_provider: Some(types::CodeActionProviderCapability::Options(
            CodeActionOptions {
                code_action_kinds: Some(
                    SupportedCodeAction::all()
                        .map(SupportedCodeAction::to_kind)
                        .collect(),
                ),
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: Some(true),
                },
                resolve_provider: Some(true),
            },
        )),
        workspace: Some(types::WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        completion_provider: Some(types::CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(vec![
                "$".to_owned(),
                "{".to_owned(),
                "/".to_owned(),
                "-".to_owned(),
                " ".to_owned(),
                "=".to_owned(),
                ":".to_owned(),
                "|".to_owned(),
                "&".to_owned(),
                ";".to_owned(),
                "<".to_owned(),
                ">".to_owned(),
                "(".to_owned(),
            ]),
            ..types::CompletionOptions::default()
        }),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(types::DeclarationCapability::Simple(true)),
        implementation_provider: Some(types::ImplementationProviderCapability::Simple(true)),
        document_link_provider: Some(types::DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: WorkDoneProgressOptions {
                work_done_progress: Some(true),
            },
        }),
        call_hierarchy_provider: Some(types::CallHierarchyServerCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(types::FoldingRangeProviderCapability::Simple(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Right(types::WorkspaceSymbolOptions {
            work_done_progress_options: WorkDoneProgressOptions {
                work_done_progress: Some(true),
            },
            resolve_provider: Some(false),
        })),
        diagnostic_provider: Some(types::DiagnosticServerCapabilities::Options(
            DiagnosticOptions {
                identifier: Some(crate::DIAGNOSTIC_NAME.into()),
                inter_file_dependencies: true,
                workspace_diagnostics: workspace_diagnostics_enabled,
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: Some(true),
                },
            },
        )),
        execute_command_provider: Some(types::ExecuteCommandOptions {
            commands: SupportedCommand::all()
                .map(|command| command.identifier().to_string())
                .collect(),
            work_done_progress_options: WorkDoneProgressOptions {
                work_done_progress: Some(false),
            },
        }),
        hover_provider: Some(types::HoverProviderCapability::Simple(true)),
        rename_provider: Some(OneOf::Right(types::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions {
                work_done_progress: Some(true),
            },
        })),
        selection_range_provider: Some(types::SelectionRangeProviderCapability::Simple(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(
            types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                types::SemanticTokensOptions {
                    work_done_progress_options: types::WorkDoneProgressOptions::default(),
                    legend: crate::handlers::semantic_tokens::semantic_tokens_legend(),
                    range: Some(false),
                    full: Some(types::SemanticTokensFullOptions::Bool(true)),
                },
            ),
        ),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                ..Default::default()
            },
        )),
        ..Default::default()
    }
}

/// Code action kinds supported by Shucked LSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SupportedCodeAction {
    /// Quick fix code actions.
    QuickFix,
    /// Fix all code actions (`source.fixAll.shucked`).
    SourceFixAll,
    /// Refactor code actions.
    Refactor,
    /// Extract refactorings.
    RefactorExtract,
    /// Inline refactorings.
    RefactorInline,
    /// Rewrite refactorings.
    RefactorRewrite,
}

impl SupportedCodeAction {
    /// Iterator over all supported code action types.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::QuickFix,
            Self::SourceFixAll,
            Self::Refactor,
            Self::RefactorExtract,
            Self::RefactorInline,
            Self::RefactorRewrite,
        ]
        .into_iter()
    }

    /// Convert to LSP `CodeActionKind`.
    pub fn to_kind(self) -> CodeActionKind {
        match self {
            Self::QuickFix => CodeActionKind::QUICKFIX,
            Self::SourceFixAll => crate::SOURCE_FIX_ALL_SHUCKED,
            Self::Refactor => CodeActionKind::REFACTOR,
            Self::RefactorExtract => CodeActionKind::REFACTOR_EXTRACT,
            Self::RefactorInline => CodeActionKind::REFACTOR_INLINE,
            Self::RefactorRewrite => CodeActionKind::REFACTOR_REWRITE,
        }
    }
}

/// Custom commands supported by Shucked LSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SupportedCommand {
    /// Apply an explicitly selected, snapshot-checked command correction.
    ApplyCommandCorrection,
    /// Apply autofix command.
    ApplyAutofix,
    /// Apply directive command.
    ApplyDirective,
    /// Print debug information command.
    PrintDebugInformation,
    /// Refresh execution environment evidence.
    RefreshEnvironment,
}

impl SupportedCommand {
    /// Iterator over all supported commands.
    pub fn all() -> impl Iterator<Item = Self> {
        [
            Self::ApplyCommandCorrection,
            Self::ApplyAutofix,
            Self::ApplyDirective,
            Self::PrintDebugInformation,
            Self::RefreshEnvironment,
        ]
        .into_iter()
    }

    /// Command identifier string advertised to the client.
    pub fn identifier(self) -> &'static str {
        match self {
            Self::ApplyCommandCorrection => "shucked.applyCommandCorrection",
            Self::ApplyAutofix => "shucked.applyAutofix",
            Self::ApplyDirective => "shucked.applyDirective",
            Self::PrintDebugInformation => "shucked.printDebugInformation",
            Self::RefreshEnvironment => "shucked.refreshEnvironment",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_formatting_capabilities() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        assert_eq!(
            capabilities.document_formatting_provider,
            Some(OneOf::Left(true))
        );
        assert_eq!(
            capabilities.document_range_formatting_provider,
            Some(OneOf::Left(true))
        );
    }

    #[test]
    fn advertises_navigation_completion_and_rename_capabilities() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        assert!(capabilities.completion_provider.is_some());
        assert_eq!(capabilities.definition_provider, Some(OneOf::Left(true)));
        assert!(matches!(
            capabilities.declaration_provider,
            Some(types::DeclarationCapability::Simple(true))
        ));
        assert!(matches!(
            capabilities.implementation_provider,
            Some(types::ImplementationProviderCapability::Simple(true))
        ));
        assert!(capabilities.document_link_provider.is_some());
        assert_eq!(capabilities.references_provider, Some(OneOf::Left(true)));
        assert_eq!(
            capabilities.document_highlight_provider,
            Some(OneOf::Left(true))
        );
        let Some(OneOf::Right(rename)) = capabilities.rename_provider else {
            panic!("expected rename options");
        };
        assert_eq!(rename.prepare_provider, Some(true));
    }

    #[test]
    fn advertises_document_symbol_capability() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        assert_eq!(
            capabilities.document_symbol_provider,
            Some(OneOf::Left(true))
        );
    }

    #[test]
    fn advertises_workspace_symbol_capability_without_resolve() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        let Some(OneOf::Right(options)) = capabilities.workspace_symbol_provider else {
            panic!("expected workspace symbol options");
        };
        assert_eq!(options.resolve_provider, Some(false));
    }

    #[test]
    fn advertises_only_non_formatting_execute_commands() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        let commands = capabilities
            .execute_command_provider
            .expect("server should advertise execute commands")
            .commands;

        assert!(commands.contains(&"shucked.applyAutofix".to_owned()));
        assert!(commands.contains(&"shucked.applyDirective".to_owned()));
        assert!(commands.contains(&"shucked.printDebugInformation".to_owned()));
        assert!(!commands.contains(&"shucked.applyFormat".to_owned()));
    }

    #[test]
    fn advertises_workspace_diagnostics_only_when_enabled() {
        for (enabled, expected) in [(false, false), (true, true)] {
            let capabilities = server_capabilities(PositionEncoding::UTF16, enabled);
            let Some(types::DiagnosticServerCapabilities::Options(options)) =
                capabilities.diagnostic_provider
            else {
                panic!("expected diagnostic options");
            };
            assert_eq!(options.workspace_diagnostics, expected);
        }
    }

    #[test]
    fn advertises_refactor_code_action_kinds() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        let Some(types::CodeActionProviderCapability::Options(options)) =
            capabilities.code_action_provider
        else {
            panic!("expected code action provider options");
        };
        let kinds = options
            .code_action_kinds
            .expect("should advertise action kinds");
        assert!(kinds.contains(&types::CodeActionKind::REFACTOR));
        assert!(kinds.contains(&types::CodeActionKind::REFACTOR_EXTRACT));
        assert!(kinds.contains(&types::CodeActionKind::REFACTOR_INLINE));
        assert!(kinds.contains(&types::CodeActionKind::REFACTOR_REWRITE));
    }

    #[test]
    fn advertises_inlay_hint_capability() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        assert_eq!(capabilities.inlay_hint_provider, Some(OneOf::Left(true)));
    }

    #[test]
    fn advertises_semantic_tokens_capability() {
        let capabilities = server_capabilities(PositionEncoding::UTF16, false);
        let Some(types::SemanticTokensServerCapabilities::SemanticTokensOptions(options)) =
            capabilities.semantic_tokens_provider
        else {
            panic!("expected semantic tokens provider options");
        };
        assert_eq!(
            options.full,
            Some(types::SemanticTokensFullOptions::Bool(true))
        );
        let legend = crate::handlers::semantic_tokens::semantic_tokens_legend();
        assert_eq!(options.legend.token_types, legend.token_types);
        assert_eq!(options.legend.token_modifiers, legend.token_modifiers);
        assert_eq!(options.legend.token_types.len(), 12);
        assert_eq!(options.legend.token_modifiers.len(), 6);
        assert!(
            options
                .legend
                .token_types
                .iter()
                .any(|kind| kind.as_str() == "shellOption")
        );
        assert!(
            options
                .legend
                .token_modifiers
                .iter()
                .any(|modifier| modifier == &types::SemanticTokenModifier::STATIC)
        );
    }
}
