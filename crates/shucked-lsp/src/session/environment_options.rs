use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Environment intelligence configuration. Execution permission is initialization-only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EnvironmentOptions {
    /// Workspace checks are the default; portable suppresses host absence checks.
    pub policy: Option<String>,
    /// Explicitly attached terminal session.
    pub session_id: Option<String>,
    /// Explicit launch directory. Otherwise the workspace folder is assumed.
    pub cwd: Option<PathBuf>,
    /// Captured target inventory; no local commands are consulted for this target.
    pub target_inventory: Option<PathBuf>,
    /// Expected project commands, keyed by name; values are generated, optional or required.
    pub declarations: BTreeMap<String, String>,
}

impl EnvironmentOptions {
    // Empty configuration paths clear inherited values; they are not filesystem targets.
    pub(crate) fn normalize(&mut self) {
        if self
            .cwd
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            self.cwd = None;
        }
        if self
            .target_inventory
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            self.target_inventory = None;
        }
        if self.session_id.as_ref().is_some_and(String::is_empty) {
            self.session_id = None;
        }
    }

    pub(crate) fn overlay(&mut self, next: &Self) {
        if next.session_id.is_some() {
            self.session_id.clone_from(&next.session_id);
        }
        if next.policy.is_some() {
            self.policy.clone_from(&next.policy);
        }
        if next.cwd.is_some() {
            self.cwd.clone_from(&next.cwd);
        }
        if next.target_inventory.is_some() {
            self.target_inventory.clone_from(&next.target_inventory);
        }
        self.declarations.extend(next.declarations.clone());
    }
}
