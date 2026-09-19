//! Source-backed command facts for environment intelligence. No host lookup is performed here.
use crate::cfg::{CommandId, RecordedCommandKind, RecordedListOperator};
use crate::{BindingId, SemanticModel, ShellDialect};
use shucked_ast::{Name, Span, Word, static_word_text};
use std::collections::{BTreeMap, BTreeSet};

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
    /// A reason why host absence cannot establish a missing command.
    pub environment_uncertain: Option<String>,
    /// Availability established in this dominated region by a supported check.
    pub guarded_available: bool,
}
impl CommandSiteFacts {
    /// The statically known effective command name.
    pub fn name(&self) -> Option<&str> {
        self.effective_words.first()?.text.as_deref()
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
}

impl SemanticModel {
    /// Collects command identity facts from the existing parser/semantic recording.
    /// Unsupported flow and dynamic mutations explicitly weaken host-absence evidence.
    pub fn command_site_facts(&self) -> Vec<CommandSiteFacts> {
        let mut aliases = BTreeMap::<String, Alias>::new();
        let mut aliases_enabled = self.shell_profile().dialect != ShellDialect::Bash;
        let mut environment_uncertain = None;
        let mut result = Vec::new();
        let guards = self.command_availability_guards();
        for &id in self.commands_in_source_order() {
            let recorded = self.recorded_program.command(id);
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
                environment_uncertain: environment_uncertain.clone(),
                guarded_available: false,
            };
            if info.changes_search_path {
                site.environment_uncertain = Some("The command changes its execution PATH".into());
            }
            if flow.in_function {
                site.environment_uncertain =
                    Some("Function execution context depends on its callers".into());
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
                    if alias.available_after_line >= site.span.start.line() {
                        break;
                    }
                    if !seen.insert(name.clone()) {
                        break;
                    }
                    let Some(expansion) = &alias.words else {
                        site.environment_uncertain =
                            Some("Alias expansion is dynamic or compound".into());
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
                        site.environment_uncertain = Some("Alias expansion limit reached".into());
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
                site.guarded_available = guards.iter().any(|(guard_name, span)| {
                    guard_name == &name && contains(*span, site.name_span())
                });
            }
            let raw_name = site.words.first().and_then(|w| w.text.as_deref());
            if raw_name == Some("alias") {
                for word in site.words.iter().skip(1) {
                    if let Some((name, expansion)) =
                        word.text.as_deref().and_then(|t| t.split_once('='))
                    {
                        aliases.insert(
                            name.into(),
                            Alias {
                                words: (!conditional)
                                    .then(|| simple_alias_words(expansion))
                                    .flatten(),
                                span: word.span,
                                available_after_line: word.span.end.line(),
                            },
                        );
                    }
                }
            } else if raw_name == Some("unalias") {
                if conditional {
                    environment_uncertain =
                        Some("Conditional alias removal changes command lookup".into());
                } else if site.words.iter().any(|w| w.text.as_deref() == Some("-a")) {
                    aliases.clear();
                } else {
                    for word in site.words.iter().skip(1) {
                        if let Some(name) = &word.text {
                            aliases.remove(name);
                        }
                    }
                }
            } else if raw_name == Some("shopt")
                && site
                    .words
                    .iter()
                    .any(|w| w.text.as_deref() == Some("expand_aliases"))
            {
                if conditional {
                    environment_uncertain =
                        Some("Conditional alias option changes command lookup".into());
                } else {
                    aliases_enabled = site.words.iter().any(|w| w.text.as_deref() == Some("-s"));
                }
            }
            if matches!(
                raw_name,
                Some("source" | "." | "eval" | "cd" | "pushd" | "popd")
            ) || info.changes_search_path
            {
                environment_uncertain = Some(
                    "Earlier source, directory, or PATH changes may alter command lookup".into(),
                );
            }
            result.push(site);
        }
        // Standalone assignments and declaration clauses are not simple commands.
        for site in &mut result {
            if self.bindings.iter().any(|binding| {
                matches!(binding.name.as_str(), "PATH" | "path")
                    && binding.span.start.offset() < site.span.start.offset()
            }) {
                site.environment_uncertain
                    .get_or_insert_with(|| "Earlier PATH assignment changes command lookup".into());
            }
        }
        result
    }
    fn command_availability_guards(&self) -> Vec<(String, Span)> {
        let mut result = vec![];
        for recorded in self.recorded_program.commands() {
            match recorded.kind {
                RecordedCommandKind::If {
                    condition,
                    then_branch,
                    ..
                } => {
                    let condition = self.recorded_program.commands_in(condition);
                    if condition.len() == 1
                        && let Some(name) = self.availability_check(condition[0])
                    {
                        for &body in self.recorded_program.commands_in(then_branch) {
                            result.push((name.clone(), self.recorded_program.command(body).span));
                        }
                    }
                }
                RecordedCommandKind::List { first, rest } => {
                    let items = self.recorded_program.list_items(rest);
                    if let Some(name) = self.availability_check(first) {
                        for item in items {
                            if !matches!(item.operator, RecordedListOperator::And) {
                                break;
                            }
                            result.push((
                                name.clone(),
                                self.recorded_program.command(item.command).span,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
        result
    }
    fn availability_check(&self, id: CommandId) -> Option<String> {
        let info = self
            .recorded_program
            .command(id)
            .command_info
            .map(|i| self.recorded_program.command_info(i))?;
        let words = &info.original_words;
        if words.len() != 3 {
            return None;
        }
        let head = words[0].text.as_deref()?;
        let flag = words[1].text.as_deref()?;
        if matches!(
            (head, flag),
            ("command", "-v" | "-V") | ("type", "-P" | "-p")
        ) {
            words[2].text.clone()
        } else {
            None
        }
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
            site.environment_uncertain
                .get_or_insert_with(|| "Command name requires runtime expansion".into());
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
            site.environment_uncertain = Some("Wrapper options change execution context".into());
            return;
        }
    }
}
