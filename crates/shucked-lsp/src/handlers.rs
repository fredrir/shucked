//! Request and notification handlers for the Shucked language server.

pub(crate) mod analysis;
pub(crate) mod call_hierarchy;
pub(crate) mod commands;
pub(crate) mod commands_actions;
pub(crate) mod commands_validation;
pub(crate) mod completion;
pub(crate) mod editor_features;
pub(crate) mod fix;
pub(crate) mod folding;
pub(crate) mod format;
pub(crate) mod inlay_hints;
pub(crate) mod lint;
pub(crate) mod refactor;
pub(crate) mod resolve;
pub(crate) mod selection;
pub(crate) mod semantic_tokens;
pub(crate) mod semantic_tokens_fish;
pub(crate) mod symbols;
pub(crate) mod workspace_diagnostics;
pub(crate) mod workspace_functions;
pub(crate) mod workspace_variables;
pub(crate) mod zsh;

pub use self::lint::{diagnostic_tags_for_rule, generate_diagnostics};
