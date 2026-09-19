//! Audited installed-tool metadata. Candidate arguments are never query inputs.
use crate::{
    CommandGrammar, EnvironmentSnapshot, EvidenceKind, ExecutableIdentity, ExecutionContext,
    Provenance, ValidationEvidence, ValidationPolicy,
};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Cached {
    context: ExecutionContext,
    paths: Vec<PathBuf>,
    generation: u64,
    identity: ExecutableIdentity,
    created: Instant,
    evidence: Option<ValidationEvidence>,
}

static CACHE: OnceLock<Mutex<VecDeque<Cached>>> = OnceLock::new();
const METADATA_TIMEOUT: Duration = Duration::from_millis(1200);
const CACHE_TTL: Duration = Duration::from_secs(20);
const SUPPORTED_TOOLS: &[&str] = &[
    "brew", "git", "eza", "rg", "fd", "fdfind", "bat", "batcat", "ls", "gls", "pacman", "curl",
];

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct CapabilityCapture {
    pub recorded: BTreeSet<String>,
    pub unknown: BTreeSet<String>,
}

/// Explicit capture executes only fixed audited queries, within a total deadline.
/// Frozen inventories and untrusted/portable contexts cannot acquire host evidence.
pub fn capture_capabilities(
    context: &ExecutionContext,
    environment: &mut EnvironmentSnapshot,
    cancellation: &dyn Fn() -> bool,
) -> CapabilityCapture {
    let mut report = CapabilityCapture::default();
    if !context.native_execution_allowed
        || matches!(
            context.policy,
            ValidationPolicy::Captured | ValidationPolicy::Portable
        )
        || context.target_id != environment.target_id
        || !environment.fresh
        || cancellation()
    {
        return report;
    }
    let started = Instant::now();
    let cancelled = || cancellation() || started.elapsed() >= Duration::from_secs(15);
    environment.validators.clear();
    crate::host::refresh_exact(
        context,
        environment,
        &SUPPORTED_TOOLS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    );
    for &name in SUPPORTED_TOOLS {
        let crate::LookupEvidence::Present(mut executable) = environment.lookup(name) else {
            continue;
        };
        if cancelled() {
            report.unknown.insert(name.into());
            continue;
        }
        let Some(evidence) = acquire_impl(
            context,
            environment,
            &executable.identity,
            &cancelled,
            false,
        ) else {
            report.unknown.insert(name.into());
            continue;
        };
        for directory in &mut environment.search_path {
            for item in directory.commands.values_mut() {
                if same_file(&item.identity, &executable.identity) {
                    item.identity = evidence.executable.clone();
                }
            }
        }
        executable.identity = evidence.executable.clone();
        environment
            .exact_lookups
            .insert(name.into(), crate::LookupEvidence::Present(executable));
        environment.validators.insert(name.into(), evidence);
        report.recorded.insert(name.into());
    }
    report
}

pub fn acquire(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    identity: &ExecutableIdentity,
    cancellation: &dyn Fn() -> bool,
) -> Option<ValidationEvidence> {
    acquire_impl(context, environment, identity, cancellation, true)
}

fn acquire_impl(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    identity: &ExecutableIdentity,
    cancellation: &dyn Fn() -> bool,
    cached: bool,
) -> Option<ValidationEvidence> {
    if !context.native_execution_allowed
        || matches!(
            context.policy,
            ValidationPolicy::Captured | ValidationPolicy::Portable
        )
        || !environment.fresh
        || context.target_id != environment.target_id
        || cancellation()
    {
        return None;
    }
    let name = identity
        .path
        .file_name()?
        .to_str()?
        .trim_end_matches(".exe");
    if !SUPPORTED_TOOLS.contains(&name) {
        return None;
    }
    if matches!(name, "brew" | "git") && !environment.is_complete() {
        return None;
    }
    let cache = CACHE.get_or_init(Mutex::default);
    if cached {
        let cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = cache.iter().find(|entry| {
            entry.context == *context
                && entry.paths.iter().eq(environment
                    .search_path
                    .iter()
                    .map(|directory| &directory.path))
                && entry.generation == environment.generation
                && entry.identity == *identity
                && entry.created.elapsed() < CACHE_TTL
        }) {
            return entry.evidence.clone();
        }
    }
    let evidence = acquire_uncached(context, environment, identity, name, cancellation);
    if cached && !cancellation() {
        let mut cache = cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.created.elapsed() < CACHE_TTL);
        cache.push_back(Cached {
            context: context.clone(),
            paths: environment
                .search_path
                .iter()
                .map(|directory| directory.path.clone())
                .collect(),
            generation: environment.generation,
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
    cancellation: &dyn Fn() -> bool,
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
    let crate::LookupEvidence::Present(current) =
        crate::host::exact_lookup(context, environment, identity.path.to_str()?)
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
    let manifest = crate::known_tool_grammar(tool, version)?;
    let crate::LookupEvidence::Present(current) =
        crate::host::exact_lookup(context, environment, identity.path.to_str()?)
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
    cancellation: &dyn Fn() -> bool,
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
    let output = crate::process::capture(&mut command, METADATA_TIMEOUT, cancellation, false)?;
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
