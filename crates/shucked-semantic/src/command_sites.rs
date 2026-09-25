//! Source-backed command facts for environment intelligence. No host lookup is performed here.
use crate::cfg::{CommandId, RecordedCommandKind, RecordedCommandRange, RecordedListOperator};
use crate::{
    Binding, BindingAttributes, BindingId, BindingKind, ReferenceKind, SemanticModel, ShellDialect,
};
use shucked_ast::{Name, Position, Span, Word, static_word_text};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

/// Why host absence cannot establish a missing command at a site.
///
/// Diagnostics treat every reason alike: none of them may claim that a command
/// is missing. Suggestion features distinguish them, because most reasons leave
/// a name that resolves on the host as the best available guess.
///
/// [`CommandSiteFacts::environment_uncertain`] carries the message text of the
/// reason; [`CommandSiteFacts::uncertainty`] recovers the structured value from
/// it, so existing consumers of the message keep working unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentUncertainty {
    /// The site is inside a function body whose callers decide the environment.
    InFunction,
    /// An earlier `source`, `.`, or `eval` may define or redefine commands.
    SourceOrEval,
    /// An earlier `autoload` may define functions.
    Autoload,
    /// An earlier `cd`, `pushd`, or `popd` changes relative command lookup.
    WorkingDirectoryChange,
    /// An earlier assignment replaced PATH without keeping its previous value.
    PathReplaced,
    /// An earlier assignment extended PATH, or an earlier command ran under a
    /// transient PATH override that does not persist.
    PathExtended,
    /// The command itself runs under a PATH override.
    SearchPathOverride,
    /// Dynamic or conditional alias changes may affect any later name.
    DynamicAlias,
    /// The alias applied to this exact name is dynamic, compound, or cyclic.
    OpaqueAlias,
    /// The command name requires runtime expansion.
    DynamicName,
    /// Wrapper options change the execution context.
    WrapperOptions,
    /// A reason recorded by another frontend or an unrecognized message.
    Other,
}

impl EnvironmentUncertainty {
    const ALL: [Self; 12] = [
        Self::InFunction,
        Self::SourceOrEval,
        Self::Autoload,
        Self::WorkingDirectoryChange,
        Self::PathReplaced,
        Self::PathExtended,
        Self::SearchPathOverride,
        Self::DynamicAlias,
        Self::OpaqueAlias,
        Self::DynamicName,
        Self::WrapperOptions,
        Self::Other,
    ];

    /// The stable message carried by [`CommandSiteFacts::environment_uncertain`].
    pub fn message(self) -> &'static str {
        match self {
            Self::InFunction => "Function execution context depends on its callers",
            Self::SourceOrEval => "An earlier source or eval may define or redefine commands",
            Self::Autoload => "An earlier autoload may define functions",
            Self::WorkingDirectoryChange => {
                "An earlier directory change may alter relative command lookup"
            }
            Self::PathReplaced => "An earlier PATH assignment replaces the command search path",
            Self::PathExtended => {
                "An earlier PATH change extends or temporarily overrides the command search path"
            }
            Self::SearchPathOverride => "The command changes its execution PATH",
            Self::DynamicAlias => "Dynamic or conditional alias changes may alter command lookup",
            Self::OpaqueAlias => "The alias for this name is dynamic, compound, or cyclic",
            Self::DynamicName => "Command name requires runtime expansion",
            Self::WrapperOptions => "Wrapper options change execution context",
            Self::Other => "The execution environment is not statically known",
        }
    }

    /// Inverts [`Self::message`]; any other text maps to [`Self::Other`].
    pub fn from_message(message: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|reason| reason.message() == message)
            .unwrap_or(Self::Other)
    }

    /// Whether the reason can change which program `name` denotes on the host.
    /// For the other reasons the host executable remains the best guess:
    /// a function body, an earlier `source`, or a PATH extension can shadow a
    /// name but cannot make its host grammar wrong, and a directory change only
    /// affects names that contain a path separator.
    pub fn changes_host_lookup(self, name: &str) -> bool {
        match self {
            Self::InFunction
            | Self::SourceOrEval
            | Self::Autoload
            | Self::PathExtended
            | Self::DynamicAlias => false,
            Self::WorkingDirectoryChange => name.contains('/'),
            Self::PathReplaced
            | Self::SearchPathOverride
            | Self::OpaqueAlias
            | Self::DynamicName
            | Self::WrapperOptions
            | Self::Other => true,
        }
    }

    /// Stronger reasons replace weaker ones when several apply to one site.
    fn rank(self) -> u8 {
        match self {
            Self::InFunction
            | Self::SourceOrEval
            | Self::Autoload
            | Self::PathExtended
            | Self::DynamicAlias => 0,
            Self::WorkingDirectoryChange => 1,
            Self::PathReplaced
            | Self::SearchPathOverride
            | Self::OpaqueAlias
            | Self::DynamicName
            | Self::WrapperOptions
            | Self::Other => 2,
        }
    }
}

impl fmt::Display for EnvironmentUncertainty {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

/// Record `reason` unless a reason at least as strong is already recorded.
fn weaken(slot: &mut Option<EnvironmentUncertainty>, reason: EnvironmentUncertainty) {
    if slot.is_none_or(|old| reason.rank() > old.rank()) {
        *slot = Some(reason);
    }
}

fn weaken_message(slot: &mut Option<String>, reason: EnvironmentUncertainty) {
    let mut current = slot.as_deref().map(EnvironmentUncertainty::from_message);
    let before = current;
    weaken(&mut current, reason);
    if current != before {
        *slot = current.map(|reason| reason.to_string());
    }
}

/// One original or alias-injected shell word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandWord {
    /// Decoded literal, or `None` for runtime expansion.
    pub text: Option<String>,
    /// Original source token; injected words point to the alias invocation.
    pub span: Span,
    /// Whether shell alias recognition is allowed for this lexical token.
    pub alias_eligible: bool,
    /// Whether this word was supplied by an alias.
    pub injected: bool,
}
impl CommandWord {
    pub(crate) fn from_word(word: &Word, source: &str) -> Self {
        let raw = source
            .get(word.span.start.offset()..word.span.end.offset())
            .unwrap_or_default();
        Self {
            text: static_word_text(word, source)
                .and_then(|_| literal_words(raw, false))
                .filter(|words| words.len() == 1)
                .map(|mut words| words.remove(0)),
            span: word.span,
            alias_eligible: !raw.contains(['\'', '"', '\\', '$', '`']),
            injected: false,
        }
    }
}
/// Lookup namespace selected by a shell wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandNamespace {
    /// Normal shell lookup, including functions.
    #[default]
    Shell,
    /// `command`/`exec` bypass shell functions.
    ExternalOrBuiltin,
    /// `builtin` restricts lookup to shell builtins.
    Builtin,
    /// `env` invokes an external executable.
    External,
}
/// An alias applied at a command site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedAlias {
    /// Alias name.
    pub name: String,
    /// Definition location in this document.
    pub definition: Span,
}
/// Static command facts shared by editor consumers.
#[derive(Debug, Clone)]
pub struct CommandSiteFacts {
    /// Full source command range.
    pub span: Span,
    /// Words exactly as represented by the parser, before aliases/wrappers.
    pub words: Vec<CommandWord>,
    /// Effective invocation after supported aliases and wrappers.
    pub effective_words: Vec<CommandWord>,
    /// Applied aliases, in expansion order.
    pub aliases: Vec<AppliedAlias>,
    /// Visible function binding for the effective name.
    pub visible_function: Option<BindingId>,
    /// Lookup namespace selected by wrappers.
    pub namespace: CommandNamespace,
    /// A reason why host absence cannot establish a missing command. The text
    /// is the message of an [`EnvironmentUncertainty`]; see [`Self::uncertainty`].
    pub environment_uncertain: Option<String>,
    /// Availability established in this dominated region by a supported check.
    pub guarded_available: bool,
}
impl CommandSiteFacts {
    /// The statically known effective command name.
    pub fn name(&self) -> Option<&str> {
        self.effective_words.first()?.text.as_deref()
    }
    /// The structured reason behind [`Self::environment_uncertain`].
    pub fn uncertainty(&self) -> Option<EnvironmentUncertainty> {
        self.environment_uncertain
            .as_deref()
            .map(EnvironmentUncertainty::from_message)
    }
    /// The token to highlight or replace, retaining original source coordinates.
    pub fn name_span(&self) -> Span {
        self.effective_words.first().map_or(self.span, |w| w.span)
    }
}
#[derive(Clone)]
struct Alias {
    words: Option<Vec<String>>,
    span: Span,
    available_after_line: usize,
    unconditional: bool,
}

enum AliasChange {
    Define(String, Alias),
    Remove(String),
    Clear,
}
fn apply_alias_changes(
    aliases: &mut BTreeMap<String, Alias>,
    pending: &mut VecDeque<(usize, AliasChange)>,
    line: usize,
) {
    while pending.front().is_some_and(|(after, _)| *after < line) {
        match pending.pop_front().expect("pending change").1 {
            AliasChange::Define(name, alias) => {
                aliases.insert(name, alias);
            }
            AliasChange::Remove(name) => {
                aliases.remove(&name);
            }
            AliasChange::Clear => aliases.clear(),
        }
    }
}

impl SemanticModel {
    /// Collects command identity facts from the existing parser/semantic recording.
    /// Unsupported flow and dynamic mutations explicitly weaken host-absence evidence.
    pub fn command_site_facts(&self) -> Vec<CommandSiteFacts> {
        self.command_facts_until(None).0
    }

    /// Literal alias names visible at this source position, respecting parse-unit
    /// timing, shell alias options, and prior removals. No source is reparsed.
    pub fn visible_aliases_at(&self, position: Position) -> Vec<AppliedAlias> {
        self.command_facts_until(Some(position)).1
    }

    fn command_facts_until(
        &self,
        cursor: Option<Position>,
    ) -> (Vec<CommandSiteFacts>, Vec<AppliedAlias>) {
        let mut cursor_parse_line = cursor.map(|position| position.line()).unwrap_or(0);
        let mut aliases = BTreeMap::<String, Alias>::new();
        let mut pending_alias_changes = VecDeque::new();
        let mut aliases_enabled = self.shell_profile().dialect != ShellDialect::Bash;
        let mut pending_alias_option = None;
        let mut environment_uncertain: Option<EnvironmentUncertainty> = None;
        let mut result = Vec::new();
        let guards = if cursor.is_none() {
            self.command_availability_guards()
        } else {
            Vec::new()
        };
        for &id in self.commands_in_source_order() {
            let recorded = self.recorded_program.command(id);
            if cursor.is_some_and(|cursor| recorded.syntax_span.start.offset() >= cursor.offset()) {
                break;
            }
            let Some(info) = recorded
                .command_info
                .map(|i| self.recorded_program.command_info(i))
            else {
                continue;
            };
            if info.original_words.is_empty() {
                continue;
            }
            let flow = recorded.flow_context.unwrap_or_default();
            let mut parse_start_line = recorded.syntax_span.start.line();
            let mut ancestor = self.syntax_backed_command_parent_id(id);
            while let Some(parent) = ancestor {
                parse_start_line = self.command_syntax_span(parent).start.line();
                ancestor = self.syntax_backed_command_parent_id(parent);
            }
            if cursor.is_some_and(|cursor| recorded.syntax_span.end.offset() >= cursor.offset()) {
                cursor_parse_line = parse_start_line;
                break;
            }
            apply_alias_changes(&mut aliases, &mut pending_alias_changes, parse_start_line);
            if let Some((line, enabled)) = pending_alias_option
                && line < parse_start_line
            {
                aliases_enabled = enabled;
                pending_alias_option = None;
            }
            let conditional = self.syntax_backed_command_parent_id(id).is_some()
                || flow.in_function
                || flow.in_subshell;
            let mut site = CommandSiteFacts {
                span: recorded.syntax_span,
                words: info.original_words.clone(),
                effective_words: info.original_words.clone(),
                aliases: vec![],
                visible_function: None,
                namespace: CommandNamespace::Shell,
                environment_uncertain: environment_uncertain.map(|reason| reason.to_string()),
                guarded_available: false,
            };
            if info.changes_search_path {
                let reason = self.search_path_override_reason(&site);
                weaken_message(&mut site.environment_uncertain, reason);
            }
            if flow.in_function {
                weaken_message(
                    &mut site.environment_uncertain,
                    EnvironmentUncertainty::InFunction,
                );
            }
            if aliases_enabled {
                let mut seen = BTreeSet::new();
                loop {
                    let Some(word) = site.effective_words.first() else {
                        break;
                    };
                    let Some(name) = word.text.as_ref().filter(|_| word.alias_eligible) else {
                        break;
                    };
                    let Some(alias) = aliases.get(name) else {
                        break;
                    };
                    if alias.available_after_line >= parse_start_line {
                        break;
                    }
                    if !seen.insert(name.clone()) {
                        if seen.len() > 1 {
                            weaken_message(
                                &mut site.environment_uncertain,
                                EnvironmentUncertainty::OpaqueAlias,
                            );
                            site.effective_words[0].text = None;
                        }
                        break;
                    }
                    let Some(expansion) = &alias.words else {
                        weaken_message(
                            &mut site.environment_uncertain,
                            EnvironmentUncertainty::OpaqueAlias,
                        );
                        site.effective_words[0].text = None;
                        break;
                    };
                    site.aliases.push(AppliedAlias {
                        name: name.clone(),
                        definition: alias.span,
                    });
                    let span = word.span;
                    site.effective_words.splice(
                        0..1,
                        expansion.iter().map(|text| CommandWord {
                            text: Some(text.clone()),
                            span,
                            alias_eligible: true,
                            injected: true,
                        }),
                    );
                    if seen.len() > 32 {
                        weaken_message(
                            &mut site.environment_uncertain,
                            EnvironmentUncertainty::OpaqueAlias,
                        );
                        break;
                    }
                }
            }
            unwrap_command(&mut site);
            if let Some(name) = site.name().map(str::to_owned) {
                if site.namespace == CommandNamespace::Shell {
                    site.visible_function =
                        self.function_binding_lookup().visible_function_binding(
                            &Name::from(name.as_str()),
                            recorded
                                .scope
                                .unwrap_or_else(|| self.scope_at(site.span.start.offset())),
                            site.span.start.offset(),
                        );
                }
                site.guarded_available = guards.iter().any(|(guard_name, span, scope)| {
                    guard_name == &name
                        && contains(*span, site.name_span())
                        && self.enclosing_function_scope(*scope)
                            == self.enclosing_function_scope(
                                recorded
                                    .scope
                                    .unwrap_or_else(|| self.scope_at(site.span.start.offset())),
                            )
                });
            }
            let alias_change_line = site
                .effective_words
                .last()
                .map_or(site.span.start.line(), |word| word.span.end.line());
            let raw_name = (site.visible_function.is_none()
                && site.namespace != CommandNamespace::External)
                .then(|| site.name())
                .flatten();
            if raw_name == Some("alias") {
                for word in site.effective_words.iter().skip(1) {
                    if word.text.is_none() {
                        pending_alias_changes.push_back((alias_change_line, AliasChange::Clear));
                        weaken(
                            &mut environment_uncertain,
                            EnvironmentUncertainty::DynamicAlias,
                        );
                    }
                    if let Some((name, expansion)) =
                        word.text.as_deref().and_then(|t| t.split_once('='))
                    {
                        pending_alias_changes.push_back((
                            alias_change_line,
                            AliasChange::Define(
                                name.into(),
                                Alias {
                                    words: (!conditional)
                                        .then(|| simple_alias_words(expansion))
                                        .flatten(),
                                    span: word.span,
                                    available_after_line: word.span.end.line(),
                                    unconditional: !conditional,
                                },
                            ),
                        ));
                    }
                }
            } else if raw_name == Some("unalias") {
                if conditional {
                    weaken(
                        &mut environment_uncertain,
                        EnvironmentUncertainty::DynamicAlias,
                    );
                    if site
                        .effective_words
                        .iter()
                        .skip(1)
                        .any(|word| word.text.is_none() || word.text.as_deref() == Some("-a"))
                    {
                        pending_alias_changes.push_back((alias_change_line, AliasChange::Clear));
                    }
                    for word in site.effective_words.iter().skip(1) {
                        if let Some(name) = &word.text {
                            pending_alias_changes
                                .push_back((alias_change_line, AliasChange::Remove(name.clone())));
                        }
                    }
                } else if site
                    .effective_words
                    .iter()
                    .any(|w| w.text.as_deref() == Some("-a"))
                {
                    pending_alias_changes.push_back((alias_change_line, AliasChange::Clear));
                } else {
                    for word in site.effective_words.iter().skip(1) {
                        if let Some(name) = &word.text {
                            pending_alias_changes
                                .push_back((alias_change_line, AliasChange::Remove(name.clone())));
                        } else {
                            pending_alias_changes
                                .push_back((alias_change_line, AliasChange::Clear));
                        }
                    }
                }
            } else if raw_name == Some("shopt")
                && site
                    .effective_words
                    .iter()
                    .any(|w| w.text.as_deref() == Some("expand_aliases"))
            {
                if conditional {
                    weaken(
                        &mut environment_uncertain,
                        EnvironmentUncertainty::DynamicAlias,
                    );
                } else {
                    pending_alias_option = Some((
                        site.effective_words
                            .last()
                            .map_or(site.span.start.line(), |word| word.span.end.line()),
                        site.effective_words
                            .iter()
                            .any(|w| w.text.as_deref() == Some("-s")),
                    ));
                }
            }
            if self.shell_profile().dialect == ShellDialect::Zsh
                && matches!(raw_name, Some("setopt" | "unsetopt"))
            {
                for word in site.effective_words.iter().skip(1) {
                    let option = word
                        .text
                        .as_deref()
                        .unwrap_or_default()
                        .replace('_', "")
                        .to_ascii_lowercase();
                    if matches!(option.as_str(), "aliases" | "noaliases") {
                        if conditional {
                            pending_alias_changes
                                .push_back((alias_change_line, AliasChange::Clear));
                        } else {
                            pending_alias_option = Some((
                                word.span.end.line(),
                                (raw_name == Some("setopt")) != (option == "noaliases"),
                            ));
                        }
                    }
                }
            }
            if matches!(raw_name, Some("source" | "." | "eval")) {
                pending_alias_changes.push_back((alias_change_line, AliasChange::Clear));
            }
            let effect = match raw_name {
                Some("source" | "." | "eval") => Some(EnvironmentUncertainty::SourceOrEval),
                Some("autoload") => Some(EnvironmentUncertainty::Autoload),
                Some("cd" | "pushd" | "popd") => {
                    Some(EnvironmentUncertainty::WorkingDirectoryChange)
                }
                // A prefix assignment does not outlive its command.
                _ if info.changes_search_path => Some(EnvironmentUncertainty::PathExtended),
                _ => None,
            };
            if let Some(effect) = effect {
                weaken(&mut environment_uncertain, effect);
            }
            if cursor.is_none() {
                result.push(site);
            }
        }
        // Standalone assignments and declaration clauses are not simple commands.
        let mut first_path_extension = None;
        let mut first_path_replacement = None;
        for binding in self
            .bindings
            .iter()
            .filter(|binding| matches!(binding.name.as_str(), "PATH" | "path"))
        {
            let offset = binding.span.start.offset();
            let slot = match self.path_assignment_reason(binding) {
                None => continue,
                Some(EnvironmentUncertainty::PathReplaced) => &mut first_path_replacement,
                Some(_) => &mut first_path_extension,
            };
            *slot = Some(slot.map_or(offset, |first: usize| first.min(offset)));
        }
        for site in &mut result {
            let start = site.span.start.offset();
            if first_path_replacement.is_some_and(|offset| offset < start) {
                weaken_message(
                    &mut site.environment_uncertain,
                    EnvironmentUncertainty::PathReplaced,
                );
            } else if first_path_extension.is_some_and(|offset| offset < start) {
                weaken_message(
                    &mut site.environment_uncertain,
                    EnvironmentUncertainty::PathExtended,
                );
            }
        }
        apply_alias_changes(&mut aliases, &mut pending_alias_changes, cursor_parse_line);
        if let Some((line, enabled)) = pending_alias_option
            && line < cursor_parse_line
        {
            aliases_enabled = enabled;
        }
        let visible = if cursor.is_some() && aliases_enabled {
            aliases
                .into_iter()
                .filter(|(_, alias)| {
                    alias.unconditional && alias.available_after_line < cursor_parse_line
                })
                .map(|(name, alias)| AppliedAlias {
                    name,
                    definition: alias.span,
                })
                .collect()
        } else {
            Vec::new()
        };
        (result, visible)
    }
    /// Whether the source between `start` and `end` reads the previous PATH.
    fn reads_search_path(&self, start: usize, end: usize) -> bool {
        self.references.iter().any(|reference| {
            reference.span.start.offset() >= start
                && reference.span.end.offset() <= end
                && matches!(reference.name.as_str(), "PATH" | "path")
                && matches!(
                    reference.kind,
                    ReferenceKind::Expansion
                        | ReferenceKind::ParameterExpansion
                        | ReferenceKind::ArrayAccess
                        | ReferenceKind::ImplicitRead
                        | ReferenceKind::ArithmeticRead
                )
        })
    }
    /// A prefix assignment (`PATH=... command`) that keeps the previous PATH
    /// leaves every host executable reachable; a replacement does not.
    fn search_path_override_reason(&self, site: &CommandSiteFacts) -> EnvironmentUncertainty {
        let name_start = site
            .words
            .first()
            .map_or(site.span.end.offset(), |word| word.span.start.offset());
        if self.reads_search_path(site.span.start.offset(), name_start) {
            EnvironmentUncertainty::PathExtended
        } else {
            EnvironmentUncertainty::SearchPathOverride
        }
    }
    /// How a standalone PATH binding affects later lookups, or `None` for a
    /// declaration without a value. Binding spans cover the name only, so the
    /// value is the rest of the innermost recorded command up to the next binding.
    fn path_assignment_reason(&self, binding: &Binding) -> Option<EnvironmentUncertainty> {
        if matches!(binding.kind, BindingKind::Declaration(_))
            && !binding
                .attributes
                .contains(BindingAttributes::DECLARATION_INITIALIZED)
        {
            return None;
        }
        if binding.kind == BindingKind::AppendAssignment {
            return Some(EnvironmentUncertainty::PathExtended);
        }
        let start = binding.span.end.offset();
        let command_end = self
            .commands_in_source_order()
            .iter()
            .map(|&id| self.command_syntax_span(id))
            .filter(|span| {
                span.start.offset() <= binding.span.start.offset() && span.end.offset() >= start
            })
            .map(|span| span.end.offset())
            .min();
        let next_binding = self
            .bindings
            .iter()
            .filter(|other| other.id != binding.id && other.span.start.offset() >= start)
            .map(|other| other.span.start.offset())
            .min();
        let end = command_end
            .into_iter()
            .chain(next_binding)
            .min()
            .unwrap_or(usize::MAX);
        Some(if self.reads_search_path(start, end) {
            EnvironmentUncertainty::PathExtended
        } else {
            EnvironmentUncertainty::PathReplaced
        })
    }
    fn command_availability_guards(&self) -> Vec<(String, Span, crate::ScopeId)> {
        let program = &self.recorded_program;
        let mut ranges = vec![program.file_commands()];
        ranges.extend(program.function_bodies().values().copied());
        for recorded in program.commands() {
            match recorded.kind {
                RecordedCommandKind::If {
                    condition,
                    then_branch,
                    elif_branches,
                    else_branch,
                } => {
                    ranges.extend([condition, then_branch, else_branch]);
                    for branch in program.elif_branches(elif_branches) {
                        ranges.extend([branch.condition, branch.body]);
                    }
                }
                RecordedCommandKind::While { condition, body }
                | RecordedCommandKind::Until { condition, body } => {
                    ranges.extend([condition, body])
                }
                RecordedCommandKind::For { body }
                | RecordedCommandKind::Select { body }
                | RecordedCommandKind::ArithmeticFor { body }
                | RecordedCommandKind::BraceGroup { body }
                | RecordedCommandKind::Subshell { body } => ranges.push(body),
                RecordedCommandKind::Always { body, always_body } => {
                    ranges.extend([body, always_body])
                }
                RecordedCommandKind::Case { arms } => {
                    for arm in program.case_arms(arms) {
                        ranges.push(arm.commands);
                    }
                }
                _ => {}
            }
        }
        let mut result = vec![];
        for range in ranges {
            let mut available = BTreeSet::<String>::new();
            for &id in program.commands_in(range) {
                let recorded = program.command(id);
                let Some(scope) = recorded.scope else {
                    continue;
                };
                for name in &available {
                    result.push((name.clone(), recorded.span, scope));
                }
                match recorded.kind {
                    RecordedCommandKind::If {
                        condition,
                        then_branch,
                        elif_branches,
                        else_branch,
                    } => {
                        let condition = program.commands_in(condition);
                        if condition.len() != 1 {
                            continue;
                        }
                        let Some((name, positive)) = self.availability_check(condition[0]) else {
                            continue;
                        };
                        let no_elif = program.elif_branches(elif_branches).is_empty();
                        let proven = if positive {
                            Some(then_branch)
                        } else if no_elif {
                            Some(else_branch)
                        } else {
                            None
                        };
                        if let Some(branch) = proven {
                            for &body in program.commands_in(branch) {
                                result.push((name.clone(), program.command(body).span, scope));
                            }
                        }
                        let absent_branch = if positive { else_branch } else { then_branch };
                        if no_elif
                            && !recorded.background
                            && self.sequence_terminates(absent_branch, 0)
                        {
                            available.insert(name);
                        }
                    }
                    RecordedCommandKind::List { first, rest } => {
                        let Some((name, positive)) = self.availability_check(first) else {
                            continue;
                        };
                        let items = program.list_items(rest);
                        for item in items {
                            let requires_success =
                                matches!(item.operator, RecordedListOperator::And);
                            if requires_success != positive {
                                break;
                            }
                            result.push((name.clone(), program.command(item.command).span, scope));
                        }
                        if let [item] = items {
                            let requires_success =
                                matches!(item.operator, RecordedListOperator::And);
                            if requires_success != positive
                                && !recorded.background
                                && self.command_terminates(item.command, 0)
                            {
                                available.insert(name);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        result
    }
    fn availability_check(&self, id: CommandId) -> Option<(String, bool)> {
        let recorded = self.recorded_program.command(id);
        if recorded.background {
            return None;
        }
        let info = recorded
            .command_info
            .map(|i| self.recorded_program.command_info(i))?;
        if info.changes_search_path {
            return None;
        }
        let words = &info.original_words;
        if words.len() != 3 {
            return None;
        }
        let head = words[0].text.as_deref()?;
        let scope = recorded.scope?;
        if self
            .function_binding_lookup()
            .visible_function_binding(&Name::from(head), scope, recorded.span.start.offset())
            .is_some()
            || self.possible_alias_before(head, recorded.span)
        {
            return None;
        }
        let flag = words[1].text.as_deref()?;
        if matches!(
            (head, flag),
            ("command", "-v" | "-V") | ("type", "-P" | "-p")
        ) {
            words[2].text.clone().map(|name| (name, !recorded.negated))
        } else {
            None
        }
    }
    fn possible_alias_before(&self, name: &str, span: Span) -> bool {
        let aliases = self.command_alias_offsets.get_or_init(|| {
            let mut aliases = BTreeMap::<String, usize>::new();
            for command in self.recorded_program.commands() {
                let Some(info) = command
                    .command_info
                    .map(|id| self.recorded_program.command_info(id))
                else {
                    continue;
                };
                let words = &info.original_words;
                let head = if words
                    .first()
                    .and_then(|word| word.text.as_deref())
                    .is_some_and(|name| matches!(name, "builtin" | "command"))
                {
                    1
                } else {
                    0
                };
                if words.get(head).and_then(|word| word.text.as_deref()) != Some("alias") {
                    continue;
                }
                for word in words.iter().skip(head + 1) {
                    let name = match word.text.as_deref() {
                        None => Some(""),
                        Some(text) => text.split_once('=').map(|(name, _)| name),
                    };
                    if let Some(name) = name {
                        aliases
                            .entry(name.to_owned())
                            .and_modify(|offset| {
                                *offset = (*offset).min(command.span.start.offset())
                            })
                            .or_insert(command.span.start.offset());
                    }
                }
            }
            aliases
        });
        aliases
            .get(name)
            .into_iter()
            .chain(aliases.get(""))
            .any(|&offset| offset < span.start.offset())
    }
    fn sequence_terminates(&self, range: RecordedCommandRange, depth: usize) -> bool {
        self.recorded_program
            .commands_in(range)
            .iter()
            .any(|&id| self.command_terminates(id, depth + 1))
    }
    fn command_terminates(&self, id: CommandId, depth: usize) -> bool {
        if depth > 128 {
            return false;
        }
        let command = self.recorded_program.command(id);
        if command.background {
            return false;
        }
        let builtin = match command.kind {
            RecordedCommandKind::Exit => Some("exit"),
            RecordedCommandKind::Return
                if command.flow_context.is_some_and(|flow| flow.in_function) =>
            {
                Some("return")
            }
            RecordedCommandKind::Break { .. } | RecordedCommandKind::Continue { .. }
                if command.flow_context.is_some_and(|flow| flow.loop_depth > 0) =>
            {
                Some("break")
            }
            RecordedCommandKind::BraceGroup { body } => {
                return self.sequence_terminates(body, depth + 1);
            }
            RecordedCommandKind::If {
                then_branch,
                elif_branches,
                else_branch,
                ..
            } => {
                return self.sequence_terminates(then_branch, depth + 1)
                    && self.sequence_terminates(else_branch, depth + 1)
                    && self
                        .recorded_program
                        .elif_branches(elif_branches)
                        .iter()
                        .all(|branch| self.sequence_terminates(branch.body, depth + 1));
            }
            _ => None,
        };
        builtin.is_some_and(|name| {
            !self.possible_alias_before(name, command.span)
                && command.scope.is_some_and(|scope| {
                    self.function_binding_lookup()
                        .visible_function_binding(
                            &Name::from(name),
                            scope,
                            command.span.start.offset(),
                        )
                        .is_none()
                })
        })
    }
}
fn contains(outer: Span, inner: Span) -> bool {
    outer.start.offset() <= inner.start.offset() && outer.end.offset() >= inner.end.offset()
}

/// Decode the deliberately limited, inert alias form supported for command resolution.
pub fn simple_alias_words(value: &str) -> Option<Vec<String>> {
    literal_words(value, true)
}
fn literal_words(value: &str, alias: bool) -> Option<Vec<String>> {
    let mut words = vec![];
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in value.chars() {
        if escaped {
            if ch == '\n' {
                escaped = false;
                continue;
            }
            if quote == Some('"') && !matches!(ch, '$' | '`' | '"' | '\\') {
                word.push('\\');
            }
            word.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if matches!(ch, '\'' | '"') {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                word.push(ch);
            }
            started = true;
            continue;
        }
        if matches!(ch, '$' | '`') && quote != Some('\'') {
            return None;
        }
        if quote.is_none()
            && matches!(
                ch,
                ';' | '|' | '&' | '<' | '>' | '(' | ')' | '\n' | '*' | '?' | '[' | '{' | '~'
            )
        {
            return None;
        }
        if quote.is_none() && ch.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if started {
        words.push(word);
    }
    // A trailing space changes alias eligibility of the next original argument.
    if words.is_empty() || value.ends_with([' ', '\t']) || alias && words[0].contains('=') {
        None
    } else {
        Some(words)
    }
}
fn unwrap_command(site: &mut CommandSiteFacts) {
    loop {
        let Some(name) = site.name() else {
            weaken_message(
                &mut site.environment_uncertain,
                EnvironmentUncertainty::DynamicName,
            );
            return;
        };
        match name {
            "command"
                if !site
                    .effective_words
                    .get(1)
                    .and_then(|w| w.text.as_deref())
                    .is_some_and(|a| a.starts_with('-') && a != "--") =>
            {
                site.namespace = CommandNamespace::ExternalOrBuiltin
            }
            "builtin" => site.namespace = CommandNamespace::Builtin,
            "exec" => site.namespace = CommandNamespace::External,
            "env" => site.namespace = CommandNamespace::External,
            "sudo" | "su" | "ssh" | "docker" | "podman" => return,
            _ => return,
        }
        if site.effective_words.len() < 2 {
            return;
        }
        site.effective_words.remove(0);
        if site.name() == Some("--") {
            site.effective_words.remove(0);
        }
        if site
            .name()
            .is_some_and(|n| n.starts_with('-') || n.contains('='))
        {
            weaken_message(
                &mut site.environment_uncertain,
                EnvironmentUncertainty::WrapperOptions,
            );
            return;
        }
    }
}
