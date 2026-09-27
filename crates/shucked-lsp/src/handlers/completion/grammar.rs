//! Option and subcommand candidates from the bundled validator grammars.
//!
//! A grammar is bound to the resolved executable the way validation binds it:
//! through the audited version query, so `apple-ls-479` follows the resolver's
//! choice. When the version cannot be identified (execution is not permitted,
//! the release is not covered, or the query is slow) the newest bundled grammar
//! for the tool answers as an unverified suggestion, because completion is
//! suggestions-only and never establishes invalidity.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use shucked_command::{
    CommandGrammar, EnvironmentSnapshot, ExecutableIdentity, ExecutionContext, FlagValue,
    ResolvedCommand, ValidationEvidence, ValidationPolicy,
};

use super::native_zsh::Candidate;
use super::offline::Request;

/// Time the request thread may spend identifying the installed version.
const IDENTIFY_BUDGET: Duration = Duration::from_millis(500);
/// A slow or cancelled identification is retried after this long.
const RETRY_AFTER: Duration = Duration::from_secs(30);
const MAX_CACHED: usize = 64;

pub(super) struct Binding {
    pub grammar: Arc<CommandGrammar>,
    /// Shown after each description: `gnu-ls 9.7`, or `gnu-ls 9.7 (unverified)`.
    pub provider: String,
}

struct Cached {
    path: PathBuf,
    size: Option<u64>,
    modified: Option<u64>,
    policy: ValidationPolicy,
    execution: bool,
    created: Instant,
    /// Identification was cut short; retry after `RETRY_AFTER`.
    retry: bool,
    binding: Option<Arc<Binding>>,
}

static CACHE: OnceLock<Mutex<VecDeque<Cached>>> = OnceLock::new();

fn cache() -> std::sync::MutexGuard<'static, VecDeque<Cached>> {
    CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn invalidate() {
    cache().clear();
}

/// The bundled grammar family for an executable name on a platform.
fn grammar_tool(name: &str, platform: &str) -> Option<&'static str> {
    Some(match name {
        "ls" => match platform {
            "macos" => "apple-ls",
            "linux" => "gnu-ls",
            _ => return None,
        },
        "gls" => "gnu-ls",
        "eza" => "eza",
        "rg" => "ripgrep",
        "fd" | "fdfind" => "fd",
        "bat" | "batcat" => "bat",
        "curl" => "curl",
        "ssh" => "openssh",
        "docker" => "docker",
        "kubectl" => "kubectl",
        "pacman" => "pacman",
        _ => return None,
    })
}

fn same_file(left: &ExecutableIdentity, right: &ExecutableIdentity) -> bool {
    left.path == right.path
        && left.size == right.size
        && left.modified_unix_ms == right.modified_unix_ms
}

fn from_evidence(evidence: &ValidationEvidence) -> Binding {
    Binding {
        grammar: Arc::new(evidence.grammar.clone()),
        provider: match (&evidence.executable.vendor, &evidence.executable.version) {
            (Some(vendor), Some(version)) => format!("{vendor} {version}"),
            (Some(vendor), None) => vendor.clone(),
            _ => "grammar".into(),
        },
    }
}

fn fallback(tool: &str) -> Option<Binding> {
    let (version, grammar) = shucked_command::newest_tool_grammar(tool)?;
    Some(Binding {
        grammar: Arc::new(grammar.grammar),
        provider: format!("{tool} {version} (unverified)"),
    })
}

pub(super) fn bind(request: &Request<'_>) -> Option<Arc<Binding>> {
    bind_resolved(
        &request.analysis.context,
        &request.analysis.environment,
        request.resolved,
        request.execution,
        &|| request.cancellation.is_cancelled(),
    )
}

/// Bind a grammar to the resolved executable. `execution` permits the audited
/// version query; without it, or when the query cannot identify a covered
/// release, the newest bundled grammar answers as unverified.
pub(super) fn bind_resolved(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    resolved: &ResolvedCommand,
    execution: bool,
    cancellation: &dyn Fn() -> bool,
) -> Option<Arc<Binding>> {
    let identity = resolved.executable.as_ref()?;
    let name = identity.path.file_name()?.to_str()?;
    let name = name.strip_suffix(".exe").unwrap_or(name);
    let tool = grammar_tool(name, &environment.platform);
    let captured = context.policy == ValidationPolicy::Captured;
    if tool.is_none() && !captured {
        // brew and git have no option grammar; their inventories are separate.
        return None;
    }
    if let Some(entry) = cache().iter().find(|entry| {
        entry.path == identity.path
            && entry.size == identity.size
            && entry.modified == identity.modified_unix_ms
            && entry.policy == context.policy
            && entry.execution == execution
    }) && (!entry.retry || entry.created.elapsed() < RETRY_AFTER)
    {
        return entry.binding.clone();
    }
    let started = Instant::now();
    let cut_short = || cancellation() || started.elapsed() >= IDENTIFY_BUDGET;
    let (exact, retry) = if captured {
        (
            environment
                .validators
                .get(&resolved.name)
                .filter(|evidence| same_file(&evidence.executable, identity))
                .map(from_evidence),
            false,
        )
    } else if execution {
        match shucked_command::metadata::acquire(context, environment, identity, &cut_short) {
            Some(evidence) if same_file(&evidence.executable, identity) => {
                (Some(from_evidence(&evidence)), false)
            }
            _ => (None, cut_short()),
        }
    } else {
        (None, false)
    };
    let binding = exact
        .or_else(|| tool.and_then(fallback))
        .filter(|binding| {
            !binding.grammar.flags.is_empty() || !binding.grammar.subcommands.is_empty()
        })
        .map(Arc::new);
    if cancellation() {
        return binding;
    }
    let mut cache = cache();
    cache.retain(|entry| entry.path != identity.path || entry.execution != execution);
    cache.push_back(Cached {
        path: identity.path.clone(),
        size: identity.size,
        modified: identity.modified_unix_ms,
        policy: context.policy,
        execution,
        created: Instant::now(),
        retry,
        binding: binding.clone(),
    });
    while cache.len() > MAX_CACHED {
        cache.pop_front();
    }
    binding
}

/// Candidates for the word being typed after the given words.
pub(super) fn candidates(binding: &Binding, words: &[String], prefix: &str) -> Vec<Candidate> {
    if words.iter().skip(1).any(|word| word == "--") {
        return Vec::new();
    }
    let Some(node) = node_at(&binding.grammar, words) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    if let Some(letters) = prefix.strip_prefix('-') {
        if prefix == "-" || prefix.starts_with("--") {
            for (name, flag) in &node.flags {
                if name.starts_with(prefix) {
                    result.push(candidate(name.clone(), flag.description.clone(), binding));
                }
            }
            return result;
        }
        // A short cluster such as `-la` grows by one more known letter.
        let cluster: Vec<char> = letters.chars().collect();
        let extendable = node.short_flag_clusters
            && cluster.iter().all(|letter| {
                node.flags
                    .get(&format!("-{letter}"))
                    .is_some_and(|flag| flag.value == FlagValue::None)
            });
        if !extendable {
            return result;
        }
        for (name, flag) in &node.flags {
            let mut chars = name.chars();
            if chars.next() != Some('-') {
                continue;
            }
            let Some(letter) = chars.next() else { continue };
            if letter == '-' || chars.next().is_some() || cluster.contains(&letter) {
                continue;
            }
            result.push(candidate(
                format!("{prefix}{letter}"),
                flag.description.clone(),
                binding,
            ));
        }
        return result;
    }
    if node.requires_subcommand {
        for name in node.subcommands.keys() {
            if name.starts_with(prefix) && !name.starts_with("__") {
                result.push(candidate(name.clone(), None, binding));
            }
        }
    }
    result
}

/// The grammar node the cursor is in, after the words already typed. An
/// unknown subcommand has no node, and so no candidates.
fn node_at<'a>(grammar: &'a CommandGrammar, words: &[String]) -> Option<&'a CommandGrammar> {
    let mut node = grammar;
    let mut index = 1;
    while let Some(word) = words.get(index) {
        if word.starts_with('-') && word != "-" {
            let (name, attached) = word
                .split_once('=')
                .map_or((word.as_str(), false), |(name, _)| (name, true));
            let consumes_next = !attached
                && node
                    .flags
                    .get(name)
                    .is_some_and(|flag| flag.value == FlagValue::Required);
            index += if consumes_next { 2 } else { 1 };
            continue;
        }
        if node.requires_subcommand {
            node = node.subcommands.get(word)?;
        }
        index += 1;
    }
    Some(node)
}

fn candidate(text: String, description: Option<String>, binding: &Binding) -> Candidate {
    Candidate {
        text,
        description: description.unwrap_or_default(),
        kind: None,
        no_space: false,
        provider: binding.provider.clone(),
    }
}
