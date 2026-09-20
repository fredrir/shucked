use super::*;
use crate::{BindingKind, BuiltinCommandKind, CommandKind, SourceRef};

const MAX_FILES: usize = 128;
const MAX_DEPTH: usize = 32;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_EVENTS: usize = 65_536;

/// File access and source search policy for path-value analysis.
pub trait SourcePathFileProvider {
    /// Candidate paths in search order, including missing paths.
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf>;

    /// Source contents, including editor overlays when available.
    fn read_source(&self, path: &Path) -> Option<String> {
        fs::read_to_string(path).ok()
    }

    /// Whether the file exists on disk or in an editor overlay.
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    /// Whether the current request was cancelled.
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Source operands affected by preceding imports.
#[derive(Default)]
pub struct ResolvedSourcePaths {
    candidates: FxHashMap<SpanKey, Option<PathBuf>>,
    dependencies: FxHashSet<PathBuf>,
    incomplete: bool,
}

impl ResolvedSourcePaths {
    /// Whether analysis finished within its work bounds.
    pub fn is_complete(&self) -> bool {
        !self.incomplete
    }

    /// `Some(None)` means a preceding import prevents a reliable resolution.
    pub fn candidate(&self, reference: &SourceRef) -> Option<Option<&Path>> {
        self.candidates
            .get(&SpanKey::new(reference.span))
            .map(|path| path.as_deref())
    }

    /// Files and missing candidates consulted during resolution.
    pub fn dependency_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.dependencies.iter()
    }
}

#[derive(Clone)]
enum PathEvent {
    Write(Name, Option<SourcePathTemplate>),
    Source(SourceRef, Option<SourcePathTemplate>),
    Invalidate,
    DefineFunction(Name),
    Call(Name),
}

#[derive(Clone)]
struct PathFile {
    events: Vec<(usize, PathEvent)>,
    profile: ShellProfile,
    has_return: bool,
}

impl PathFile {
    fn project(model: &SemanticModel) -> Self {
        let persistent = |scope| {
            model.enclosing_function_scope(scope).is_none()
                && model
                    .innermost_transient_scope_within_function(scope)
                    .is_none()
        };
        let mut events = Vec::new();
        for binding in model.bindings() {
            if !persistent(binding.scope) || matches!(binding.kind, BindingKind::Imported) {
                continue;
            }
            let end = crate::editor::binding_definition_span(binding).end.offset();
            if matches!(binding.kind, BindingKind::FunctionDefinition) {
                events.push((end, PathEvent::DefineFunction(binding.name.clone())));
            } else if binding.attributes.contains(BindingAttributes::NAMEREF) {
                events.push((end, PathEvent::Invalidate));
            } else {
                events.push((
                    end,
                    PathEvent::Write(
                        binding.name.clone(),
                        model.source_path_expressions.get(&binding.id).cloned(),
                    ),
                ));
            }
        }
        for reference in model.source_refs() {
            if persistent(model.scope_at(reference.span.start.offset())) {
                let expression = model
                    .recorded_program()
                    .command_info_for_span(reference.span)
                    .and_then(|info| info.source_path_expression.clone());
                let mut reference = reference.clone();
                reference.conditionally_executed |= model
                    .recorded_program()
                    .command_info_for_span(reference.span)
                    .is_some_and(|info| info.source_path_environment_unknown);
                events.push((
                    reference.span.end.offset(),
                    PathEvent::Source(reference, expression),
                ));
            }
        }
        let mut has_return = false;
        for command in model.commands() {
            let Some(context) = model.command_context(*command) else {
                continue;
            };
            if !persistent(context.scope()) {
                continue;
            }
            let span = model.command_span(*command);
            if model.command_kind(*command) == CommandKind::Builtin(BuiltinCommandKind::Return) {
                has_return = true;
            }
            if let Some(info) = model.recorded_program().command_info_for_span(span)
                && let Some(name) = info.static_callee.as_deref()
            {
                if matches!(name, "eval" | "unset") {
                    events.push((span.end.offset(), PathEvent::Invalidate));
                } else if !matches!(name, "source" | ".") {
                    events.push((span.end.offset(), PathEvent::Call(Name::from(name))));
                }
            }
        }
        events.sort_by_key(|(offset, _)| *offset);
        Self {
            events,
            profile: model.shell_profile().clone(),
            has_return,
        }
    }
}

/// Bounded path-value analysis shared by CLI, editor, and source closure.
#[derive(Default)]
pub struct SourcePathAnalyzer {
    files: FxHashMap<HelperSummaryKey, Option<PathFile>>,
    bytes: usize,
    halted: bool,
    incomplete: bool,
}

impl SourcePathAnalyzer {
    /// Resolve imports in execution order without running shell commands.
    pub fn resolve(
        &mut self,
        model: &SemanticModel,
        path: &Path,
        provider: &dyn SourcePathFileProvider,
    ) -> ResolvedSourcePaths {
        let mut result = ResolvedSourcePaths::default();
        if model.source_refs().len() < 2
            || !model.source_refs().iter().any(|reference| {
                matches!(
                    reference.kind,
                    SourceRefKind::Dynamic | SourceRefKind::SingleVariableStaticTail { .. }
                )
            })
        {
            return result;
        }
        self.halted = false;
        self.incomplete = false;
        let mut active = FxHashSet::default();
        let mut remaining = MAX_EVENTS;
        let mut values = FxHashMap::default();
        self.evaluate(
            &PathFile::project(model),
            path,
            provider,
            &mut values,
            &mut FxHashSet::default(),
            &mut active,
            &mut remaining,
            Some(&mut result),
        );
        if self.halted {
            for reference in model.source_refs() {
                if matches!(
                    reference.kind,
                    SourceRefKind::Dynamic | SourceRefKind::SingleVariableStaticTail { .. }
                ) {
                    result
                        .candidates
                        .entry(SpanKey::new(reference.span))
                        .or_insert(None);
                }
            }
        }
        result.incomplete = self.incomplete;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate(
        &mut self,
        file: &PathFile,
        path: &Path,
        provider: &dyn SourcePathFileProvider,
        values: &mut FxHashMap<Name, String>,
        functions: &mut FxHashSet<Name>,
        active: &mut FxHashSet<PathBuf>,
        remaining: &mut usize,
        mut result: Option<&mut ResolvedSourcePaths>,
    ) -> FxHashSet<PathBuf> {
        let mut dependencies = FxHashSet::default();
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if active.len() >= MAX_DEPTH || !active.insert(canonical.clone()) {
            self.halted = true;
            self.incomplete |= active.len() >= MAX_DEPTH;
            values.clear();
            return dependencies;
        }
        let mut imported = false;
        for (_, event) in &file.events {
            if self.halted || *remaining == 0 || provider.is_cancelled() {
                self.incomplete |= *remaining == 0 || provider.is_cancelled();
                self.halted = true;
                values.clear();
                break;
            }
            *remaining -= 1;
            match event {
                PathEvent::Write(name, expression) => {
                    let value = expression
                        .as_ref()
                        .and_then(|expression| evaluate_expression(expression, path, values));
                    values.remove(name);
                    if let Some(value) = value {
                        values.insert(name.clone(), value);
                    }
                }
                PathEvent::Invalidate => values.clear(),
                PathEvent::DefineFunction(name) => {
                    functions.insert(name.clone());
                }
                PathEvent::Call(name) => {
                    if functions.contains(name) {
                        values.clear();
                    }
                }
                PathEvent::Source(reference, expression) => {
                    let dynamic = !matches!(
                        reference.kind,
                        SourceRefKind::Literal(_)
                            | SourceRefKind::Directive(_)
                            | SourceRefKind::DirectiveDevNull
                    );
                    let value = expression
                        .as_ref()
                        .and_then(|expression| evaluate_expression(expression, path, values))
                        .filter(|value| Path::new(value).is_absolute());
                    if dynamic
                        && imported
                        && let Some(result) = result.as_deref_mut()
                    {
                        result.candidates.insert(
                            SpanKey::new(reference.span),
                            value.as_ref().map(PathBuf::from),
                        );
                    }
                    let candidates = if dynamic {
                        value.map(PathBuf::from).into_iter().collect()
                    } else {
                        match &reference.kind {
                            SourceRefKind::Literal(value) | SourceRefKind::Directive(value) => {
                                provider.candidates(path, value)
                            }
                            _ => Vec::new(),
                        }
                    };
                    let target = candidates
                        .into_iter()
                        .inspect(|candidate| {
                            dependencies.insert(candidate.clone());
                        })
                        .find(|candidate| provider.is_file(candidate));
                    imported = true;
                    if matches!(reference.kind, SourceRefKind::DirectiveDevNull) {
                        continue;
                    }
                    if reference.conditionally_executed {
                        values.clear();
                        continue;
                    }
                    let Some(target) = target else {
                        values.clear();
                        continue;
                    };
                    let Some(helper) = self.load(&target, &file.profile, provider) else {
                        values.clear();
                        continue;
                    };
                    dependencies.extend(self.evaluate(
                        &helper, &target, provider, values, functions, active, remaining, None,
                    ));
                }
            }
        }
        if file.has_return {
            values.clear();
        }
        active.remove(&canonical);
        if let Some(result) = result {
            result.dependencies.extend(dependencies.iter().cloned());
        }
        dependencies
    }

    fn load(
        &mut self,
        path: &Path,
        inherited: &ShellProfile,
        provider: &dyn SourcePathFileProvider,
    ) -> Option<PathFile> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let key = HelperSummaryKey {
            path: canonical.clone(),
            shell_profile: ShellProfileKey::from_profile(inherited),
        };
        if let Some(file) = self.files.get(&key) {
            return file.clone();
        }
        if self.files.len() >= MAX_FILES || self.bytes >= MAX_BYTES {
            self.halted = true;
            self.incomplete = true;
            return None;
        }
        let source = provider.read_source(&canonical)?;
        self.bytes = self.bytes.saturating_add(source.len());
        if self.bytes > MAX_BYTES {
            self.halted = true;
            self.incomplete = true;
            return None;
        }
        let profile = helper_shell_profile(&source, &canonical, inherited);
        let parse = Parser::with_profile(&source, profile.clone()).parse();
        let file = if parse.is_err() {
            None
        } else {
            let indexer = Indexer::new(&source, &parse);
            let model = build_semantic_model_base(
                &parse.file,
                &source,
                &indexer,
                &mut crate::NoopTraversalObserver,
                Some(&canonical),
                Some(profile),
                None,
            );
            Some(PathFile::project(&model))
        };
        self.files.insert(key, file.clone());
        file
    }
}

fn evaluate_expression(
    expression: &SourcePathTemplate,
    path: &Path,
    values: &FxHashMap<Name, String>,
) -> Option<String> {
    fn substitute(
        parts: &[TemplatePart],
        values: &FxHashMap<Name, String>,
    ) -> Option<Vec<TemplatePart>> {
        parts
            .iter()
            .map(|part| {
                Some(match part {
                    TemplatePart::Variable(name) => {
                        TemplatePart::Literal(values.get(name)?.clone())
                    }
                    TemplatePart::LogicalDirectory(parts) => {
                        TemplatePart::LogicalDirectory(substitute(parts, values)?)
                    }
                    other => other.clone(),
                })
            })
            .collect()
    }
    let SourcePathTemplate::Interpolated(parts) = expression;
    let parts = substitute(parts, values)?;
    let mut remaining = MAX_SOURCE_PATH_TEMPLATE_PARTS;
    let mut bytes = MAX_SOURCE_PATH_TEMPLATE_LITERAL_BYTES;
    if !template_parts_within_budget(&parts, &mut remaining, &mut bytes) {
        return None;
    }
    let value = render_template_parts(&parts, &[], path)?;
    (value.len() <= MAX_SOURCE_PATH_TEMPLATE_LITERAL_BYTES).then_some(value)
}

pub(super) struct DiskSourcePathProvider<'a> {
    pub(super) resolver: Option<&'a (dyn SourcePathResolver + Send + Sync)>,
}

impl SourcePathFileProvider for DiskSourcePathProvider<'_> {
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        let mut paths = candidate_path_variants(candidate)
            .into_iter()
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    from.parent().unwrap_or(Path::new(".")).join(path)
                }
            })
            .collect::<Vec<_>>();
        if let Some(resolver) = self.resolver {
            paths.extend(resolver.resolve_candidate_paths(from, candidate));
        }
        paths
    }
}
