//! Audited command metadata interfaces, separate from completion candidates.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use shucked_ast::Span;
use shucked_command::{
    CommandGrammar, CommandKind, CommandResolution, EnvironmentSnapshot, EvidenceKind,
    ExecutableIdentity, ExecutionContext, Provenance, ValidationEvidence, ValidationIssueKind,
    ValidationPolicy, ValidationResult,
};
use shucked_semantic::CommandSiteFacts;

use crate::session::RequestCancellationToken;

pub(crate) struct ValidationDiagnostic {
    pub span: Span,
    pub code: &'static str,
    pub message: String,
    pub suggestions: Vec<String>,
}

struct Cached {
    target: String,
    generation: u64,
    cwd: Option<PathBuf>,
    identity: ExecutableIdentity,
    created: Instant,
    evidence: Option<ValidationEvidence>,
}

static CACHE: OnceLock<Mutex<VecDeque<Cached>>> = OnceLock::new();
const METADATA_TIMEOUT: Duration = Duration::from_millis(1200);
const CACHE_TTL: Duration = Duration::from_secs(20);

pub(crate) fn validate(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    sites: &[(CommandSiteFacts, CommandResolution)],
    cancellation: &RequestCancellationToken,
) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    if context.policy == ValidationPolicy::Portable || !environment.fresh {
        return diagnostics;
    }
    let mut acquired = BTreeMap::new();
    for (site, resolution) in sites {
        if cancellation.is_cancelled() {
            break;
        }
        let CommandResolution::Resolved(resolved) = resolution else {
            continue;
        };
        if resolved.kind != CommandKind::Executable || site.environment_uncertain.is_some() {
            continue;
        }
        let Some(identity) = &resolved.executable else {
            continue;
        };
        let evidence = if context.policy == ValidationPolicy::Captured {
            environment.validators.get(&resolved.name).cloned()
        } else if context.native_execution_allowed {
            acquired
                .entry(identity.path.clone())
                .or_insert_with(|| acquire(context, environment, identity, cancellation))
                .clone()
        } else {
            None
        };
        let Some(evidence) = evidence else {
            continue;
        };
        if !same_file(identity, &evidence.executable) {
            continue;
        }
        let mut invocation = resolved.clone();
        // Acquisition added a tool-reported version, while filesystem lookup
        // intentionally never runs a program just to populate identity fields.
        invocation.executable = Some(evidence.executable.clone());
        let ValidationResult::Invalid(issues) =
            shucked_command::validate_invocation(&invocation, &evidence, &environment.platform)
        else {
            continue;
        };
        for issue in issues {
            let injected = resolved
                .effective_words
                .len()
                .saturating_sub(site.effective_words.len());
            let Some(source_index) = issue.word_index.checked_sub(injected) else {
                continue;
            };
            let Some(word) = site
                .effective_words
                .get(source_index)
                .filter(|word| !word.injected)
            else {
                continue;
            };
            if word.text.as_deref() != Some(issue.value.as_str()) {
                continue;
            }
            let (code, label) = match issue.kind {
                ValidationIssueKind::UnknownSubcommand => ("ENV002", "Unrecognized subcommand"),
                ValidationIssueKind::UnknownFlag => ("ENV003", "Unrecognized flag"),
                ValidationIssueKind::InvalidValue => ("ENV004", "Unrecognized argument value"),
            };
            diagnostics.push(ValidationDiagnostic {
                span: word.span,
                code,
                message: format!(
                    "{label} for {} on {}: {}",
                    resolved.name, context.target_id, issue.value
                ),
                suggestions: issue.suggestions,
            });
        }
    }
    diagnostics
}

fn acquire(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    identity: &ExecutableIdentity,
    cancellation: &RequestCancellationToken,
) -> Option<ValidationEvidence> {
    let name = identity
        .path
        .file_name()?
        .to_str()?
        .trim_end_matches(".exe");
    if !matches!(
        name,
        "brew"
            | "git"
            | "eza"
            | "rg"
            | "fd"
            | "fdfind"
            | "bat"
            | "batcat"
            | "ls"
            | "gls"
            | "pacman"
    ) {
        return None;
    }
    if matches!(name, "brew" | "git") && !environment.is_complete() {
        return None;
    }
    let cache = CACHE.get_or_init(Mutex::default);
    {
        let cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = cache.iter().find(|entry| {
            entry.target == context.target_id
                && entry.generation == environment.generation
                && entry.cwd == context.cwd
                && entry.identity == *identity
                && entry.created.elapsed() < CACHE_TTL
        }) {
            return entry.evidence.clone();
        }
    }
    let evidence = acquire_uncached(context, environment, identity, name, cancellation);
    if !cancellation.is_cancelled() {
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.created.elapsed() < CACHE_TTL);
        cache.push_back(Cached {
            target: context.target_id.clone(),
            generation: environment.generation,
            cwd: context.cwd.clone(),
            identity: identity.clone(),
            created: Instant::now(),
            evidence: evidence.clone(),
        });
        while cache.len() > 64 {
            cache.pop_front();
        }
    }
    evidence
}

fn acquire_uncached(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    identity: &ExecutableIdentity,
    name: &str,
    cancellation: &RequestCancellationToken,
) -> Option<ValidationEvidence> {
    let version_output = query(
        context,
        environment,
        &identity.path,
        if name == "curl" {
            &["-q", "--version"]
        } else {
            &["--version"]
        },
        cancellation,
    )?;
    if !matches!(name, "brew" | "git") {
        return acquire_flag_manifest(context, environment, identity, name, &version_output);
    }
    let version_prefix = if name == "brew" {
        "Homebrew "
    } else {
        "git version "
    };
    let version = version_output
        .lines()
        .next()?
        .strip_prefix(version_prefix)?
        .trim();
    if version.is_empty() || !version.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let mut commands = if name == "brew" {
        let text = query(
            context,
            environment,
            &identity.path,
            &["commands", "--quiet", "--include-aliases"],
            cancellation,
        )?;
        let mut names = parse_command_names(&text)?;
        if !["install", "list", "commands"]
            .iter()
            .all(|name| names.contains(*name))
        {
            return None;
        }
        // The public command listing omits hidden internal commands. Inspect
        // the matching installation, and retain every internal file stem.
        let repository = query(
            context,
            environment,
            &identity.path,
            &["--repository"],
            cancellation,
        )?;
        let repository = Path::new(repository.trim());
        if !repository.is_absolute() {
            return None;
        }
        extend_file_commands(&repository.join("Library/Homebrew/cmd"), &mut names, false)?;
        extend_file_commands(
            &repository.join("Library/Homebrew/dev-cmd"),
            &mut names,
            false,
        )?;
        // Homebrew also accepts executable brew-* names on its execution PATH.
        for directory in &environment.search_path {
            for command in directory
                .commands
                .keys()
                .filter_map(|name| name.strip_prefix("brew-"))
            {
                names.insert(command.trim_end_matches(".rb").to_owned());
            }
        }
        // User aliases can exist independently of PATH visibility. We inspect
        // filenames only; their contents may contain secrets or shell code.
        if let Some(home) = std::env::var_os("HOME") {
            extend_file_commands(&PathBuf::from(home).join(".brew-aliases"), &mut names, true)?;
        }
        names
    } else {
        let text = query(
            context,
            environment,
            &identity.path,
            &["--list-cmds=builtins,main,others,alias"],
            cancellation,
        )?;
        let names = parse_command_names(&text)?;
        if !["add", "commit", "help"]
            .iter()
            .all(|name| names.contains(*name))
        {
            return None;
        }
        names
    };
    commands.retain(|name| !name.is_empty());
    let shucked_command::LookupEvidence::Present(current) =
        shucked_command::host::exact_lookup(context, environment, identity.path.to_str()?)
    else {
        return None;
    };
    if !same_file(identity, &current.identity) {
        return None;
    }
    let mut identified = identity.clone();
    identified.version = Some(version.into());
    identified.vendor = Some(if name == "brew" { "Homebrew" } else { "Git" }.into());
    let grammar = CommandGrammar {
        subcommands: commands
            .into_iter()
            .map(|name| (name, CommandGrammar::default()))
            .collect(),
        subcommands_complete: true,
        requires_subcommand: true,
        // Only bare first-subcommand positions are authoritative here. Global
        // options and child arguments intentionally stay outside this scope.
        ..CommandGrammar::default()
    };
    Some(ValidationEvidence {
        executable: identified,
        platform: environment.platform.clone(),
        kind: EvidenceKind::StructuredInterface,
        grammar,
        extensions: BTreeSet::new(),
        extensions_complete: true,
        plugin_extensible: true,
        fresh: true,
        provenance: Provenance::new(format!("{name} installed command metadata")),
    })
}

fn acquire_flag_manifest(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    identity: &ExecutableIdentity,
    name: &str,
    output: &str,
) -> Option<ValidationEvidence> {
    let (tool, version) = match name {
        "eza"
            if output
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("eza ")) =>
        {
            (
                "eza",
                output
                    .lines()
                    .find_map(|line| line.strip_prefix('v'))?
                    .split_whitespace()
                    .next()?,
            )
        }
        "curl" => (
            "curl",
            output
                .lines()
                .next()?
                .strip_prefix("curl ")?
                .split_whitespace()
                .next()?,
        ),
        "rg" => (
            "ripgrep",
            output
                .lines()
                .next()?
                .strip_prefix("ripgrep ")?
                .split_whitespace()
                .next()?,
        ),
        "fd" | "fdfind" => (
            "fd",
            output
                .lines()
                .next()?
                .strip_prefix("fd ")?
                .split_whitespace()
                .next()?,
        ),
        "bat" | "batcat" => (
            "bat",
            output
                .lines()
                .next()?
                .strip_prefix("bat ")?
                .split_whitespace()
                .next()?,
        ),
        "pacman" => (
            "pacman",
            output
                .lines()
                .find_map(|line| line.split_once("Pacman v").map(|(_, version)| version))?
                .split_whitespace()
                .next()?,
        ),
        "ls" | "gls" => (
            "gnu-ls",
            output
                .lines()
                .next()?
                .strip_prefix("ls (GNU coreutils) ")?
                .trim(),
        ),
        _ => return None,
    };
    let manifest = shucked_command::known_tool_grammar(tool, version)?;
    let shucked_command::LookupEvidence::Present(current) =
        shucked_command::host::exact_lookup(context, environment, identity.path.to_str()?)
    else {
        return None;
    };
    if !same_file(identity, &current.identity) {
        return None;
    }
    let mut identified = identity.clone();
    identified.version = Some(version.into());
    identified.vendor = Some(tool.into());
    Some(ValidationEvidence {
        executable: identified,
        platform: environment.platform.clone(),
        kind: EvidenceKind::VersionedManifest,
        grammar: manifest.grammar,
        extensions: BTreeSet::new(),
        extensions_complete: true,
        plugin_extensible: false,
        fresh: true,
        provenance: Provenance {
            source: format!("{tool} {version} option-name grammar"),
            location: Some(manifest.source),
        },
    })
}

fn same_file(left: &ExecutableIdentity, right: &ExecutableIdentity) -> bool {
    left.path == right.path
        && left.size == right.size
        && left.modified_unix_ms == right.modified_unix_ms
}

fn query(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    executable: &Path,
    arguments: &[&'static str],
    cancellation: &RequestCancellationToken,
) -> Option<String> {
    let path = std::env::join_paths(
        environment
            .search_path
            .iter()
            .map(|directory| &directory.path),
    )
    .ok()?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env("PATH", path)
        .env("LC_ALL", "C")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("NONINTERACTIVE", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "");
    if let Some(cwd) = &context.cwd {
        command.current_dir(cwd);
    }
    let output = super::completion::native_process::capture(
        &mut command,
        METADATA_TIMEOUT,
        cancellation,
        false,
    )?;
    String::from_utf8(output).ok()
}

fn parse_command_names(text: &str) -> Option<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    for name in text.split_whitespace() {
        if result.len() >= 20_000
            || name.len() > 256
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
        {
            return None;
        }
        result.insert(name.to_owned());
    }
    (!result.is_empty()).then_some(result)
}

fn extend_file_commands(
    directory: &Path,
    names: &mut BTreeSet<String>,
    optional: bool,
) -> Option<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Some(()),
        Err(_) => return None,
    };
    for (index, entry) in entries.enumerate() {
        if index >= 20_000 {
            return None;
        }
        let entry = entry.ok()?;
        if !entry.metadata().ok()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str()?;
        let stem = if optional {
            name
        } else {
            name.strip_suffix(".rb")
                .or_else(|| name.strip_suffix(".sh"))
                .unwrap_or(name)
        };
        names.insert(stem.into());
    }
    Some(())
}

#[cfg(all(test, unix))]
#[path = "../../tests/commands/validation.rs"]
mod tests;
