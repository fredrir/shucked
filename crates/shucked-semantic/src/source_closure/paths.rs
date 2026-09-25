pub(crate) mod loops;

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

    /// Directory members, bounded to avoid unbounded glob expansion.
    fn directory_entries(&self, path: &Path) -> Option<Vec<PathBuf>> {
        let mut entries = if path.exists() {
            loops::directory_entries(path)?
        } else {
            Vec::new()
        };
        entries.extend(self.open_paths_in(path));
        entries.sort();
        entries.dedup();
        (entries.len() <= loops::MAX_DIRECTORY_ENTRIES).then_some(entries)
    }

    /// Unsaved files whose parent is the requested directory.
    fn open_paths_in(&self, _path: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    /// The home directory that seeds `HOME`, expands a leading `~`, and
    /// locates the installed Zsh startup files.
    ///
    /// The default reads the process environment; tests and editor providers
    /// override it for deterministic resolution.
    fn home_dir(&self) -> Option<PathBuf> {
        crate::source_resolve::home_dir()
    }

    /// A process environment value used to seed well-known path variables
    /// (`XDG_CONFIG_HOME`, `ZDOTDIR`, ...). Empty values count as unset.
    fn environment_variable(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }

    /// [`candidates`](Self::candidates) after expanding a leading `~` through
    /// [`home_dir`](Self::home_dir). A home-anchored operand names exactly one
    /// file, so it bypasses the search roots.
    fn search_candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        match crate::source_resolve::expand_home_tilde(candidate, self.home_dir().as_deref()) {
            Some(expanded) => vec![expanded],
            None => self.candidates(from, candidate),
        }
    }

    /// Search candidates for a determinable (literal or directive) source
    /// reference, in precedence order; dynamic references contribute nothing.
    fn source_ref_candidates(&self, from: &Path, source_ref: &SourceRef) -> Vec<PathBuf> {
        match &source_ref.kind {
            SourceRefKind::Literal(candidate) | SourceRefKind::Directive(candidate) => {
                self.search_candidates(from, candidate)
            }
            SourceRefKind::DirectiveDevNull
            | SourceRefKind::Dynamic
            | SourceRefKind::SingleVariableStaticTail { .. } => Vec::new(),
        }
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
    sequences: FxHashMap<SpanKey, Vec<PathBuf>>,
    uncertain_sequences: FxHashSet<SpanKey>,
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

    /// Ordered files loaded by a bounded source loop at this site.
    pub fn sequence(&self, reference: &SourceRef) -> Option<&[PathBuf]> {
        self.sequences
            .get(&SpanKey::new(reference.span))
            .map(Vec::as_slice)
    }

    /// Expand loop effects for variable usage without relaxing function rename checks.
    pub fn variable_effects(
        &self,
        effects: &[crate::CallFactSourceEffect],
    ) -> Vec<crate::CallFactSourceEffect> {
        effects
            .iter()
            .flat_map(|effect| {
                if let Some(paths) = self.sequences.get(&SpanKey::new(effect.span)) {
                    paths
                        .iter()
                        .map(|path| {
                            let mut effect = effect.clone();
                            effect.path = Some(crate::canonical_workspace_path(path));
                            effect
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![effect.clone()]
                }
            })
            .collect()
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
    SourceLoop {
        reference: SourceRef,
        variable: Name,
        words: Vec<loops::LoopWord>,
        template: Vec<TemplatePart>,
    },
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
            RecordedCommandKind::For { body } => {
                // A loop whose body is exactly one `source` of a path built
                // from the loop variable (`source "$f"`, `source "$dir/$f"`)
                // loads one file per word; anything else needs a fixed point.
                let words = program.source_loop_words.get(&SpanKey::new(command.span));
                let body_events = Self::sequence(model, body);
                if let Some((name, words)) = words
                    && let [
                        PathEvent::Source(
                            reference,
                            Some(SourcePathTemplate::Interpolated(parts)),
                            false,
                            args,
                        ),
                    ] = body_events.as_slice()
                    && args.is_empty()
                    && parts.iter().any(
                        |part| matches!(part, TemplatePart::Variable(variable) if variable == name),
                    )
                {
                    events.push(PathEvent::SourceLoop {
                        reference: reference.clone(),
                        variable: name.clone(),
                        words: words.clone(),
                        template: parts.clone(),
                    });
                } else {
                    events.push(PathEvent::Invalidate);
                }
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
    early_return: bool,
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
        self.early_return |= other.early_return;
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
        let mut environment = PathEnvironment::default();
        seed_process_environment(
            model.shell_profile().dialect,
            path,
            provider,
            &mut environment,
        );
        if model.shell_profile().dialect == ParseShellDialect::Zsh {
            self.zsh_startup_environment(
                path,
                provider,
                &mut environment,
                &mut remaining,
                &mut result,
            );
        }
        self.evaluate(
            &PathFile::project(model),
            path,
            path,
            provider,
            &mut environment,
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
        result.incomplete |= self.incomplete;
        result
    }

    /// Applies the Zsh startup files that run before `path`.
    ///
    /// The user's `.zshenv` is evaluated in full only for a startup file: one
    /// named `.zshrc` (its sibling `.zshenv`) or the installed `~/.zshrc`
    /// (`~/.zshenv`). Every other Zsh file takes just `ZDOTDIR` from
    /// `~/.zshenv`, because that variable only tells the shell where the
    /// startup files live and cannot be set anywhere later.
    fn zsh_startup_environment(
        &mut self,
        path: &Path,
        provider: &dyn SourcePathFileProvider,
        environment: &mut PathEnvironment,
        remaining: &mut usize,
        result: &mut ResolvedSourcePaths,
    ) {
        let home = provider.home_dir();
        let installed = home.as_ref().map(|home| home.join(".zshrc"));
        if let Some(installed) = &installed {
            result.dependencies.insert(installed.clone());
        }
        let user_zshenv = environment
            .values
            .get(&Name::from("ZDOTDIR"))
            .map(|zdotdir| PathBuf::from(zdotdir).join(".zshenv"))
            .filter(|zshenv| {
                crate::canonical_workspace_path(zshenv) != crate::canonical_workspace_path(path)
            });
        let startup = if path.file_name().is_some_and(|name| name == ".zshrc") {
            path.parent().map(|parent| parent.join(".zshenv"))
        } else if installed.as_ref().is_some_and(|installed| {
            provider.is_file(installed)
                && crate::canonical_workspace_path(installed)
                    == crate::canonical_workspace_path(path)
        }) {
            home.as_ref().map(|home| home.join(".zshenv"))
        } else {
            None
        };
        let Some(startup) = startup else {
            if let Some(user_zshenv) = user_zshenv {
                self.zsh_dotdir_from_startup_file(
                    &user_zshenv,
                    path,
                    provider,
                    environment,
                    remaining,
                    result,
                );
            }
            return;
        };
        result.dependencies.insert(startup.clone());
        if provider.is_file(&startup)
            && let Some(file) = self.load(
                &startup,
                &ShellProfile::native(ParseShellDialect::Zsh),
                provider,
            )
        {
            self.evaluate(
                &file,
                &startup,
                path,
                provider,
                environment,
                &[],
                &mut FxHashSet::default(),
                remaining,
                0,
                result,
            );
            environment.imported = true;
            environment.returned = false;
            environment.early_return = false;
        }
    }

    /// Copies a `ZDOTDIR` assignment out of `startup` without importing any
    /// other value it sets: a non-startup file only inherits the location of
    /// the startup files, not the whole startup environment.
    fn zsh_dotdir_from_startup_file(
        &mut self,
        startup: &Path,
        root: &Path,
        provider: &dyn SourcePathFileProvider,
        environment: &mut PathEnvironment,
        remaining: &mut usize,
        result: &mut ResolvedSourcePaths,
    ) {
        result.dependencies.insert(startup.to_path_buf());
        if !provider.is_file(startup) {
            return;
        }
        let Some(file) = self.load(
            startup,
            &ShellProfile::native(ParseShellDialect::Zsh),
            provider,
        ) else {
            return;
        };
        let mut scratch = PathEnvironment {
            values: environment.values.clone(),
            ..PathEnvironment::default()
        };
        let mut scratch_result = ResolvedSourcePaths::default();
        self.evaluate(
            &file,
            startup,
            root,
            provider,
            &mut scratch,
            &[],
            &mut FxHashSet::default(),
            remaining,
            0,
            &mut scratch_result,
        );
        result.dependencies.extend(scratch_result.dependencies);
        result.incomplete |= scratch_result.incomplete;
        let zdotdir = Name::from("ZDOTDIR");
        if let Some(value) = scratch.values.get(&zdotdir)
            && Path::new(value).is_absolute()
        {
            environment.values.insert(zdotdir, value.clone());
        }
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
        if environment.returned || environment.early_return {
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
                PathEvent::SourceLoop {
                    reference,
                    variable,
                    words,
                    template,
                } => {
                    if path == root {
                        let paths = words
                            .iter()
                            .map(|word| {
                                word.expand_sources(
                                    variable,
                                    template,
                                    path,
                                    &environment.values,
                                    args,
                                    provider,
                                    result,
                                )
                            })
                            .collect::<Option<Vec<_>>>()
                            .map(|paths| paths.into_iter().flatten().collect::<Vec<_>>());
                        let key = SpanKey::new(reference.span);
                        if paths.as_ref().is_some_and(|paths| paths.len() > MAX_FILES) {
                            result.incomplete = true;
                        }
                        if let Some(paths) = paths.filter(|paths| paths.len() <= MAX_FILES) {
                            result.dependencies.extend(paths.iter().cloned());
                            if result
                                .sequences
                                .get(&key)
                                .is_some_and(|previous| *previous != paths)
                            {
                                result.uncertain_sequences.insert(key);
                            }
                            if !result.uncertain_sequences.contains(&key) {
                                result.sequences.insert(key, paths);
                            }
                        } else {
                            result.uncertain_sequences.insert(key);
                        }
                        if result.uncertain_sequences.contains(&key) {
                            result.sequences.remove(&key);
                        }
                        result.candidates.insert(key, None);
                    }
                    environment.values.clear();
                    environment.functions.clear();
                    environment.imported = true;
                    environment.unknown_dispatch = true;
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
                        let caller_early_return = environment.early_return;
                        environment.returned = false;
                        environment.early_return = false;
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
                        if environment.returned || environment.early_return {
                            environment.values.clear();
                        }
                        environment.returned = caller_returned;
                        environment.early_return = caller_early_return;
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
                    let mut early_return = false;
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
                        if branch_environment.returned {
                            early_return = true;
                            continue;
                        }
                        if let Some(joined) = &mut joined {
                            joined.join(&branch_environment);
                        } else {
                            joined = Some(branch_environment);
                        }
                    }
                    if let Some(joined) = joined {
                        *environment = joined;
                        environment.early_return |= early_return;
                    } else {
                        environment.returned = true;
                        environment.values.clear();
                        break;
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
                        provider.source_ref_candidates(path, reference)
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
                        let caller_returned = environment.returned;
                        let caller_early_return = environment.early_return;
                        environment.returned = false;
                        environment.early_return = false;
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
                        environment.returned = caller_returned;
                        environment.early_return = caller_early_return;
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

/// Seeds the path variables every shell inherits from its process environment.
///
/// `HOME` comes from the provider. The XDG base directories follow the XDG
/// Base Directory specification: the environment value when it is set to an
/// absolute path, otherwise the documented default under `HOME`. Zsh files
/// also get `ZDOTDIR`, which the shell resolves in the same order it locates
/// its startup files: the environment, then the directory of a startup file
/// being edited in place (`.zshrc` next to its siblings), then `HOME`.
/// Assignments in the file, and the startup files evaluated afterwards,
/// override every seed.
fn seed_process_environment(
    dialect: ParseShellDialect,
    path: &Path,
    provider: &dyn SourcePathFileProvider,
    environment: &mut PathEnvironment,
) {
    let Some(home) = provider.home_dir() else {
        return;
    };
    let home_text = path_to_template_string(&home);
    environment
        .values
        .insert(Name::from("HOME"), home_text.clone());
    for (name, default) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_CACHE_HOME", ".cache"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
    ] {
        let value = provider
            .environment_variable(name)
            .filter(|value| Path::new(value).is_absolute())
            .unwrap_or_else(|| path_to_template_string(&home.join(default)));
        environment.values.insert(Name::from(name), value);
    }
    if dialect == ParseShellDialect::Zsh {
        let value = provider
            .environment_variable("ZDOTDIR")
            .filter(|value| Path::new(value).is_absolute())
            .or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| is_zsh_startup_file_name(name))
                    .and_then(|_| path.parent())
                    .filter(|parent| parent.is_absolute())
                    .map(path_to_template_string)
            })
            .unwrap_or(home_text);
        environment.values.insert(Name::from("ZDOTDIR"), value);
    }
}

fn is_zsh_startup_file_name(name: &str) -> bool {
    matches!(
        name,
        ".zshenv" | ".zprofile" | ".zshrc" | ".zlogin" | ".zlogout"
    )
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
