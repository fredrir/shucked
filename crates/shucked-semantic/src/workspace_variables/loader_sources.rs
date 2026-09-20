use super::*;
use crate::cfg::{CommandId, RecordedCommandKind, RecordedCommandRange};
use crate::{BindingId, SpanKey};

pub(super) fn project(facts: &mut FileVariableFacts, model: &SemanticModel) {
    if !facts
        .source_effects
        .iter()
        .any(|effect| effect.enclosing_function.is_some())
    {
        return;
    }
    let mut projection = Projection {
        model,
        effects: facts.source_effects.clone(),
        expanded: Vec::new(),
        origins: BTreeMap::new(),
        shadows: BTreeMap::new(),
        remaining: 4096,
        active: rustc_hash::FxHashSet::default(),
    };
    projection.sequence(
        model.recorded_program().file_commands(),
        None,
        false,
        &BTreeSet::new(),
    );
    facts.source_effects.extend(projection.expanded);
    facts.source_shadows = projection.shadows;
}

pub(crate) struct LoaderImportSite {
    pub(crate) source: Span,
    pub(crate) call: Span,
    pub(crate) conditional: bool,
    pub(crate) locals: BTreeSet<Name>,
}

pub(crate) fn import_sites(model: &SemanticModel) -> Vec<LoaderImportSite> {
    if !model.source_refs().iter().any(|reference| {
        model
            .enclosing_function_scope(model.scope_at(reference.span.start.offset()))
            .is_some()
    }) {
        return Vec::new();
    }
    let effects = model
        .source_refs()
        .iter()
        .map(|reference| CallFactSourceEffect {
            path: None,
            span: reference.span,
            conditional: reference.conditionally_executed,
            enclosing_function: None,
            persistent: true,
        })
        .collect();
    let mut projection = Projection {
        model,
        effects,
        expanded: Vec::new(),
        origins: BTreeMap::new(),
        shadows: BTreeMap::new(),
        remaining: 4096,
        active: rustc_hash::FxHashSet::default(),
    };
    projection.sequence(
        model.recorded_program().file_commands(),
        None,
        false,
        &BTreeSet::new(),
    );
    let blocked = projection
        .expanded
        .iter()
        .enumerate()
        .filter_map(|(index, effect)| {
            (!projection
                .origins
                .contains_key(&(projection.effects.len() + index)))
            .then_some(SpanKey::new(effect.span))
        })
        .collect::<rustc_hash::FxHashSet<_>>();
    projection
        .expanded
        .into_iter()
        .enumerate()
        .filter_map(|(index, effect)| {
            if blocked.contains(&SpanKey::new(effect.span)) {
                return None;
            }
            let index = projection.effects.len() + index;
            Some(LoaderImportSite {
                source: *projection.origins.get(&index)?,
                call: effect.span,
                conditional: effect.conditional,
                locals: projection.shadows.remove(&index).unwrap_or_default(),
            })
        })
        .collect()
}

struct Projection<'a> {
    model: &'a SemanticModel,
    effects: Vec<CallFactSourceEffect>,
    expanded: Vec<CallFactSourceEffect>,
    origins: BTreeMap<usize, Span>,
    shadows: BTreeMap<usize, BTreeSet<Name>>,
    remaining: usize,
    active: rustc_hash::FxHashSet<BindingId>,
}

impl Projection<'_> {
    fn sequence(
        &mut self,
        range: RecordedCommandRange,
        call: Option<Span>,
        conditional: bool,
        locals: &BTreeSet<Name>,
    ) {
        for id in self.model.recorded_program().commands_in(range) {
            self.command(*id, call, conditional, locals);
        }
    }

    fn unknown(&mut self, span: Span, conditional: bool) {
        self.expanded.push(CallFactSourceEffect {
            path: None,
            span,
            conditional,
            enclosing_function: None,
            persistent: true,
        });
    }

    fn command(
        &mut self,
        id: CommandId,
        call: Option<Span>,
        conditional: bool,
        locals: &BTreeSet<Name>,
    ) {
        let program = self.model.recorded_program();
        let command = program.command(id);
        if command.background
            || matches!(
                command.kind,
                RecordedCommandKind::Subshell { .. } | RecordedCommandKind::Pipeline { .. }
            )
        {
            return;
        }
        if self.remaining == 0 {
            self.unknown(call.unwrap_or(command.span), conditional);
            return;
        }
        self.remaining -= 1;
        match command.kind {
            RecordedCommandKind::Linear => {
                if let Some(call) = call
                    && let Some(effect) = self
                        .effects
                        .iter()
                        .find(|effect| effect.span == command.span)
                {
                    let mut effect = effect.clone();
                    let origin = effect.span;
                    effect.span = call;
                    effect.enclosing_function = None;
                    effect.conditional |= conditional;
                    let source_index = self.effects.len() + self.expanded.len();
                    self.origins.insert(source_index, origin);
                    self.expanded.push(effect);
                    self.shadows
                        .entry(source_index)
                        .or_default()
                        .extend(locals.iter().cloned());
                    return;
                }
                let Some(info) = program.command_info_for_span(command.span) else {
                    return;
                };
                let Some(word) = info.original_words.first() else {
                    return;
                };
                let Some(binding) = self
                    .model
                    .visible_function_call_bindings()
                    .get(&SpanKey::new(word.span))
                    .copied()
                else {
                    return;
                };
                let Some(scope) = program.function_body_scopes.get(&binding).copied() else {
                    return;
                };
                let call = call.unwrap_or(command.span);
                if self.active.len() >= 32 || !self.active.insert(binding) {
                    self.unknown(call, conditional);
                    return;
                }
                let mut locals = locals.clone();
                let mut unknown = info.source_path_environment_unknown;
                for binding in self.model.bindings().iter().filter(|binding| {
                    self.model.enclosing_function_scope(binding.scope) == Some(scope)
                        && self
                            .model
                            .innermost_transient_scope_within_function(binding.scope)
                            .is_none()
                }) {
                    if binding.attributes.contains(BindingAttributes::LOCAL) {
                        locals.insert(binding.name.clone());
                    } else if !matches!(
                        binding.kind,
                        BindingKind::FunctionDefinition | BindingKind::Imported
                    ) {
                        unknown = true;
                    }
                }
                // Early exits and runtime mutation make the loader's exports uncertain.
                unknown |= program.commands().iter().any(|command| {
                    command
                        .scope
                        .is_some_and(|s| self.model.enclosing_function_scope(s) == Some(scope))
                        && (matches!(
                            command.kind,
                            RecordedCommandKind::Return | RecordedCommandKind::Exit
                        ) || command.command_info.is_some_and(|id| {
                            matches!(
                                program.command_info(id).static_callee.as_deref(),
                                Some("eval" | "unset")
                            )
                        }))
                });
                if unknown {
                    self.unknown(call, conditional);
                } else {
                    self.sequence(
                        program.function_body(scope),
                        Some(call),
                        conditional,
                        &locals,
                    );
                }
                self.active.remove(&binding);
            }
            RecordedCommandKind::List { first, rest } => {
                self.command(first, call, conditional, locals);
                for item in program.list_items(rest) {
                    self.command(item.command, call, true, locals);
                }
            }
            RecordedCommandKind::If {
                condition,
                then_branch,
                elif_branches,
                else_branch,
            } => {
                self.sequence(condition, call, conditional, locals);
                self.sequence(then_branch, call, true, locals);
                for branch in program.elif_branches(elif_branches) {
                    self.sequence(branch.condition, call, true, locals);
                    self.sequence(branch.body, call, true, locals);
                }
                self.sequence(else_branch, call, true, locals);
            }
            RecordedCommandKind::BraceGroup { body } => {
                self.sequence(body, call, conditional, locals)
            }
            RecordedCommandKind::Always { body, always_body } => {
                self.sequence(body, call, conditional, locals);
                self.sequence(always_body, call, conditional, locals);
            }
            RecordedCommandKind::While { condition, body }
            | RecordedCommandKind::Until { condition, body } => {
                self.sequence(condition, call, conditional, locals);
                self.sequence(body, call, true, locals);
            }
            RecordedCommandKind::For { body }
            | RecordedCommandKind::Select { body }
            | RecordedCommandKind::ArithmeticFor { body } => {
                self.sequence(body, call, true, locals)
            }
            RecordedCommandKind::Case { arms } => {
                for arm in program.case_arms(arms) {
                    self.sequence(arm.commands, call, true, locals);
                }
            }
            _ => {}
        }
    }
}
