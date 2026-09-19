use std::collections::BTreeMap;

use lsp_types::Url;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer};
use shucked_config::{FormatConfig, LintConfig, ShuckConfig};

use crate::session::settings::GlobalClientSettings;
use crate::{Client, logging};

pub(crate) type WorkspaceOptionsMap = FxHashMap<Url, ClientOptions>;

/// Global initialization options accepted by the Shucked LSP server.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GlobalOptions {
    #[serde(flatten)]
    client: ClientOptions,
    #[serde(default)]
    pub(crate) tracing: TracingOptions,
}

impl GlobalOptions {
    /// Resolve client-provided options into runtime global settings.
    pub fn into_settings(self, _client: Client) -> GlobalClientSettings {
        GlobalClientSettings::new(self.client)
    }
}

/// Per-client or per-workspace Shucked options supplied through LSP settings.
#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    /// Permission from initialization; later workspace settings cannot grant it.
    pub native_execution_allowed: bool,
    /// Command resolution and execution target options.
    pub environment: Option<super::environment_options::EnvironmentOptions>,
    /// Shared per-file shell dialect overrides.
    pub per_file_shell: Option<BTreeMap<String, String>>,
    /// Lint configuration overrides.
    pub lint: Option<LintConfig>,
    /// Format configuration overrides.
    pub format: Option<FormatConfig>,
    /// Whether source-level fix-all actions are enabled.
    pub fix_all: Option<bool>,
    /// Whether unsafe fixes may be offered.
    pub unsafe_fixes: Option<bool>,
    /// Whether parser diagnostics should be shown.
    pub show_syntax_errors: Option<bool>,
    /// Code action options.
    pub code_action: Option<CodeActionOptions>,
    /// Server-only editor feature options.
    pub server: ServerOptions,
}

fn deserialize_bool_or_enable<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolOrEnable {
        Bool(bool),
        Enable {
            #[serde(alias = "enabled")]
            enable: bool,
        },
        Enabled {
            enabled: bool,
        },
    }

    Ok(
        Option::<BoolOrEnable>::deserialize(deserializer)?.map(|value| match value {
            BoolOrEnable::Bool(b)
            | BoolOrEnable::Enable { enable: b }
            | BoolOrEnable::Enabled { enabled: b } => b,
        }),
    )
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLintOptions {
    #[serde(default, deserialize_with = "deserialize_bool_or_enable")]
    show_syntax_errors: Option<bool>,
    #[serde(default)]
    #[allow(dead_code)]
    enable: Option<bool>,
    #[serde(flatten)]
    config: LintConfig,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawFormatOptions {
    #[serde(default)]
    #[allow(dead_code)]
    enable: Option<bool>,
    #[serde(flatten)]
    config: FormatConfig,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawClientOptions {
    #[serde(default)]
    native_execution_allowed: bool,
    #[serde(default)]
    environment: Option<super::environment_options::EnvironmentOptions>,
    #[serde(default)]
    per_file_shell: Option<BTreeMap<String, String>>,
    #[serde(default)]
    lint: Option<RawLintOptions>,
    #[serde(default)]
    format: Option<RawFormatOptions>,
    #[serde(default, deserialize_with = "deserialize_bool_or_enable")]
    fix_all: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_bool_or_enable")]
    unsafe_fixes: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_bool_or_enable")]
    show_syntax_errors: Option<bool>,
    #[serde(default)]
    code_action: Option<CodeActionOptions>,
    #[serde(default)]
    server: ServerOptions,
}

impl<'de> Deserialize<'de> for ClientOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawClientOptions::deserialize(deserializer)?;
        let show_syntax_errors = raw
            .show_syntax_errors
            .or_else(|| raw.lint.as_ref().and_then(|l| l.show_syntax_errors));
        let lint = raw.lint.and_then(|l| {
            if l.config != LintConfig::default() {
                Some(l.config)
            } else {
                None
            }
        });
        let format = raw.format.and_then(|f| {
            if f.config != FormatConfig::default() {
                Some(f.config)
            } else {
                None
            }
        });

        Ok(Self {
            native_execution_allowed: raw.native_execution_allowed,
            environment: raw.environment,
            per_file_shell: raw.per_file_shell,
            lint,
            format,
            fix_all: raw.fix_all,
            unsafe_fixes: raw.unsafe_fixes,
            show_syntax_errors,
            code_action: raw.code_action,
            server: raw.server,
        })
    }
}

/// Options for code actions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionOptions {
    #[serde(default)]
    /// Options for suppression comment actions.
    pub disable_rule_comment: Option<DisableRuleCommentOptions>,
}

/// Options for suppression comment actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisableRuleCommentOptions {
    /// Whether suppression comment actions are enabled.
    pub enable: Option<bool>,
}

impl<'de> Deserialize<'de> for DisableRuleCommentOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Bool(bool),
            Object {
                #[serde(default, alias = "enabled")]
                enable: Option<bool>,
            },
        }

        let helper = Option::<Helper>::deserialize(deserializer)?;
        let enable = helper.and_then(|h| match h {
            Helper::Bool(b) => Some(b),
            Helper::Object { enable } => enable,
        });
        Ok(Self { enable })
    }
}

impl ClientOptions {
    pub(crate) fn to_config_overrides(&self) -> ShuckConfig {
        ShuckConfig {
            per_file_shell: self.per_file_shell.clone(),
            lint: self.lint.clone().unwrap_or_default(),
            format: self.format.clone().unwrap_or_default(),
            ..ShuckConfig::default()
        }
    }
}

/// Options for server-only editor features.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerOptions {
    /// Workspace-wide symbol search configuration.
    pub workspace_symbols: WorkspaceSymbolFeatureOptions,
    /// Completion configuration.
    pub completion: CompletionFeatureOptions,
    /// Rename configuration.
    pub rename: RenameFeatureOptions,
    /// Cross-file call hierarchy configuration.
    pub call_hierarchy: CallHierarchyFeatureOptions,
    /// Workspace-wide diagnostic pull configuration.
    pub workspace_diagnostics: WorkspaceDiagnosticsFeatureOptions,
    workspace_symbols_overrides: WorkspaceSymbolFeatureOptionsOverrides,
    completion_overrides: CompletionFeatureOptionsOverrides,
    rename_overrides: RenameFeatureOptionsOverrides,
    call_hierarchy_overrides: CallHierarchyFeatureOptionsOverrides,
    workspace_diagnostics_overrides: WorkspaceDiagnosticsFeatureOptionsOverrides,
}

impl ServerOptions {
    pub(crate) fn workspace_symbols_layered_over(
        &self,
        base: WorkspaceSymbolFeatureOptions,
    ) -> WorkspaceSymbolFeatureOptions {
        if self.workspace_symbols_overrides.has_overrides() {
            self.workspace_symbols_overrides.apply_to(base)
        } else if self.workspace_symbols != WorkspaceSymbolFeatureOptions::default() {
            self.workspace_symbols
        } else {
            base
        }
    }

    pub(crate) fn completion_layered_over(
        &self,
        base: CompletionFeatureOptions,
    ) -> CompletionFeatureOptions {
        if self.completion_overrides.has_overrides() {
            self.completion_overrides.apply_to(base)
        } else if self.completion != CompletionFeatureOptions::default() {
            self.completion
        } else {
            base
        }
    }

    pub(crate) fn rename_layered_over(&self, base: RenameFeatureOptions) -> RenameFeatureOptions {
        if self.rename_overrides.has_overrides() {
            self.rename_overrides.apply_to(base)
        } else if self.rename != RenameFeatureOptions::default() {
            self.rename
        } else {
            base
        }
    }

    #[allow(dead_code)]
    pub(crate) fn call_hierarchy_layered_over(
        &self,
        base: CallHierarchyFeatureOptions,
    ) -> CallHierarchyFeatureOptions {
        if self.call_hierarchy_overrides.has_overrides() {
            self.call_hierarchy_overrides.apply_to(base)
        } else if self.call_hierarchy != CallHierarchyFeatureOptions::default() {
            self.call_hierarchy
        } else {
            base
        }
    }

    pub(crate) fn workspace_diagnostics_layered_over(
        &self,
        base: WorkspaceDiagnosticsFeatureOptions,
    ) -> WorkspaceDiagnosticsFeatureOptions {
        if self.workspace_diagnostics_overrides.has_overrides() {
            self.workspace_diagnostics_overrides.apply_to(base)
        } else if self.workspace_diagnostics != WorkspaceDiagnosticsFeatureOptions::default() {
            self.workspace_diagnostics
        } else {
            base
        }
    }
}

impl<'de> Deserialize<'de> for ServerOptions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct RawServerOptions {
            #[serde(default)]
            workspace_symbols: WorkspaceSymbolFeatureOptionsOverrides,
            #[serde(default)]
            completion: CompletionFeatureOptionsOverrides,
            #[serde(default)]
            rename: RenameFeatureOptionsOverrides,
            #[serde(default)]
            call_hierarchy: CallHierarchyFeatureOptionsOverrides,
            #[serde(default)]
            workspace_diagnostics: WorkspaceDiagnosticsFeatureOptionsOverrides,
        }

        let raw = RawServerOptions::deserialize(deserializer)?;
        Ok(Self {
            workspace_symbols: raw
                .workspace_symbols
                .apply_to(WorkspaceSymbolFeatureOptions::default()),
            completion: raw.completion.apply_to(CompletionFeatureOptions::default()),
            rename: raw.rename.apply_to(RenameFeatureOptions::default()),
            call_hierarchy: raw
                .call_hierarchy
                .apply_to(CallHierarchyFeatureOptions::default()),
            workspace_diagnostics: raw
                .workspace_diagnostics
                .apply_to(WorkspaceDiagnosticsFeatureOptions::default()),
            workspace_symbols_overrides: raw.workspace_symbols,
            completion_overrides: raw.completion,
            rename_overrides: raw.rename,
            call_hierarchy_overrides: raw.call_hierarchy,
            workspace_diagnostics_overrides: raw.workspace_diagnostics,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WorkspaceSymbolFeatureOptionsOverrides {
    enabled: Option<bool>,
    max_files: Option<usize>,
}

impl<'de> Deserialize<'de> for WorkspaceSymbolFeatureOptionsOverrides {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct RawOverrides {
            #[serde(default, alias = "enable")]
            enabled: Option<bool>,
            #[serde(default)]
            max_files: Option<usize>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Bool(bool),
            Object(RawOverrides),
        }

        let helper = Option::<Helper>::deserialize(deserializer)?;
        Ok(match helper {
            Some(Helper::Bool(b)) => Self {
                enabled: Some(b),
                ..Self::default()
            },
            Some(Helper::Object(raw)) => Self {
                enabled: raw.enabled,
                max_files: raw.max_files,
            },
            None => Self::default(),
        })
    }
}

impl WorkspaceSymbolFeatureOptionsOverrides {
    fn has_overrides(self) -> bool {
        self.enabled.is_some() || self.max_files.is_some()
    }

    fn apply_to(self, base: WorkspaceSymbolFeatureOptions) -> WorkspaceSymbolFeatureOptions {
        WorkspaceSymbolFeatureOptions {
            enabled: self.enabled.unwrap_or(base.enabled),
            max_files: self.max_files.unwrap_or(base.max_files),
        }
    }
}

/// Configuration for `workspace/symbol`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbolFeatureOptions {
    /// Whether the workspace symbol index should serve requests.
    #[serde(default = "default_workspace_symbols_enabled")]
    pub enabled: bool,
    /// Maximum number of closed workspace files to index.
    #[serde(default = "default_workspace_symbols_max_files")]
    pub max_files: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CompletionFeatureOptionsOverrides {
    #[serde(default)]
    include_runtime_names: Option<bool>,
    #[serde(default)]
    include_keywords: Option<bool>,
    include_environment: Option<bool>,
    include_paths: Option<bool>,
    include_command_arguments: Option<bool>,
    include_native: Option<bool>,
    use_shell_config: Option<bool>,
    max_items: Option<usize>,
}

impl CompletionFeatureOptionsOverrides {
    fn has_overrides(self) -> bool {
        self.include_runtime_names.is_some()
            || self.include_keywords.is_some()
            || self.include_environment.is_some()
            || self.include_paths.is_some()
            || self.include_command_arguments.is_some()
            || self.include_native.is_some()
            || self.use_shell_config.is_some()
            || self.max_items.is_some()
    }

    fn apply_to(self, base: CompletionFeatureOptions) -> CompletionFeatureOptions {
        CompletionFeatureOptions {
            include_runtime_names: self
                .include_runtime_names
                .unwrap_or(base.include_runtime_names),
            include_keywords: self.include_keywords.unwrap_or(base.include_keywords),
            include_environment: self.include_environment.unwrap_or(base.include_environment),
            include_paths: self.include_paths.unwrap_or(base.include_paths),
            include_command_arguments: self
                .include_command_arguments
                .unwrap_or(base.include_command_arguments),
            include_native: self.include_native.unwrap_or(base.include_native),
            use_shell_config: self.use_shell_config.unwrap_or(base.use_shell_config),
            max_items: self.max_items.unwrap_or(base.max_items),
        }
    }
}

/// Configuration for `textDocument/completion`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionFeatureOptions {
    /// Include runtime-provided parameter names.
    #[serde(default = "default_completion_include_runtime_names")]
    pub include_runtime_names: bool,
    /// Include shell keywords in command-position completion.
    #[serde(default = "default_completion_include_keywords")]
    pub include_keywords: bool,
    /// Include executable and variable names from the server host.
    #[serde(default = "default_completion_include_keywords")]
    pub include_environment: bool,
    /// Complete paths relative to the document directory.
    #[serde(default = "default_completion_include_keywords")]
    pub include_paths: bool,
    /// Include known command flags and subcommands.
    #[serde(default = "default_completion_include_keywords")]
    pub include_command_arguments: bool,
    /// Query native tools in trusted workspaces.
    #[serde(default = "default_completion_include_keywords")]
    pub include_native: bool,
    /// Opt in to personal Zsh aliases, completion functions, and startup files.
    #[serde(default)]
    pub use_shell_config: bool,
    /// Maximum candidates in a response; clamped to 1..=2000.
    #[serde(default = "default_completion_max_items")]
    pub max_items: usize,
}

impl Default for CompletionFeatureOptions {
    fn default() -> Self {
        Self {
            include_runtime_names: true,
            include_keywords: true,
            include_environment: true,
            include_paths: true,
            include_command_arguments: true,
            include_native: true,
            use_shell_config: false,
            max_items: default_completion_max_items(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RenameFeatureOptionsOverrides {
    #[serde(default)]
    allow_cross_file: Option<bool>,
}

impl RenameFeatureOptionsOverrides {
    fn has_overrides(self) -> bool {
        self.allow_cross_file.is_some()
    }

    fn apply_to(self, base: RenameFeatureOptions) -> RenameFeatureOptions {
        RenameFeatureOptions {
            allow_cross_file: self.allow_cross_file.unwrap_or(base.allow_cross_file),
        }
    }
}

/// Configuration for rename requests.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RenameFeatureOptions {
    /// Allow rename edits outside the current document.
    #[serde(default = "default_cross_file_rename_enabled")]
    pub allow_cross_file: bool,
}

impl Default for RenameFeatureOptions {
    fn default() -> Self {
        Self {
            allow_cross_file: default_cross_file_rename_enabled(),
        }
    }
}

fn default_cross_file_rename_enabled() -> bool {
    true
}

impl Default for WorkspaceSymbolFeatureOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_files: 5000,
        }
    }
}

fn default_workspace_symbols_enabled() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CallHierarchyFeatureOptionsOverrides {
    #[serde(default)]
    max_files: Option<usize>,
}

#[allow(dead_code)]
impl CallHierarchyFeatureOptionsOverrides {
    fn has_overrides(self) -> bool {
        self.max_files.is_some()
    }

    fn apply_to(self, base: CallHierarchyFeatureOptions) -> CallHierarchyFeatureOptions {
        CallHierarchyFeatureOptions {
            max_files: self.max_files.unwrap_or(base.max_files),
        }
    }
}

/// Configuration for cross-file call hierarchy.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyFeatureOptions {
    /// Maximum number of workspace files to index for the call graph.
    #[serde(default = "default_call_hierarchy_max_files")]
    pub max_files: usize,
}

impl Default for CallHierarchyFeatureOptions {
    fn default() -> Self {
        Self {
            max_files: default_call_hierarchy_max_files(),
        }
    }
}

fn default_call_hierarchy_max_files() -> usize {
    10_000
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct WorkspaceDiagnosticsFeatureOptionsOverrides {
    enabled: Option<bool>,
    max_files: Option<usize>,
    max_entries: Option<usize>,
    max_source_bytes: Option<usize>,
}

impl<'de> Deserialize<'de> for WorkspaceDiagnosticsFeatureOptionsOverrides {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct RawOverrides {
            #[serde(default, alias = "enable")]
            enabled: Option<bool>,
            #[serde(default)]
            max_files: Option<usize>,
            #[serde(default)]
            max_entries: Option<usize>,
            #[serde(default)]
            max_source_bytes: Option<usize>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Bool(bool),
            Object(RawOverrides),
        }

        let helper = Option::<Helper>::deserialize(deserializer)?;
        Ok(match helper {
            Some(Helper::Bool(b)) => Self {
                enabled: Some(b),
                ..Self::default()
            },
            Some(Helper::Object(raw)) => Self {
                enabled: raw.enabled,
                max_files: raw.max_files,
                max_entries: raw.max_entries,
                max_source_bytes: raw.max_source_bytes,
            },
            None => Self::default(),
        })
    }
}

impl WorkspaceDiagnosticsFeatureOptionsOverrides {
    fn has_overrides(self) -> bool {
        self.enabled.is_some()
            || self.max_files.is_some()
            || self.max_entries.is_some()
            || self.max_source_bytes.is_some()
    }

    fn apply_to(
        self,
        base: WorkspaceDiagnosticsFeatureOptions,
    ) -> WorkspaceDiagnosticsFeatureOptions {
        WorkspaceDiagnosticsFeatureOptions {
            enabled: self.enabled.unwrap_or(base.enabled),
            max_files: self.max_files.unwrap_or(base.max_files),
            max_entries: self.max_entries.unwrap_or(base.max_entries),
            max_source_bytes: self.max_source_bytes.unwrap_or(base.max_source_bytes),
        }
    }
}

/// Configuration for bounded `workspace/diagnostic` requests.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDiagnosticsFeatureOptions {
    /// Whether workspace diagnostic requests are advertised and served.
    #[serde(default = "default_workspace_diagnostics_enabled")]
    pub enabled: bool,
    /// Maximum number of workspace files returned by one request.
    #[serde(default = "default_workspace_diagnostics_max_files")]
    pub max_files: usize,
    /// Maximum number of filesystem entries visited by one request.
    #[serde(default = "default_workspace_diagnostics_max_entries")]
    pub max_entries: usize,
    /// Maximum source bytes read and analyzed by one request.
    #[serde(default = "default_workspace_diagnostics_max_source_bytes")]
    pub max_source_bytes: usize,
}

impl Default for WorkspaceDiagnosticsFeatureOptions {
    fn default() -> Self {
        Self {
            enabled: default_workspace_diagnostics_enabled(),
            max_files: default_workspace_diagnostics_max_files(),
            max_entries: default_workspace_diagnostics_max_entries(),
            max_source_bytes: default_workspace_diagnostics_max_source_bytes(),
        }
    }
}

fn default_workspace_diagnostics_enabled() -> bool {
    true
}

fn default_workspace_diagnostics_max_files() -> usize {
    1_000
}

fn default_workspace_diagnostics_max_entries() -> usize {
    10_000
}

fn default_workspace_diagnostics_max_source_bytes() -> usize {
    32 * 1024 * 1024
}

fn default_workspace_symbols_max_files() -> usize {
    5000
}

fn default_completion_max_items() -> usize {
    200
}

fn default_completion_include_runtime_names() -> bool {
    true
}

fn default_completion_include_keywords() -> bool {
    true
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TracingOptions {
    pub(crate) log_file: Option<std::path::PathBuf>,
    pub(crate) log_level: Option<logging::LogLevel>,
}

#[derive(Debug, Default)]
pub(crate) struct AllOptions {
    pub(crate) global: GlobalOptions,
    pub(crate) workspace: Option<WorkspaceOptionsMap>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct InitializationOptions {
    #[serde(default)]
    shucked: Option<GlobalOptions>,
    #[serde(default)]
    workspace: Option<WorkspaceOptionsMap>,
}

impl AllOptions {
    pub(crate) fn from_value(mut value: serde_json::Value) -> Self {
        if let Some(settings) = value.as_object_mut().and_then(|obj| obj.remove("settings")) {
            value = settings;
        }

        if value
            .as_object()
            .is_some_and(|object| object.contains_key("shucked"))
        {
            let options =
                serde_json::from_value::<InitializationOptions>(value).unwrap_or_default();
            return Self {
                global: options.shucked.unwrap_or_default(),
                workspace: options.workspace,
            };
        }

        let global = serde_json::from_value::<GlobalOptions>(value).unwrap_or_default();
        Self {
            global,
            workspace: None,
        }
    }

    pub(crate) fn workspace_diagnostics_enabled(&self) -> bool {
        let global = self.global.client.server.workspace_diagnostics;
        global.enabled
            || self.workspace.as_ref().is_some_and(|workspaces| {
                workspaces.values().any(|options| {
                    options
                        .server
                        .workspace_diagnostics_layered_over(global)
                        .enabled
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::AllOptions;

    #[test]
    fn vscode_settings_format_deserializes_correctly() {
        let options = AllOptions::from_value(serde_json::json!({
            "settings": {
                "shucked": {
                    "fixAll": { "enable": true },
                    "unsafeFixes": { "enable": true },
                    "lint": { "enable": true, "showSyntaxErrors": true },
                    "format": { "enable": true },
                    "codeAction": {
                        "disableRuleComment": { "enable": false }
                    },
                    "server": {
                        "workspaceDiagnostics": { "enable": false },
                        "workspaceSymbols": { "enable": false }
                    }
                }
            }
        }));

        assert_eq!(options.global.client.unsafe_fixes, Some(true));
        assert_eq!(options.global.client.fix_all, Some(true));
        assert_eq!(options.global.client.show_syntax_errors, Some(true));
        assert_eq!(
            options
                .global
                .client
                .code_action
                .as_ref()
                .and_then(|ca| ca.disable_rule_comment)
                .and_then(|drc| drc.enable),
            Some(false)
        );
        assert!(!options.workspace_diagnostics_enabled());
        assert!(!options.global.client.server.workspace_symbols.enabled);
    }

    #[test]
    fn flat_and_nested_boolean_options_deserialize_correctly() {
        let options = AllOptions::from_value(serde_json::json!({
            "shucked": {
                "unsafeFixes": true,
                "fixAll": false,
                "showSyntaxErrors": true,
                "codeAction": {
                    "disableRuleComment": true
                },
                "server": {
                    "workspaceDiagnostics": false,
                    "workspaceSymbols": true
                }
            }
        }));

        assert_eq!(options.global.client.unsafe_fixes, Some(true));
        assert_eq!(options.global.client.fix_all, Some(false));
        assert_eq!(options.global.client.show_syntax_errors, Some(true));
        assert_eq!(
            options
                .global
                .client
                .code_action
                .as_ref()
                .and_then(|ca| ca.disable_rule_comment)
                .and_then(|drc| drc.enable),
            Some(true)
        );
        assert!(!options.workspace_diagnostics_enabled());
        assert!(options.global.client.server.workspace_symbols.enabled);
    }

    #[test]
    fn workspace_diagnostics_are_enabled_by_default_and_can_be_disabled() {
        let defaults = AllOptions::from_value(serde_json::json!({}));
        assert!(defaults.workspace_diagnostics_enabled());

        let disabled = AllOptions::from_value(serde_json::json!({
            "shucked": {
                "server": {
                    "workspaceDiagnostics": {
                        "enabled": false
                    }
                }
            }
        }));
        assert!(!disabled.workspace_diagnostics_enabled());
    }

    #[test]
    fn unrelated_workspace_options_preserve_the_global_diagnostic_opt_out() {
        let disabled = AllOptions::from_value(serde_json::json!({
            "shucked": {
                "server": {
                    "workspaceDiagnostics": {
                        "enabled": false
                    }
                }
            },
            "workspace": {
                "file:///workspace": {
                    "showSyntaxErrors": true
                }
            }
        }));
        assert!(!disabled.workspace_diagnostics_enabled());

        let enabled_for_workspace = AllOptions::from_value(serde_json::json!({
            "shucked": {
                "server": {
                    "workspaceDiagnostics": {
                        "enabled": false
                    }
                }
            },
            "workspace": {
                "file:///workspace": {
                    "server": {
                        "workspaceDiagnostics": {
                            "enabled": true
                        }
                    }
                }
            }
        }));
        assert!(enabled_for_workspace.workspace_diagnostics_enabled());
    }
}
