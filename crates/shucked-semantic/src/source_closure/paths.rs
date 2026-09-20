use super::*;
use crate::cfg::{CommandId, RecordedCommandKind, RecordedCommandRange};
use crate::{BindingKind, SourceRef};
use std::sync::Arc;

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
    Write(Name, Option<SourcePathTemplate>, bool),
    Source(
        SourceRef,
        Option<SourcePathTemplate>,
        bool,
        Box<[Option<compact_str::CompactString>]>,
    ),
    InvalidateArguments,
    Invalidate,
    DefineFunction(Name, Arc<PathFile>),
    Call(Name, Box<[Option<compact_str::CompactString>]>),
    Branch(Vec<Vec<PathEvent>>),
    Stop,
}

#[derive(Clone)]
struct PathFile {
    events: Vec<PathEvent>,
    profile: ShellProfile,
}

impl PathFile {
    fn project(model: &SemanticModel) -> Self {
        Self {
            events: Self::sequence(model, model.recorded_program().file_commands()),
            profile: model.shell_profile().clone(),
        }
    }

    fn sequence(model: &SemanticModel, range: RecordedCommandRange) -> Vec<PathEvent> {
        model
            .recorded_program()
            .commands_in(range)
            .iter()
            .flat_map(|id| Self::command(model, *id))
            .collect()
    }

    fn list_chain(
        model: &SemanticModel,
        id: CommandId,
        operator: crate::cfg::RecordedListOperator,
        chain: &mut Vec<CommandId>,
    ) {
        let program = model.recorded_program();
        if let RecordedCommandKind::List { first, rest } = program.command(id).kind
            && program
                .list_items(rest)
                .iter()
                .all(|item| item.operator == operator)
        {
            Self::list_chain(model, first, operator, chain);
            for item in program.list_items(rest) {
                Self::list_chain(model, item.command, operator, chain);
            }
        } else {
            chain.push(id);
        }
    }

    fn command(model: &SemanticModel, id: CommandId) -> Vec<PathEvent> {
        let program = model.recorded_program();
        let command = program.command(id);
        if matches!(
            command.kind,
            RecordedCommandKind::Subshell { .. } | RecordedCommandKind::Pipeline { .. }
        ) || command.background
            || command.scope.is_some_and(|scope| {
                model
                    .innermost_transient_scope_within_function(scope)
                    .is_some()
            })
        {
            return Vec::new();
        }
        let mut events = Vec::new();
        if let Some(bindings) = model.command_bindings.get(&SpanKey::new(command.span)) {
            for id in bindings {
                let binding = model.binding(*id);
                if let Some(scope) = program.function_body_scopes.get(id) {
                    events.push(PathEvent::DefineFunction(
                        binding.name.clone(),
                        Arc::new(Self {
                            events: Self::sequence(model, program.function_body(*scope)),
                            profile: model.shell_profile().clone(),
                        }),
                    ));
                } else if binding.attributes.contains(BindingAttributes::NAMEREF) {
                    events.push(PathEvent::Invalidate);
                } else if !matches!(
                    binding.kind,
                    BindingKind::Imported | BindingKind::FunctionDefinition
                ) {
                    events.push(PathEvent::Write(
                        binding.name.clone(),
                        model.source_path_expressions.get(id).cloned(),
                        binding.attributes.contains(BindingAttributes::LOCAL),
                    ));
                }
            }
        }
        match command.kind {
            RecordedCommandKind::Linear => {
                if let Some(info) = program.command_info_for_span(command.span) {
                    if let Some(reference) =
                        model.source_refs().iter().find(|r| r.span == command.span)
                    {
                        events.push(PathEvent::Source(
                            reference.clone(),
                            info.source_path_expression.clone(),
                            info.source_path_environment_unknown,
                            info.static_args.iter().skip(1).cloned().collect(),
                        ));
                    } else if (info.source_path_environment_unknown
                        && info
                            .static_callee
                            .as_deref()
                            .is_some_and(|name| !name.is_empty()))
                        || info.dynamic_name_span.is_some()
                    {
                        events.push(PathEvent::Invalidate);
                    } else if let Some(name) = info
                        .static_callee
                        .as_deref()
                        .filter(|name| !name.is_empty())
                    {
                        if matches!(name, "set" | "shift") {
                            events.push(PathEvent::InvalidateArguments);
                        } else if matches!(name, "eval" | "unset") {
                            events.push(PathEvent::Invalidate);
                        } else {
                            events
                                .push(PathEvent::Call(Name::from(name), info.static_args.clone()));
                        }
                    }
                }
            }
            RecordedCommandKind::List { first, rest } => {
                let items = program.list_items(rest);
                if let Some(item) = items.first()
                    && items.iter().all(|other| other.operator == item.operator)
                {
                    let mut chain = Vec::new();
                    Self::list_chain(model, first, item.operator, &mut chain);
                    for item in items {
                        Self::list_chain(model, item.command, item.operator, &mut chain);
                    }
                    events.extend(Self::command(model, chain[0]));
                    let mut tail = Vec::new();
                    for id in chain[1..].iter().rev() {
                        let mut body = Self::command(model, *id);
                        body.extend(tail);
                        tail = vec![PathEvent::Branch(vec![Vec::new(), body])];
                    }
                    events.extend(tail);
                } else {
                    events.extend(Self::command(model, first));
                    for item in items {
                        events.push(PathEvent::Branch(vec![
                            Vec::new(),
                            Self::command(model, item.command),
                        ]));
                    }
                }
            }
            RecordedCommandKind::If {
                condition,
                then_branch,
                elif_branches,
                else_branch,
            } => {
                events.extend(Self::sequence(model, condition));
                let mut alternative = Self::sequence(model, else_branch);
                for branch in program.elif_branches(elif_branches).iter().rev() {
                    let mut body = Self::sequence(model, branch.condition);
                    body.push(PathEvent::Branch(vec![
                        Self::sequence(model, branch.body),
                        alternative,
                    ]));
                    alternative = body;
                }
                events.push(PathEvent::Branch(vec![
                    Self::sequence(model, then_branch),
                    alternative,
                ]));
            }
            RecordedCommandKind::Case { arms }
                if program
                    .case_arms(arms)
                    .iter()
                    .all(|arm| matches!(arm.terminator, shucked_ast::CaseTerminator::Break)) =>
            {
                let arms = program.case_arms(arms);
                let mut branches = arms
                    .iter()
                    .map(|arm| Self::sequence(model, arm.commands))
                    .collect::<Vec<_>>();
                if !arms.iter().any(|arm| arm.matches_anything) {
                    branches.push(Vec::new());
                }
                events.push(PathEvent::Branch(branches));
            }
            RecordedCommandKind::BraceGroup { body } => events.extend(Self::sequence(model, body)),
            RecordedCommandKind::Always { body, always_body } => {
                events.extend(Self::sequence(model, body));
                events.extend(Self::sequence(model, always_body));
            }
            RecordedCommandKind::Subshell { .. } | RecordedCommandKind::Pipeline { .. } => {}
            RecordedCommandKind::Return | RecordedCommandKind::Exit => events.push(PathEvent::Stop),
            // Repeated execution and case fallthrough need a fixed point.
            _ => events.push(PathEvent::Invalidate),
        }
        events
    }
}

#[derive(Clone, Default)]
struct PathEnvironment {
    values: FxHashMap<Name, String>,
    functions: FxHashMap<Name, (PathBuf, Arc<PathFile>)>,
    locals: Vec<FxHashMap<Name, Option<String>>>,
    imported: bool,
    unknown_functions: FxHashSet<Name>,
    unknown_dispatch: bool,
    returned: bool,
    unknown_arguments: bool,
}

impl PathEnvironment {
    fn join(&mut self, other: &Self) {
        self.unknown_functions
            .extend(other.unknown_functions.iter().cloned());
        self.unknown_functions
            .extend(self.functions.keys().chain(other.functions.keys()).cloned());
        self.functions.retain(|name, (path, body)| {
            other
                .functions
                .get(name)
                .is_some_and(|(other_path, other_body)| {
                    path == other_path && Arc::ptr_eq(body, other_body)
                })
        });
        for (frame, other_frame) in self.locals.iter_mut().zip(&other.locals) {
            let names = frame
                .keys()
                .chain(other_frame.keys())
                .cloned()
                .collect::<FxHashSet<_>>();
            for name in names {
                let restored = frame
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| self.values.get(&name).cloned());
                let other_restored = other_frame
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| other.values.get(&name).cloned());
                frame.insert(
                    name,
                    if restored == other_restored {
                        restored
                    } else {
                        None
                    },
                );
            }
        }
        self.values
            .retain(|name, value| other.values.get(name) == Some(value));
        self.unknown_functions
            .retain(|name| !self.functions.contains_key(name));
        self.unknown_dispatch |= other.unknown_dispatch;
        self.returned |= other.returned;
        self.unknown_arguments |= other.unknown_arguments;
        self.imported |= other.imported;
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
        if !model.source_refs().iter().any(|reference| {
            matches!(
                reference.kind,
                SourceRefKind::Dynamic | SourceRefKind::SingleVariableStaticTail { .. }
            )
        }) {
            return result;
        }
        self.halted = false;
        self.incomplete = false;
        let mut remaining = MAX_EVENTS;
        self.evaluate(
            &PathFile::project(model),
            path,
            path,
            provider,
            &mut PathEnvironment::default(),
            &[],
            &mut FxHashSet::default(),
            &mut remaining,
            0,
            &mut result,
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
        root: &Path,
        provider: &dyn SourcePathFileProvider,
        environment: &mut PathEnvironment,
        args: &[Option<compact_str::CompactString>],
        active: &mut FxHashSet<PathBuf>,
        remaining: &mut usize,
        depth: usize,
        result: &mut ResolvedSourcePaths,
    ) {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if depth >= MAX_DEPTH || !active.insert(canonical.clone()) {
            self.halted = true;
            self.incomplete |= depth >= MAX_DEPTH;
            environment.values.clear();
            return;
        }
        self.events(
            &file.events,
            &file.profile,
            path,
            root,
            provider,
            environment,
            args,
            active,
            remaining,
            depth,
            result,
        );
        if environment.returned {
            environment.values.clear();
        }
        active.remove(&canonical);
    }

    #[allow(clippy::too_many_arguments)]
    fn events(
        &mut self,
        events: &[PathEvent],
        profile: &ShellProfile,
        path: &Path,
        root: &Path,
        provider: &dyn SourcePathFileProvider,
        environment: &mut PathEnvironment,
        args: &[Option<compact_str::CompactString>],
        active: &mut FxHashSet<PathBuf>,
        remaining: &mut usize,
        depth: usize,
        result: &mut ResolvedSourcePaths,
    ) {
        for event in events {
            if self.halted || *remaining == 0 || depth >= MAX_DEPTH || provider.is_cancelled() {
                self.incomplete |= *remaining == 0 || depth >= MAX_DEPTH || provider.is_cancelled();
                self.halted = true;
                environment.values.clear();
                break;
            }
            *remaining -= 1;
            match event {
                PathEvent::Write(name, expression, local) => {
                    let value = expression.as_ref().and_then(|expression| {
                        evaluate_expression(
                            expression,
                            path,
                            &environment.values,
                            if environment.unknown_arguments {
                                &[]
                            } else {
                                args
                            },
                        )
                    });
                    if *local && let Some(frame) = environment.locals.last_mut() {
                        frame
                            .entry(name.clone())
                            .or_insert_with(|| environment.values.get(name).cloned());
                    }
                    environment.values.remove(name);
                    if let Some(value) = value {
                        environment.values.insert(name.clone(), value);
                    }
                }
                PathEvent::InvalidateArguments => {
                    environment.unknown_arguments = true;
                    environment.imported = true;
                }
                PathEvent::Invalidate => {
                    environment.unknown_arguments = true;
                    environment.values.clear();
                    environment.functions.clear();
                    environment.unknown_dispatch = true;
                    environment.imported = true;
                }
                PathEvent::Stop => {
                    environment.returned = true;
                    environment.values.clear();
                    break;
                }
                PathEvent::DefineFunction(name, body) => {
                    environment.unknown_functions.remove(name);
                    environment
                        .functions
                        .insert(name.clone(), (path.to_path_buf(), body.clone()));
                }
                PathEvent::Call(name, call_args) => {
                    if let Some((origin, body)) = environment.functions.get(name).cloned() {
                        let caller_arguments_unknown = environment.unknown_arguments;
                        environment.unknown_arguments = false;
                        let caller_returned = environment.returned;
                        environment.returned = false;
                        environment.locals.push(FxHashMap::default());
                        self.events(
                            &body.events,
                            &body.profile,
                            &origin,
                            root,
                            provider,
                            environment,
                            call_args,
                            active,
                            remaining,
                            depth + 1,
                            result,
                        );
                        if environment.returned {
                            environment.values.clear();
                        }
                        environment.returned = caller_returned;
                        environment.unknown_arguments = caller_arguments_unknown;
                        for (name, value) in environment.locals.pop().unwrap_or_default() {
                            environment.values.remove(&name);
                            if let Some(value) = value {
                                environment.values.insert(name, value);
                            }
                        }
                        environment.imported = true;
                    } else if environment.unknown_dispatch
                        || environment.unknown_functions.contains(name)
                    {
                        environment.values.clear();
                        environment.imported = true;
                    }
                }
                PathEvent::Branch(branches) => {
                    let before = environment.clone();
                    let mut joined: Option<PathEnvironment> = None;
                    for branch in branches {
                        let mut branch_environment = before.clone();
                        self.events(
                            branch,
                            profile,
                            path,
                            root,
                            provider,
                            &mut branch_environment,
                            args,
                            active,
                            remaining,
                            depth + 1,
                            result,
                        );
                        if let Some(joined) = &mut joined {
                            joined.join(&branch_environment);
                        } else {
                            joined = Some(branch_environment);
                        }
                    }
                    if let Some(joined) = joined {
                        *environment = joined;
                    }
                }
                PathEvent::Source(reference, expression, unknown_environment, source_args) => {
                    let dynamic = !matches!(
                        reference.kind,
                        SourceRefKind::Literal(_)
                            | SourceRefKind::Directive(_)
                            | SourceRefKind::DirectiveDevNull
                    );
                    let value = expression
                        .as_ref()
                        .filter(|_| !*unknown_environment)
                        .and_then(|expression| {
                            evaluate_expression(
                                expression,
                                path,
                                &environment.values,
                                if environment.unknown_arguments {
                                    &[]
                                } else {
                                    args
                                },
                            )
                        })
                        .filter(|value| Path::new(value).is_absolute());
                    if dynamic
                        && (*unknown_environment
                            || environment.imported
                            || value.is_some()
                            || result
                                .candidates
                                .contains_key(&SpanKey::new(reference.span)))
                        && path == root
                    {
                        let candidate = value.as_ref().map(PathBuf::from);
                        result
                            .candidates
                            .entry(SpanKey::new(reference.span))
                            .and_modify(|previous| {
                                if *previous != candidate {
                                    *previous = None;
                                }
                            })
                            .or_insert(candidate);
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
                            result.dependencies.insert(candidate.clone());
                        })
                        .find(|candidate| provider.is_file(candidate));
                    environment.imported = true;
                    if matches!(reference.kind, SourceRefKind::DirectiveDevNull) {
                        continue;
                    }
                    if *unknown_environment {
                        environment.values.clear();
                        environment.functions.clear();
                        environment.unknown_dispatch = true;
                        continue;
                    }
                    let helper = target
                        .as_ref()
                        .and_then(|target| self.load(target, profile, provider));
                    if let (Some(target), Some(helper)) = (target, helper) {
                        let caller_arguments_unknown = environment.unknown_arguments;
                        if !source_args.is_empty() {
                            environment.unknown_arguments = false;
                        }
                        self.evaluate(
                            &helper,
                            &target,
                            root,
                            provider,
                            environment,
                            if source_args.is_empty() {
                                args
                            } else {
                                source_args
                            },
                            active,
                            remaining,
                            depth + 1,
                            result,
                        );
                        if !source_args.is_empty() {
                            environment.unknown_arguments = caller_arguments_unknown;
                        }
                    } else {
                        environment.values.clear();
                        environment.functions.clear();
                        environment.unknown_dispatch = true;
                    }
                }
            }
        }
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
    args: &[Option<compact_str::CompactString>],
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
    let value = render_template_parts(&parts, args, path)?;
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
