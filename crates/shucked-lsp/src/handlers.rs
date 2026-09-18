//! Request and notification handlers for the Shucked language server.

pub(crate) mod analysis;
pub(crate) mod call_hierarchy;
pub(crate) mod editor_features;
pub(crate) mod fix;
pub(crate) mod folding;
pub(crate) mod format;
pub(crate) mod lint;
pub(crate) mod resolve;
pub(crate) mod selection;
pub(crate) mod symbols;
pub(crate) mod workspace_diagnostics;
pub(crate) mod workspace_functions;
pub(crate) mod workspace_variables;

pub use self::lint::generate_diagnostics;
