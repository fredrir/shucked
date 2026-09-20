//! Shared workspace index for cross-file function and variable editor features.
//!
//! The index projects each shell file into compact semantic function and
//! variable facts, resolves determinable `source` edges, and retains just
//! enough source metadata to turn byte spans back into LSP ranges. Open
//! buffers shadow disk content. File analysis survives workspace invalidation;
//! source effects are reused only while their content and dependencies match.

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
    FileCallFacts, FileVariableFacts, SemanticModel, VisibleSourcedFunction, WorkspaceCallIndex,
    source_ref_candidate_paths,
};

use crate::PositionEncoding;
use crate::edit::DocumentVersion;
use crate::editor::analyze_editor_document;
use crate::session::{ClientOptions, RequestCancellationToken, WorkspaceSettingsSnapshot};
use crate::symbols::WorkspaceOpenDocument;
use crate::workspace_variables::{WorkspaceVariableIndex, WorkspaceVariableTarget};

const MAX_RETAINED_MODELS: usize = 128;
const MAX_RETAINED_MODEL_SOURCE_BYTES: usize = 16 * 1024 * 1024;

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
}

impl WorkspaceFunctionIndexCache {
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
        return Some(built);
    }
    let built = Arc::new(WorkspaceFunctionIndex::build(context)?);
    if context.cancellation.is_cancelled() || context.cache.current_epoch() != context.epoch {
        return None;
    }
    context.cache.store(context.epoch, built.clone());
    Some(built)
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

/// Shared index queried by cross-file editor features.
pub(crate) struct WorkspaceFunctionIndex {
    graph: WorkspaceCallIndex,
    variables: WorkspaceVariableIndex,
    variable_usage: OnceLock<Arc<shucked_semantic::WorkspaceVariableUsage>>,
    files: BTreeMap<PathBuf, IndexedWorkspaceFile>,
    encoding: PositionEncoding,
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
        let previous = context.cache.previous();
        let mut graph = WorkspaceCallIndex::new();
        let mut variables = WorkspaceVariableIndex::default();
        let mut files = BTreeMap::new();
        let mut complete = true;
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
            if context.cancellation.is_cancelled() {
                return None;
            }
            if graph.file_count() >= max_files {
                complete = false;
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

        if context.cancellation.is_cancelled() {
            return None;
        }
        let remaining = max_files.saturating_sub(graph.file_count());
        let discovery = discover_closed_shell_files(
            &context.workspace_roots,
            &open_paths,
            remaining,
            &context.cancellation,
        )?;
        complete &= discovery.complete;
        for file in discovery.files {
            if context.cancellation.is_cancelled() {
                return None;
            }
            let Some(source) = path_provider.source(&file) else {
                complete = false;
                continue;
            };
            let Ok(uri) = types::Url::from_file_path(&file) else {
                complete = false;
                continue;
            };
            let resolution = path_provider
                .source_paths
                .borrow_mut()
                .resolve(&file, context);
            complete &= resolution.complete;
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

        'expand: loop {
            if context.cancellation.is_cancelled() {
                return None;
            }
            let missing = graph
                .files()
                .flat_map(|(_, facts)| facts.source_edges.iter().map(|edge| edge.path.clone()))
                .filter(|target| !graph.contains(target))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            if missing.is_empty() {
                break;
            }
            for target in missing {
                if context.cancellation.is_cancelled() {
                    return None;
                }
                if graph.file_count() >= max_files {
                    complete = false;
                    tracing::warn!(
                        "workspace functions: source-edge targets exceed the {max_files}-file \
                         limit; cross-file results may be incomplete"
                    );
                    break 'expand;
                }
                let Some(open) = open_docs
                    .iter()
                    .find_map(|(path, open)| (path == &target).then_some(*open))
                else {
                    let Some(source) = path_provider.source(&target) else {
                        complete = false;
                        graph.insert(target.clone(), FileCallFacts::default());
                        continue;
                    };
                    let Ok(uri) = types::Url::from_file_path(&target) else {
                        complete = false;
                        graph.insert(target.clone(), FileCallFacts::default());
                        continue;
                    };
                    let resolution = path_provider
                        .source_paths
                        .borrow_mut()
                        .resolve(&target, context);
                    complete &= resolution.complete;
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

        Some(Self {
            variable_usage: OnceLock::new(),
            graph,
            variables,
            files,
            encoding: context.encoding,
            complete,
        })
    }

    pub(crate) fn resolve_call_site(
        &self,
        from_path: &Path,
        name_span: Span,
    ) -> Option<CrossFileCall> {
        self.graph.resolve_call_site(from_path, name_span)
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

    pub(crate) fn exact_function_reference_locations(
        &self,
        target_path: &Path,
        target_node: &CallNodeKind,
        cancellation: &RequestCancellationToken,
    ) -> Option<Vec<types::Location>> {
        if !self.complete || cancellation.is_cancelled() {
            return None;
        }
        let references = self
            .graph
            .exact_function_references(target_path, target_node, || cancellation.is_cancelled())?;
        let mut locations = Vec::with_capacity(references.len());
        for reference in references {
            if cancellation.is_cancelled() {
                return None;
            }
            let file = self.file(&reference.path)?;
            locations.push(types::Location {
                uri: file.editor_uri().clone(),
                range: crate::edit::to_lsp_range(
                    reference.span.to_range(),
                    file.source(),
                    file.line_index(),
                    self.encoding,
                ),
            });
        }
        Some(locations)
    }

    pub(crate) fn variable_definition_locations(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        cancellation: &RequestCancellationToken,
    ) -> Option<Vec<types::Location>> {
        if !self.complete || cancellation.is_cancelled() {
            return None;
        }
        let occurrences = self
            .variables
            .definitions(from_path, target, &|| cancellation.is_cancelled())?;
        self.variable_locations(occurrences, cancellation)
    }

    pub(crate) fn variable_reference_locations(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        include_declaration: bool,
        cancellation: &RequestCancellationToken,
    ) -> Option<Vec<types::Location>> {
        if !self.complete || cancellation.is_cancelled() {
            return None;
        }
        let occurrences =
            self.variables
                .references(from_path, target, include_declaration, &|| {
                    cancellation.is_cancelled()
                })?;
        self.variable_locations(occurrences, cancellation)
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

    pub(crate) fn visible_sourced_functions(
        &self,
        from_path: &Path,
        source_spans: &[Span],
    ) -> Vec<VisibleSourcedFunction> {
        self.graph
            .visible_sourced_functions_from_source_spans(from_path, source_spans)
    }

    pub(crate) fn incoming(
        &self,
        target_path: &Path,
        target_node: &CallNodeKind,
    ) -> Vec<CrossFileCall> {
        self.graph.incoming(target_path, target_node)
    }

    pub(crate) fn outgoing(
        &self,
        from_path: &Path,
        from_node: &CallNodeKind,
    ) -> Vec<CrossFileCall> {
        self.graph.outgoing(from_path, from_node)
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

    fn read_source(&self, path: &Path) -> Option<String> {
        self.source(path).map(|source| source.to_string())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.snapshot(path).is_file
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
    let key = canonical_path(input.path);
    let open_uri = input.version.is_some().then(|| input.uri.clone());
    let uri = std::fs::canonicalize(input.path)
        .ok()
        .and_then(|path| types::Url::from_file_path(path).ok())
        .unwrap_or(input.uri);
    let previous = previous.and_then(|index| index.files.get(&key));
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
                input.path,
                source_paths,
                path_analyzer,
                path_provider,
            ))
        });
    let analysis = path_provider.retain_analysis(analysis);
    let complete = projection.complete;
    variables.insert_facts(key.clone(), projection.variables.clone());
    graph.insert(key.clone(), projection.calls.clone());
    files.insert(
        key,
        IndexedWorkspaceFile {
            analysis,
            projection,
            uri,
            open_uri,
            version: input.version,
        },
    );
    complete
}

fn project_file(
    model: &SemanticModel,
    path: &Path,
    source_paths: &SourcePathResolution,
    path_analyzer: &mut shucked_semantic::SourcePathAnalyzer,
    path_provider: &WorkspacePathProvider<'_>,
) -> WorkspaceFileProjection {
    let mut dependencies = vec![path.to_path_buf()];
    let resolved_paths = path_analyzer.resolve(model, path, path_provider);
    dependencies.extend(resolved_paths.dependency_paths().cloned());
    let edges = model
        .source_refs()
        .iter()
        .filter_map(|source_ref| {
            let scope = model.scope_at(source_ref.span.start.offset());
            let mut candidates = if let Some(candidate) = resolved_paths.candidate(source_ref) {
                candidate.map(PathBuf::from).into_iter().collect()
            } else {
                source_ref_candidate_paths(
                    path,
                    source_ref,
                    &source_paths.roots,
                    &source_paths.project_root,
                )
            };
            if resolved_paths.candidate(source_ref).is_none()
                && candidates.is_empty()
                && let Some(candidate) = model.current_file_source_candidate(source_ref, path)
            {
                candidates.push(candidate);
            }
            candidates
                .into_iter()
                .inspect(|path| dependencies.push(path.clone()))
                .find_map(|candidate| {
                    let snapshot = path_provider.snapshot(&candidate);
                    snapshot.is_file.then(|| snapshot.canonical_path.clone())
                })
                .map(|target| CallFactSourceEdge {
                    path: target,
                    span: source_ref.span,
                    conditional: source_ref.conditionally_executed,
                    completion_visible: !source_ref.conditionally_executed
                        && model.enclosing_function_scope(scope).is_none()
                        && model
                            .innermost_transient_scope_within_function(scope)
                            .is_none(),
                })
        })
        .collect::<Vec<_>>();
    let call_facts = FileCallFacts::project_with_source_edges(model, edges);
    let variables = FileVariableFacts::project(model, &call_facts.source_effects);
    WorkspaceFileProjection {
        source_paths: source_paths.clone(),
        dependencies: dependencies
            .into_iter()
            .map(|path| {
                let fingerprint = path_provider.fingerprint(&path);
                (path, fingerprint)
            })
            .collect(),
        calls: call_facts,
        variables,
        complete: resolved_paths.is_complete(),
    }
}

struct ClosedFileDiscovery {
    files: Vec<PathBuf>,
    complete: bool,
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
    fn incomplete_indexes_do_not_return_variable_definitions() {
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

        assert!(
            built
                .variable_definition_locations(&path, &target, &context.cancellation)
                .is_none()
        );
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
        let resolved = built.resolve_call_site(&caller_path, call_span).unwrap();
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
                .resolve_call_site(&canonical_caller, call.name_span)
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
                .resolve_call_site(&caller, call.name_span)
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
