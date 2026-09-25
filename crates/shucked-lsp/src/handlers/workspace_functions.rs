//! Shared workspace index for cross-file function and variable editor features.
//!
//! The index projects each shell file into compact semantic function and
//! variable facts, resolves determinable `source` edges, and retains just
//! enough source metadata to turn byte spans back into LSP ranges. Open
//! buffers shadow disk content. Shell startup files outside the workspace
//! roots that source a workspace file join the index as loaders, so the
//! definitions they establish before the `source` line resolve inside the
//! workspace. File analysis survives workspace invalidation; source effects
//! are reused only while their content and dependencies match.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use lsp_types as types;
use sha2::{Digest, Sha256};
use shucked_ast::Span;
use shucked_config::{
    ConfigArguments, apply_config_overrides, load_project_config, resolve_project_root_for_file,
};
use shucked_indexer::LineIndex;
use shucked_linter::ShellDialect;
use shucked_semantic::{
    CallFactSourceEdge, CallNodeKind, CrossFileCall, ExactFunctionRename, ExactFunctionRenameError,
    FileCallFacts, FileVariableFacts, SemanticModel, SourcePathFileProvider, SourceRefKind,
    VisibleSourcedFunction, WorkspaceCallIndex,
};

use crate::PositionEncoding;
use crate::edit::DocumentVersion;
use crate::editor::analyze_editor_document;
use crate::session::{ClientOptions, RequestCancellationToken, WorkspaceSettingsSnapshot};
use crate::symbols::WorkspaceOpenDocument;
use crate::workspace_variables::{WorkspaceVariableIndex, WorkspaceVariableTarget};

const MAX_RETAINED_MODELS: usize = 128;
const MAX_RETAINED_MODEL_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound on files that are indexed only because a shell startup file
/// outside the workspace roots loads a workspace file: the loaders themselves
/// plus everything reached through them. The workspace file limit still
/// applies on top.
const MAX_LOADER_FILES: usize = 64;
/// Startup files larger than this are not inspected as loader candidates.
const MAX_LOADER_FILE_BYTES: u64 = 1024 * 1024;

/// Immutable session state needed to build or query the cross-file symbol index.
#[derive(Clone)]
pub(crate) struct WorkspaceFunctionContext {
    pub(crate) workspace_roots: Vec<PathBuf>,
    pub(crate) settings_workspace_roots: Vec<PathBuf>,
    pub(crate) workspace_settings: Vec<WorkspaceSettingsSnapshot>,
    pub(crate) global_options: ClientOptions,
    pub(crate) open_documents: Vec<WorkspaceOpenDocument>,
    pub(crate) encoding: PositionEncoding,
    /// Hard bound on indexed files. A build that reaches this limit is marked
    /// incomplete so mutation features can fail closed.
    pub(crate) max_files: usize,
    pub(crate) cache: Arc<WorkspaceFunctionIndexCache>,
    pub(crate) epoch: u64,
    pub(crate) cancellation: RequestCancellationToken,
}

/// Session-lifetime cache of the built cross-file symbol index.
#[derive(Default)]
pub(crate) struct WorkspaceFunctionIndexCache {
    epoch: AtomicU64,
    built: Mutex<Option<(u64, Arc<WorkspaceFunctionIndex>)>>,
    building: Mutex<()>,
    projections: Mutex<BTreeMap<PathBuf, IndexedWorkspaceFile>>,
}

impl WorkspaceFunctionIndexCache {
    fn build_guard<'a>(
        &'a self,
        context: &WorkspaceFunctionContext,
    ) -> Option<std::sync::MutexGuard<'a, ()>> {
        loop {
            if context.cancellation.is_cancelled() || self.current_epoch() != context.epoch {
                return None;
            }
            match self.building.try_lock() {
                Ok(guard) => return Some(guard),
                Err(std::sync::TryLockError::Poisoned(error)) => return Some(error.into_inner()),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
            }
        }
    }
    pub(crate) fn dependency_paths(&self) -> Vec<PathBuf> {
        self.previous()
            .map(|index| {
                index
                    .files
                    .values()
                    .flat_map(|file| file.projection.dependencies.keys().cloned())
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Invalidates queries while retaining file analysis for the next build.
    pub(crate) fn invalidate(&self) {
        let _slot = self.built.lock();
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn previous(&self) -> Option<Arc<WorkspaceFunctionIndex>> {
        self.built
            .lock()
            .ok()?
            .as_ref()
            .map(|(_, index)| index.clone())
    }

    pub(crate) fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    fn get(&self, epoch: u64) -> Option<Arc<WorkspaceFunctionIndex>> {
        let slot = self.built.lock().ok()?;
        if epoch != self.current_epoch() {
            return None;
        }
        slot.as_ref()
            .filter(|(built_epoch, _)| *built_epoch == epoch)
            .map(|(_, built)| built.clone())
    }

    fn store(&self, epoch: u64, built: Arc<WorkspaceFunctionIndex>) {
        if let Ok(mut slot) = self.built.lock()
            && epoch == self.current_epoch()
        {
            *slot = Some((epoch, built));
        }
    }
}

/// Returns the cached index for this context, building it on a miss.
///
/// Cancellation is checked between index-population steps. An aborted miss does
/// not populate the cache, and the request wrapper suppresses its response.
pub(crate) fn workspace_function_index(
    context: &WorkspaceFunctionContext,
) -> Option<Arc<WorkspaceFunctionIndex>> {
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    if let Some(built) = context.cache.get(context.epoch) {
        built.prepare_functions(&context.cancellation)?;
        return Some(built);
    }
    let _guard = context.cache.build_guard(context)?;
    if let Some(built) = context.cache.get(context.epoch) {
        built.prepare_functions(&context.cancellation)?;
        return Some(built);
    }
    let built = Arc::new(WorkspaceFunctionIndex::build(context)?);
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    context.cache.store(context.epoch, built.clone());
    Some(built)
}

/// Completion only evaluates the source-connected component of the current file.
/// The same complete file projections remain available to navigation and edits.
pub(crate) fn completion_workspace_function_index(
    context: &WorkspaceFunctionContext,
) -> Option<Arc<WorkspaceFunctionIndex>> {
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    if let Some(built) = context.cache.get(context.epoch) {
        return Some(built);
    }
    let _guard = context.cache.build_guard(context)?;
    if let Some(built) = context.cache.get(context.epoch) {
        return Some(built);
    }
    let built = Arc::new(WorkspaceFunctionIndex::build_projections(context)?);
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    context.cache.store(context.epoch, built.clone());
    Some(built)
}

pub(crate) fn cached_workspace_function_index(
    context: &WorkspaceFunctionContext,
) -> Option<Arc<WorkspaceFunctionIndex>> {
    if context.cancellation.is_cancelled() {
        return None;
    }
    context.cache.get(context.epoch)
}

/// Builds a new index for a mutation request instead of trusting a cached
/// discovery snapshot.
///
/// Cross-file edits need to observe closed files that changed without a file
/// watcher notification, including files whose previous contents were not
/// connected to the selected function. Epoch checks on both sides of the build
/// reject concurrent session invalidation.
pub(crate) fn fresh_workspace_function_index(
    context: &WorkspaceFunctionContext,
) -> Option<Arc<WorkspaceFunctionIndex>> {
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    let built = Arc::new(WorkspaceFunctionIndex::build(context)?);
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    context.cache.store(context.epoch, built.clone());
    Some(built)
}

/// Source snapshot retained for one indexed file.
#[derive(Clone)]
pub(crate) struct IndexedWorkspaceFile {
    analysis: Arc<WorkspaceFileAnalysis>,
    projection: Arc<WorkspaceFileProjection>,
    uri: types::Url,
    open_uri: Option<types::Url>,
    version: Option<DocumentVersion>,
}

struct WorkspaceFileAnalysis {
    source: Arc<str>,
    model: Option<Arc<SemanticModel>>,
    line_index: LineIndex,
    content_hash: [u8; 32],
}

struct WorkspaceFileProjection {
    source_paths: SourcePathResolution,
    dependencies: BTreeMap<PathBuf, DependencyFingerprint>,
    calls: FileCallFacts,
    functions: Arc<shucked_semantic::FileFunctionEffects>,
    sources: Vec<WorkspaceSourceDetails>,
    variables: FileVariableFacts,
    complete: bool,
}

impl IndexedWorkspaceFile {
    pub(crate) fn uri(&self) -> &types::Url {
        &self.uri
    }

    /// URI used by the editor for an open buffer, falling back to the
    /// canonical file URI for disk snapshots.
    pub(crate) fn editor_uri(&self) -> &types::Url {
        self.open_uri.as_ref().unwrap_or(&self.uri)
    }

    pub(crate) fn source(&self) -> &str {
        &self.analysis.source
    }

    pub(crate) fn line_index(&self) -> &LineIndex {
        &self.analysis.line_index
    }

    /// Open-document version captured by the request, or `None` for disk input.
    pub(crate) fn version(&self) -> Option<DocumentVersion> {
        self.version
    }

    /// SHA-256 of the exact content used to build semantic facts and ranges.
    pub(crate) fn content_hash(&self) -> [u8; 32] {
        self.analysis.content_hash
    }
}

#[derive(Clone)]
pub(crate) struct WorkspaceSourceDetails {
    pub(crate) span: Span,
    pub(crate) path_span: Span,
    pub(crate) directive_span: Option<Span>,
    pub(crate) target: Option<PathBuf>,
    pub(crate) candidates: Vec<PathBuf>,
    pub(crate) sequence: Option<Vec<PathBuf>>,
    pub(crate) reason: SourceResolutionReason,
    pub(crate) conditional: bool,
    pub(crate) in_function: bool,
    /// The plugin framework whose contract produced `sequence`, when the
    /// statement is a framework bootstrap or plugin load rather than a plain
    /// `source` operand.
    pub(crate) framework: Option<shucked_semantic::PluginFramework>,
}

#[derive(Clone, Copy)]
pub(crate) enum SourceResolutionReason {
    Resolved,
    Missing,
    Dynamic,
    UnknownValue,
    AnalysisLimit,
    Ignored,
    Unreadable,
    /// A load inside a plugin framework's own bootstrap file: its effect is
    /// attached to the statement that sources the framework.
    Framework,
}

pub(crate) struct WorkspaceVariableDetails {
    pub(crate) name: String,
    pub(crate) definitions: Vec<types::Location>,
    pub(crate) references: Vec<types::Location>,
    pub(crate) incomplete: bool,
    /// Source operations that could read the selected bindings but whose
    /// target file could not be inspected.
    pub(crate) unfollowed_sources: Vec<UnfollowedSource>,
    pub(crate) conditional: bool,
}

/// A source operation whose target could not be followed by the index.
#[derive(Clone)]
pub(crate) struct UnfollowedSource {
    pub(crate) location: types::Location,
    pub(crate) reason: Option<SourceResolutionReason>,
    /// Source text of the operation, trimmed to one line.
    pub(crate) text: String,
}

impl UnfollowedSource {
    /// Short human-readable reason suitable for a message.
    pub(crate) fn reason_text(&self) -> &'static str {
        match self.reason {
            Some(SourceResolutionReason::Resolved) => "target not indexed",
            Some(SourceResolutionReason::Missing) => "file not found",
            Some(SourceResolutionReason::Dynamic) => "runtime expression",
            Some(SourceResolutionReason::UnknownValue) => "unknown value",
            Some(SourceResolutionReason::AnalysisLimit) => "analysis limit",
            Some(SourceResolutionReason::Ignored) => "ignored by directive",
            Some(SourceResolutionReason::Unreadable) => "unreadable file",
            Some(SourceResolutionReason::Framework) => "modelled by the framework",
            None => "not followed",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum WorkspaceIssue {
    FileLimit,
    SourceLimit,
    Configuration,
    Discovery,
}

/// Shared index queried by cross-file editor features.
pub(crate) struct WorkspaceFunctionIndex {
    graph: WorkspaceCallIndex,
    functions: OnceLock<shucked_semantic::WorkspaceFunctionIndex>,
    completion_functions: Mutex<BTreeMap<PathBuf, Arc<CompletionFunctions>>>,
    variables: WorkspaceVariableIndex,
    variable_usage: OnceLock<Arc<shucked_semantic::WorkspaceVariableUsage>>,
    files: BTreeMap<PathBuf, IndexedWorkspaceFile>,
    encoding: PositionEncoding,
    complete: bool,
    issues: BTreeSet<WorkspaceIssue>,
    file_limit: usize,
}

struct CompletionFunctions {
    index: shucked_semantic::WorkspaceFunctionIndex,
    complete: bool,
}

impl WorkspaceFunctionIndex {
    pub(crate) fn variable_usage(
        &self,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<Arc<shucked_semantic::WorkspaceVariableUsage>> {
        if is_cancelled() {
            return None;
        }
        if let Some(usage) = self.variable_usage.get() {
            return Some(usage.clone());
        }
        let usage = Arc::new(self.variables.usage(is_cancelled)?);
        let _ = self.variable_usage.set(usage.clone());
        Some(usage)
    }

    fn build(context: &WorkspaceFunctionContext) -> Option<Self> {
        let result = Self::build_projections(context)?;
        result.prepare_functions(&context.cancellation)?;
        Some(result)
    }

    fn prepare_functions(&self, cancellation: &RequestCancellationToken) -> Option<()> {
        if self.functions.get().is_none() {
            let functions = shucked_semantic::WorkspaceFunctionIndex::build(
                self.files
                    .iter()
                    .map(|(path, file)| (path.clone(), file.projection.functions.clone()))
                    .collect(),
                &|| cancellation.is_cancelled(),
            )?;
            let _ = self.functions.set(functions);
        }
        (!cancellation.is_cancelled()).then_some(())
    }

    fn functions(&self) -> &shucked_semantic::WorkspaceFunctionIndex {
        self.functions.get_or_init(|| {
            shucked_semantic::WorkspaceFunctionIndex::build(
                self.files
                    .iter()
                    .map(|(path, file)| (path.clone(), file.projection.functions.clone()))
                    .collect(),
                &|| false,
            )
            .expect("uncancelled bounded workspace evaluation")
        })
    }

    fn completion_functions(&self, path: &Path) -> Arc<CompletionFunctions> {
        if let Some(index) = self
            .completion_functions
            .lock()
            .ok()
            .and_then(|cache| cache.get(path).cloned())
        {
            return index;
        }
        // Include incoming loaders, their other imports, and recursively sourced
        // files: module visibility depends on their execution order together.
        let mut connected = BTreeSet::from([path.to_path_buf()]);
        loop {
            let before = connected.len();
            for (source, facts) in self.graph.files() {
                for edge in &facts.source_edges {
                    if connected.contains(source) || connected.contains(&edge.path) {
                        connected.insert(source.to_path_buf());
                        connected.insert(edge.path.clone());
                    }
                }
            }
            if connected.len() == before {
                break;
            }
        }
        let complete = connected.iter().all(|path| {
            self.files.get(path).is_some_and(|file| {
                file.projection.complete && file.projection.source_paths.complete
            })
        });
        let index = Arc::new(CompletionFunctions {
            index: shucked_semantic::WorkspaceFunctionIndex::build(
                connected
                    .iter()
                    .filter_map(|path| {
                        self.files
                            .get(path)
                            .map(|file| (path.clone(), file.projection.functions.clone()))
                    })
                    .collect(),
                &|| false,
            )
            .expect("uncancelled bounded component evaluation"),
            complete,
        });
        if let Ok(mut cache) = self.completion_functions.lock() {
            for path in connected {
                cache.insert(path, index.clone());
            }
        }
        index
    }

    pub(crate) fn completion_function_resolution(
        &self,
        path: &Path,
        span: Span,
    ) -> shucked_semantic::WorkspaceFunctionResolution {
        let functions = self.completion_functions(path);
        let mut resolution = functions.index.resolve(path, span);
        resolution.incomplete |= !functions.complete;
        resolution
    }

    fn build_projections(context: &WorkspaceFunctionContext) -> Option<Self> {
        let started = std::time::Instant::now();
        let previous = context.cache.previous();
        let mut graph = WorkspaceCallIndex::new();
        let mut variables = WorkspaceVariableIndex::default();
        let mut files = BTreeMap::new();
        let mut complete = true;
        let mut issues = BTreeSet::new();
        let max_files = context.max_files;
        let mut path_analyzer = shucked_semantic::SourcePathAnalyzer::default();

        let mut open_docs = context
            .open_documents
            .iter()
            .filter_map(|open| {
                let path = canonical_path(&open.uri.to_file_path().ok()?);
                Some((path, open))
            })
            .collect::<Vec<_>>();
        open_docs.sort_by(|(left, _), (right, _)| left.cmp(right));
        let open_paths = open_docs
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<BTreeSet<_>>();

        let path_provider = WorkspacePathProvider::new(context);

        for (path, open) in &open_docs {
            if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch
            {
                return None;
            }
            if graph.file_count() >= max_files {
                complete = false;
                issues.insert(WorkspaceIssue::FileLimit);
                tracing::warn!(
                    "workspace functions: open documents exceed the {max_files}-file limit; \
                     indexing only the first {max_files}"
                );
                break;
            }
            let resolution = path_provider
                .source_paths
                .borrow_mut()
                .resolve(path, context);
            complete &= resolution.complete;
            if !resolution.complete {
                issues.insert(WorkspaceIssue::Configuration);
            }
            complete &= insert_file(
                &mut graph,
                &mut variables,
                &mut files,
                previous.as_deref(),
                WorkspaceFileInput {
                    path,
                    uri: open.uri.clone(),
                    source: open.document.contents(),
                    version: Some(open.document.version()),
                },
                &resolution,
                &mut path_analyzer,
                &path_provider,
            );
        }

        if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
            return None;
        }
        let remaining = max_files.saturating_sub(graph.file_count());
        let discovery = discover_closed_shell_files(
            &context.workspace_roots,
            &open_paths,
            remaining,
            &context.cancellation,
        )?;
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            files = discovery.files.len(),
            "workspace index discovery complete"
        );
        complete &= discovery.complete;
        if !discovery.complete {
            issues.insert(if discovery.limited {
                WorkspaceIssue::FileLimit
            } else {
                WorkspaceIssue::Discovery
            });
        }
        for file in discovery.files {
            if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch
            {
                return None;
            }
            let Some(source) = path_provider.source(&file) else {
                complete = false;
                issues.insert(WorkspaceIssue::Discovery);
                continue;
            };
            let Ok(uri) = types::Url::from_file_path(&file) else {
                complete = false;
                issues.insert(WorkspaceIssue::Discovery);
                continue;
            };
            let resolution = path_provider
                .source_paths
                .borrow_mut()
                .resolve(&file, context);
            complete &= resolution.complete;
            if !resolution.complete {
                issues.insert(WorkspaceIssue::Configuration);
            }
            complete &= insert_file(
                &mut graph,
                &mut variables,
                &mut files,
                previous.as_deref(),
                WorkspaceFileInput {
                    path: &file,
                    uri,
                    source: &source,
                    version: None,
                },
                &resolution,
                &mut path_analyzer,
                &path_provider,
            );
        }

        // Startup files outside the roots that load a workspace file are part
        // of its execution context: their definitions are visible inside the
        // workspace file, so they join the index as loaders.
        let workspace_roots = context
            .workspace_roots
            .iter()
            .map(|root| canonical_path(root))
            .collect::<Vec<_>>();
        let mut loader_origin = BTreeSet::new();
        let mut loader_budget = MAX_LOADER_FILES;
        let mut skipped = BTreeSet::new();
        for candidate in startup_loader_candidates(&path_provider, &mut path_analyzer) {
            if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch
            {
                return None;
            }
            let key = canonical_path(&candidate);
            if files.contains_key(&key)
                || workspace_roots.iter().any(|root| key.starts_with(root))
                || graph.file_count() >= max_files
                || loader_budget == 0
            {
                continue;
            }
            let Some(metadata) = std::fs::metadata(&candidate).ok().filter(|m| m.is_file()) else {
                continue;
            };
            if metadata.len() > MAX_LOADER_FILE_BYTES {
                tracing::debug!(
                    "workspace functions: startup file {} exceeds the loader size limit",
                    candidate.display()
                );
                continue;
            }
            let Some(source) = path_provider.source(&candidate) else {
                continue;
            };
            let Ok(uri) = types::Url::from_file_path(&candidate) else {
                continue;
            };
            let resolution = path_provider
                .source_paths
                .borrow_mut()
                .resolve(&candidate, context);
            let prepared = prepare_file(
                previous.as_deref(),
                WorkspaceFileInput {
                    path: &candidate,
                    uri,
                    source: &source,
                    version: None,
                },
                &resolution,
                &mut path_analyzer,
                &path_provider,
            );
            let loads_workspace = prepared
                .file
                .projection
                .calls
                .source_edges
                .iter()
                .any(|edge| {
                    files.contains_key(&edge.path)
                        || workspace_roots
                            .iter()
                            .any(|root| edge.path.starts_with(root))
                });
            if !loads_workspace {
                continue;
            }
            tracing::debug!(
                "workspace functions: indexing {} as a loader of workspace files",
                candidate.display()
            );
            complete &= resolution.complete;
            if !resolution.complete {
                issues.insert(WorkspaceIssue::Configuration);
            }
            loader_origin.insert(prepared.key.clone());
            loader_budget -= 1;
            complete &= commit_file(&mut graph, &mut variables, &mut files, prepared);
        }

        'expand: loop {
            if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch
            {
                return None;
            }
            // Missing targets, with whether a workspace file (rather than only
            // a loader) requests them; loader-only targets count against the
            // loader budget instead of joining the workspace closure freely.
            let mut missing = BTreeMap::<PathBuf, bool>::new();
            for (source, facts) in graph.files() {
                let from_workspace = !loader_origin.contains(source);
                for edge in &facts.source_edges {
                    if graph.contains(&edge.path) || skipped.contains(&edge.path) {
                        continue;
                    }
                    *missing.entry(edge.path.clone()).or_default() |= from_workspace;
                }
            }
            if missing.is_empty() {
                break;
            }
            for (target, from_workspace) in missing {
                if context.cancellation.is_cancelled()
                    || context.cache.current_epoch() != context.epoch
                {
                    return None;
                }
                if graph.file_count() >= max_files {
                    complete = false;
                    issues.insert(WorkspaceIssue::FileLimit);
                    tracing::warn!(
                        "workspace functions: source-edge targets exceed the {max_files}-file \
                         limit; cross-file results may be incomplete"
                    );
                    break 'expand;
                }
                if !from_workspace {
                    if loader_budget == 0 {
                        tracing::debug!(
                            "workspace functions: loader closure limit ({MAX_LOADER_FILES}) \
                             reached; not indexing {}",
                            target.display()
                        );
                        skipped.insert(target);
                        continue;
                    }
                    loader_budget -= 1;
                    loader_origin.insert(target.clone());
                }
                let Some(open) = open_docs
                    .iter()
                    .find_map(|(path, open)| (path == &target).then_some(*open))
                else {
                    let Some(source) = path_provider.source(&target) else {
                        complete = false;
                        issues.insert(WorkspaceIssue::Discovery);
                        graph.insert(target.clone(), FileCallFacts::default());
                        continue;
                    };
                    let Ok(uri) = types::Url::from_file_path(&target) else {
                        complete = false;
                        issues.insert(WorkspaceIssue::Discovery);
                        graph.insert(target.clone(), FileCallFacts::default());
                        continue;
                    };
                    let resolution = path_provider
                        .source_paths
                        .borrow_mut()
                        .resolve(&target, context);
                    complete &= resolution.complete;
                    if !resolution.complete {
                        issues.insert(WorkspaceIssue::Configuration);
                    }
                    complete &= insert_file(
                        &mut graph,
                        &mut variables,
                        &mut files,
                        previous.as_deref(),
                        WorkspaceFileInput {
                            path: &target,
                            uri,
                            source: &source,
                            version: None,
                        },
                        &resolution,
                        &mut path_analyzer,
                        &path_provider,
                    );
                    continue;
                };
                let resolution = path_provider
                    .source_paths
                    .borrow_mut()
                    .resolve(&target, context);
                complete &= resolution.complete;
                if !resolution.complete {
                    issues.insert(WorkspaceIssue::Configuration);
                }
                complete &= insert_file(
                    &mut graph,
                    &mut variables,
                    &mut files,
                    previous.as_deref(),
                    WorkspaceFileInput {
                        path: &target,
                        uri: open.uri.clone(),
                        source: open.document.contents(),
                        version: Some(open.document.version()),
                    },
                    &resolution,
                    &mut path_analyzer,
                    &path_provider,
                );
            }
        }

        if files.values().any(|file| !file.projection.complete) {
            issues.insert(WorkspaceIssue::SourceLimit);
        }
        tracing::debug!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            files = files.len(),
            "workspace index projections complete"
        );
        Some(Self {
            functions: OnceLock::new(),
            completion_functions: Mutex::default(),
            issues,
            file_limit: max_files,
            variable_usage: OnceLock::new(),
            graph,
            variables,
            files,
            encoding: context.encoding,
            complete,
        })
    }

    pub(crate) fn function_resolution(
        &self,
        path: &Path,
        span: Span,
    ) -> shucked_semantic::WorkspaceFunctionResolution {
        let mut result = self.functions().resolve(path, span);
        result.incomplete |=
            !self.complete && result.exact().is_none_or(|target| target.path != path);
        result
    }

    pub(crate) fn function_definitions(
        &self,
        path: &Path,
        offset: usize,
    ) -> Vec<shucked_semantic::WorkspaceFunctionDefinition> {
        self.files
            .get(path)
            .map(|file| {
                file.projection
                    .calls
                    .definitions
                    .iter()
                    .filter(|definition| {
                        definition.selection_span.start.offset() <= offset
                            && offset < definition.selection_span.end.offset()
                    })
                    .map(|definition| shucked_semantic::WorkspaceFunctionDefinition {
                        path: path.to_path_buf(),
                        definition: definition.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every indexed definition of the function called `name`, in path order.
    ///
    /// Used by implementation requests, where each redefinition (for example a
    /// per-platform override sourced later) is a candidate body.
    pub(crate) fn function_definitions_named(
        &self,
        name: &str,
    ) -> Vec<shucked_semantic::WorkspaceFunctionDefinition> {
        self.files
            .iter()
            .flat_map(|(path, file)| {
                file.projection
                    .calls
                    .definitions
                    .iter()
                    .filter(|definition| definition.name.as_str() == name)
                    .map(|definition| shucked_semantic::WorkspaceFunctionDefinition {
                        path: path.clone(),
                        definition: definition.clone(),
                    })
            })
            .collect()
    }

    pub(crate) fn function_locations(
        &self,
        definitions: &[shucked_semantic::WorkspaceFunctionDefinition],
    ) -> Vec<types::Location> {
        definitions
            .iter()
            .filter_map(|target| {
                Some(types::Location {
                    uri: self.file(&target.path)?.editor_uri().clone(),
                    range: self.range_of(&target.path, target.definition.selection_span)?,
                })
            })
            .collect()
    }

    pub(crate) fn function_references(
        &self,
        definitions: &[shucked_semantic::WorkspaceFunctionDefinition],
    ) -> (Vec<types::Location>, bool) {
        let mut incomplete = !self.complete || self.functions().incomplete;
        let locations = self
            .functions()
            .calls()
            .filter(|call| {
                call.resolution
                    .definitions
                    .iter()
                    .any(|target| definitions.contains(target))
            })
            .filter_map(|call| {
                incomplete |= call.resolution.incomplete
                    || call.resolution.may_be_absent
                    || call.resolution.definitions.len() > 1;
                Some(types::Location {
                    uri: self.file(&call.path)?.editor_uri().clone(),
                    range: self.range_of(&call.path, call.span)?,
                })
            })
            .collect();
        (locations, incomplete)
    }

    pub(crate) fn function_completions(
        &self,
        path: &Path,
        offset: usize,
    ) -> Vec<VisibleSourcedFunction> {
        let functions = self.completion_functions(path);
        functions
            .index
            .visible(path, offset)
            .into_iter()
            .flat_map(|(_, resolution)| {
                let possible = resolution.exact().is_none()
                    || !functions.complete
                    || functions.index.incomplete;
                resolution
                    .definitions
                    .into_iter()
                    .filter(move |target| possible || target.path != path)
                    .map(move |target| VisibleSourcedFunction {
                        possible,
                        name: target.definition.name,
                        path: target.path,
                        def_span: target.definition.def_span,
                        selection_span: target.definition.selection_span,
                        import_span: Span {
                            start: shucked_ast::Position::at(1, 1, offset),
                            end: shucked_ast::Position::at(1, 1, offset),
                        },
                    })
            })
            .collect()
    }

    pub(crate) fn resolve_call_site_exact(
        &self,
        from_path: &Path,
        name_span: Span,
        cancellation: &RequestCancellationToken,
    ) -> Option<CrossFileCall> {
        self.graph
            .resolve_call_site_exact_cancellable(from_path, name_span, || {
                cancellation.is_cancelled()
            })
    }

    /// Definitions for navigation.
    ///
    /// Exact reaching definitions are preferred. When source effects make the
    /// exact answer ambiguous, or the workspace index is partial, the possible
    /// definitions known to the index are returned instead of nothing: a
    /// navigation target that may be one of several is more useful than no
    /// target, and mutation features apply their own stricter checks.
    pub(crate) fn variable_definition_locations(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        cancellation: &RequestCancellationToken,
    ) -> Option<Vec<types::Location>> {
        if cancellation.is_cancelled() {
            return None;
        }
        let is_cancelled = || cancellation.is_cancelled();
        if self.complete
            && let Some(occurrences) = self.variables.definitions(from_path, target, &is_cancelled)
            && !occurrences.is_empty()
        {
            return self.variable_locations(occurrences, cancellation);
        }
        if cancellation.is_cancelled() {
            return None;
        }
        let explanation = self.variables.explain(from_path, target, &is_cancelled)?;
        self.variable_locations(explanation.definitions, cancellation)
    }

    pub(crate) fn variable_details(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        cancellation: &RequestCancellationToken,
    ) -> Option<WorkspaceVariableDetails> {
        let explanation = self
            .variables
            .explain(from_path, target, &|| cancellation.is_cancelled())?;
        let unfollowed_sources = explanation
            .unfollowed_sources
            .iter()
            .map(|occurrence| {
                let details = self.source_details(&occurrence.path, occurrence.span.start.offset());
                let text = self
                    .files
                    .get(&occurrence.path)
                    .and_then(|file| {
                        file.source()
                            .get(occurrence.span.start.offset()..occurrence.span.end.offset())
                    })
                    .map(|text| text.lines().next().unwrap_or_default().trim().to_owned())
                    .unwrap_or_default();
                (
                    occurrence.clone(),
                    details.map(|details| details.reason),
                    text,
                )
            })
            .collect::<Vec<_>>();
        let unfollowed_locations = self.variable_locations(
            unfollowed_sources
                .iter()
                .map(|(occurrence, _, _)| occurrence.clone())
                .collect(),
            cancellation,
        )?;
        Some(WorkspaceVariableDetails {
            name: explanation.name.to_string(),
            definitions: self.variable_locations(explanation.definitions, cancellation)?,
            references: self.variable_locations(explanation.references, cancellation)?,
            incomplete: explanation.incomplete || !self.complete,
            unfollowed_sources: unfollowed_locations
                .into_iter()
                .zip(unfollowed_sources)
                .map(|(location, (_, reason, text))| UnfollowedSource {
                    location,
                    reason,
                    text,
                })
                .collect(),
            conditional: explanation.conditional,
        })
    }

    pub(crate) fn source_details(
        &self,
        from_path: &Path,
        offset: usize,
    ) -> Option<&WorkspaceSourceDetails> {
        self.files
            .get(from_path)?
            .projection
            .sources
            .iter()
            .find(|source| {
                (source.span.start.offset() <= offset && offset < source.span.end.offset())
                    || source.directive_span.is_some_and(|span| {
                        span.start.offset() <= offset && offset < span.end.offset()
                    })
            })
    }

    pub(crate) fn incomplete_reason(&self) -> Option<String> {
        if self.complete && !self.functions().incomplete {
            return None;
        }
        let mut reasons = self
            .issues
            .iter()
            .map(|issue| match issue {
                WorkspaceIssue::FileLimit => {
                    format!("workspace file limit ({}) reached", self.file_limit)
                }
                WorkspaceIssue::SourceLimit => {
                    "source analysis reached a file, depth, size, or work limit".into()
                }
                WorkspaceIssue::Configuration => {
                    "some source-path settings could not be read".into()
                }
                WorkspaceIssue::Discovery => {
                    "some workspace files could not be discovered or read".into()
                }
            })
            .collect::<Vec<_>>();
        if self.functions().incomplete {
            reasons.push("workspace function analysis limit reached".into());
        }
        if reasons.is_empty() {
            reasons.push("workspace discovery did not finish".into());
        }
        Some(reasons.join("; "))
    }

    fn variable_locations(
        &self,
        occurrences: Vec<crate::workspace_variables::WorkspaceVariableOccurrence>,
        cancellation: &RequestCancellationToken,
    ) -> Option<Vec<types::Location>> {
        let mut locations = Vec::with_capacity(occurrences.len());
        for occurrence in occurrences {
            if cancellation.is_cancelled() {
                return None;
            }
            let file = self.file(&occurrence.path)?;
            locations.push(types::Location {
                uri: file.editor_uri().clone(),
                range: crate::edit::to_lsp_range(
                    occurrence.span.to_range(),
                    file.source(),
                    file.line_index(),
                    self.encoding,
                ),
            });
        }
        Some(locations)
    }

    pub(crate) fn exact_function_rename(
        &self,
        target_path: &Path,
        target_node: &CallNodeKind,
        cancellation: &RequestCancellationToken,
    ) -> Option<Result<ExactFunctionRename, ExactFunctionRenameError>> {
        if cancellation.is_cancelled() {
            return None;
        }
        self.graph
            .exact_function_rename(target_path, target_node, || cancellation.is_cancelled())
    }

    pub(crate) fn incoming(
        &self,
        target_path: &Path,
        target_node: &CallNodeKind,
    ) -> Vec<CrossFileCall> {
        let mut result: Vec<CrossFileCall> = Vec::new();
        for call in self.functions().calls().filter(|call| {
            call.resolution.definitions.iter().any(|target| {
                target.path == target_path
                    && CallNodeKind::Function(target.definition.identity()) == *target_node
            })
        }) {
            if let Some(existing) = result
                .iter_mut()
                .find(|entry| entry.path == call.path && entry.node == call.enclosing)
            {
                existing.call_spans.push(call.span);
            } else {
                let definition = self.files.get(&call.path).and_then(|f| {
                    f.projection
                        .calls
                        .definitions
                        .iter()
                        .find(|d| CallNodeKind::Function(d.identity()) == call.enclosing)
                });
                result.push(CrossFileCall {
                    path: call.path.clone(),
                    node: call.enclosing.clone(),
                    def_span: definition.map(|d| d.def_span),
                    selection_span: definition.map(|d| d.selection_span),
                    call_spans: vec![call.span],
                });
            }
        }
        result
    }

    pub(crate) fn outgoing(
        &self,
        from_path: &Path,
        from_node: &CallNodeKind,
    ) -> Vec<CrossFileCall> {
        let mut result: Vec<CrossFileCall> = Vec::new();
        for call in self
            .functions()
            .calls()
            .filter(|call| call.path == from_path && call.enclosing == *from_node)
        {
            for target in &call.resolution.definitions {
                let node = CallNodeKind::Function(target.definition.identity());
                if let Some(existing) = result
                    .iter_mut()
                    .find(|entry| entry.path == target.path && entry.node == node)
                {
                    existing.call_spans.push(call.span);
                } else {
                    result.push(CrossFileCall {
                        path: target.path.clone(),
                        node,
                        def_span: Some(target.definition.def_span),
                        selection_span: Some(target.definition.selection_span),
                        call_spans: vec![call.span],
                    });
                }
            }
        }
        result
    }

    pub(crate) fn file(&self, path: &Path) -> Option<&IndexedWorkspaceFile> {
        self.files.get(path)
    }

    pub(crate) fn source_target(
        &self,
        from_path: &Path,
        source_span: Span,
    ) -> Option<(&Path, &types::Url)> {
        let target = self.graph.source_target(from_path, source_span)?;
        self.file(target).map(|file| (target, file.editor_uri()))
    }

    pub(crate) fn range_of(&self, path: &Path, span: Span) -> Option<types::Range> {
        let file = self.file(path)?;
        Some(crate::edit::to_lsp_range(
            span.to_range(),
            file.source(),
            file.line_index(),
            self.encoding,
        ))
    }

    pub(crate) fn encoding(&self) -> PositionEncoding {
        self.encoding
    }

    pub(crate) fn ranges_in(&self, path: &Path, spans: &[Span]) -> Vec<types::Range> {
        spans
            .iter()
            .filter_map(|span| self.range_of(path, *span))
            .collect()
    }

    /// Whether discovery and source-edge expansion completed within the file
    /// budget and without unreadable inputs.
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    /// Returns whether a closed file still matches the content used to build
    /// this index. Open documents are versioned by the LSP session instead.
    pub(crate) fn closed_file_is_current(&self, path: &Path) -> bool {
        let Some(file) = self.file(path) else {
            return false;
        };
        if file.version().is_some() {
            return true;
        }
        std::fs::read(path)
            .map(|contents| content_hash(&contents) == file.content_hash())
            .unwrap_or(false)
    }

    /// Verifies every closed snapshot retained by this index, including files
    /// which were disconnected from the selected binding when the index was
    /// built. The outer `None` denotes cancellation.
    pub(crate) fn validate_closed_files(
        &self,
        cancellation: &RequestCancellationToken,
    ) -> Option<Result<(), PathBuf>> {
        for (path, file) in &self.files {
            if cancellation.is_cancelled() {
                return None;
            }
            if file.version().is_none() && !self.closed_file_is_current(path) {
                return Some(Err(path.clone()));
            }
        }
        Some(Ok(()))
    }

    #[cfg(test)]
    fn file_count(&self) -> usize {
        self.graph.file_count()
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.graph.contains(path)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SourcePathResolution {
    roots: Vec<String>,
    project_root: PathBuf,
    complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SourcePathsCacheKey {
    project_root: PathBuf,
    workspace_root: Option<PathBuf>,
}

#[derive(Default)]
struct SourcePathsCache {
    by_project: HashMap<SourcePathsCacheKey, SourcePathResolution>,
}

impl SourcePathsCache {
    fn resolve(&mut self, path: &Path, context: &WorkspaceFunctionContext) -> SourcePathResolution {
        let workspace = workspace_settings_for_path(context, path);
        let fallback = context
            .settings_workspace_roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned()
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let (project_root, root_complete) =
            match resolve_project_root_for_file(path, &fallback, true) {
                Ok(project_root) => (project_root, true),
                Err(error) => {
                    tracing::warn!(
                        "workspace functions: failed to resolve the project root for {}: {error}",
                        path.display()
                    );
                    (fallback, false)
                }
            };
        let key = SourcePathsCacheKey {
            project_root: project_root.clone(),
            workspace_root: workspace.map(|workspace| workspace.root.clone()),
        };
        let mut resolution = self
            .by_project
            .entry(key)
            .or_insert_with(|| {
                let (mut config, config_complete) =
                    match load_project_config(&project_root, &ConfigArguments::default()) {
                        Ok(config) => (config, true),
                        Err(error) => {
                            tracing::warn!(
                                "workspace functions: failed to load config from {}: {error}",
                                project_root.display()
                            );
                            (Default::default(), false)
                        }
                    };
                apply_config_overrides(&mut config, context.global_options.to_config_overrides());
                if let Some(options) = workspace.and_then(|workspace| workspace.options.as_ref()) {
                    apply_config_overrides(&mut config, options.to_config_overrides());
                }
                SourcePathResolution {
                    roots: config.lint.source_paths.unwrap_or_default(),
                    project_root: project_root.clone(),
                    complete: root_complete && config_complete,
                }
            })
            .clone();
        resolution.complete &= root_complete;
        resolution
    }
}

fn workspace_settings_for_path<'a>(
    context: &'a WorkspaceFunctionContext,
    path: &Path,
) -> Option<&'a WorkspaceSettingsSnapshot> {
    context
        .workspace_settings
        .iter()
        .filter_map(|workspace| {
            [Some(&workspace.root), workspace.canonical_root.as_ref()]
                .into_iter()
                .flatten()
                .filter(|root| path.starts_with(root))
                .map(|root| root.components().count())
                .max()
                .map(|length| (workspace, length))
        })
        .max_by_key(|(_, length)| *length)
        .map(|(workspace, _)| workspace)
}

#[derive(Clone, PartialEq, Eq)]
struct DependencyFingerprint {
    canonical_path: PathBuf,
    is_file: bool,
    content_hash: Option<[u8; 32]>,
    directory_entries: Option<Vec<PathBuf>>,
    source_paths: SourcePathResolution,
}

struct WorkspaceSourceSnapshot {
    canonical_path: PathBuf,
    is_file: bool,
    source: Option<Arc<str>>,
    content_hash: Option<[u8; 32]>,
}

pub(crate) struct WorkspacePathProvider<'a> {
    context: &'a WorkspaceFunctionContext,
    source_paths: std::cell::RefCell<SourcePathsCache>,
    open_sources: BTreeMap<PathBuf, &'a str>,
    sources: std::cell::RefCell<BTreeMap<PathBuf, Arc<WorkspaceSourceSnapshot>>>,
    retained_models: std::cell::Cell<usize>,
    retained_source_bytes: std::cell::Cell<usize>,
}

impl<'a> WorkspacePathProvider<'a> {
    fn retain_analysis(&self, analysis: Arc<WorkspaceFileAnalysis>) -> Arc<WorkspaceFileAnalysis> {
        let Some(model) = &analysis.model else {
            return analysis;
        };
        let bytes = self
            .retained_source_bytes
            .get()
            .saturating_add(analysis.source.len());
        if !model.source_refs().is_empty()
            && self.retained_models.get() < MAX_RETAINED_MODELS
            && bytes <= MAX_RETAINED_MODEL_SOURCE_BYTES
        {
            self.retained_models.set(self.retained_models.get() + 1);
            self.retained_source_bytes.set(bytes);
            analysis
        } else {
            Arc::new(WorkspaceFileAnalysis {
                source: analysis.source.clone(),
                model: None,
                line_index: analysis.line_index.clone(),
                content_hash: analysis.content_hash,
            })
        }
    }

    fn snapshot(&self, path: &Path) -> Arc<WorkspaceSourceSnapshot> {
        if let Some(snapshot) = self.sources.borrow().get(path) {
            return snapshot.clone();
        }
        let canonical_path = canonical_path(path);
        let overlay = self.open_sources.get(&canonical_path);
        let is_file = overlay.is_some() || path.is_file();
        let source: Option<Arc<str>> = overlay
            .map(|source| Arc::from(*source))
            .or_else(|| std::fs::read_to_string(path).ok().map(Arc::from));
        let content_hash = source
            .as_ref()
            .map(|source| content_hash(source.as_bytes()));
        let snapshot = Arc::new(WorkspaceSourceSnapshot {
            canonical_path,
            is_file,
            source,
            content_hash,
        });
        self.sources
            .borrow_mut()
            .insert(path.to_path_buf(), snapshot.clone());
        snapshot
    }

    fn source(&self, path: &Path) -> Option<Arc<str>> {
        self.snapshot(path).source.clone()
    }

    fn fingerprint(&self, path: &Path) -> DependencyFingerprint {
        let snapshot = self.snapshot(path);
        DependencyFingerprint {
            canonical_path: snapshot.canonical_path.clone(),
            is_file: snapshot.is_file,
            content_hash: snapshot.content_hash,
            directory_entries: (!snapshot.is_file)
                .then(|| self.directory_entries(path))
                .flatten(),
            source_paths: self.source_paths.borrow_mut().resolve(path, self.context),
        }
    }

    pub(crate) fn new(context: &'a WorkspaceFunctionContext) -> Self {
        Self {
            context,
            sources: Default::default(),
            retained_models: Default::default(),
            retained_source_bytes: Default::default(),
            source_paths: std::cell::RefCell::new(SourcePathsCache::default()),
            open_sources: context
                .open_documents
                .iter()
                .filter_map(|open| {
                    Some((
                        canonical_path(&open.uri.to_file_path().ok()?),
                        open.document.contents(),
                    ))
                })
                .collect(),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Home directory override for tests on this thread; see [`with_test_home_dir`].
    static TEST_HOME_DIR: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `home` as the home directory that workspace source
/// resolution on this thread sees (`~/...` operands and the seeded `HOME`).
///
/// Tests use this instead of assigning `HOME`: the environment is process
/// wide and the test runner is parallel, so an environment change would race
/// with every other test reading it.
#[cfg(test)]
pub(crate) fn with_test_home_dir<T>(home: &Path, f: impl FnOnce() -> T) -> T {
    let previous = TEST_HOME_DIR.with(|cell| cell.replace(Some(home.to_path_buf())));
    let result = f();
    TEST_HOME_DIR.with(|cell| *cell.borrow_mut() = previous);
    result
}

impl shucked_semantic::SourcePathFileProvider for WorkspacePathProvider<'_> {
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        let resolution = self.source_paths.borrow_mut().resolve(from, self.context);
        shucked_semantic::source_candidate_paths(
            from,
            candidate,
            &resolution.roots,
            &resolution.project_root,
        )
    }

    /// Production builds keep the trait default (the process environment);
    /// tests can pin the home directory per thread.
    #[cfg(test)]
    fn home_dir(&self) -> Option<PathBuf> {
        TEST_HOME_DIR
            .with(|cell| cell.borrow().clone())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }

    /// A pinned test home directory stands in for the whole process
    /// environment, so `ZDOTDIR` or `XDG_*` values of the machine running the
    /// tests cannot leak into the expectations.
    #[cfg(test)]
    fn environment_variable(&self, name: &str) -> Option<String> {
        if TEST_HOME_DIR.with(|cell| cell.borrow().is_some()) {
            return None;
        }
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    fn read_source(&self, path: &Path) -> Option<String> {
        self.source(path).map(|source| source.to_string())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.snapshot(path).is_file
    }

    fn open_paths_in(&self, path: &Path) -> Vec<PathBuf> {
        let directory = canonical_path(path);
        self.open_sources
            .keys()
            .filter(|path| path.parent() == Some(directory.as_path()))
            .filter_map(|open| open.file_name().map(|name| path.join(name)))
            .collect()
    }

    fn is_cancelled(&self) -> bool {
        self.context.cancellation.is_cancelled()
    }
}

struct WorkspaceFileInput<'a> {
    path: &'a Path,
    uri: types::Url,
    source: &'a str,
    version: Option<DocumentVersion>,
}

/// A file analysed and projected for the index but not yet inserted.
struct PreparedWorkspaceFile {
    key: PathBuf,
    file: IndexedWorkspaceFile,
}

#[allow(clippy::too_many_arguments)]
fn insert_file(
    graph: &mut WorkspaceCallIndex,
    variables: &mut WorkspaceVariableIndex,
    files: &mut BTreeMap<PathBuf, IndexedWorkspaceFile>,
    previous: Option<&WorkspaceFunctionIndex>,
    input: WorkspaceFileInput<'_>,
    source_paths: &SourcePathResolution,
    path_analyzer: &mut shucked_semantic::SourcePathAnalyzer,
    path_provider: &WorkspacePathProvider<'_>,
) -> bool {
    let prepared = prepare_file(previous, input, source_paths, path_analyzer, path_provider);
    commit_file(graph, variables, files, prepared)
}

/// Analyses and projects one file, reusing the previous build's work when the
/// content and every dependency are unchanged. The compact projection is
/// retained across builds whether or not the file ends up in the index.
fn prepare_file(
    previous: Option<&WorkspaceFunctionIndex>,
    input: WorkspaceFileInput<'_>,
    source_paths: &SourcePathResolution,
    path_analyzer: &mut shucked_semantic::SourcePathAnalyzer,
    path_provider: &WorkspacePathProvider<'_>,
) -> PreparedWorkspaceFile {
    let key = canonical_path(input.path);
    let open_uri = input.version.is_some().then(|| input.uri.clone());
    let uri = std::fs::canonicalize(input.path)
        .ok()
        .and_then(|path| types::Url::from_file_path(path).ok())
        .unwrap_or(input.uri);
    let retained = path_provider
        .context
        .cache
        .projections
        .lock()
        .ok()
        .and_then(|files| files.get(&key).cloned());
    let previous = previous
        .and_then(|index| index.files.get(&key))
        .filter(|file| file.source() == input.source)
        .or(retained.as_ref());
    let analysis = previous
        .filter(|file| file.source() == input.source)
        .map(|file| file.analysis.clone())
        .unwrap_or_else(|| {
            Arc::new(WorkspaceFileAnalysis {
                model: Some(Arc::new(analyze_editor_document(
                    input.source,
                    Some(input.path),
                    ShellDialect::infer(input.source, Some(input.path)),
                ))),
                source: Arc::from(input.source),
                line_index: LineIndex::new(input.source),
                content_hash: content_hash(input.source.as_bytes()),
            })
        });
    let projection = previous
        .filter(|file| {
            Arc::ptr_eq(&file.analysis, &analysis)
                && file.projection.complete
                && file.projection.source_paths == *source_paths
                && file
                    .projection
                    .dependencies
                    .iter()
                    .all(|(path, fingerprint)| {
                        !path_provider.context.cancellation.is_cancelled()
                            && path_provider.fingerprint(path) == *fingerprint
                    })
        })
        .map(|file| file.projection.clone())
        .unwrap_or_else(|| {
            let model = analysis.model.clone().unwrap_or_else(|| {
                Arc::new(analyze_editor_document(
                    input.source,
                    Some(input.path),
                    ShellDialect::infer(input.source, Some(input.path)),
                ))
            });
            Arc::new(project_file(
                &model,
                input.source,
                input.path,
                source_paths,
                path_analyzer,
                path_provider,
            ))
        });
    let analysis = path_provider.retain_analysis(analysis);
    let file = IndexedWorkspaceFile {
        analysis,
        projection,
        uri,
        open_uri,
        version: input.version,
    };
    if let Ok(mut retained) = path_provider.context.cache.projections.lock() {
        let mut compact = file.clone();
        // Progress retention must not accumulate heavyweight semantic models
        // across cancelled builds; validated compact projections are sufficient.
        compact.analysis = Arc::new(WorkspaceFileAnalysis {
            source: file.analysis.source.clone(),
            model: None,
            line_index: file.analysis.line_index.clone(),
            content_hash: file.analysis.content_hash,
        });
        retained.insert(key.clone(), compact);
        // Retain interrupted build progress while bounding churn across workspace edits.
        while retained.len() > path_provider.context.max_files.saturating_mul(2).max(1) {
            retained.pop_first();
        }
    }
    PreparedWorkspaceFile { key, file }
}

/// Inserts a prepared file into the graph, the variable index and the file
/// table, returning whether its source analysis was complete.
fn commit_file(
    graph: &mut WorkspaceCallIndex,
    variables: &mut WorkspaceVariableIndex,
    files: &mut BTreeMap<PathBuf, IndexedWorkspaceFile>,
    prepared: PreparedWorkspaceFile,
) -> bool {
    let PreparedWorkspaceFile { key, file } = prepared;
    let complete = file.projection.complete;
    variables.insert_facts(key.clone(), file.projection.variables.clone());
    graph.insert(key.clone(), file.projection.calls.clone());
    files.insert(key, file);
    complete
}

fn project_file(
    model: &SemanticModel,
    source: &str,
    path: &Path,
    source_paths: &SourcePathResolution,
    path_analyzer: &mut shucked_semantic::SourcePathAnalyzer,
    path_provider: &WorkspacePathProvider<'_>,
) -> WorkspaceFileProjection {
    let mut dependencies = vec![path.to_path_buf()];
    let mut resolved_paths = path_analyzer.resolve(model, path, path_provider);
    dependencies.extend(resolved_paths.dependency_paths().cloned());

    // Framework loads: a bootstrap `source` loads the framework's own files,
    // and plugin or module statements load their entrypoints. Both are known
    // from the framework's layout rather than from the operand text.
    let framework_loads =
        crate::handlers::zsh_frameworks::framework_loads(model, source, path, path_provider);
    let mut framework_sequences = BTreeMap::new();
    let mut framework_edges = Vec::new();
    let mut sources = Vec::new();
    let mut complete = true;
    for load in framework_loads {
        dependencies.extend(load.dependencies.iter().cloned());
        complete &= !load.truncated;
        let files = load
            .files
            .iter()
            .map(|file| canonical_path(file))
            .collect::<Vec<_>>();
        if let Some(source_ref) = model
            .source_refs()
            .iter()
            .find(|source_ref| source_ref.span.start.offset() == load.span.start.offset())
        {
            resolved_paths.insert_sequence(source_ref, files);
            framework_sequences.insert(source_ref.span.start.offset(), load.framework);
            continue;
        }
        let scope = model.scope_at(load.span.start.offset());
        let conditional = model.flow_context_at(&load.span).is_some_and(|context| {
            context.in_block || context.loop_depth > 0 || context.in_subshell
        });
        sources.push(WorkspaceSourceDetails {
            span: load.span,
            path_span: load.span,
            directive_span: None,
            target: None,
            candidates: Vec::new(),
            sequence: Some(files.clone()),
            reason: SourceResolutionReason::Resolved,
            conditional,
            in_function: model.enclosing_function_scope(scope).is_some(),
            framework: Some(load.framework),
        });
        framework_edges.extend(files.into_iter().map(|file| {
            CallFactSourceEdge {
                path: file,
                span: load.span,
                conditional,
                completion_visible: !conditional
                    && model.enclosing_function_scope(scope).is_none()
                    && model
                        .innermost_transient_scope_within_function(scope)
                        .is_none(),
            }
        }));
    }
    // Inside a framework's own bootstrap the dynamic loads are the
    // framework's contract, already attached to the statement that sources
    // it; they must not invalidate the environment a second time.
    let bootstrap_contract = crate::handlers::zsh_frameworks::bootstrap_file_framework(path);

    let mut edges = framework_edges;
    for source_ref in model.source_refs() {
        let scope = model.scope_at(source_ref.span.start.offset());
        let in_function = model.enclosing_function_scope(scope).is_some();
        let completion_visible = !source_ref.conditionally_executed
            && !in_function
            && model
                .innermost_transient_scope_within_function(scope)
                .is_none();
        if let Some(sequence) = resolved_paths.sequence(source_ref) {
            let sequence = sequence
                .iter()
                .map(|path| canonical_path(path))
                .collect::<Vec<_>>();
            let framework = framework_sequences
                .get(&source_ref.span.start.offset())
                .cloned();
            // A loop's iterations are conditional; a framework bootstrap is
            // as conditional as the statement itself.
            let conditional = framework.is_none() || source_ref.conditionally_executed;
            sources.push(WorkspaceSourceDetails {
                span: source_ref.span,
                path_span: source_ref.path_span,
                directive_span: source_ref.directive_path_span,
                target: None,
                candidates: Vec::new(),
                sequence: Some(sequence.clone()),
                reason: SourceResolutionReason::Resolved,
                conditional,
                in_function,
                framework,
            });
            edges.extend(sequence.into_iter().map(|path| CallFactSourceEdge {
                path,
                span: source_ref.span,
                conditional,
                completion_visible: completion_visible && !conditional,
            }));
            continue;
        }
        let mut candidates = if let Some(candidate) = resolved_paths.candidate(source_ref) {
            candidate.map(PathBuf::from).into_iter().collect()
        } else {
            path_provider.source_ref_candidates(path, source_ref)
        };
        if resolved_paths.candidate(source_ref).is_none()
            && candidates.is_empty()
            && let Some(candidate) = model.current_file_source_candidate(source_ref, path)
        {
            candidates.push(candidate);
        }
        let mut checked = Vec::new();
        let target = candidates
            .into_iter()
            .inspect(|candidate| {
                dependencies.push(candidate.clone());
                checked.push(candidate.clone());
            })
            .find_map(|candidate| {
                let snapshot = path_provider.snapshot(&candidate);
                snapshot.is_file.then(|| snapshot.canonical_path.clone())
            });
        let ignored = matches!(source_ref.kind, SourceRefKind::DirectiveDevNull);
        if target.is_none()
            && !ignored
            && let Some(framework) = &bootstrap_contract
        {
            resolved_paths.insert_sequence(source_ref, Vec::new());
            sources.push(WorkspaceSourceDetails {
                span: source_ref.span,
                path_span: source_ref.path_span,
                directive_span: source_ref.directive_path_span,
                target: None,
                candidates: checked,
                sequence: Some(Vec::new()),
                reason: SourceResolutionReason::Framework,
                conditional: source_ref.conditionally_executed,
                in_function,
                framework: Some(framework.clone()),
            });
            continue;
        }
        let reason = if ignored {
            SourceResolutionReason::Ignored
        } else if let Some(target) = &target {
            if path_provider.source(target).is_some() {
                SourceResolutionReason::Resolved
            } else {
                SourceResolutionReason::Unreadable
            }
        } else if !resolved_paths.is_complete() {
            SourceResolutionReason::AnalysisLimit
        } else if matches!(resolved_paths.candidate(source_ref), Some(None))
            || (checked.is_empty()
                && matches!(
                    source_ref.kind,
                    SourceRefKind::SingleVariableStaticTail { .. }
                ))
        {
            SourceResolutionReason::UnknownValue
        } else if checked.is_empty() {
            SourceResolutionReason::Dynamic
        } else {
            SourceResolutionReason::Missing
        };
        sources.push(WorkspaceSourceDetails {
            span: source_ref.span,
            path_span: source_ref.path_span,
            directive_span: source_ref.directive_path_span,
            target: target.clone(),
            candidates: checked,
            sequence: None,
            reason,
            conditional: source_ref.conditionally_executed,
            in_function,
            framework: None,
        });
        edges.extend(target.into_iter().map(|target| CallFactSourceEdge {
            path: target,
            span: source_ref.span,
            conditional: source_ref.conditionally_executed,
            completion_visible,
        }));
    }
    let call_facts = FileCallFacts::project_with_source_edges(model, edges);
    let variables = FileVariableFacts::project(
        model,
        &resolved_paths.variable_effects(&call_facts.source_effects),
    );
    WorkspaceFileProjection {
        source_paths: source_paths.clone(),
        dependencies: dependencies
            .into_iter()
            .map(|path| {
                let fingerprint = path_provider.fingerprint(&path);
                (path, fingerprint)
            })
            .collect(),
        functions: Arc::new(shucked_semantic::FileFunctionEffects::project(
            model,
            &call_facts,
            &resolved_paths,
        )),
        calls: call_facts,
        sources,
        variables,
        complete: complete && resolved_paths.is_complete(),
    }
}

/// Shell startup files that may load workspace files: the user's zsh
/// startup files in the resolved `ZDOTDIR` (and `~/.zshenv`, which selects
/// it) plus the bash and POSIX login and interactive files in the home
/// directory. Only these well-known names are inspected; whatever they source
/// joins through the normal source-edge expansion under the loader budget.
fn startup_loader_candidates(
    provider: &WorkspacePathProvider<'_>,
    path_analyzer: &mut shucked_semantic::SourcePathAnalyzer,
) -> Vec<PathBuf> {
    let Some(home) = provider.home_dir() else {
        return Vec::new();
    };
    let zdotdir = path_analyzer
        .zsh_startup_directory(provider)
        .unwrap_or_else(|| home.clone());
    let mut candidates = Vec::new();
    let mut push = |path: PathBuf| {
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    };
    push(home.join(".zshenv"));
    for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
        push(zdotdir.join(name));
    }
    for name in [".bash_profile", ".bash_login", ".profile", ".bashrc"] {
        push(home.join(name));
    }
    candidates
}

struct ClosedFileDiscovery {
    files: Vec<PathBuf>,
    complete: bool,
    limited: bool,
}

fn discover_closed_shell_files(
    roots: &[PathBuf],
    open_paths: &BTreeSet<PathBuf>,
    max_files: usize,
    cancellation: &RequestCancellationToken,
) -> Option<ClosedFileDiscovery> {
    use shucked_discover::{DiscoveryOptions, FileKind, discover_files};

    let mut files = BTreeSet::new();
    let mut complete = true;
    for root in roots {
        if cancellation.is_cancelled() {
            return None;
        }
        let discovered = match discover_files(
            std::slice::from_ref(root),
            root,
            &DiscoveryOptions {
                respect_gitignore: true,
                parallel: true,
                use_config_roots: true,
                ..DiscoveryOptions::default()
            },
        ) {
            Ok(files) => files,
            Err(error) => {
                complete = false;
                tracing::warn!(
                    "workspace functions: failed to discover files in {}: {error}",
                    root.display()
                );
                continue;
            }
        };
        for file in discovered {
            if cancellation.is_cancelled() {
                return None;
            }
            if file.kind != FileKind::Shell {
                continue;
            }
            let path = canonical_path(&file.absolute_path);
            if open_paths.contains(&path) {
                continue;
            }
            files.insert(path);
        }
    }
    if files.len() > max_files {
        complete = false;
        tracing::warn!(
            "workspace functions: workspace has {} closed shell files; indexing only {max_files}",
            files.len()
        );
    }
    Some(ClosedFileDiscovery {
        limited: files.len() > max_files,
        files: files.into_iter().take(max_files).collect(),
        complete,
    })
}

pub(crate) fn canonical_path(path: &Path) -> PathBuf {
    shucked_semantic::canonical_workspace_path(path)
}

fn content_hash(contents: &[u8]) -> [u8; 32] {
    Sha256::digest(contents).into()
}

#[cfg(test)]
#[path = "../../tests/requests/completion_index.rs"]
mod completion_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextDocument;
    use shucked_config::LintConfig;

    fn context_for(workspace: &Path, max_files: usize) -> WorkspaceFunctionContext {
        let open_path = workspace.join("open_a.sh");
        let open_doc = WorkspaceOpenDocument {
            uri: types::Url::from_file_path(&open_path).unwrap(),
            document: Arc::new(
                TextDocument::new(
                    "# shucked: source=vendored/edge.sh\nsource \"$DIR/edge.sh\"\nedge_fn\n"
                        .to_owned(),
                    1,
                )
                .with_language_id("shellscript"),
            ),
        };
        let open_b = WorkspaceOpenDocument {
            uri: types::Url::from_file_path(workspace.join("open_b.sh")).unwrap(),
            document: Arc::new(
                TextDocument::new("b() { :; }\n".to_owned(), 1).with_language_id("shellscript"),
            ),
        };
        WorkspaceFunctionContext {
            workspace_roots: vec![workspace.to_path_buf()],
            settings_workspace_roots: vec![workspace.to_path_buf()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: vec![open_doc, open_b],
            encoding: PositionEncoding::UTF16,
            max_files,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        }
    }

    fn populate_workspace(workspace: &Path) {
        std::fs::write(workspace.join("open_a.sh"), "stale() { :; }\n").unwrap();
        std::fs::write(workspace.join("open_b.sh"), "stale() { :; }\n").unwrap();
        for index in 0..4 {
            std::fs::write(
                workspace.join(format!("closed_{index}.sh")),
                "closed() { :; }\n",
            )
            .unwrap();
        }
        std::fs::create_dir_all(workspace.join("vendored")).unwrap();
        std::fs::write(workspace.join(".gitignore"), "vendored/\n").unwrap();
        std::fs::write(workspace.join("vendored/edge.sh"), "edge_fn() { :; }\n").unwrap();
    }

    #[test]
    fn max_files_is_a_hard_bound_and_marks_partial_builds() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        populate_workspace(&workspace);

        for max_files in [1, 3, 5] {
            let context = context_for(&workspace, max_files);
            let built = WorkspaceFunctionIndex::build(&context).unwrap();
            assert!(built.file_count() <= max_files);
            assert!(!built.is_complete());
        }
    }

    #[test]
    fn incomplete_indexes_still_navigate_to_known_variable_definitions() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        populate_workspace(&workspace);

        let context = context_for(&workspace, 1);
        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        assert!(!built.is_complete());

        let path = workspace.join("open_a.sh");
        let model = analyze_editor_document("SHARED=1\n", Some(&path), ShellDialect::Bash);
        let symbol = model
            .editor_query()
            .target_at_offset(1)
            .expect("the assignment should be an editor target");
        let target = crate::workspace_variables::variable_target(&model, &symbol)
            .expect("the assignment should be a file variable");

        // A partial workspace index limits cross-file certainty, but it must
        // not fail closed: the query answers with what the index knows (here
        // nothing, because the indexed buffer does not contain the assignment)
        // and the request handler then falls back to the document's own binding.
        let locations = built
            .variable_definition_locations(&path, &target, &context.cancellation)
            .expect("navigation should not fail closed on a partial index");
        assert!(locations.is_empty(), "{locations:?}");
    }

    #[test]
    fn generous_limit_indexes_open_discovered_and_edge_targets() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        populate_workspace(&workspace);

        let built = WorkspaceFunctionIndex::build(&context_for(&workspace, 100)).unwrap();
        assert_eq!(built.file_count(), 7);
        assert!(built.contains(&workspace.join("vendored/edge.sh")));
        assert!(built.is_complete());
    }

    #[test]
    fn open_unsaved_source_target_participates_without_disk_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        let caller_path = workspace.join("caller.sh");
        let target_path = workspace.join("new_target.sh");
        let caller_source = "source new_target.sh\nfrom_buffer\n";
        let caller = WorkspaceOpenDocument {
            uri: types::Url::from_file_path(&caller_path).unwrap(),
            document: Arc::new(
                TextDocument::new(caller_source.to_owned(), 1).with_language_id("shellscript"),
            ),
        };
        let target = WorkspaceOpenDocument {
            uri: types::Url::from_file_path(&target_path).unwrap(),
            document: Arc::new(
                TextDocument::new("from_buffer() { :; }\n".to_owned(), 2)
                    .with_language_id("shellscript"),
            ),
        };
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![workspace.clone()],
            settings_workspace_roots: vec![workspace.clone()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: vec![caller, target],
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };

        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        let caller_facts = built
            .graph
            .files()
            .find_map(|(path, facts)| (path == caller_path).then_some(facts))
            .unwrap();
        assert_eq!(caller_facts.source_edges.len(), 1);
        assert_eq!(caller_facts.source_edges[0].path, target_path);
        let target_facts = built
            .graph
            .files()
            .find_map(|(path, facts)| (path == target_path).then_some(facts))
            .unwrap();
        assert_eq!(target_facts.definitions.len(), 1);
        let call = caller_facts
            .call_sites
            .iter()
            .find(|call| call.callee.as_str() == "from_buffer")
            .unwrap();
        let call_span = call.name_span;
        let resolution = built.function_resolution(&caller_path, call_span);
        let resolved = resolution.exact().unwrap();
        assert_eq!(resolved.path, target_path);
    }

    #[cfg(unix)]
    #[test]
    fn open_unsaved_source_target_resolves_through_symlinked_workspace_root() {
        use std::os::unix::fs::symlink;

        let tempdir = tempfile::tempdir().unwrap();
        let real_workspace = tempdir.path().join("real");
        let linked_workspace = tempdir.path().join("linked");
        std::fs::create_dir(&real_workspace).unwrap();
        let real_workspace = std::fs::canonicalize(real_workspace).unwrap();
        symlink(&real_workspace, &linked_workspace).unwrap();

        let caller_path = linked_workspace.join("caller.sh");
        let target_path = linked_workspace.join("new_target.sh");
        std::fs::write(real_workspace.join("caller.sh"), "stale\n").unwrap();
        let caller = WorkspaceOpenDocument {
            uri: types::Url::from_file_path(&caller_path).unwrap(),
            document: Arc::new(
                TextDocument::new("source new_target.sh\nfrom_buffer\n".to_owned(), 1)
                    .with_language_id("shellscript"),
            ),
        };
        let target_uri = types::Url::from_file_path(&target_path).unwrap();
        let target = WorkspaceOpenDocument {
            uri: target_uri.clone(),
            document: Arc::new(
                TextDocument::new("from_buffer() { :; }\n".to_owned(), 2)
                    .with_language_id("shellscript"),
            ),
        };
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![linked_workspace.clone()],
            settings_workspace_roots: vec![linked_workspace.clone(), real_workspace.clone()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: vec![caller, target],
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };

        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        let canonical_caller = real_workspace.join("caller.sh");
        let canonical_target = real_workspace.join("new_target.sh");
        let facts = built
            .graph
            .files()
            .find_map(|(path, facts)| (path == canonical_caller).then_some(facts))
            .unwrap();
        let call = facts
            .call_sites
            .iter()
            .find(|call| call.callee.as_str() == "from_buffer")
            .unwrap();
        assert_eq!(
            built
                .function_resolution(&canonical_caller, call.name_span)
                .exact()
                .unwrap()
                .path,
            canonical_target
        );
        assert_eq!(built.file(&canonical_target).unwrap().uri(), &target_uri);
    }

    #[cfg(unix)]
    #[test]
    fn existing_open_target_retains_the_editors_symlink_uri() {
        use std::os::unix::fs::symlink;

        let tempdir = tempfile::tempdir().unwrap();
        let real_workspace = tempdir.path().join("real");
        let linked_workspace = tempdir.path().join("linked");
        std::fs::create_dir(&real_workspace).unwrap();
        let real_workspace = std::fs::canonicalize(real_workspace).unwrap();
        std::fs::write(
            real_workspace.join("caller.sh"),
            "source target.sh\ntarget\n",
        )
        .unwrap();
        std::fs::write(real_workspace.join("target.sh"), "stale() { :; }\n").unwrap();
        symlink(&real_workspace, &linked_workspace).unwrap();

        let caller_uri = types::Url::from_file_path(linked_workspace.join("caller.sh")).unwrap();
        let target_uri = types::Url::from_file_path(linked_workspace.join("target.sh")).unwrap();
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![linked_workspace.clone()],
            settings_workspace_roots: vec![linked_workspace, real_workspace.clone()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: vec![
                WorkspaceOpenDocument {
                    uri: caller_uri,
                    document: Arc::new(
                        TextDocument::new("source target.sh\ntarget\n".to_owned(), 1)
                            .with_language_id("shellscript"),
                    ),
                },
                WorkspaceOpenDocument {
                    uri: target_uri.clone(),
                    document: Arc::new(
                        TextDocument::new("target() { :; }\n".to_owned(), 2)
                            .with_language_id("shellscript"),
                    ),
                },
            ],
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };

        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        let file = built.file(&real_workspace.join("target.sh")).unwrap();
        assert_ne!(file.uri(), &target_uri);
        assert_eq!(file.editor_uri(), &target_uri);
    }

    #[test]
    fn closed_file_freshness_uses_indexed_content_hash() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        let file = workspace.join("closed.sh");
        std::fs::write(&file, "one() { :; }\n").unwrap();
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![workspace.clone()],
            settings_workspace_roots: vec![workspace.clone()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: Vec::new(),
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };
        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        assert!(built.closed_file_is_current(&file));
        assert_eq!(
            built.validate_closed_files(&context.cancellation),
            Some(Ok(()))
        );
        std::fs::write(&file, "two() { :; }\n").unwrap();
        assert!(!built.closed_file_is_current(&file));
        assert_eq!(
            built.validate_closed_files(&context.cancellation),
            Some(Err(file))
        );
    }

    #[test]
    fn workspace_source_paths_layer_over_global_client_options() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        std::fs::create_dir(workspace.join("scripts")).unwrap();
        std::fs::create_dir(workspace.join("lib")).unwrap();
        let caller = workspace.join("scripts/main.sh");
        let target = workspace.join("lib/util.sh");
        std::fs::write(&caller, "source util.sh\nfrom_root\n").unwrap();
        std::fs::write(&target, "from_root() { :; }\n").unwrap();

        let global_options = ClientOptions {
            lint: Some(LintConfig {
                source_paths: Some(vec!["missing".to_owned()]),
                ..LintConfig::default()
            }),
            ..ClientOptions::default()
        };
        let workspace_options = ClientOptions {
            lint: Some(LintConfig {
                source_paths: Some(vec!["lib".to_owned()]),
                ..LintConfig::default()
            }),
            ..ClientOptions::default()
        };
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![workspace.clone()],
            settings_workspace_roots: vec![workspace.clone()],
            workspace_settings: vec![WorkspaceSettingsSnapshot {
                root: workspace.clone(),
                canonical_root: Some(workspace.clone()),
                options: Some(workspace_options),
            }],
            global_options,
            open_documents: Vec::new(),
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };

        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        let facts = built
            .graph
            .files()
            .find_map(|(path, facts)| (path == caller).then_some(facts))
            .unwrap();
        let call = facts
            .call_sites
            .iter()
            .find(|call| call.callee.as_str() == "from_root")
            .unwrap();
        assert_eq!(
            built
                .function_resolution(&caller, call.name_span)
                .exact()
                .unwrap()
                .path,
            target
        );
    }

    #[test]
    fn malformed_project_config_marks_the_index_incomplete() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        std::fs::write(workspace.join(".shucked.toml"), "[lint\n").unwrap();
        std::fs::write(workspace.join("main.sh"), "main() { :; }\n").unwrap();
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![workspace.clone()],
            settings_workspace_roots: vec![workspace.clone()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: Vec::new(),
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };

        let resolution = SourcePathsCache::default().resolve(&workspace.join("main.sh"), &context);
        assert!(!resolution.complete);
        let built = WorkspaceFunctionIndex::build(&context).unwrap();
        assert!(!built.is_complete());
    }

    #[cfg(unix)]
    #[test]
    fn project_root_resolution_failure_marks_source_paths_incomplete() {
        use std::os::unix::fs::symlink;

        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        std::fs::write(workspace.join(".shucked.toml"), "").unwrap();
        let loop_path = workspace.join("loop");
        symlink("loop", &loop_path).unwrap();
        let context = WorkspaceFunctionContext {
            workspace_roots: vec![workspace.clone()],
            settings_workspace_roots: vec![workspace.clone()],
            workspace_settings: Vec::new(),
            global_options: ClientOptions::default(),
            open_documents: Vec::new(),
            encoding: PositionEncoding::UTF16,
            max_files: 100,
            cache: Arc::new(WorkspaceFunctionIndexCache::default()),
            epoch: 0,
            cancellation: RequestCancellationToken::default(),
        };

        let mut source_paths = SourcePathsCache::default();
        assert!(
            source_paths
                .resolve(&workspace.join("main.sh"), &context)
                .complete
        );
        let resolution = source_paths.resolve(&loop_path.join("main.sh"), &context);
        assert!(!resolution.complete);
    }

    #[test]
    fn cancelled_build_does_not_scan_or_populate_the_cache() {
        let tempdir = tempfile::tempdir().unwrap();
        let workspace = std::fs::canonicalize(tempdir.path()).unwrap();
        populate_workspace(&workspace);
        let context = context_for(&workspace, 100);
        context.cancellation.cancel();

        assert!(workspace_function_index(&context).is_none());
        assert!(context.cache.get(context.epoch).is_none());
    }
}

#[cfg(test)]
#[path = "../../tests/unit/workspace_incremental.rs"]
mod incremental_tests;

#[cfg(test)]
#[path = "../../tests/unit/workspace_loaders.rs"]
mod loader_tests;
