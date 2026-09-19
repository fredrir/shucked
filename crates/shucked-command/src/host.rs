//! Filesystem inspection only. This module never invokes shell startup files or
//! executes programs to discover their identities or capabilities.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    EnvironmentSnapshot, Executable, ExecutableIdentity, ExecutionContext, LookupEvidence,
    Provenance, SearchDirectory,
};

const MAX_DIRECTORIES: usize = 256;
const MAX_ENTRIES_PER_DIRECTORY: usize = 40_000;

pub fn capture_current(context: &ExecutionContext, generation: u64) -> EnvironmentSnapshot {
    let Some(path) = std::env::var_os("PATH") else {
        let mut snapshot = EnvironmentSnapshot::empty(context);
        snapshot.generation = generation;
        snapshot.captured_unix_ms = now_unix_ms();
        return snapshot;
    };
    let mut snapshot = capture(context, std::env::split_paths(&path).collect(), generation);
    if cfg!(windows) {
        snapshot.executable_extensions = windows_extensions();
    }
    snapshot
}

/// Capture exactly these execution PATH entries. Discovery directories and
/// private completion helpers must not be added to this list.
pub fn capture(
    context: &ExecutionContext,
    paths: Vec<PathBuf>,
    generation: u64,
) -> EnvironmentSnapshot {
    let mut snapshot = EnvironmentSnapshot::empty(context);
    snapshot.generation = generation;
    snapshot.captured_unix_ms = now_unix_ms();
    snapshot.path_known = true;
    snapshot.case_sensitive = !cfg!(windows);
    snapshot.executable_extensions = if cfg!(windows) {
        windows_extensions()
    } else {
        Vec::new()
    };
    for (index, path) in paths.into_iter().enumerate() {
        let mut directory = SearchDirectory {
            path: path.clone(),
            commands: BTreeMap::new(),
            complete: false,
            failure: None,
        };
        if index >= MAX_DIRECTORIES {
            directory.failure = Some("Execution PATH exceeds the directory scan limit".into());
        } else if let Some(absolute) = absolute_path(context, &path) {
            scan_directory(&absolute, &snapshot.executable_extensions, &mut directory);
        } else {
            directory.failure =
                Some("A relative PATH entry requires a known launch directory".into());
        }
        snapshot.search_path.push(directory);
    }
    snapshot
}

/// Supplement a bounded listing with exact point queries. Errors are preserved
/// as Unknown; a later PATH match cannot hide uncertainty in an earlier entry.
pub fn refresh_exact(
    context: &ExecutionContext,
    snapshot: &mut EnvironmentSnapshot,
    names: &[String],
) {
    if context.target_id != snapshot.target_id
        || context.policy == crate::ValidationPolicy::Captured
    {
        return;
    }
    for name in names.iter().take(4096) {
        let evidence = exact_lookup(context, snapshot, name);
        snapshot.exact_lookups.insert(name.clone(), evidence);
    }
}

pub fn exact_lookup(
    context: &ExecutionContext,
    snapshot: &EnvironmentSnapshot,
    name: &str,
) -> LookupEvidence {
    if context.target_id != snapshot.target_id {
        return LookupEvidence::Unknown("The environment belongs to another target".into());
    }
    if context.policy == crate::ValidationPolicy::Captured {
        return LookupEvidence::Unknown(
            "Captured targets cannot query the local filesystem".into(),
        );
    }
    if !snapshot.fresh {
        return LookupEvidence::Unknown("The environment snapshot is stale".into());
    }
    if name.is_empty() || name.contains('\0') || name.len() > 4096 {
        return LookupEvidence::Unknown(
            "The command name cannot be inspected as a filesystem path".into(),
        );
    }
    let explicit = name.contains('/') || (cfg!(windows) && name.contains('\\'));
    if explicit {
        let Some(path) = absolute_path(context, Path::new(name)) else {
            return LookupEvidence::Unknown(
                "A relative executable path requires a known launch directory".into(),
            );
        };
        return inspect_executable(&path, &snapshot.executable_extensions);
    }
    if !snapshot.path_known {
        return LookupEvidence::Unknown("The target execution PATH is unknown".into());
    }
    for directory in &snapshot.search_path {
        let Some(path) = absolute_path(context, &directory.path) else {
            return LookupEvidence::Unknown(
                "A relative PATH entry requires a known launch directory".into(),
            );
        };
        let evidence = inspect_executable(&path.join(name), &snapshot.executable_extensions);
        if !matches!(evidence, LookupEvidence::Missing) {
            return evidence;
        }
    }
    LookupEvidence::Missing
}

fn absolute_path(context: &ExecutionContext, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_owned());
    }
    if !context.cwd_known {
        return None;
    }
    context
        .cwd
        .as_ref()
        .filter(|cwd| cwd.is_absolute())
        .map(|cwd| cwd.join(path))
}

fn scan_directory(path: &Path, extensions: &[String], directory: &mut SearchDirectory) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if absent(&error) => {
            directory.complete = true;
            return;
        }
        Err(error) => {
            directory.failure = Some(format!("Cannot inspect PATH directory: {error}"));
            return;
        }
    };
    directory.complete = true;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_ENTRIES_PER_DIRECTORY {
            directory.complete = false;
            directory.failure = Some("PATH directory exceeds the entry scan limit".into());
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                directory.complete = false;
                directory.failure = Some(format!("Cannot inspect a PATH entry: {error}"));
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        match inspect_one(&entry.path(), extensions) {
            LookupEvidence::Present(executable) => {
                let key = if cfg!(windows) {
                    name.to_lowercase()
                } else {
                    name.to_owned()
                };
                directory.commands.insert(key.clone(), executable.clone());
                if cfg!(windows)
                    && let Some((stem, suffix)) = key.rsplit_once('.')
                    && extensions
                        .iter()
                        .any(|extension| extension.eq_ignore_ascii_case(&format!(".{suffix}")))
                {
                    // Exact lookups disambiguate collisions according to
                    // PATHEXT; directory entries are suggestions only there.
                    directory.commands.entry(stem.into()).or_insert(executable);
                }
            }
            LookupEvidence::Unknown(detail) => {
                directory.complete = false;
                directory.failure = Some(detail);
            }
            LookupEvidence::Missing => {}
        }
    }
    if cfg!(windows) {
        // Enumeration does not establish PATHEXT precedence for collisions.
        // Exact lookups provide that evidence when requested.
        directory.complete = false;
        directory.failure =
            Some("Windows command lookup requires a PATHEXT-aware point query".into());
    }
}

fn inspect_executable(path: &Path, extensions: &[String]) -> LookupEvidence {
    let direct = inspect_one(path, extensions);
    if !matches!(direct, LookupEvidence::Missing) {
        return direct;
    }
    if cfg!(windows) {
        for extension in extensions {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(extension);
            let evidence = inspect_one(Path::new(&candidate), extensions);
            if !matches!(evidence, LookupEvidence::Missing) {
                return evidence;
            }
        }
    }
    LookupEvidence::Missing
}

fn inspect_one(path: &Path, extensions: &[String]) -> LookupEvidence {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if absent(&error) => return LookupEvidence::Missing,
        Err(error) => {
            return LookupEvidence::Unknown(format!("Cannot inspect executable: {error}"));
        }
    };
    if !metadata.is_file() || !executable(&metadata, path, extensions) {
        return LookupEvidence::Missing;
    }
    match executable_access(path) {
        Ok(true) => {}
        Ok(false) => return LookupEvidence::Missing,
        Err(detail) => return LookupEvidence::Unknown(detail),
    }
    LookupEvidence::Present(Executable {
        identity: ExecutableIdentity {
            path: path.to_owned(),
            size: Some(metadata.len()),
            modified_unix_ms: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
            version: None,
            vendor: None,
        },
        provenance: Provenance {
            source: "target execution PATH".into(),
            location: Some(path.to_string_lossy().into_owned()),
        },
    })
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata, _: &Path, _: &[String]) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn executable(_: &fs::Metadata, path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&format!(".{extension}")))
        })
}

#[cfg(not(any(unix, windows)))]
fn executable(_: &fs::Metadata, _: &Path, _: &[String]) -> bool {
    false
}

fn absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(unix)]
fn executable_access(path: &Path) -> Result<bool, String> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "Executable path contains a NUL byte".to_owned())?;
    // The C string stays alive for this call; no pointer is retained. Effective
    // IDs match the credentials a subsequently launched process would use.
    let result =
        unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), libc::X_OK, libc::AT_EACCESS) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if absent(&error) || error.kind() == std::io::ErrorKind::PermissionDenied {
        Ok(false)
    } else {
        Err(format!("Cannot determine executable access: {error}"))
    }
}

#[cfg(not(unix))]
fn executable_access(_: &Path) -> Result<bool, String> {
    Ok(true)
}

fn windows_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|extension| extension.starts_with('.') && !extension.contains(['/', '\\']))
        .map(str::to_owned)
        .collect()
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
