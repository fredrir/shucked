use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    Alias, CommandResolution, CommandSite, ExecutionContext, Provenance, ValidationEvidence,
    ValidationPolicy, resolve,
};

pub const INVENTORY_SCHEMA_VERSION: u32 = 1;
pub const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableIdentity {
    pub path: PathBuf,
    pub size: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    pub version: Option<String>,
    pub vendor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Executable {
    pub identity: ExecutableIdentity,
    pub provenance: Provenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchDirectory {
    /// Original PATH entry, retaining empty and relative entries.
    pub path: PathBuf,
    pub commands: BTreeMap<String, Executable>,
    pub complete: bool,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "evidence", rename_all = "camelCase")]
pub enum LookupEvidence {
    Present(Executable),
    Missing,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentSnapshot {
    pub target_id: String,
    pub platform: String,
    pub generation: u64,
    pub captured_unix_ms: u64,
    /// Captured targets deliberately freeze time; live snapshots must be
    /// invalidated by their owner's watcher or TTL before reusing old evidence.
    pub fresh: bool,
    pub search_path: Vec<SearchDirectory>,
    pub path_known: bool,
    pub case_sensitive: bool,
    pub executable_extensions: Vec<String>,
    pub builtins: BTreeSet<String>,
    pub builtins_complete: bool,
    /// Session state is consulted only for InteractiveSession contexts.
    pub functions: BTreeSet<String>,
    pub aliases: BTreeMap<String, Alias>,
    /// Exact queries supplement bounded enumeration without claiming the
    /// entire directory inventory is complete.
    pub exact_lookups: BTreeMap<String, LookupEvidence>,
    pub validators: BTreeMap<String, ValidationEvidence>,
}

impl EnvironmentSnapshot {
    pub fn empty(context: &ExecutionContext) -> Self {
        Self {
            target_id: context.target_id.clone(),
            platform: std::env::consts::OS.into(),
            generation: 0,
            captured_unix_ms: 0,
            fresh: true,
            search_path: Vec::new(),
            path_known: false,
            case_sensitive: true,
            executable_extensions: Vec::new(),
            builtins: crate::builtins(context.dialect)
                .into_iter()
                .map(str::to_owned)
                .collect(),
            builtins_complete: false,
            functions: BTreeSet::new(),
            aliases: BTreeMap::new(),
            exact_lookups: BTreeMap::new(),
            validators: BTreeMap::new(),
        }
    }

    pub fn command_names(&self) -> BTreeSet<String> {
        self.search_path
            .iter()
            .flat_map(|directory| directory.commands.keys().cloned())
            .chain(self.exact_lookups.iter().filter_map(|(name, evidence)| {
                matches!(evidence, LookupEvidence::Present(_)).then_some(name.clone())
            }))
            .chain(self.builtins.iter().cloned())
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.path_known && self.fresh && self.search_path.iter().all(|directory| directory.complete)
    }

    pub fn lookup(&self, name: &str) -> LookupEvidence {
        if !self.fresh {
            return LookupEvidence::Unknown("Environment evidence is stale".into());
        }
        if let Some(evidence) = self.exact_lookups.get(name) {
            return evidence.clone();
        }
        if name.contains('/') || (self.platform == "windows" && name.contains('\\')) {
            return LookupEvidence::Unknown(
                "This explicit executable path was not captured".into(),
            );
        }
        if !self.path_known {
            return LookupEvidence::Unknown("The target execution PATH is unknown".into());
        }
        let key = if self.case_sensitive {
            name.to_owned()
        } else {
            name.to_lowercase()
        };
        for directory in &self.search_path {
            if let Some(executable) = directory.commands.get(&key) {
                return LookupEvidence::Present(executable.clone());
            }
            if !directory.complete {
                return LookupEvidence::Unknown(
                    directory
                        .failure
                        .clone()
                        .unwrap_or_else(|| "The execution PATH inventory is incomplete".into()),
                );
            }
        }
        LookupEvidence::Missing
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InventoryPrivacy {
    pub aliases_included: bool,
    pub function_names_included: bool,
    pub history_included: bool,
    pub environment_values_included: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInventory {
    pub schema_version: u32,
    pub label: String,
    pub context: ExecutionContext,
    pub snapshot: EnvironmentSnapshot,
    pub privacy: InventoryPrivacy,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InventoryEnvelope {
    schema_version: u32,
    sha256: String,
    inventory: TargetInventory,
}

#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("Inventory exceeds the {MAX_INVENTORY_BYTES} byte size limit")]
    TooLarge,
    #[error("Unsupported inventory schema version {0}")]
    UnsupportedVersion(u32),
    #[error("Inventory checksum does not match its contents")]
    Checksum,
    #[error("Invalid inventory: {0}")]
    Invalid(String),
    #[error("Invalid inventory JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl TargetInventory {
    /// Freeze a snapshot without carrying personal aliases, function names,
    /// interpreter options, or native-execution authority into the export.
    pub fn capture(
        label: impl Into<String>,
        context: &ExecutionContext,
        snapshot: &EnvironmentSnapshot,
    ) -> Self {
        let mut context = context.clone();
        context.policy = ValidationPolicy::Captured;
        context.native_execution_allowed = false;
        context.workspace = None;
        if let Some(interpreter) = context.interpreter.as_mut() {
            interpreter.options.clear();
        }
        let mut snapshot = snapshot.clone();
        snapshot.aliases.clear();
        snapshot.functions.clear();
        Self {
            schema_version: INVENTORY_SCHEMA_VERSION,
            label: label.into(),
            context,
            snapshot,
            privacy: InventoryPrivacy::default(),
        }
    }

    pub fn to_json(&self) -> Result<String, InventoryError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        let envelope = InventoryEnvelope {
            schema_version: INVENTORY_SCHEMA_VERSION,
            sha256: digest(&bytes),
            inventory: self.clone(),
        };
        let json = serde_json::to_string_pretty(&envelope)?;
        if json.len() > MAX_INVENTORY_BYTES {
            return Err(InventoryError::TooLarge);
        }
        Ok(json)
    }

    pub fn from_json(json: &str) -> Result<Self, InventoryError> {
        if json.len() > MAX_INVENTORY_BYTES {
            return Err(InventoryError::TooLarge);
        }
        let envelope: InventoryEnvelope = serde_json::from_str(json)?;
        if envelope.schema_version != INVENTORY_SCHEMA_VERSION {
            return Err(InventoryError::UnsupportedVersion(envelope.schema_version));
        }
        let bytes = serde_json::to_vec(&envelope.inventory)?;
        if digest(&bytes) != envelope.sha256 {
            return Err(InventoryError::Checksum);
        }
        envelope.inventory.validate()?;
        Ok(envelope.inventory)
    }

    pub fn age_ms(&self, now_unix_ms: u64) -> u64 {
        now_unix_ms.saturating_sub(self.snapshot.captured_unix_ms)
    }

    fn validate(&self) -> Result<(), InventoryError> {
        if self.schema_version != INVENTORY_SCHEMA_VERSION {
            return Err(InventoryError::UnsupportedVersion(self.schema_version));
        }
        if self.context.target_id != self.snapshot.target_id {
            return Err(InventoryError::Invalid(
                "Context and environment target IDs differ".into(),
            ));
        }
        if self.context.policy != ValidationPolicy::Captured
            || self.context.native_execution_allowed
        {
            return Err(InventoryError::Invalid(
                "Imported targets must be frozen and cannot authorize native execution".into(),
            ));
        }
        if self.privacy.history_included || self.privacy.environment_values_included {
            return Err(InventoryError::Invalid(
                "History and environment values are not accepted in target inventories".into(),
            ));
        }
        if !self.privacy.aliases_included && !self.snapshot.aliases.is_empty() {
            return Err(InventoryError::Invalid(
                "Alias payload does not match the privacy declaration".into(),
            ));
        }
        if !self.privacy.function_names_included && !self.snapshot.functions.is_empty() {
            return Err(InventoryError::Invalid(
                "Function payload does not match the privacy declaration".into(),
            ));
        }
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetComparison {
    pub targets: Vec<TargetColumn>,
    pub commands: Vec<CommandComparison>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetColumn {
    pub target_id: String,
    pub label: String,
    pub platform: String,
    pub captured_unix_ms: u64,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandComparison {
    pub name: Option<String>,
    pub results: Vec<CommandResolution>,
    pub validation: Vec<crate::ValidationResult>,
}

/// Compare only recorded evidence. This operation never consults the local host.
pub fn compare_targets(targets: &[TargetInventory], sites: &[CommandSite]) -> TargetComparison {
    TargetComparison {
        targets: targets
            .iter()
            .map(|target| TargetColumn {
                target_id: target.context.target_id.clone(),
                label: target.label.clone(),
                platform: target.snapshot.platform.clone(),
                captured_unix_ms: target.snapshot.captured_unix_ms,
                complete: target.snapshot.is_complete(),
            })
            .collect(),
        commands: sites
            .iter()
            .map(|site| CommandComparison {
                name: site.name.clone(),
                validation: targets.iter().map(|target| {
                    let resolution = resolve(&target.context, &target.snapshot, site);
                    let Some(command) = resolution.resolved() else { return crate::ValidationResult::Unknown("Command identity is unresolved".into()); };
                    let Some(evidence) = target.snapshot.validators.get(&command.name) else { return crate::ValidationResult::Unknown("No capability evidence was captured".into()); };
                    crate::validate_invocation(command, evidence, &target.snapshot.platform)
                }).collect(),
                results: targets
                    .iter()
                    .map(|target| resolve(&target.context, &target.snapshot, site))
                    .collect(),
            })
            .collect(),
    }
}
