use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShellDialect {
    Posix,
    #[default]
    Bash,
    Zsh,
    Fish,
    Mksh,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionMode {
    #[default]
    Script,
    StartupFile,
    InteractiveSession,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationPolicy {
    #[default]
    Workspace,
    Portable,
    Captured,
    Session,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionContext {
    pub target_id: String,
    pub workspace: Option<String>,
    pub dialect: ShellDialect,
    pub interpreter: Option<InterpreterIdentity>,
    pub mode: ExecutionMode,
    pub policy: ValidationPolicy,
    /// Launch directory, which may differ from the script's directory.
    pub cwd: Option<PathBuf>,
    /// False means relative execution paths cannot prove absence.
    pub cwd_known: bool,
    /// Set by the client capability, never promoted by a workspace setting.
    pub native_execution_allowed: bool,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            target_id: "workspace".into(),
            workspace: None,
            dialect: ShellDialect::Bash,
            interpreter: None,
            mode: ExecutionMode::Script,
            policy: ValidationPolicy::Workspace,
            cwd: None,
            cwd_known: false,
            native_execution_allowed: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterpreterIdentity {
    pub executable: PathBuf,
    pub version: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolutionSnapshotKey {
    pub document_uri: String,
    pub document_version: i32,
    pub analysis_generation: u64,
    pub target_id: String,
    pub environment_generation: u64,
    pub provider_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub source: String,
    pub location: Option<String>,
}

impl Provenance {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            location: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alias {
    /// Only literal, independently parsed words belong here. Complex aliases
    /// are represented by `opaque`, rather than executing or splitting them.
    pub words: Vec<String>,
    #[serde(default)]
    pub opaque: bool,
    pub provenance: Option<Provenance>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeclarationKind {
    #[default]
    Expected,
    Generated,
    Optional,
    Deployment,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDeclaration {
    pub kind: DeclarationKind,
    pub target_id: Option<String>,
    pub provenance: Option<Provenance>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LookupMode {
    #[default]
    Normal,
    ExternalOnly,
    BuiltinOnly,
    /// `command name` bypasses functions but still recognizes builtins.
    Command,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSite {
    /// None denotes an expanded or otherwise dynamic command name.
    pub name: Option<String>,
    pub arguments: Vec<String>,
    pub alias_eligible: bool,
    pub lookup: LookupMode,
    /// Source-ordered, scope-appropriate definitions supplied by semantics.
    pub functions: BTreeSet<String>,
    pub aliases: BTreeMap<String, Alias>,
    pub declared: BTreeMap<String, CommandDeclaration>,
    pub guarded: BTreeSet<String>,
    /// Dynamic PATH, cwd, wrappers, or source effects invalidate host inference.
    pub environment_uncertain: bool,
}

impl Default for CommandSite {
    fn default() -> Self {
        Self {
            name: None,
            arguments: Vec::new(),
            alias_eligible: true,
            lookup: LookupMode::Normal,
            functions: BTreeSet::new(),
            aliases: BTreeMap::new(),
            declared: BTreeMap::new(),
            guarded: BTreeSet::new(),
            environment_uncertain: false,
        }
    }
}

impl CommandSite {
    pub fn literal(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }
}
