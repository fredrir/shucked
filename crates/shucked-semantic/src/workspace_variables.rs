//! Cross-file variable usage and editor navigation.
//!
//! Known conditional imports contribute possible uses. Called loaders contribute
//! source effects at their call sites; function locals and transient scopes stay isolated.

pub(crate) mod loader_sources;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::{
    Binding, BindingAttributes, BindingKind, BindingOrigin, CallFactSourceEffect,
    EditorSymbolTarget, ReferenceKind, SemanticModel,
};
use shucked_ast::{Name, Span};

/// Canonical workspace identity, including files that exist only in an editor.
pub fn canonical_workspace_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }

    let normalized = normalize_path(path);
    let mut ancestor = normalized.as_path();
    let mut suffix = Vec::new();
    while let Some(name) = ancestor.file_name() {
        suffix.push(name.to_owned());
        let Some(parent) = ancestor.parent() else {
            break;
        };
        if let Ok(mut canonical) = std::fs::canonicalize(parent) {
            for component in suffix.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }
        ancestor = parent;
    }
    normalized
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// A variable target under the active editor cursor.
pub struct WorkspaceVariableTarget {
    selection: Option<Span>,
    name: Name,
    cutoff: usize,
    local_family: bool,
}

/// Known origins and consumers of a selected variable assignment or read.
#[derive(Clone, Debug)]
pub struct WorkspaceVariableExplanation {
    /// Variable name.
    pub name: Name,
    /// Assignments that may reach the selected read.
    pub definitions: Vec<WorkspaceVariableOccurrence>,
    /// Reads reached by the selected assignments.
    pub references: Vec<WorkspaceVariableOccurrence>,
    /// Some source effects could not be followed.
    pub incomplete: bool,
    /// Conditional execution contributes possible uses.
    pub conditional: bool,
}

/// One path/span pair returned by a workspace variable query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceVariableOccurrence {
    /// File containing the occurrence.
    pub path: PathBuf,
    /// Source-backed variable span.
    pub span: Span,
}

#[derive(Clone, Debug)]
struct VariableDefinition {
    name: Name,
    definition_span: Span,
    occurrence_span: Span,
    definite_write: bool,
}

#[derive(Clone, Debug)]
struct VariableReference {
    name: Name,
    occurrence_span: Span,
    cutoff: usize,
    local_family: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct IncomingVariableSource {
    source_index: usize,
    execution_offset: usize,
    shadows: BTreeSet<Name>,
    parent: PathBuf,
    cutoff: usize,
}

/// Reusable variable facts for one file and its resolved source effects.
#[derive(Clone, Debug, Default)]
pub struct FileVariableFacts {
    definitions: Vec<VariableDefinition>,
    references: Vec<VariableReference>,
    source_effects: Vec<CallFactSourceEffect>,
    source_shadows: BTreeMap<usize, BTreeSet<Name>>,
    clears: BTreeMap<Name, Vec<usize>>,
    return_offsets: Vec<usize>,
}

impl FileVariableFacts {
    /// Project file facts without resolving cross-file reads.
    pub fn project(model: &SemanticModel, source_effects: &[CallFactSourceEffect]) -> Self {
        let unconditional = crate::function_resolution::collect_unconditional_bindings(
            &model.recorded_program,
            &model.command_bindings,
        );
        let definitions = model
            .bindings()
            .iter()
            .filter(|binding| persistent_file_variable_binding(model, binding))
            .map(|binding| VariableDefinition {
                name: binding.name.clone(),
                definition_span: binding_definition_span(binding),
                occurrence_span: binding_occurrence_span(binding),
                definite_write: unconditional.contains(&binding.id)
                    && (matches!(binding.kind, BindingKind::Assignment)
                        || (matches!(binding.kind, BindingKind::Declaration(_))
                            && binding
                                .attributes
                                .contains(BindingAttributes::DECLARATION_INITIALIZED)))
                    && !binding
                        .attributes
                        .intersects(BindingAttributes::ARRAY | BindingAttributes::ASSOC),
            })
            .collect();
        let references = model
            .references()
            .iter()
            .filter_map(|reference| {
                if !source_backed_reference(reference.kind, reference.name_span)
                    || !valid_variable_name(reference.name.as_str())
                {
                    return None;
                }
                let local_family = match model.resolved_binding(reference.id) {
                    Some(binding) if persistent_file_variable_binding(model, binding) => true,
                    Some(_) => return None,
                    None if model.name_is_predefined_runtime(reference.name.as_str()) => {
                        return None;
                    }
                    None => false,
                };
                Some(VariableReference {
                    name: reference.name.clone(),
                    occurrence_span: reference.name_span,
                    cutoff: if model.enclosing_function_scope(reference.scope).is_some() {
                        usize::MAX
                    } else if model
                        .self_referential_assignment_refs
                        .contains(&reference.id)
                    {
                        model
                            .bindings()
                            .iter()
                            .filter(|binding| {
                                binding.name == reference.name
                                    && binding.span.start.offset()
                                        < reference.name_span.start.offset()
                            })
                            .map(|binding| binding.span.start.offset())
                            .max()
                            .unwrap_or(reference.name_span.start.offset())
                    } else {
                        reference.name_span.start.offset()
                    },
                    local_family,
                })
            })
            .collect();

        let mut clears = BTreeMap::<Name, Vec<usize>>::new();
        for ((scope, name), offsets) in &model.cleared_variables {
            if model.enclosing_function_scope(*scope).is_none()
                && model
                    .innermost_transient_scope_within_function(*scope)
                    .is_none()
            {
                clears.entry(name.clone()).or_default().extend(offsets);
            }
        }
        let return_offsets = model
            .commands()
            .iter()
            .filter_map(|command| {
                if model.command_kind(*command)
                    != crate::CommandKind::Builtin(crate::BuiltinCommandKind::Return)
                {
                    return None;
                }
                let context = model.command_context(*command)?;
                (model.enclosing_function_scope(context.scope()).is_none()
                    && model
                        .innermost_transient_scope_within_function(context.scope())
                        .is_none())
                .then_some(model.command_span(*command).start.offset())
            })
            .collect();
        let mut facts = Self {
            source_shadows: BTreeMap::new(),
            return_offsets,
            clears,
            definitions,
            references,
            source_effects: source_effects.to_vec(),
        };
        loader_sources::project(&mut facts, model);
        facts
    }

    fn has_definition(&self, name: &Name) -> bool {
        self.definitions
            .iter()
            .any(|definition| definition.name == *name)
    }

    fn has_definition_before(&self, name: &Name, cutoff: usize) -> bool {
        self.definitions.iter().any(|definition| {
            definition.name == *name && definition.definition_span.start.offset() < cutoff
        })
    }

    fn resolved_top_level_source_edges(
        &self,
        cutoff: usize,
    ) -> impl Iterator<Item = (&Path, usize, usize)> {
        self.source_effects
            .iter()
            .enumerate()
            .filter_map(move |(index, effect)| {
                (effect.persistent
                    && effect.enclosing_function.is_none()
                    && effect.span.start.offset() < cutoff)
                    .then_some(
                        effect
                            .path
                            .as_deref()
                            .map(|path| (path, effect.span.start.offset(), index)),
                    )
                    .flatten()
            })
    }

    fn unconditional_top_level_source_paths(&self, cutoff: usize) -> impl Iterator<Item = &Path> {
        self.source_effects.iter().filter_map(move |effect| {
            (effect.persistent
                && !effect.conditional
                && effect.enclosing_function.is_none()
                && effect.span.start.offset() < cutoff)
                .then_some(effect.path.as_deref())
                .flatten()
        })
    }

    fn has_ambiguous_top_level_source_before(&self, cutoff: usize) -> bool {
        self.source_effects.iter().any(|effect| {
            effect.persistent
                && effect.enclosing_function.is_none()
                && effect.span.start.offset() < cutoff
                && (effect.conditional || effect.path.is_none())
        })
    }
}

/// Compact variable projection over the same files as the workspace function index.
#[derive(Default)]
pub struct WorkspaceVariableIndex {
    files: BTreeMap<PathBuf, FileVariableFacts>,
    incoming: BTreeMap<PathBuf, BTreeSet<IncomingVariableSource>>,
}

/// A source-backed binding consumed by another workspace file.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorkspaceConsumedBinding {
    /// Variable name.
    pub name: Name,
    /// Start of the binding's name span, in bytes.
    pub start: usize,
    /// End of the binding's name span, in bytes.
    pub end: usize,
}

impl WorkspaceConsumedBinding {
    fn new(name: Name, span: Span) -> Self {
        Self {
            name,
            start: span.start.offset(),
            end: span.end.offset(),
        }
    }
}

/// Cross-file reads of individual bindings, keyed by canonical path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceVariableUsage {
    consumed: BTreeMap<PathBuf, BTreeSet<WorkspaceConsumedBinding>>,
}

impl WorkspaceVariableUsage {
    /// Bindings read by another file in the same source environment.
    pub fn consumed_bindings(&self, path: &Path) -> Vec<WorkspaceConsumedBinding> {
        let path = canonical_workspace_path(path);
        self.consumed
            .get(&path)
            .map(|bindings| bindings.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Names read by another file in the same source environment.
    pub fn consumed_names(&self, path: &Path) -> Vec<Name> {
        self.consumed_bindings(path)
            .into_iter()
            .map(|binding| binding.name)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Update binding locations after edits; discard replaced bindings.
    pub fn remap_file_bindings(
        &mut self,
        path: &Path,
        map_range: impl Fn(std::ops::Range<usize>) -> Option<std::ops::Range<usize>>,
    ) {
        let path = canonical_workspace_path(path);
        if let Some(bindings) = self.consumed.get_mut(&path) {
            *bindings = bindings
                .iter()
                .filter_map(|binding| {
                    let range = map_range(binding.start..binding.end)?;
                    Some(WorkspaceConsumedBinding {
                        name: binding.name.clone(),
                        start: range.start,
                        end: range.end,
                    })
                })
                .collect();
        }
    }

    pub(crate) fn apply(&self, path: &Path, model: &mut SemanticModel) {
        let consumed = self
            .consumed_bindings(path)
            .into_iter()
            .collect::<BTreeSet<_>>();
        for binding in &mut model.bindings {
            if consumed.contains(&WorkspaceConsumedBinding::new(
                binding.name.clone(),
                binding.span,
            )) {
                binding.attributes |= BindingAttributes::WORKSPACE_CONSUMED;
            }
        }
        model.heuristic_unused_assignments.retain(|id| {
            !model.bindings[id.index()]
                .attributes
                .contains(BindingAttributes::WORKSPACE_CONSUMED)
        });
        model.invalidate_semantic_caches();
    }
}

#[derive(Default)]
struct ReachingDefinitions {
    bindings: BTreeSet<(PathBuf, WorkspaceConsumedBinding)>,
    inherits: bool,
}

impl WorkspaceVariableIndex {
    /// Replace the variable facts for a canonical file path.
    pub fn insert(
        &mut self,
        path: PathBuf,
        model: &SemanticModel,
        source_effects: &[CallFactSourceEffect],
    ) {
        self.insert_facts(path, FileVariableFacts::project(model, source_effects));
    }

    /// Replace one file with previously projected facts.
    pub fn insert_facts(&mut self, path: PathBuf, facts: FileVariableFacts) {
        if let Some(previous) = self.files.remove(&path) {
            for (target, cutoff, source_index) in
                previous.resolved_top_level_source_edges(usize::MAX)
            {
                if let Some(sources) = self.incoming.get_mut(target) {
                    sources.remove(&IncomingVariableSource {
                        source_index,
                        execution_offset: previous.source_effects[source_index].span.end.offset(),
                        shadows: previous
                            .source_shadows
                            .get(&source_index)
                            .cloned()
                            .unwrap_or_default(),
                        parent: path.clone(),
                        cutoff,
                    });
                }
            }
        }
        for (target, cutoff, source_index) in facts.resolved_top_level_source_edges(usize::MAX) {
            self.incoming
                .entry(target.to_path_buf())
                .or_default()
                .insert(IncomingVariableSource {
                    source_index,
                    execution_offset: facts.source_effects[source_index].span.end.offset(),
                    shadows: facts
                        .source_shadows
                        .get(&source_index)
                        .cloned()
                        .unwrap_or_default(),
                    parent: path.clone(),
                    cutoff,
                });
        }
        self.files.insert(path, facts);
    }

    /// Resolve cross-file reads in source order. Unknown source effects do not prove usage.
    pub fn usage(&self, is_cancelled: &dyn Fn() -> bool) -> Option<WorkspaceVariableUsage> {
        let mut usage = WorkspaceVariableUsage::default();
        for (path, facts) in &self.files {
            for reference in &facts.references {
                if is_cancelled() {
                    return None;
                }
                let Some(definitions) = self.reaching_variable_paths(
                    path,
                    &reference.name,
                    reference.cutoff,
                    None,
                    true,
                    &mut BTreeSet::new(),
                    is_cancelled,
                ) else {
                    continue;
                };
                for (definition, binding) in definitions.bindings {
                    if definition != *path {
                        usage
                            .consumed
                            .entry(definition)
                            .or_default()
                            .insert(binding);
                    }
                }
            }
        }
        (!is_cancelled()).then_some(usage)
    }

    #[allow(clippy::too_many_arguments)]
    fn reaching_variable_paths(
        &self,
        path: &Path,
        name: &Name,
        cutoff: usize,
        source_limit: Option<usize>,
        inherit: bool,
        active: &mut BTreeSet<PathBuf>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<ReachingDefinitions> {
        if is_cancelled() || !active.insert(path.to_path_buf()) {
            return None;
        }
        let result = (|| {
            let mut result = self.reaching_variable_paths_inner(
                path,
                name,
                cutoff,
                source_limit,
                inherit,
                active,
                is_cancelled,
            )?;
            if cutoff == usize::MAX {
                // Sourced files also export values at top-level return sites.
                for offset in &self.files.get(path)?.return_offsets {
                    let returned = self.reaching_variable_paths_inner(
                        path,
                        name,
                        *offset,
                        None,
                        inherit,
                        active,
                        is_cancelled,
                    )?;
                    result.bindings.extend(returned.bindings);
                    result.inherits |= returned.inherits;
                }
            }
            Some(result)
        })();
        active.remove(path);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn reaching_variable_paths_inner(
        &self,
        path: &Path,
        name: &Name,
        cutoff: usize,
        source_limit: Option<usize>,
        inherit: bool,
        active: &mut BTreeSet<PathBuf>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<ReachingDefinitions> {
        enum Effect<'a> {
            Write(&'a VariableDefinition),
            Source(usize, &'a CallFactSourceEffect),
            Clear,
        }
        let facts = self.files.get(path)?;
        let mut effects = Vec::new();
        for definition in &facts.definitions {
            if definition.name == *name && definition.definition_span.end.offset() <= cutoff {
                effects.push((
                    definition.definition_span.end.offset(),
                    Effect::Write(definition),
                ));
            }
        }
        for (index, effect) in facts.source_effects.iter().enumerate() {
            if effect.persistent
                && effect.enclosing_function.is_none()
                && effect.span.end.offset() <= cutoff
                && (effect.span.end.offset() < cutoff
                    || source_limit.is_none_or(|limit| index < limit))
            {
                effects.push((effect.span.end.offset(), Effect::Source(index, effect)));
            }
        }
        if let Some(offsets) = facts.clears.get(name) {
            effects.extend(
                offsets
                    .iter()
                    .filter(|offset| **offset < cutoff)
                    .map(|offset| (*offset, Effect::Clear)),
            );
        }
        effects.sort_by_key(|(offset, _)| *offset);
        let mut result = ReachingDefinitions::default();
        for (_, effect) in effects.into_iter().rev() {
            if is_cancelled() {
                return None;
            }
            match effect {
                Effect::Write(definition) => {
                    result.bindings.insert((
                        path.to_path_buf(),
                        WorkspaceConsumedBinding::new(name.clone(), definition.occurrence_span),
                    ));
                    if definition.definite_write {
                        return Some(result);
                    }
                }
                Effect::Clear => return Some(result),
                Effect::Source(index, effect) => {
                    if facts
                        .source_shadows
                        .get(&index)
                        .is_some_and(|names| names.contains(name))
                    {
                        continue;
                    }
                    let provided = self.reaching_variable_paths(
                        effect.path.as_deref()?,
                        name,
                        usize::MAX,
                        None,
                        false,
                        active,
                        is_cancelled,
                    )?;
                    result.bindings.extend(provided.bindings);
                    if !effect.conditional && !provided.inherits {
                        return Some(result);
                    }
                }
            }
        }
        result.inherits = true;
        if inherit && let Some(incoming) = self.incoming.get(path) {
            for source in incoming {
                if source.shadows.contains(name) {
                    continue;
                }
                if is_cancelled() {
                    return None;
                }
                if let Some(inherited) = self.reaching_variable_paths(
                    &source.parent,
                    name,
                    source.execution_offset,
                    Some(source.source_index),
                    true,
                    active,
                    is_cancelled,
                ) {
                    result.bindings.extend(inherited.bindings);
                }
            }
        }
        Some(result)
    }

    /// Explain known uses without requiring a complete rename family.
    pub fn explain(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<WorkspaceVariableExplanation> {
        if is_cancelled() {
            return None;
        }
        let mut incomplete = false;
        let cutoff = self
            .files
            .get(from_path)
            .and_then(|facts| {
                facts.references.iter().find(|reference| {
                    reference.name == target.name
                        && reference.occurrence_span.start.offset() == target.cutoff
                })
            })
            .map_or(target.cutoff, |reference| reference.cutoff);
        let seeds = if let Some(span) = target.selection {
            BTreeSet::from([(
                from_path.to_path_buf(),
                WorkspaceConsumedBinding::new(target.name.clone(), span),
            )])
        } else {
            match self.reaching_variable_paths(
                from_path,
                &target.name,
                cutoff,
                None,
                true,
                &mut BTreeSet::new(),
                is_cancelled,
            ) {
                Some(definitions) => definitions.bindings,
                None => {
                    incomplete = true;
                    BTreeSet::new()
                }
            }
        };
        let definitions = seeds
            .iter()
            .filter_map(|(path, binding)| {
                let definition = self
                    .files
                    .get(path)?
                    .definitions
                    .iter()
                    .find(|definition| {
                        definition.name == binding.name
                            && definition.occurrence_span.start.offset() == binding.start
                            && definition.occurrence_span.end.offset() == binding.end
                    })?;
                Some(WorkspaceVariableOccurrence {
                    path: path.clone(),
                    span: definition.occurrence_span,
                })
            })
            .collect::<Vec<_>>();
        let paths = seeds
            .iter()
            .map(|(path, _)| path.clone())
            .chain(std::iter::once(from_path.to_path_buf()))
            .collect();
        let family = self.environment_paths_from(&paths, is_cancelled)?;
        let mut references = Vec::new();
        let mut conditional = seeds.len() > 1;
        for path in family {
            let Some(facts) = self.files.get(&path) else {
                incomplete = true;
                continue;
            };
            conditional |= facts.source_effects.iter().any(|effect| effect.conditional);
            incomplete |= facts.source_effects.iter().any(|effect| {
                effect.persistent
                    && effect
                        .path
                        .as_ref()
                        .is_none_or(|path| !self.files.contains_key(path))
            });
            for reference in facts
                .references
                .iter()
                .filter(|reference| reference.name == target.name)
            {
                if is_cancelled() {
                    return None;
                }
                match self.reaching_variable_paths(
                    &path,
                    &reference.name,
                    reference.cutoff,
                    None,
                    true,
                    &mut BTreeSet::new(),
                    is_cancelled,
                ) {
                    Some(reaching) if !seeds.is_disjoint(&reaching.bindings) => {
                        references.push(WorkspaceVariableOccurrence {
                            path: path.clone(),
                            span: reference.occurrence_span,
                        })
                    }
                    None => incomplete = true,
                    _ => {}
                }
            }
        }
        if is_cancelled() {
            return None;
        }
        sort_dedup_occurrences(&mut references);
        Some(WorkspaceVariableExplanation {
            name: target.name.clone(),
            definitions,
            references,
            incomplete,
            conditional,
        })
    }

    /// Returns `None` when source effects are ambiguous or the query is cancelled.
    pub fn definitions(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<Vec<WorkspaceVariableOccurrence>> {
        let paths = self.definition_paths(
            from_path,
            &target.name,
            target.cutoff,
            target.local_family,
            is_cancelled,
        )?;
        self.definition_occurrences(&paths, &target.name, is_cancelled)
    }

    /// Returns `None` when the complete reference family cannot be proven or queried.
    pub fn references(
        &self,
        from_path: &Path,
        target: &WorkspaceVariableTarget,
        include_declaration: bool,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<Vec<WorkspaceVariableOccurrence>> {
        let definitions = self.definitions(from_path, target, is_cancelled)?;
        let seed_paths = definitions
            .iter()
            .map(|definition| definition.path.clone())
            .collect::<BTreeSet<_>>();
        if seed_paths.is_empty() {
            return Some(Vec::new());
        }

        let family_paths = self.environment_paths_from(&seed_paths, is_cancelled)?;

        let mut occurrences = Vec::new();
        let mut inherited_cache = BTreeMap::new();
        for path in &family_paths {
            if is_cancelled() {
                return None;
            }
            let Some(facts) = self.files.get(path) else {
                continue;
            };
            if facts.has_ambiguous_top_level_source_before(usize::MAX) {
                return None;
            }
            if include_declaration {
                for definition in &facts.definitions {
                    if is_cancelled() {
                        return None;
                    }
                    if definition.name == target.name {
                        occurrences.push(WorkspaceVariableOccurrence {
                            path: path.clone(),
                            span: definition.occurrence_span,
                        });
                    }
                }
            }
            for reference in &facts.references {
                if is_cancelled() {
                    return None;
                }
                if reference.name != target.name {
                    continue;
                }
                let definition_paths = self.definition_paths_with_cache(
                    path,
                    &reference.name,
                    reference.cutoff,
                    reference.local_family,
                    &mut inherited_cache,
                    is_cancelled,
                )?;
                if definition_paths
                    .iter()
                    .any(|definition_path| seed_paths.contains(definition_path))
                {
                    occurrences.push(WorkspaceVariableOccurrence {
                        path: path.clone(),
                        span: reference.occurrence_span,
                    });
                }
            }
        }
        if is_cancelled() {
            return None;
        }
        sort_dedup_occurrences(&mut occurrences);
        Some(occurrences)
    }

    fn definition_paths(
        &self,
        from_path: &Path,
        name: &Name,
        cutoff: usize,
        local_family: bool,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<BTreeSet<PathBuf>> {
        let mut inherited_cache = BTreeMap::new();
        self.definition_paths_with_cache(
            from_path,
            name,
            cutoff,
            local_family,
            &mut inherited_cache,
            is_cancelled,
        )
    }

    fn definition_paths_with_cache(
        &self,
        from_path: &Path,
        name: &Name,
        cutoff: usize,
        local_family: bool,
        inherited_cache: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<BTreeSet<PathBuf>> {
        let mut paths = self.descendants_before(
            from_path,
            cutoff,
            local_family || cutoff == usize::MAX,
            is_cancelled,
        )?;

        if !self.paths_have_definition(&paths, name, is_cancelled)? {
            paths =
                self.inherited_definition_paths(from_path, name, inherited_cache, is_cancelled)?;
        }
        let mut definitions = BTreeSet::new();
        for path in paths {
            if is_cancelled() {
                return None;
            }
            if self
                .files
                .get(&path)
                .is_some_and(|facts| facts.has_definition(name))
            {
                definitions.insert(path);
            }
        }
        Some(definitions)
    }

    fn inherited_definition_paths(
        &self,
        path: &Path,
        name: &Name,
        cache: &mut BTreeMap<PathBuf, BTreeSet<PathBuf>>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<BTreeSet<PathBuf>> {
        if let Some(cached) = cache.get(path) {
            return Some(cached.clone());
        }

        let mut inherited = BTreeSet::new();
        let mut visited = BTreeSet::from([path.to_path_buf()]);
        let mut pending = vec![path.to_path_buf()];
        while let Some(current) = pending.pop() {
            if is_cancelled() {
                return None;
            }
            let Some(sources) = self.incoming.get(&current) else {
                continue;
            };
            for source in sources {
                if is_cancelled() {
                    return None;
                }
                let candidates =
                    self.descendants_before(&source.parent, source.cutoff, true, is_cancelled)?;
                let mut definitions = BTreeSet::new();
                for candidate in candidates {
                    if is_cancelled() {
                        return None;
                    }
                    if self.files.get(&candidate).is_some_and(|facts| {
                        if candidate == source.parent {
                            facts.has_definition_before(name, source.cutoff)
                        } else {
                            facts.has_definition(name)
                        }
                    }) {
                        definitions.insert(candidate);
                    }
                }
                if definitions.is_empty() && visited.insert(source.parent.clone()) {
                    pending.push(source.parent.clone());
                }
                inherited.extend(definitions);
            }
        }
        cache.insert(path.to_path_buf(), inherited.clone());
        Some(inherited)
    }

    fn descendants_before(
        &self,
        root: &Path,
        cutoff: usize,
        include_root: bool,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<BTreeSet<PathBuf>> {
        let mut paths = BTreeSet::new();
        let mut active = BTreeSet::new();
        self.collect_descendants(
            root,
            cutoff,
            include_root,
            &mut active,
            &mut paths,
            is_cancelled,
        )?;
        Some(paths)
    }

    fn collect_descendants(
        &self,
        path: &Path,
        cutoff: usize,
        include_path: bool,
        active: &mut BTreeSet<PathBuf>,
        paths: &mut BTreeSet<PathBuf>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<()> {
        if is_cancelled() {
            return None;
        }
        if !active.insert(path.to_path_buf()) {
            return Some(());
        }
        if include_path {
            paths.insert(path.to_path_buf());
        }
        let Some(facts) = self.files.get(path) else {
            active.remove(path);
            return Some(());
        };
        if facts.has_ambiguous_top_level_source_before(cutoff) {
            active.remove(path);
            return None;
        }
        let targets = facts
            .unconditional_top_level_source_paths(cutoff)
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        for target in targets {
            if is_cancelled() {
                active.remove(path);
                return None;
            }
            paths.insert(target.clone());
            self.collect_descendants(&target, usize::MAX, true, active, paths, is_cancelled)?;
        }
        active.remove(path);
        Some(())
    }

    fn environment_paths_from(
        &self,
        paths: &BTreeSet<PathBuf>,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<BTreeSet<PathBuf>> {
        let mut ancestors = paths.clone();
        let mut pending = paths.iter().cloned().collect::<Vec<_>>();
        while let Some(target) = pending.pop() {
            if is_cancelled() {
                return None;
            }
            let Some(sources) = self.incoming.get(&target) else {
                continue;
            };
            for source in sources {
                if is_cancelled() {
                    return None;
                }
                if ancestors.insert(source.parent.clone()) {
                    pending.push(source.parent.clone());
                }
            }
        }

        let mut environment = ancestors.clone();
        let mut pending = ancestors.into_iter().collect::<Vec<_>>();
        while let Some(parent) = pending.pop() {
            if is_cancelled() {
                return None;
            }
            let Some(facts) = self.files.get(&parent) else {
                continue;
            };
            for target in facts
                .resolved_top_level_source_edges(usize::MAX)
                .map(|(path, _, _)| path)
            {
                if is_cancelled() {
                    return None;
                }
                if environment.insert(target.to_path_buf()) {
                    pending.push(target.to_path_buf());
                }
            }
        }
        Some(environment)
    }

    fn paths_have_definition(
        &self,
        paths: &BTreeSet<PathBuf>,
        name: &Name,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<bool> {
        for path in paths {
            if is_cancelled() {
                return None;
            }
            if self
                .files
                .get(path)
                .is_some_and(|facts| facts.has_definition(name))
            {
                return Some(true);
            }
        }
        Some(false)
    }

    fn definition_occurrences(
        &self,
        paths: &BTreeSet<PathBuf>,
        name: &Name,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Option<Vec<WorkspaceVariableOccurrence>> {
        let mut occurrences = Vec::new();
        for path in paths {
            if is_cancelled() {
                return None;
            }
            let Some(facts) = self.files.get(path) else {
                continue;
            };
            for definition in &facts.definitions {
                if is_cancelled() {
                    return None;
                }
                if definition.name == *name {
                    occurrences.push(WorkspaceVariableOccurrence {
                        path: path.clone(),
                        span: definition.definition_span,
                    });
                }
            }
        }
        sort_dedup_occurrences(&mut occurrences);
        Some(occurrences)
    }
}

/// Project an editor symbol into a workspace variable query.
pub fn variable_target(
    model: &SemanticModel,
    target: &EditorSymbolTarget,
) -> Option<WorkspaceVariableTarget> {
    match target {
        EditorSymbolTarget::Binding(binding_id) => {
            let binding = model.binding(*binding_id);
            persistent_file_variable_binding(model, binding).then(|| WorkspaceVariableTarget {
                selection: Some(binding_occurrence_span(binding)),
                name: binding.name.clone(),
                cutoff: usize::MAX,
                local_family: true,
            })
        }
        EditorSymbolTarget::Reference(reference_id) => {
            let reference = model.reference(*reference_id);
            if !source_backed_reference(reference.kind, reference.name_span)
                || !valid_variable_name(reference.name.as_str())
            {
                return None;
            }
            let local_family = match model.resolved_binding(*reference_id) {
                Some(binding) if persistent_file_variable_binding(model, binding) => true,
                Some(_) => return None,
                None if model.name_is_predefined_runtime(reference.name.as_str()) => return None,
                None => false,
            };
            Some(WorkspaceVariableTarget {
                selection: None,
                name: reference.name.clone(),
                cutoff: if model.enclosing_function_scope(reference.scope).is_some() {
                    usize::MAX
                } else {
                    reference.name_span.start.offset()
                },
                local_family,
            })
        }
        EditorSymbolTarget::FunctionCall(_) | EditorSymbolTarget::RuntimeName(_) => None,
    }
}

fn persistent_file_variable_binding(model: &SemanticModel, binding: &Binding) -> bool {
    !matches!(
        binding.kind,
        BindingKind::FunctionDefinition | BindingKind::Imported | BindingKind::Nameref
    ) && !binding.attributes.contains(BindingAttributes::NAMEREF)
        && model.enclosing_function_scope(binding.scope).is_none()
        && model
            .innermost_transient_scope_within_function(binding.scope)
            .is_none()
        && valid_variable_name(binding.name.as_str())
}

fn binding_definition_span(binding: &Binding) -> Span {
    match binding.origin {
        BindingOrigin::Assignment {
            definition_span, ..
        }
        | BindingOrigin::LoopVariable {
            definition_span, ..
        }
        | BindingOrigin::ParameterDefaultAssignment {
            definition_span, ..
        }
        | BindingOrigin::Imported { definition_span }
        | BindingOrigin::FunctionDefinition { definition_span }
        | BindingOrigin::BuiltinTarget {
            definition_span, ..
        }
        | BindingOrigin::ArithmeticAssignment {
            definition_span, ..
        }
        | BindingOrigin::Declaration { definition_span }
        | BindingOrigin::Nameref { definition_span } => definition_span,
    }
}

fn binding_occurrence_span(binding: &Binding) -> Span {
    match binding.origin {
        BindingOrigin::ParameterDefaultAssignment { target_span, .. }
        | BindingOrigin::ArithmeticAssignment { target_span, .. } => target_span,
        _ => binding.span,
    }
}

fn source_backed_reference(kind: ReferenceKind, span: Span) -> bool {
    span.start.offset() < span.end.offset()
        && !matches!(
            kind,
            ReferenceKind::DeclarationName
                | ReferenceKind::ImplicitRead
                | ReferenceKind::RequiredRead
        )
}

fn valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn sort_dedup_occurrences(occurrences: &mut Vec<WorkspaceVariableOccurrence>) {
    occurrences.sort_by(|left, right| {
        left.path.cmp(&right.path).then_with(|| {
            (left.span.start.offset(), left.span.end.offset())
                .cmp(&(right.span.start.offset(), right.span.end.offset()))
        })
    });
    occurrences.dedup();
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use shucked_ast::Position;

    use super::*;

    fn span(offset: usize) -> Span {
        Span::from_positions(
            Position::at(1, offset + 1, offset),
            Position::at(1, offset + 2, offset + 1),
        )
    }

    fn definition(name: &str, offset: usize) -> VariableDefinition {
        VariableDefinition {
            name: Name::from(name),
            definition_span: span(offset),
            occurrence_span: span(offset),
            definite_write: true,
        }
    }

    fn reference(name: &str, offset: usize) -> VariableReference {
        VariableReference {
            name: Name::from(name),
            occurrence_span: span(offset),
            cutoff: offset,
            local_family: false,
        }
    }

    fn source_effect(path: Option<&str>, offset: usize, conditional: bool) -> CallFactSourceEffect {
        CallFactSourceEffect {
            path: path.map(PathBuf::from),
            span: span(offset),
            conditional,
            enclosing_function: None,
            persistent: true,
        }
    }

    fn target(name: &str, cutoff: usize, local_family: bool) -> WorkspaceVariableTarget {
        WorkspaceVariableTarget {
            selection: None,
            name: Name::from(name),
            cutoff,
            local_family,
        }
    }

    #[test]
    fn conditional_and_dynamic_sources_make_definition_queries_ambiguous() {
        let never_cancelled = || false;
        let cases = [
            vec![source_effect(Some("vars.sh"), 10, true)],
            vec![
                source_effect(Some("vars.sh"), 5, false),
                source_effect(None, 10, false),
            ],
        ];
        for source_effects in cases {
            let mut index = WorkspaceVariableIndex::default();
            index.insert_facts(
                PathBuf::from("vars.sh"),
                FileVariableFacts {
                    definitions: vec![definition("SHARED", 1)],
                    ..FileVariableFacts::default()
                },
            );
            index.insert_facts(
                PathBuf::from("main.sh"),
                FileVariableFacts {
                    references: vec![reference("SHARED", 20)],
                    source_effects,
                    ..FileVariableFacts::default()
                },
            );

            assert!(
                index
                    .definitions(
                        Path::new("main.sh"),
                        &target("SHARED", 20, false),
                        &never_cancelled,
                    )
                    .is_none()
            );
            assert!(
                index
                    .references(
                        Path::new("vars.sh"),
                        &target("SHARED", usize::MAX, true),
                        false,
                        &never_cancelled,
                    )
                    .is_none()
            );
        }
    }

    #[test]
    fn cyclic_source_graphs_do_not_poison_inherited_definition_caches() {
        let never_cancelled = || false;
        let mut index = WorkspaceVariableIndex::default();
        index.insert_facts(
            PathBuf::from("entry.sh"),
            FileVariableFacts {
                definitions: vec![definition("SHARED", 1)],
                source_effects: vec![source_effect(Some("a.sh"), 10, false)],
                ..FileVariableFacts::default()
            },
        );
        index.insert_facts(
            PathBuf::from("a.sh"),
            FileVariableFacts {
                references: vec![reference("SHARED", 20)],
                source_effects: vec![source_effect(Some("b.sh"), 10, false)],
                ..FileVariableFacts::default()
            },
        );
        index.insert_facts(
            PathBuf::from("b.sh"),
            FileVariableFacts {
                references: vec![reference("SHARED", 20)],
                source_effects: vec![source_effect(Some("a.sh"), 10, false)],
                ..FileVariableFacts::default()
            },
        );

        let references = index
            .references(
                Path::new("entry.sh"),
                &target("SHARED", usize::MAX, true),
                false,
                &never_cancelled,
            )
            .expect("the deterministic cycle should resolve");
        let paths = references
            .into_iter()
            .map(|reference| reference.path)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            paths,
            BTreeSet::from([PathBuf::from("a.sh"), PathBuf::from("b.sh")])
        );
    }

    #[test]
    fn graph_traversal_observes_cancellation() {
        let mut index = WorkspaceVariableIndex::default();
        for file in 0..64 {
            let next = (file < 63)
                .then(|| source_effect(Some(&format!("{next}.sh", next = file + 1)), 1, false));
            index.insert_facts(
                PathBuf::from(format!("{file}.sh")),
                FileVariableFacts {
                    definitions: (file == 63)
                        .then(|| definition("SHARED", 2))
                        .into_iter()
                        .collect(),
                    source_effects: next.into_iter().collect(),
                    ..FileVariableFacts::default()
                },
            );
        }

        let polls = Cell::new(0);
        let result = index.definitions(
            Path::new("0.sh"),
            &target("SHARED", usize::MAX, true),
            &|| {
                let next = polls.get() + 1;
                polls.set(next);
                next > 10
            },
        );

        assert!(result.is_none());
        assert!(polls.get() > 10);
    }
}
