//! Shared command resolution for every editor surface.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::session::DocumentSnapshot;
use lsp_types as types;
use shucked_command::{
    CommandResolution, CommandSite, EnvironmentSnapshot, ExecutionContext, ShellDialect,
    ValidationPolicy,
};
use shucked_semantic::{CommandNamespace, CommandSiteFacts};

pub(crate) struct CommandService {
    generation: AtomicU64,
    native_allowed: bool,
    path: Option<Vec<PathBuf>>,
    cache: Mutex<VecDeque<(String, Instant, Arc<CommandAnalysis>)>>,
    sessions: Mutex<BTreeMap<String, ShellSessionState>>,
    completed_diagnostics: Mutex<VecDeque<(String, Vec<types::Diagnostic>)>>,
    host_snapshots: Mutex<VecDeque<CachedHostSnapshot>>,
}

struct CachedHostSnapshot {
    context: ExecutionContext,
    paths: Vec<PathBuf>,
    generation: u64,
    created: Instant,
    entries: usize,
    environment: EnvironmentSnapshot,
}

pub(crate) struct CommandAnalysis {
    pub context: ExecutionContext,
    pub environment: EnvironmentSnapshot,
    pub sites: Vec<(CommandSiteFacts, CommandResolution)>,
    pub failure: Option<String>,
    validation: Mutex<Option<Arc<Vec<super::commands_validation::ValidationDiagnostic>>>>,
    pub local_environment: bool,
}

impl CommandService {
    pub(crate) fn watch_directories(&self, launch_directories: &[PathBuf]) -> Vec<PathBuf> {
        let cwd = std::env::current_dir().unwrap_or_default();
        let mut paths = self
            .path
            .iter()
            .flatten()
            .map(|path| cwd.join(path))
            .collect::<Vec<_>>();
        for cwd in launch_directories {
            paths.extend(self.path.iter().flatten().map(|path| cwd.join(path)));
        }
        for cached in self
            .host_snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
        {
            if let Some(cwd) = &cached.context.cwd {
                paths.extend(cached.paths.iter().map(|path| cwd.join(path)));
            }
        }
        for state in self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|state| state.connected)
        {
            paths.extend(state.path.iter().map(|path| state.cwd.join(path)));
        }
        paths
    }
    pub fn new(native_allowed: bool) -> Self {
        Self {
            generation: AtomicU64::new(0),
            native_allowed,
            path: std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect()),
            cache: Mutex::default(),
            sessions: Mutex::default(),
            completed_diagnostics: Mutex::default(),
            host_snapshots: Mutex::default(),
        }
    }
    #[cfg(test)]
    pub(crate) fn fixture(paths: Vec<PathBuf>) -> Self {
        let mut service = Self::new(false);
        service.path = Some(paths);
        service
    }
    pub fn update_session(&self, state: ShellSessionState) {
        if !self.native_allowed
            || state.id.len() > 256
            || state.path.len() > 1024
            || state.aliases.len() > 10000
            || state.functions.len() > 10000
        {
            return;
        }
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions
            .get(&state.id)
            .is_some_and(|previous| previous.generation >= state.generation)
        {
            return;
        }
        if sessions.len() >= 64 && !sessions.contains_key(&state.id) {
            if let Some(retired) = sessions
                .iter()
                .find(|(_, session)| !session.connected)
                .map(|(id, _)| id.clone())
            {
                sessions.remove(&retired);
            } else {
                return;
            }
        }
        sessions.insert(state.id.clone(), state);
        drop(sessions);
        self.invalidate();
    }
    pub(crate) fn session(&self, id: &str) -> Option<ShellSessionState> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.completed_diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.host_snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn capture_host(
        &self,
        context: &ExecutionContext,
        paths: &[PathBuf],
        snapshot: &DocumentSnapshot,
    ) -> EnvironmentSnapshot {
        const TTL: Duration = Duration::from_secs(2);
        let generation = snapshot.environment_generation();
        {
            let cache = self
                .host_snapshots
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = cache.iter().find(|entry| {
                entry.generation == generation
                    && entry.context == *context
                    && entry.paths == paths
                    && entry.created.elapsed() < TTL
            }) {
                return entry.environment.clone();
            }
        }
        let environment = shucked_command::host::capture(context, paths.to_vec(), generation);
        let entries = environment
            .search_path
            .iter()
            .map(|directory| directory.commands.len())
            .sum::<usize>();
        let mut cache = self
            .host_snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation != self.generation()
            || snapshot.analysis_cancellation().is_cancelled()
            || entries > 100_000
        {
            return environment;
        }
        cache.retain(|entry| {
            entry.created.elapsed() < TTL
                && !(entry.generation == generation
                    && entry.context == *context
                    && entry.paths == paths)
        });
        cache.push_back(CachedHostSnapshot {
            context: context.clone(),
            paths: paths.to_vec(),
            generation,
            created: Instant::now(),
            entries,
            environment: environment.clone(),
        });
        while cache.len() > 16 || cache.iter().map(|entry| entry.entries).sum::<usize>() > 100_000 {
            cache.pop_front();
        }
        environment
    }
    pub fn analysis(&self, snapshot: &DocumentSnapshot) -> Arc<CommandAnalysis> {
        let key = analysis_key(snapshot);
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((_, _, result)) = cache
                .iter()
                .find(|(k, created, _)| k == &key && created.elapsed() < Duration::from_secs(30))
            {
                return result.clone();
            }
        }
        let session = snapshot
            .client_settings()
            .environment()
            .session_id
            .as_ref()
            .and_then(|id| {
                self.sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(id)
                    .cloned()
            });
        let result = Arc::new(build(
            snapshot,
            self.native_allowed,
            session,
            self.path.as_deref(),
        ));
        if snapshot.analysis_cancellation().is_cancelled()
            || self.generation() != snapshot.environment_generation()
        {
            return result;
        }
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Another request can finish the same immutable analysis while this one builds.
        // Reuse its instance so diagnostics and pull requests share validation state.
        if let Some((_, _, result)) = cache
            .iter()
            .find(|(k, created, _)| k == &key && created.elapsed() < Duration::from_secs(30))
        {
            return result.clone();
        }
        cache.retain(|(k, created, _)| k != &key && created.elapsed() < Duration::from_secs(30));
        cache.push_back((key, Instant::now(), result.clone()));
        while cache.len() > 64 {
            cache.pop_front();
        }
        result
    }
}

fn analysis_key(snapshot: &DocumentSnapshot) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    snapshot.query().document().contents().hash(&mut hash);
    format!(
        "{}:{}:{}:{}:{}",
        snapshot.query().file_url(),
        snapshot.query().document().version(),
        snapshot.analysis_settings_epoch(),
        snapshot.environment_generation(),
        hash.finish()
    )
}

pub(crate) fn dialect(snapshot: &DocumentSnapshot) -> &'static str {
    let doc = snapshot.query().document();
    if doc.language_id() == Some(crate::edit::LanguageId::Fish)
        || snapshot
            .query()
            .file_path()
            .is_some_and(|p| p.extension().is_some_and(|e| e == "fish"))
        || doc.contents().lines().next().is_some_and(|line| {
            line.starts_with("#!")
                && shucked_parser::shebang::interpreter_name(line) == Some("fish")
        })
    {
        return "fish";
    }
    let shell = crate::lint::infer_document_shell_from_parts(
        snapshot.shuck_settings(),
        doc.language_id(),
        doc.contents(),
        snapshot.query().file_path().as_deref(),
    );
    match shell {
        Some(shucked_linter::ShellDialect::Zsh) => "zsh",
        Some(shucked_linter::ShellDialect::Sh) => "sh",
        Some(shucked_linter::ShellDialect::Ksh) => "ksh",
        _ => "bash",
    }
}

pub(crate) fn cwd(snapshot: &DocumentSnapshot) -> PathBuf {
    snapshot
        .client_settings()
        .environment()
        .session_id
        .as_ref()
        .and_then(|id| snapshot.command_service.session(id))
        .map(|state| state.cwd)
        .or_else(|| snapshot.client_settings().environment().cwd.clone())
        .or_else(|| snapshot.workspace_cwd.clone())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default()
}

fn build(
    snapshot: &DocumentSnapshot,
    native_allowed: bool,
    session: Option<ShellSessionState>,
    path: Option<&[PathBuf]>,
) -> CommandAnalysis {
    let options = snapshot.client_settings().environment();
    let mut context = ExecutionContext {
        dialect: match dialect(snapshot) {
            "fish" => ShellDialect::Fish,
            "zsh" => ShellDialect::Zsh,
            "sh" => ShellDialect::Posix,
            "ksh" => ShellDialect::Mksh,
            _ => ShellDialect::Bash,
        },
        workspace: snapshot
            .workspace_cwd
            .as_ref()
            .map(|p| p.display().to_string()),
        cwd: Some(cwd(snapshot)),
        cwd_known: options.cwd.as_ref().is_some_and(|p| p.is_absolute()),
        policy: if options.policy.as_deref() == Some("portable") {
            ValidationPolicy::Portable
        } else {
            ValidationPolicy::Workspace
        },
        native_execution_allowed: native_allowed,
        ..ExecutionContext::default()
    };
    let startup = snapshot
        .query()
        .file_path()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .is_some_and(|name| {
            matches!(
                name.as_str(),
                ".zshrc"
                    | ".zprofile"
                    | ".zshenv"
                    | ".bashrc"
                    | ".bash_profile"
                    | ".profile"
                    | "config.fish"
            )
        });
    if startup {
        context.mode = shucked_command::ExecutionMode::StartupFile;
    }
    if let Some(session) = &session {
        context.target_id = session.id.clone();
        context.policy = ValidationPolicy::Session;
        context.cwd = Some(session.cwd.clone());
        context.cwd_known = true;
        if !startup {
            context.mode = shucked_command::ExecutionMode::InteractiveSession;
        }
    }
    let mut failure = None;
    let mut environment = if let Some(path) = &options.target_inventory {
        let mut file_options = std::fs::OpenOptions::new();
        file_options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file_options.custom_flags(libc::O_NONBLOCK);
        }
        let inventory = file_options
            .open(path)
            .map_err(|e| e.to_string())
            .and_then(|file| {
                use std::io::Read;
                let metadata = file.metadata().map_err(|e| e.to_string())?;
                if !metadata.is_file() {
                    return Err("Target inventory must be a regular file".into());
                }
                if metadata.len() > shucked_command::MAX_INVENTORY_BYTES as u64 {
                    return Err("Target inventory exceeds the size limit".into());
                }
                let mut data = String::new();
                file.take((shucked_command::MAX_INVENTORY_BYTES + 1) as u64)
                    .read_to_string(&mut data)
                    .map_err(|e| e.to_string())?;
                shucked_command::TargetInventory::from_json(&data).map_err(|e| e.to_string())
            });
        match inventory {
            Ok(mut target) => {
                let dialect = context.dialect;
                let mode = context.mode;
                let portable = context.policy == ValidationPolicy::Portable;
                if target.context.dialect != dialect {
                    target.snapshot.builtins.clear();
                    target.snapshot.builtins_complete = false;
                    target.context.interpreter = None;
                }
                context = target.context;
                context.dialect = dialect;
                context.mode = mode;
                context.native_execution_allowed = false;
                context.policy = if portable {
                    ValidationPolicy::Portable
                } else {
                    ValidationPolicy::Captured
                };
                target.snapshot
            }
            Err(error) => {
                failure = Some(format!("Target inventory unavailable: {error}"));
                context.policy = ValidationPolicy::Captured;
                EnvironmentSnapshot::empty(&context)
            }
        }
    } else if let Some(session) = &session {
        let mut captured = if startup {
            context.cwd = None;
            context.cwd_known = false;
            context.native_execution_allowed = false;
            failure = Some(
                "Attached session state was captured after startup; entry environment is unknown"
                    .into(),
            );
            EnvironmentSnapshot::empty(&context)
        } else {
            snapshot
                .command_service
                .capture_host(&context, &session.path, snapshot)
        };
        captured.fresh = session.connected;
        captured.functions = session.functions.clone();
        let aliases_enabled = session
            .options
            .get("aliases")
            .is_none_or(|value| value != "off")
            && session
                .options
                .get("expand_aliases")
                .is_none_or(|value| value != "0");
        captured.aliases = session
            .aliases
            .iter()
            .filter(|_| aliases_enabled)
            .map(|(name, words)| {
                (
                    name.clone(),
                    shucked_command::Alias {
                        words: words.clone(),
                        opaque: words.is_empty(),
                        provenance: Some(shucked_command::Provenance::new("attached terminal")),
                    },
                )
            })
            .collect();
        captured
    } else if options.session_id.is_some() {
        failure = Some("Terminal session is unavailable".into());
        context.policy = ValidationPolicy::Session;
        EnvironmentSnapshot::empty(&context)
    } else {
        path.map(|paths| {
            snapshot
                .command_service
                .capture_host(&context, paths, snapshot)
        })
        .unwrap_or_else(|| EnvironmentSnapshot::empty(&context))
    };
    if options.policy.as_deref() == Some("portable") {
        context.policy = ValidationPolicy::Portable;
    }
    let fish = (dialect(snapshot) == "fish")
        .then(|| shucked_semantic::analyze_fish(snapshot.query().document().contents()));
    let fish_functions: BTreeSet<_> = fish
        .as_ref()
        .map(|f| {
            f.function_calls
                .iter()
                .map(|(span, _)| span.start.offset())
                .collect()
        })
        .unwrap_or_default();
    let facts = fish.map(|f| f.commands).unwrap_or_else(|| {
        snapshot
            .analysis()
            .map(|a| a.semantic().command_site_facts())
            .unwrap_or_default()
    });
    if matches!(
        context.policy,
        ValidationPolicy::Workspace | ValidationPolicy::Session
    ) && environment.fresh
        && !(startup && session.is_some())
    {
        let mut names: Vec<String> = facts
            .iter()
            .filter_map(|f| f.name().map(str::to_owned))
            .collect();
        let mut visited: BTreeSet<String> = names.iter().cloned().collect();
        // Simple live aliases may point at explicit executable paths absent from PATH listings.
        for name in names.clone() {
            let mut current = name;
            for _ in 0..32 {
                let Some(target) = environment
                    .aliases
                    .get(&current)
                    .filter(|alias| !alias.opaque)
                    .and_then(|alias| alias.words.first())
                else {
                    break;
                };
                if !visited.insert(target.clone()) {
                    break;
                }
                names.push(target.clone());
                current = target.clone();
            }
        }
        shucked_command::host::refresh_exact(&context, &mut environment, &names);
    }
    let mut declared: BTreeMap<_, _> = options
        .declarations
        .iter()
        .map(|(name, kind)| {
            (
                name.clone(),
                shucked_command::CommandDeclaration {
                    kind: match kind.as_str() {
                        "optional" => shucked_command::DeclarationKind::Optional,
                        "generated" => shucked_command::DeclarationKind::Generated,
                        "deployment" => shucked_command::DeclarationKind::Deployment,
                        _ => shucked_command::DeclarationKind::Expected,
                    },
                    ..Default::default()
                },
            )
        })
        .collect();
    for (name, declaration) in snapshot.shuck_settings().command_declarations() {
        if declared.contains_key(name)
            || (!declaration.targets.is_empty()
                && !declaration.targets.contains(&context.target_id))
        {
            continue;
        }
        let path = snapshot.query().file_path();
        let relative = path.as_deref().map(|path| {
            snapshot
                .shuck_settings()
                .project_root()
                .and_then(|root| path.strip_prefix(root).ok())
                .unwrap_or(path)
        });
        if !declaration.files.is_empty()
            && !relative.is_some_and(|path| {
                declaration.files.iter().any(|pattern| {
                    globset::Glob::new(pattern)
                        .is_ok_and(|glob| glob.compile_matcher().is_match(path))
                })
            })
        {
            continue;
        }
        declared.insert(
            name.clone(),
            shucked_command::CommandDeclaration {
                kind: match declaration.kind {
                    shucked_config::CommandRequirement::Required => {
                        shucked_command::DeclarationKind::Expected
                    }
                    shucked_config::CommandRequirement::Optional => {
                        shucked_command::DeclarationKind::Optional
                    }
                    shucked_config::CommandRequirement::Generated => {
                        shucked_command::DeclarationKind::Generated
                    }
                    shucked_config::CommandRequirement::Deployment => {
                        shucked_command::DeclarationKind::Deployment
                    }
                },
                provenance: Some(shucked_command::Provenance::new("project declaration")),
                ..Default::default()
            },
        );
    }
    let source_analysis = snapshot.analysis();
    let has_source_refs = source_analysis
        .as_ref()
        .is_some_and(|analysis| !analysis.semantic().source_refs().is_empty());
    let source_index = source_analysis
        .as_ref()
        .filter(|_| has_source_refs)
        .and(snapshot.workspace_functions.as_ref())
        .and_then(crate::workspace_functions::workspace_function_index);
    let source_path = snapshot
        .query()
        .file_path()
        .map(|path| crate::workspace_functions::canonical_path(&path));
    let sites: Vec<_> = facts
        .into_iter()
        .map(|facts| {
            let name = facts.name().map(str::to_owned);
            let has_visible_source = source_analysis.as_ref().is_some_and(|analysis| {
                analysis.semantic().source_refs().iter().any(|source| {
                    analysis
                        .semantic()
                        .source_ref_visible_at_offset(source, facts.name_span().start.offset())
                })
            });
            let sourced_function =
                source_index
                    .as_ref()
                    .zip(source_path.as_ref())
                    .and_then(|(index, path)| {
                        index.resolve_call_site_exact(
                            path,
                            facts.name_span(),
                            snapshot.analysis_cancellation(),
                        )
                    });
            let site = CommandSite {
                name: name.clone(),
                arguments: facts
                    .effective_words
                    .iter()
                    .skip(1)
                    .map(|w| w.text.clone().unwrap_or_default())
                    .collect(),
                alias_eligible: facts.aliases.is_empty()
                    && facts.words.first().is_some_and(|w| w.alias_eligible),
                lookup: match facts.namespace {
                    CommandNamespace::Shell => shucked_command::LookupMode::Normal,
                    CommandNamespace::Builtin => shucked_command::LookupMode::BuiltinOnly,
                    CommandNamespace::ExternalOrBuiltin => shucked_command::LookupMode::Command,
                    CommandNamespace::External => shucked_command::LookupMode::ExternalOnly,
                },
                functions: if sourced_function.is_some()
                    || (!has_visible_source && facts.visible_function.is_some())
                    || fish_functions.contains(&facts.name_span().start.offset())
                {
                    name.iter().cloned().collect()
                } else {
                    BTreeSet::new()
                },
                guarded: if facts.guarded_available {
                    name.iter().cloned().collect()
                } else {
                    BTreeSet::new()
                },
                environment_uncertain: facts.environment_uncertain.is_some(),
                declared: declared.clone(),
                ..Default::default()
            };
            let mut resolution = shucked_command::resolve(&context, &environment, &site);
            if let Some(function) = sourced_function
                && source_path
                    .as_ref()
                    .is_some_and(|path| function.path != *path)
                && let CommandResolution::Resolved(command) = &mut resolution
            {
                command.provenance.push(shucked_command::Provenance {
                    source: "sourced function".into(),
                    location: Some(function.path.display().to_string()),
                });
            }
            (facts, resolution)
        })
        .collect();
    CommandAnalysis {
        context,
        environment,
        sites,
        failure,
        validation: Mutex::default(),
        local_environment: options.target_inventory.is_none()
            && !(startup && session.is_some())
            && (options.session_id.is_none()
                || session.as_ref().is_some_and(|state| state.connected)),
    }
}

pub(crate) fn range(snapshot: &DocumentSnapshot, span: shucked_ast::Span) -> types::Range {
    crate::edit::to_lsp_range(
        span.to_range(),
        snapshot.query().document().contents(),
        snapshot.query().document().index(),
        snapshot.encoding(),
    )
}

pub(crate) fn validation(
    snapshot: &DocumentSnapshot,
    analysis: &CommandAnalysis,
) -> Arc<Vec<super::commands_validation::ValidationDiagnostic>> {
    if let Some(cached) = analysis
        .validation
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return cached;
    }
    let findings = Arc::new(super::commands_validation::validate(
        &analysis.context,
        &analysis.environment,
        &analysis.sites,
        snapshot.analysis_cancellation(),
    ));
    if !snapshot.analysis_cancellation().is_cancelled() {
        *analysis
            .validation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(findings.clone());
    }
    findings
}

pub(crate) fn diagnostics(snapshot: &DocumentSnapshot) -> Vec<types::Diagnostic> {
    let analysis = snapshot.command_service.analysis(snapshot);
    let validation = validation(snapshot, &analysis);
    let diagnostics = diagnostics_from_analysis(snapshot, &analysis, &validation);
    if !snapshot.analysis_cancellation().is_cancelled()
        && snapshot.command_service.generation() == snapshot.environment_generation()
    {
        let key = analysis_key(snapshot);
        let mut completed = snapshot
            .command_service
            .completed_diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        completed.retain(|(previous, _)| previous != &key);
        completed.push_back((key, diagnostics.clone()));
        while completed.len() > 128 {
            completed.pop_front();
        }
    }
    diagnostics
}

pub(crate) fn cached_diagnostics(snapshot: &DocumentSnapshot) -> Vec<types::Diagnostic> {
    // Pull requests must retain the last completed result while a fresh metadata
    // query is pending, without starting a query or bypassing the typing debounce.
    let key = analysis_key(snapshot);
    snapshot
        .command_service
        .completed_diagnostics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(previous, _)| previous == &key)
        .map(|(_, diagnostics)| diagnostics.clone())
        .unwrap_or_default()
}

fn diagnostics_from_analysis(
    snapshot: &DocumentSnapshot,
    analysis: &CommandAnalysis,
    validation: &[super::commands_validation::ValidationDiagnostic],
) -> Vec<types::Diagnostic> {
    let mut diagnostics = Vec::new();
    for (site, resolution) in &analysis.sites {
        if let CommandResolution::Missing(missing) = resolution {
            let name = site.name().unwrap_or_default();
            diagnostics.push(types::Diagnostic {
                range: range(snapshot, site.name_span()),
                severity: Some(types::DiagnosticSeverity::WARNING),
                code: Some(types::NumberOrString::String(
                    if missing.declaration.is_some() {
                        "ENV004"
                    } else {
                        "ENV001"
                    }
                    .into(),
                )),
                source: Some("shucked".into()),
                message: if missing.declaration.is_some() {
                    format!(
                        "Declared dependency unavailable in {}: {name}",
                        analysis.context.target_id
                    )
                } else {
                    format!(
                        "Command not found in {}: {name}",
                        analysis.context.target_id
                    )
                },
                ..Default::default()
            });
        }
    }
    diagnostics.extend(validation.iter().map(|finding| types::Diagnostic {
        range: range(snapshot, finding.span),
        severity: Some(types::DiagnosticSeverity::WARNING),
        code: Some(types::NumberOrString::String(finding.code.into())),
        source: Some("shucked".into()),
        message: finding.message.clone(),
        ..Default::default()
    }));
    diagnostics
}

pub(crate) fn hover(snapshot: &DocumentSnapshot, offset: usize) -> Option<types::Hover> {
    let analysis = snapshot.command_service.analysis(snapshot);
    let (site, resolution) = analysis.sites.iter().find(|(site, _)| {
        site.name_span().start.offset() <= offset && offset < site.name_span().end.offset()
    })?;
    let resolution_text = match resolution {
        CommandResolution::Resolved(command) => format!(
            "{:?} · {}{}",
            command.kind,
            command.name,
            command
                .executable
                .as_ref()
                .map(|identity| format!(" · {}", identity.path.display()))
                .unwrap_or_default()
        ),
        CommandResolution::Missing(missing) => {
            if missing.declaration.is_some() {
                "Declared dependency unavailable".into()
            } else {
                "Command not found".into()
            }
        }
        CommandResolution::Unknown(unknown) => format!("Unknown · {}", unknown.detail),
    };
    let alias_text = if site.aliases.is_empty() {
        String::new()
    } else {
        format!(
            "\nAlias: {}",
            site.aliases
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(" → ")
        )
    };
    let captured_age = if snapshot
        .client_settings()
        .environment()
        .target_inventory
        .is_some()
        && analysis.environment.captured_unix_ms > 0
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        format!(
            "\nInventory: frozen capture, {} seconds old",
            now.saturating_sub(u128::from(analysis.environment.captured_unix_ms)) / 1000
        )
    } else {
        String::new()
    };
    let value = format!(
        "Command: {}\nTarget: {}\nShell: {:?} · {:?}\nLaunch directory: {} ({})\nResolution: {}{}{}{}",
        site.name().unwrap_or("dynamic"),
        analysis.context.target_id,
        analysis.context.dialect,
        analysis.context.mode,
        analysis
            .context
            .cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".into()),
        if analysis.context.cwd_known {
            "explicit"
        } else {
            "assumed"
        },
        resolution_text,
        alias_text,
        captured_age,
        analysis
            .failure
            .as_ref()
            .map(|s| format!("\n{s}"))
            .unwrap_or_default()
    );
    Some(types::Hover {
        contents: types::HoverContents::Markup(types::MarkupContent {
            kind: types::MarkupKind::PlainText,
            value,
        }),
        range: Some(range(snapshot, site.name_span())),
    })
}

pub(crate) fn code_actions(
    snapshot: &DocumentSnapshot,
    requested: &types::Range,
) -> Vec<types::CodeActionOrCommand> {
    super::commands_actions::code_actions(snapshot, requested)
}

pub(crate) fn fish_syntax_diagnostics(snapshot: &DocumentSnapshot) -> Vec<types::Diagnostic> {
    if snapshot.client_settings().show_syntax_errors() {
        shucked_semantic::analyze_fish(snapshot.query().document().contents())
            .diagnostics
            .into_iter()
            .map(|diagnostic| types::Diagnostic {
                range: range(snapshot, diagnostic.span),
                severity: Some(types::DiagnosticSeverity::ERROR),
                source: Some("shucked".into()),
                message: diagnostic.message,
                ..Default::default()
            })
            .collect()
    } else {
        Vec::new()
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellSessionState {
    pub id: String,
    pub generation: u64,
    pub cwd: PathBuf,
    pub path: Vec<PathBuf>,
    #[serde(default)]
    pub aliases: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub functions: BTreeSet<String>,
    pub connected: bool,
    #[serde(default)]
    pub live_completion: bool,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

pub(crate) fn source_fingerprint(snapshot: &DocumentSnapshot) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    snapshot.query().document().contents().hash(&mut hash);
    hash.finish()
}

#[cfg(all(test, unix))]
#[path = "../../tests/commands/service.rs"]
mod tests;
