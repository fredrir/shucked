//! Ordered, bounded function environments shared by read-only workspace queries.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shucked_ast::Span;

use crate::cfg::{CommandId, RecordedCommandKind, RecordedCommandRange};
use crate::{
    CallFactDefinition, CallNodeKind, CommandNamespace, FileCallFacts, ResolvedSourcePaths,
    SemanticModel, SpanKey,
};

/// A function identity independent of its spelling at a call site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceFunctionDefinition {
    /// Canonical source file.
    pub path: PathBuf,
    /// Definition and selection spans.
    pub definition: CallFactDefinition,
}

/// Possible bindings and the evidence available at a workspace call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceFunctionResolution {
    /// Known candidate definitions, including candidates across unknown effects.
    pub definitions: Vec<WorkspaceFunctionDefinition>,
    /// Some execution paths have no known function binding.
    pub may_be_absent: bool,
    /// Dynamic effects or analysis limits prevent a complete answer.
    pub incomplete: bool,
    /// Files whose source operations established this environment.
    pub loaders: BTreeSet<PathBuf>,
}
impl WorkspaceFunctionResolution {
    /// One binding proven in every analyzed context.
    pub fn exact(&self) -> Option<&WorkspaceFunctionDefinition> {
        (!self.may_be_absent && !self.incomplete && self.definitions.len() == 1)
            .then(|| &self.definitions[0])
    }
    fn absent(incomplete: bool) -> Self {
        Self {
            may_be_absent: true,
            incomplete,
            ..Self::default()
        }
    }
    fn join(&mut self, other: &Self) {
        for definition in &other.definitions {
            if !self.definitions.contains(definition) {
                self.definitions.push(definition.clone());
            }
        }
        self.definitions.sort_by(|a, b| {
            (&a.path, a.definition.def_span.start.offset())
                .cmp(&(&b.path, b.definition.def_span.start.offset()))
        });
        self.may_be_absent |= other.may_be_absent;
        self.incomplete |= other.incomplete;
        self.loaders.extend(other.loaders.iter().cloned());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Environment {
    bindings: Arc<BTreeMap<String, WorkspaceFunctionResolution>>,
    unknown: bool,
}
impl Environment {
    fn lookup(&self, name: &str) -> WorkspaceFunctionResolution {
        self.bindings
            .get(name)
            .cloned()
            .unwrap_or_else(|| WorkspaceFunctionResolution::absent(self.unknown))
    }
    fn invalidate(&mut self) {
        self.unknown = true;
        for binding in Arc::make_mut(&mut self.bindings).values_mut() {
            binding.incomplete = true;
        }
    }
    fn join(&mut self, other: &Self) {
        if self == other {
            return;
        }
        let names = self
            .bindings
            .keys()
            .chain(other.bindings.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for name in names {
            let mut value = self.lookup(&name);
            value.join(&other.lookup(&name));
            Arc::make_mut(&mut self.bindings).insert(name, value);
        }
        self.unknown |= other.unknown;
    }
}

#[derive(Clone, Debug)]
enum Event {
    Point(usize),
    Define(CallFactDefinition),
    Call {
        name: String,
        span: Span,
        enclosing: CallNodeKind,
        shell: bool,
    },
    Source(Option<Vec<PathBuf>>),
    Branch(Vec<Vec<Event>>),
    Repeat(Vec<Event>),
    Finally(Vec<Event>, Vec<Event>),
    LoopStop,
    Isolated(Vec<Event>),
    Clear(Vec<String>),
    Unknown,
    Return,
    Exit,
}

/// Cached projection of a file's structured function effects.
#[derive(Clone, Debug, Default)]
pub struct FileFunctionEffects {
    events: Vec<Event>,
    bodies: BTreeMap<usize, Vec<Event>>,
    targets: BTreeSet<PathBuf>,
}
impl FileFunctionEffects {
    /// Projects the recorded semantic program without walking the syntax tree.
    pub fn project(
        model: &SemanticModel,
        calls: &FileCallFacts,
        paths: &ResolvedSourcePaths,
    ) -> Self {
        let mut projection = Projection {
            model,
            calls,
            paths,
            commands: model
                .command_site_facts()
                .into_iter()
                .map(|fact| (SpanKey::new(fact.span), fact))
                .collect(),
            file: Self::default(),
        };
        projection.file.events = projection.sequence(model.recorded_program().file_commands());
        projection.file
    }
}
struct Projection<'a> {
    model: &'a SemanticModel,
    calls: &'a FileCallFacts,
    paths: &'a ResolvedSourcePaths,
    commands: rustc_hash::FxHashMap<SpanKey, crate::CommandSiteFacts>,
    file: FileFunctionEffects,
}
impl Projection<'_> {
    fn sequence(&mut self, range: RecordedCommandRange) -> Vec<Event> {
        self.model
            .recorded_program()
            .commands_in(range)
            .iter()
            .flat_map(|id| self.command(*id))
            .collect()
    }
    fn list_chain(
        &self,
        id: CommandId,
        operator: crate::cfg::RecordedListOperator,
        chain: &mut Vec<CommandId>,
    ) {
        let program = self.model.recorded_program();
        if let RecordedCommandKind::List { first, rest } = program.command(id).kind
            && program
                .list_items(rest)
                .iter()
                .all(|item| item.operator == operator)
        {
            self.list_chain(first, operator, chain);
            for item in program.list_items(rest) {
                self.list_chain(item.command, operator, chain);
            }
        } else {
            chain.push(id);
        }
    }
    fn command(&mut self, id: CommandId) -> Vec<Event> {
        let program = self.model.recorded_program();
        let command = program.command(id);
        let span = command.span;
        let mut events = vec![Event::Point(span.start.offset())];
        for region in program.nested_regions(command.nested_regions) {
            events.push(Event::Isolated(self.sequence(region.commands)));
        }
        if matches!(command.kind, RecordedCommandKind::Linear)
            && let Some(bindings) = self.model.command_bindings.get(&SpanKey::new(span))
        {
            for binding in bindings {
                if let Some(scope) = program.function_body_scopes.get(binding)
                    && let Some(definition) =
                        self.calls.definitions.iter().find(|d| d.def_span == span)
                {
                    let body = self.sequence(program.function_body(*scope));
                    self.file
                        .bodies
                        .insert(definition.def_span.start.offset(), body);
                    events.push(Event::Define(definition.clone()));
                }
            }
        }
        match command.kind {
            RecordedCommandKind::Linear => {
                // Loads attached to a command that is not a `source` (a plugin
                // manager's module load, resolved by the caller) run in place.
                if !self.model.source_refs().iter().any(|r| r.span == span) {
                    let loaded = self
                        .calls
                        .source_edges
                        .iter()
                        .filter(|edge| edge.span == span)
                        .map(|edge| edge.path.clone())
                        .collect::<Vec<_>>();
                    if !loaded.is_empty() {
                        self.file.targets.extend(loaded.iter().cloned());
                        events.push(Event::Source(Some(loaded)));
                    }
                }
                if let Some(reference) = self.model.source_refs().iter().find(|r| r.span == span) {
                    let targets =
                        if matches!(reference.kind, crate::SourceRefKind::DirectiveDevNull) {
                            Some(Vec::new())
                        } else if let Some(sequence) = self.paths.sequence(reference) {
                            Some(
                                sequence
                                    .iter()
                                    .map(|p| crate::canonical_workspace_path(p))
                                    .collect::<Vec<_>>(),
                            )
                        } else {
                            self.calls
                                .source_effects
                                .iter()
                                .find(|e| e.span == span)
                                .and_then(|e| e.path.clone())
                                .map(|p| vec![p])
                        };
                    if let Some(paths) = &targets {
                        self.file.targets.extend(paths.iter().cloned());
                    }
                    events.push(Event::Source(targets));
                } else if let Some(fact) = self.commands.get(&SpanKey::new(span))
                    && fact.name_span().start.offset() < fact.name_span().end.offset()
                {
                    let name = fact.name().unwrap_or_default().to_owned();
                    let shell = fact.namespace == CommandNamespace::Shell;
                    let enclosing = self
                        .calls
                        .call_sites
                        .iter()
                        .find(|c| c.name_span == fact.name_span())
                        .map(|c| c.enclosing.clone())
                        .unwrap_or(CallNodeKind::TopLevel);
                    events.push(Event::Call {
                        name: name.clone(),
                        span: fact.name_span(),
                        enclosing,
                        shell,
                    });
                    if (name == "eval" && fact.namespace != CommandNamespace::External)
                        || (shell && name.is_empty())
                    {
                        events.push(Event::Unknown);
                    } else if fact.namespace != CommandNamespace::External
                        && (name == "unfunction" || name == "unset")
                    {
                        let args = fact
                            .effective_words
                            .iter()
                            .skip(1)
                            .map(|w| w.text.as_deref())
                            .collect::<Vec<_>>();
                        if name == "unfunction"
                            || args
                                .iter()
                                .flatten()
                                .any(|a| a.starts_with('-') && a.contains('f'))
                        {
                            if args
                                .iter()
                                .any(|a| a.is_none_or(|value| value.contains(['*', '?', '['])))
                            {
                                events.push(Event::Unknown);
                            } else {
                                events.push(Event::Clear(
                                    args.into_iter()
                                        .flatten()
                                        .filter(|a| !a.starts_with('-'))
                                        .map(str::to_owned)
                                        .collect(),
                                ));
                            }
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
                    self.list_chain(first, item.operator, &mut chain);
                    for item in items {
                        self.list_chain(item.command, item.operator, &mut chain);
                    }
                    events.extend(self.command(chain[0]));
                    let mut tail = Vec::new();
                    for id in chain[1..].iter().rev() {
                        let mut body = self.command(*id);
                        body.extend(tail);
                        tail = vec![Event::Branch(vec![Vec::new(), body])];
                    }
                    events.extend(tail);
                } else {
                    events.extend(self.command(first));
                    for item in items {
                        events.push(Event::Branch(vec![Vec::new(), self.command(item.command)]));
                    }
                }
            }
            RecordedCommandKind::If {
                condition,
                then_branch,
                elif_branches,
                else_branch,
            } => {
                events.extend(self.sequence(condition));
                let mut alternative = self.sequence(else_branch);
                for branch in program.elif_branches(elif_branches).iter().rev() {
                    let mut body = self.sequence(branch.condition);
                    body.push(Event::Branch(vec![self.sequence(branch.body), alternative]));
                    alternative = body;
                }
                events.push(Event::Branch(vec![self.sequence(then_branch), alternative]));
            }
            RecordedCommandKind::Case { arms } => {
                let arms = program.case_arms(arms);
                let mut branches = arms
                    .iter()
                    .map(|arm| self.sequence(arm.commands))
                    .collect::<Vec<_>>();
                if !arms.iter().any(|arm| arm.matches_anything) {
                    branches.push(Vec::new());
                }
                events.push(Event::Branch(branches));
                if arms
                    .iter()
                    .any(|arm| arm.terminator != shucked_ast::CaseTerminator::Break)
                {
                    events.push(Event::Unknown);
                }
            }
            RecordedCommandKind::For { body } => {
                let body = self.sequence(body);
                if self.model.source_refs().iter().any(|r| {
                    span.start.offset() <= r.span.start.offset()
                        && r.span.end.offset() <= span.end.offset()
                        && self.paths.sequence(r).is_some()
                }) && body
                    .iter()
                    .all(|e| matches!(e, Event::Point(_) | Event::Source(Some(_))))
                {
                    events.extend(body);
                } else {
                    events.push(Event::Repeat(body));
                }
            }
            RecordedCommandKind::While { condition, body }
            | RecordedCommandKind::Until { condition, body } => {
                events.extend(self.sequence(condition));
                events.push(Event::Repeat(self.sequence(body)));
            }
            RecordedCommandKind::Select { body } | RecordedCommandKind::ArithmeticFor { body } => {
                events.push(Event::Repeat(self.sequence(body)));
            }
            RecordedCommandKind::BraceGroup { body } => events.extend(self.sequence(body)),
            RecordedCommandKind::Always { body, always_body } => {
                events.push(Event::Finally(
                    self.sequence(body),
                    self.sequence(always_body),
                ));
            }
            RecordedCommandKind::Subshell { body } => {
                events.push(Event::Isolated(self.sequence(body)))
            }
            RecordedCommandKind::Pipeline { segments } => {
                for segment in program.pipeline_segments(segments) {
                    let body = self.command(segment.command);
                    if self
                        .model
                        .innermost_transient_scope_within_function(segment.scope)
                        .is_some()
                    {
                        events.push(Event::Isolated(body));
                    } else {
                        events.extend(body);
                    }
                }
            }
            RecordedCommandKind::Return => events.push(Event::Return),
            RecordedCommandKind::Exit => events.push(Event::Exit),
            RecordedCommandKind::Break { .. } | RecordedCommandKind::Continue { .. } => {
                events.push(Event::LoopStop)
            }
        }
        events.push(Event::Point(span.end.offset()));
        if command.background {
            vec![Event::Isolated(events)]
        } else {
            events
        }
    }
}

/// One call occurrence with all its observed source contexts.
#[derive(Clone, Debug)]
pub struct WorkspaceFunctionCall {
    /// Calling file.
    pub path: PathBuf,
    /// Command name token.
    pub span: Span,
    /// Enclosing function or file body.
    pub enclosing: CallNodeKind,
    /// Function binding evidence.
    pub resolution: WorkspaceFunctionResolution,
}

/// Shared read-only function answers. Rename uses the separate exact binding checks.
#[derive(Default)]
pub struct WorkspaceFunctionIndex {
    calls: BTreeMap<(PathBuf, usize), WorkspaceFunctionCall>,
    points: BTreeMap<PathBuf, BTreeMap<usize, Environment>>,
    /// Whether execution exceeded its bounded analysis budget.
    pub incomplete: bool,
}
impl WorkspaceFunctionIndex {
    /// Analyze connected source contexts, preserving order and branch alternatives.
    pub fn build(
        files: BTreeMap<PathBuf, Arc<FileFunctionEffects>>,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<Self> {
        let targets = files
            .values()
            .flat_map(|f| f.targets.iter().cloned())
            .collect::<BTreeSet<_>>();
        let roots = files
            .keys()
            .filter(|p| !targets.contains(*p))
            .cloned()
            .collect::<Vec<_>>();
        let mut evaluator = Evaluator {
            files: &files,
            result: Self::default(),
            remaining: 200_000,
            point_budget: 1_000_000,
            stack: Vec::new(),
            chain: Vec::new(),
            cancelled,
            visited: BTreeSet::new(),
            invoked: BTreeSet::new(),
        };
        for root in roots {
            evaluator.root(&root, Environment::default());
        }
        // Cycles and targets only reachable from deferred dispatch still receive partial answers.
        for path in files.keys() {
            if !evaluator.visited.contains(path) {
                let mut environment = Environment::default();
                environment.invalidate();
                evaluator.root(path, environment);
            }
        }
        if cancelled() {
            None
        } else {
            Some(evaluator.result)
        }
    }
    /// Resolution at a command token. Missing analysis is explicitly incomplete.
    pub fn resolve(&self, path: &Path, span: Span) -> WorkspaceFunctionResolution {
        let mut result = self
            .calls
            .get(&(path.to_path_buf(), span.start.offset()))
            .map(|c| c.resolution.clone())
            .unwrap_or_else(|| WorkspaceFunctionResolution::absent(true));
        result.incomplete |= self.incomplete;
        result
    }
    /// All known command occurrences, in deterministic source order.
    pub fn calls(&self) -> impl Iterator<Item = &WorkspaceFunctionCall> {
        self.calls.values()
    }
    /// Candidate function names visible at an editor position.
    pub fn visible(
        &self,
        path: &Path,
        offset: usize,
    ) -> Vec<(String, WorkspaceFunctionResolution)> {
        self.points
            .get(path)
            .and_then(|points| points.range(..=offset).next_back())
            .map(|(_, env)| {
                env.bindings
                    .iter()
                    .filter(|(_, r)| !r.definitions.is_empty())
                    .map(|(n, r)| (n.clone(), r.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct Flow {
    active: Option<Environment>,
    returned: Option<Environment>,
    stopped: Option<Environment>,
}
fn merge(slot: &mut Option<Environment>, environment: Environment) {
    if let Some(previous) = slot {
        previous.join(&environment);
    } else {
        *slot = Some(environment);
    }
}
impl Flow {
    fn boundary(mut self) -> Option<Environment> {
        if let Some(returned) = self.returned {
            merge(&mut self.active, returned);
        }
        if let Some(mut stopped) = self.stopped {
            stopped.invalidate();
            merge(&mut self.active, stopped);
        }
        self.active
    }
}
struct Evaluator<'a> {
    files: &'a BTreeMap<PathBuf, Arc<FileFunctionEffects>>,
    result: WorkspaceFunctionIndex,
    remaining: usize,
    point_budget: usize,
    stack: Vec<(PathBuf, Option<usize>)>,
    chain: Vec<PathBuf>,
    cancelled: &'a dyn Fn() -> bool,
    visited: BTreeSet<PathBuf>,
    invoked: BTreeSet<(PathBuf, usize)>,
}
impl Evaluator<'_> {
    fn root(&mut self, path: &Path, environment: Environment) {
        let Some(final_env) = self.file(path, environment) else {
            return;
        };
        // Analyze uninvoked function bodies with the final environment, without exporting effects.
        let deferred = final_env
            .bindings
            .values()
            .flat_map(|r| r.definitions.iter().cloned())
            .collect::<Vec<_>>();
        for target in deferred {
            let key = (
                target.path.clone(),
                target.definition.def_span.start.offset(),
            );
            if !self.invoked.contains(&key) {
                self.function(&target, final_env.clone());
            }
        }
    }
    fn point(&mut self, path: &Path, offset: usize, env: &Environment) {
        let cost = env.bindings.len().max(1);
        if self.point_budget < cost {
            self.remaining = 0;
            self.result.incomplete = true;
            return;
        }
        self.point_budget -= cost;
        let points = self.result.points.entry(path.to_path_buf()).or_default();
        if let Some(previous) = points.get_mut(&offset) {
            previous.join(env);
        } else {
            points.insert(offset, env.clone());
        }
    }
    fn file(&mut self, path: &Path, mut env: Environment) -> Option<Environment> {
        let key = (path.to_path_buf(), None);
        if self.stack.contains(&key) || self.stack.len() >= 64 {
            env.invalidate();
            return Some(env);
        }
        let Some(file) = self.files.get(path).cloned() else {
            env.invalidate();
            return Some(env);
        };
        self.visited.insert(path.to_path_buf());
        self.point(path, 0, &env);
        self.stack.push(key);
        let result = self.sequence(path, &file.events, env).boundary();
        self.stack.pop();
        result
    }
    fn function(
        &mut self,
        target: &WorkspaceFunctionDefinition,
        mut env: Environment,
    ) -> Option<Environment> {
        let start = target.definition.def_span.start.offset();
        let key = (target.path.clone(), Some(start));
        if self.stack.contains(&key) || self.stack.len() >= 64 {
            env.invalidate();
            return Some(env);
        }
        let Some(file) = self.files.get(&target.path).cloned() else {
            env.invalidate();
            return Some(env);
        };
        let Some(body) = file.bodies.get(&start) else {
            env.invalidate();
            return Some(env);
        };
        self.invoked.insert((target.path.clone(), start));
        self.stack.push(key);
        let result = self.sequence(&target.path, body, env).boundary();
        self.stack.pop();
        result
    }
    fn sequence(&mut self, path: &Path, events: &[Event], environment: Environment) -> Flow {
        let mut flow = Flow {
            active: Some(environment),
            returned: None,
            stopped: None,
        };
        for event in events {
            let Some(mut env) = flow.active.take() else {
                break;
            };
            if self.remaining == 0 || (self.cancelled)() {
                self.result.incomplete = true;
                env.invalidate();
                flow.active = Some(env);
                break;
            }
            self.remaining -= 1;
            match event {
                Event::Point(offset) => self.point(path, *offset, &env),
                Event::Define(definition) => {
                    Arc::make_mut(&mut env.bindings).insert(
                        definition.name.to_string(),
                        WorkspaceFunctionResolution {
                            definitions: vec![WorkspaceFunctionDefinition {
                                path: path.to_path_buf(),
                                definition: definition.clone(),
                            }],
                            loaders: self.chain.iter().cloned().collect(),
                            ..Default::default()
                        },
                    );
                }
                Event::Source(targets) => {
                    if let Some(targets) = targets {
                        self.chain.push(path.to_path_buf());
                        let mut next = Some(env);
                        for target in targets {
                            if let Some(environment) = next.take() {
                                next = self.file(target, environment);
                            }
                        }
                        self.chain.pop();
                        flow.active = next;
                        continue;
                    }
                    env.invalidate();
                }
                Event::Call {
                    name,
                    span,
                    enclosing,
                    shell,
                } => {
                    let resolution = if *shell {
                        env.lookup(name)
                    } else {
                        WorkspaceFunctionResolution::absent(false)
                    };
                    let key = (path.to_path_buf(), span.start.offset());
                    if let Some(call) = self.result.calls.get_mut(&key) {
                        call.resolution.join(&resolution);
                    } else {
                        self.result.calls.insert(
                            key,
                            WorkspaceFunctionCall {
                                path: path.to_path_buf(),
                                span: *span,
                                enclosing: enclosing.clone(),
                                resolution: resolution.clone(),
                            },
                        );
                    }
                    if *shell && !resolution.definitions.is_empty() {
                        let mut next = (resolution.may_be_absent || resolution.incomplete)
                            .then(|| env.clone());
                        for target in &resolution.definitions {
                            if let Some(result) = self.function(target, env.clone()) {
                                merge(&mut next, result);
                            }
                        }
                        flow.active = next;
                        continue;
                    }
                }
                Event::Branch(branches) => {
                    for branch in branches {
                        let branch = self.sequence(path, branch, env.clone());
                        if let Some(active) = branch.active {
                            merge(&mut flow.active, active);
                        }
                        if let Some(returned) = branch.returned {
                            merge(&mut flow.returned, returned);
                        }
                        if let Some(stopped) = branch.stopped {
                            merge(&mut flow.stopped, stopped);
                        }
                    }
                    continue;
                }
                Event::Repeat(body) => {
                    // Join zero iterations and repeat until function bindings stabilize.
                    let mut head = env;
                    for iteration in 0..8 {
                        let body = self.sequence(path, body, head.clone());
                        if let Some(returned) = body.returned {
                            merge(&mut flow.returned, returned);
                        }
                        let previous = head.clone();
                        if let Some(active) = body.active {
                            head.join(&active);
                        }
                        if let Some(stopped) = body.stopped {
                            head.join(&stopped);
                        }
                        if head == previous {
                            break;
                        }
                        if iteration == 7 {
                            head.invalidate();
                        }
                    }
                    flow.active = Some(head);
                    continue;
                }
                Event::Finally(body, always) => {
                    let body = self.sequence(path, body, env);
                    for (environment, returning, stopping) in [
                        (body.active, false, false),
                        (body.returned, true, false),
                        (body.stopped, false, true),
                    ] {
                        if let Some(environment) = environment {
                            let final_flow = self.sequence(path, always, environment);
                            if let Some(active) = final_flow.active {
                                merge(
                                    if returning {
                                        &mut flow.returned
                                    } else if stopping {
                                        &mut flow.stopped
                                    } else {
                                        &mut flow.active
                                    },
                                    active,
                                );
                            }
                            if let Some(returned) = final_flow.returned {
                                merge(&mut flow.returned, returned);
                            }
                            if let Some(stopped) = final_flow.stopped {
                                merge(&mut flow.stopped, stopped);
                            }
                        }
                    }
                    continue;
                }
                Event::LoopStop => {
                    merge(&mut flow.stopped, env);
                    continue;
                }
                Event::Isolated(events) => {
                    self.sequence(path, events, env.clone());
                }
                Event::Clear(names) => {
                    for name in names {
                        Arc::make_mut(&mut env.bindings)
                            .insert(name.clone(), WorkspaceFunctionResolution::absent(false));
                    }
                }
                Event::Unknown => env.invalidate(),
                Event::Return => {
                    merge(&mut flow.returned, env);
                    continue;
                }
                Event::Exit => continue,
            }
            flow.active = Some(env);
        }
        flow
    }
}
