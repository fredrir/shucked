//! Refactoring code actions for shell scripts.

use lsp_types as types;

use crate::session::DocumentSnapshot;

pub fn refactor_code_actions(
    _snapshot: &DocumentSnapshot,
    _params: &types::CodeActionParams,
) -> Vec<types::CodeActionOrCommand> {
    Vec::new()
}
