use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    CommandDeclaration, CommandSite, DeclarationKind, EnvironmentSnapshot, ExecutableIdentity,
    ExecutionContext, ExecutionMode, LookupEvidence, LookupMode, Provenance, ShellDialect,
    ValidationPolicy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommandKind {
    Function,
    Builtin,
    Executable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedCommand {
    pub name: String,
    pub kind: CommandKind,
    pub executable: Option<ExecutableIdentity>,
    pub effective_words: Vec<String>,
    pub alias_chain: Vec<String>,
    pub provenance: Vec<Provenance>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingCommand {
    pub name: String,
    pub target_id: String,
    pub searched_path: Vec<std::path::PathBuf>,
    pub declaration: Option<CommandDeclaration>,
    pub suggestions: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownReason {
    DynamicCommand,
    DynamicEnvironment,
    PortablePolicy,
    WrongTarget,
    StaleEnvironment,
    IncompleteInventory,
    OpaqueAlias,
    RecursiveAlias,
    GuardedDependency,
    OptionalDependency,
    UnknownBuiltin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownCommand {
    pub name: Option<String>,
    pub reason: UnknownReason,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "command", rename_all = "camelCase")]
pub enum CommandResolution {
    Resolved(ResolvedCommand),
    Missing(MissingCommand),
    Unknown(UnknownCommand),
}

impl CommandResolution {
    pub fn resolved(&self) -> Option<&ResolvedCommand> {
        if let Self::Resolved(command) = self {
            Some(command)
        } else {
            None
        }
    }
}

/// Detail attached to an [`UnknownReason::DynamicEnvironment`] result that stems
/// from the site's own source-level uncertainty, as opposed to a workspace
/// binding or launch-directory mismatch reported by other layers.
pub const DYNAMIC_ENVIRONMENT_DETAIL: &str =
    "The invocation changes the execution environment in a way that is not statically known";

/// Detail attached to an [`UnknownReason::DynamicEnvironment`] result for a
/// relative lookup whose launch directory differs from the captured one.
pub const LAUNCH_DIRECTORY_DETAIL: &str =
    "The launch directory differs from the captured execution PATH context";

/// Resolve source-visible definitions and host evidence without performing I/O.
/// The caller supplies language-aware alias eligibility and visibility; command
/// arguments are never parsed or executed by this layer.
pub fn resolve(
    context: &ExecutionContext,
    snapshot: &EnvironmentSnapshot,
    site: &CommandSite,
) -> CommandResolution {
    let Some(original_name) = site.name.as_deref().filter(|name| !name.is_empty()) else {
        return unknown(
            site,
            UnknownReason::DynamicCommand,
            "The command name is computed at runtime",
        );
    };
    if context.target_id != snapshot.target_id {
        return unknown(
            site,
            UnknownReason::WrongTarget,
            "Environment evidence belongs to a different target",
        );
    }
    let session = context.mode == ExecutionMode::InteractiveSession;
    let mut name = original_name.to_owned();
    let mut words = vec![name.clone()];
    words.extend(site.arguments.iter().cloned());
    let mut alias_chain = Vec::new();
    let mut provenance = Vec::new();
    if site.alias_eligible && site.lookup == LookupMode::Normal {
        loop {
            let alias = site
                .aliases
                .get(&name)
                .or_else(|| session.then(|| snapshot.aliases.get(&name)).flatten());
            let Some(alias) = alias else { break };
            if !site.aliases.contains_key(&name) && !snapshot.fresh {
                return unknown(
                    site,
                    UnknownReason::StaleEnvironment,
                    "The attached session alias state is stale",
                );
            }
            if alias.opaque || alias.words.is_empty() {
                return unknown(
                    site,
                    UnknownReason::OpaqueAlias,
                    "The alias does not have a statically known simple invocation",
                );
            }
            if alias_chain.contains(&name) || alias_chain.len() >= 32 {
                return unknown(
                    site,
                    UnknownReason::RecursiveAlias,
                    "Alias expansion is recursive or exceeds the analysis limit",
                );
            }
            alias_chain.push(name.clone());
            if let Some(source) = &alias.provenance {
                provenance.push(source.clone());
            }
            let next_name = alias.words[0].clone();
            words.splice(0..1, alias.words.iter().cloned());
            // Shells suppress the currently expanding alias, allowing the
            // common `alias ls='ls --color=auto'` form.
            if next_name == name {
                break;
            }
            name = next_name;
        }
        name = words[0].clone();
    }
    let make_resolved = |kind, executable, mut sources: Vec<Provenance>| {
        let mut all_sources = provenance.clone();
        all_sources.append(&mut sources);
        CommandResolution::Resolved(ResolvedCommand {
            name: name.clone(),
            kind,
            executable,
            effective_words: words.clone(),
            alias_chain: alias_chain.clone(),
            provenance: all_sources,
        })
    };
    let builtin_allowed = !matches!(site.lookup, LookupMode::ExternalOnly);
    let special_first =
        context.dialect == ShellDialect::Posix && special_builtins().contains(&name.as_str());
    let is_builtin = snapshot.builtins.contains(&name);
    if builtin_allowed && special_first && is_builtin {
        return make_resolved(
            CommandKind::Builtin,
            None,
            vec![Provenance::new("shell special builtin")],
        );
    }
    if site.lookup == LookupMode::Normal {
        if site.functions.contains(&name) {
            return make_resolved(
                CommandKind::Function,
                None,
                vec![Provenance::new("source-visible function")],
            );
        }
        if session && snapshot.functions.contains(&name) {
            if !snapshot.fresh {
                return unknown(
                    site,
                    UnknownReason::StaleEnvironment,
                    "The attached session state is stale",
                );
            }
            return make_resolved(
                CommandKind::Function,
                None,
                vec![Provenance::new("attached shell function")],
            );
        }
    }
    if builtin_allowed && is_builtin {
        return make_resolved(
            CommandKind::Builtin,
            None,
            vec![Provenance::new("shell builtin")],
        );
    }
    if site.lookup == LookupMode::BuiltinOnly {
        if !snapshot.builtins_complete {
            return unknown(
                site,
                UnknownReason::UnknownBuiltin,
                "The target builtin inventory is incomplete",
            );
        }
        return missing(context, snapshot, site, &name);
    }
    if site.environment_uncertain {
        return unknown(
            site,
            UnknownReason::DynamicEnvironment,
            DYNAMIC_ENVIRONMENT_DETAIL,
        );
    }
    if context.policy == ValidationPolicy::Portable {
        return unknown(
            site,
            UnknownReason::PortablePolicy,
            "Portable checks do not assume host executables are available",
        );
    }
    let relative_lookup = !std::path::Path::new(&name).is_absolute()
        && (name.contains('/')
            || (snapshot.platform == "windows" && name.contains('\\'))
            || snapshot
                .search_path
                .iter()
                .any(|directory| !directory.path.is_absolute()));
    if relative_lookup
        && (context.cwd != snapshot.search_cwd || context.cwd_known != snapshot.search_cwd_known)
    {
        return unknown(
            site,
            UnknownReason::DynamicEnvironment,
            LAUNCH_DIRECTORY_DETAIL,
        );
    }
    if !snapshot.fresh {
        return unknown(
            site,
            UnknownReason::StaleEnvironment,
            "The target environment needs to be refreshed",
        );
    }
    match snapshot.lookup(&name) {
        LookupEvidence::Present(executable) => make_resolved(
            CommandKind::Executable,
            Some(executable.identity),
            vec![executable.provenance],
        ),
        LookupEvidence::Missing => {
            if site.guarded.contains(&name) || site.guarded.contains(original_name) {
                return unknown(
                    site,
                    UnknownReason::GuardedDependency,
                    "This invocation is protected by a matching availability check",
                );
            }
            let declaration = applicable_declaration(context, site, &name);
            if declaration.is_some_and(|declaration| declaration.kind == DeclarationKind::Optional)
            {
                return unknown(
                    site,
                    UnknownReason::OptionalDependency,
                    "This command is declared as an optional dependency",
                );
            }
            missing(context, snapshot, site, &name)
        }
        LookupEvidence::Unknown(detail) => {
            unknown(site, UnknownReason::IncompleteInventory, detail)
        }
    }
}

fn applicable_declaration<'a>(
    context: &ExecutionContext,
    site: &'a CommandSite,
    name: &str,
) -> Option<&'a CommandDeclaration> {
    site.declared.get(name).filter(|declaration| {
        declaration
            .target_id
            .as_ref()
            .is_none_or(|target| target == &context.target_id)
    })
}

fn missing(
    context: &ExecutionContext,
    snapshot: &EnvironmentSnapshot,
    site: &CommandSite,
    name: &str,
) -> CommandResolution {
    let mut candidates = snapshot.command_names();
    candidates.extend(site.functions.iter().cloned());
    candidates.extend(site.aliases.keys().cloned());
    CommandResolution::Missing(MissingCommand {
        name: name.into(),
        target_id: context.target_id.clone(),
        searched_path: snapshot
            .search_path
            .iter()
            .map(|directory| directory.path.clone())
            .collect(),
        declaration: applicable_declaration(context, site, name).cloned(),
        suggestions: typo_candidates(name, candidates.iter().map(String::as_str), 3),
    })
}

fn unknown(
    site: &CommandSite,
    reason: UnknownReason,
    detail: impl Into<String>,
) -> CommandResolution {
    CommandResolution::Unknown(UnknownCommand {
        name: site.name.clone(),
        reason,
        detail: detail.into(),
    })
}

/// Bounded Unicode-aware edit-distance suggestions, ordered deterministically.
/// Single-character names are excluded to avoid noisy replacements.
pub fn typo_candidates<'a>(
    input: &str,
    candidates: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<String> {
    let input_len = input.chars().count();
    if !(2..=128).contains(&input_len) || limit == 0 {
        return Vec::new();
    }
    let threshold = if input_len <= 4 { 1 } else { 2 };
    let mut matches = BTreeSet::new();
    for candidate in candidates.into_iter().take(100_000) {
        let length = candidate.chars().count();
        if candidate == input || length.abs_diff(input_len) > threshold || length > 128 {
            continue;
        }
        let distance = edit_distance(input, candidate);
        if distance <= threshold {
            matches.insert((distance, candidate.to_owned()));
        }
    }
    matches
        .into_iter()
        .take(limit)
        .map(|(_, candidate)| candidate)
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut before_previous = previous.clone();
    let mut previous_char = None;
    for (row, left_char) in left.chars().enumerate() {
        let mut current = vec![row + 1; right.len() + 1];
        for (column, right_char) in right.iter().enumerate() {
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + usize::from(left_char != *right_char));
            if row > 0
                && column > 0
                && left_char == right[column - 1]
                && previous_char == Some(*right_char)
            {
                current[column + 1] = current[column + 1].min(before_previous[column - 1] + 1);
            }
        }
        before_previous = previous;
        previous = current;
        previous_char = Some(left_char);
    }
    previous[right.len()]
}

fn special_builtins() -> &'static [&'static str] {
    &[
        ":", ".", "break", "continue", "eval", "exec", "exit", "export", "readonly", "return",
        "set", "shift", "times", "trap", "unset",
    ]
}

/// Stable builtin names supported by the selected language. This intentionally
/// is not a claim that the inventory covers version-specific or loadable names.
pub fn builtins(dialect: ShellDialect) -> Vec<&'static str> {
    if dialect == ShellDialect::Fish {
        return vec![
            "abbr",
            "and",
            "argparse",
            "begin",
            "bg",
            "bind",
            "block",
            "break",
            "breakpoint",
            "builtin",
            "case",
            "cd",
            "command",
            "commandline",
            "complete",
            "contains",
            "continue",
            "count",
            "disown",
            "echo",
            "else",
            "emit",
            "end",
            "eval",
            "exec",
            "exit",
            "false",
            "fg",
            "for",
            "function",
            "functions",
            "history",
            "if",
            "isatty",
            "jobs",
            "math",
            "not",
            "or",
            "path",
            "printf",
            "pwd",
            "random",
            "read",
            "realpath",
            "return",
            "set",
            "set_color",
            "source",
            "status",
            "string",
            "switch",
            "test",
            "time",
            "true",
            "type",
            "ulimit",
            "umask",
            "wait",
            "while",
            "[",
        ];
    }
    let mut names = special_builtins().to_vec();
    names.extend([
        "alias", "bg", "cd", "command", "false", "fc", "fg", "getopts", "hash", "jobs", "kill",
        "printf", "pwd", "read", "true", "type", "ulimit", "umask", "unalias", "wait",
    ]);
    match dialect {
        ShellDialect::Bash => names.extend([
            "[",
            "bind",
            "builtin",
            "caller",
            "compgen",
            "complete",
            "compopt",
            "declare",
            "dirs",
            "disown",
            "echo",
            "enable",
            "help",
            "history",
            "let",
            "local",
            "logout",
            "mapfile",
            "popd",
            "pushd",
            "readarray",
            "shopt",
            "source",
            "suspend",
            "test",
            "typeset",
        ]),
        ShellDialect::Zsh => names.extend([
            "[",
            "autoload",
            "builtin",
            "bye",
            "chdir",
            "declare",
            "dirs",
            "disable",
            "disown",
            "echo",
            "echotc",
            "echoti",
            "emulate",
            "enable",
            "float",
            "functions",
            "getln",
            "hash",
            "history",
            "integer",
            "let",
            "limit",
            "local",
            "log",
            "logout",
            "noglob",
            "popd",
            "print",
            "pushd",
            "pushln",
            "rehash",
            "sched",
            "setopt",
            "source",
            "suspend",
            "test",
            "ttyctl",
            "typeset",
            "unfunction",
            "unhash",
            "unlimit",
            "unsetopt",
            "vared",
            "whence",
            "where",
            "which",
            "zcompile",
            "zmodload",
        ]),
        ShellDialect::Mksh => names.extend([
            "[", "builtin", "echo", "global", "hash", "let", "local", "print", "read", "source",
            "test", "typeset", "whence",
        ]),
        ShellDialect::Posix => {}
        ShellDialect::Fish => {}
    }
    names.sort_unstable();
    names.dedup();
    names
}
