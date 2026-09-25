use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(in super::super) fn root() -> Option<PathBuf> {
    let root = std::env::var_os("SHUCKED_PROVIDER_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.join("packs/manifest.json").is_file())
        .or_else(|| {
            let path = std::env::current_exe().ok()?.parent()?.join("providers");
            path.join("packs/manifest.json").is_file().then_some(path)
        })
        .or_else(|| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tooling/providers");
            path.join("packs/manifest.json").is_file().then_some(path)
        });
    if root.is_none() {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::warn!(
                "bundled completion providers are unavailable: set SHUCKED_PROVIDER_ROOT or install \
                 the providers directory next to the server; only installed shell completion \
                 definitions are offered"
            );
        });
    }
    root
}

/// The OS cache directory for the server, mirroring the CLI's resolution.
fn cache_directory() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("SHUCKED_CACHE_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(explicit);
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("shucked"));
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".cache"))
        })
        .map(|cache| cache.join("shucked"))
}

/// A completion dump file for the managed Zsh engine, keyed by everything that
/// determines its function path: the engine binary, the bundled packs, and the
/// installed definition directories including their modification times. A
/// changed pack set or definition directory yields a new file, so `compinit -C`
/// can trust the dump without rescanning the whole function path.
pub(in super::super) fn completion_dump(
    shell: &Path,
    root: Option<&Path>,
    installed: &[PathBuf],
) -> Option<PathBuf> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let stamp = |hasher: &mut sha2::Sha256, path: &Path| {
        hasher.update(path.as_os_str().as_encoded_bytes());
        hasher.update([0]);
        if let Ok(metadata) = std::fs::metadata(path) {
            hasher.update(metadata.len().to_le_bytes());
            if let Ok(modified) = metadata.modified()
                && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                hasher.update(since.as_nanos().to_le_bytes());
            }
        }
        hasher.update([0]);
    };
    stamp(&mut hasher, shell);
    if let Some(root) = root {
        stamp(&mut hasher, &root.join("packs/manifest.json"));
        for pack in ["zsh-extra", "zsh-completions/src", "zsh/Completion"] {
            stamp(&mut hasher, &root.join("packs").join(pack));
        }
    }
    for directory in installed {
        stamp(&mut hasher, directory);
    }
    let digest = hasher.finalize();
    let key = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = cache_directory()?.join("completion").join("zsh");
    std::fs::create_dir_all(&directory).ok()?;
    let dump = directory.join(format!("{key}.zcompdump"));
    if !dump.exists() {
        prune_dumps(&directory);
    }
    Some(dump)
}

/// Remove dump files that no current key has used for a week.
fn prune_dumps(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten().take(256) {
        let path = entry.path();
        let stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(7 * 24 * 60 * 60));
        if stale
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".zcompdump"))
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(in super::super) fn shell(name: &str) -> Option<PathBuf> {
    root()
        .and_then(|path| managed_shell(&path, name, cfg!(windows)))
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .filter(|path| path.is_absolute())
                    .flat_map(|path| shell_candidates(&path, name, cfg!(windows)))
                    .find(|path| path.is_file())
            })
        })
        .or_else(|| {
            let path = Path::new("/bin").join(name);
            path.is_file().then_some(path)
        })
}

/// Extend only a private worker's PATH. Target discovery keeps the original PATH.
pub(in super::super) fn configure_worker_path(
    command: &mut std::process::Command,
    root: &Path,
    execution_path: Option<&std::ffi::OsStr>,
) {
    let original = execution_path
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let mut paths: Vec<_> = std::env::split_paths(&original).collect();
    for helper in helper_directories(root) {
        if helper.is_dir() && !paths.contains(&helper) {
            paths.push(helper);
        }
    }
    command.env("SHUCKED_TARGET_PATH", &original);
    if let Ok(path) = std::env::join_paths(paths) {
        command.env("PATH", path);
    }
}

pub(in super::super) fn private_command(root: &Path, name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) {
        return false;
    }
    helper_directories(root).iter().any(|directory| {
        let path = directory.join(name);
        path.is_file() || (cfg!(windows) && path.with_extension("exe").is_file())
    })
}

/// Pin hardcoded provider queries to the resolved primary executable.
/// The private directory must outlive the worker and never enters target discovery.
///
/// When the worker's PATH already resolves the name to that very file, no
/// binding is made: the tool then sees its real location, which matters for
/// programs that derive their prefix from their own path.
pub(in super::super) fn bind_primary(
    command: &mut std::process::Command,
    primary: Option<&str>,
) -> std::io::Result<Option<tempfile::TempDir>> {
    let Some(primary) = primary.map(Path::new).filter(|path| path.is_absolute()) else {
        return Ok(None);
    };
    let name = primary.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "primary command has no filename",
        )
    })?;
    let current_path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value.map(ToOwned::to_owned))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    if resolves_to(&current_path, name, primary) {
        return Ok(None);
    }
    let directory = tempfile::Builder::new()
        .prefix("shucked-primary-")
        .tempdir()?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(primary, directory.path().join(name))?;
    #[cfg(windows)]
    {
        // Moving/copying a PE binary can break its adjacent DLL lookup. A
        // private MSYS script preserves the executable location and needs no
        // Windows symlink privilege.
        let name = name.to_string_lossy();
        if primary.extension().is_some_and(|extension| {
            extension.to_string_lossy().eq_ignore_ascii_case("cmd")
                || extension.to_string_lossy().eq_ignore_ascii_case("bat")
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "batch completion binding unavailable",
            ));
        }
        let alias = directory.path().join(without_exe_suffix(&name));
        let target = shell_path(primary);
        let quoted = target.to_string_lossy().replace('\'', "'\"'\"'");
        std::fs::write(alias, format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n"))?;
    }
    #[cfg(not(any(unix, windows)))]
    return Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "primary binding unavailable",
    ));
    let mut paths = vec![directory.path().to_path_buf()];
    paths.extend(std::env::split_paths(&current_path));
    let path = std::env::join_paths(paths)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    command.env("PATH", path);
    Ok(Some(directory))
}

/// Whether the first PATH entry holding `name` is the `primary` file itself.
fn resolves_to(path: &std::ffi::OsStr, name: &std::ffi::OsStr, primary: &Path) -> bool {
    let Ok(target) = std::fs::canonicalize(primary) else {
        return false;
    };
    for directory in std::env::split_paths(path).filter(|directory| directory.is_absolute()) {
        for candidate in shell_candidates(&directory, &name.to_string_lossy(), cfg!(windows)) {
            if candidate.is_file() {
                return std::fs::canonicalize(&candidate).is_ok_and(|found| found == target);
            }
        }
    }
    false
}

/// Function directories of the Zsh engine itself, for provider discovery when
/// the bundled packs do not cover a name. Derived once from the engine's own
/// `$fpath`, or from the conventional layouts when that query is unavailable.
pub(in super::super) fn engine_function_directories() -> Vec<PathBuf> {
    static DIRECTORIES: OnceLock<Vec<PathBuf>> = OnceLock::new();
    DIRECTORIES
        .get_or_init(|| {
            let Some(zsh) = shell("zsh") else {
                return Vec::new();
            };
            let mut directories = engine_fpath(&zsh);
            if directories.is_empty() {
                let mut prefixes: Vec<PathBuf> = zsh
                    .parent()
                    .and_then(Path::parent)
                    .map(Path::to_owned)
                    .into_iter()
                    .collect();
                prefixes.extend(
                    ["/usr", "/usr/local", "/opt/homebrew", "/opt/local"].map(PathBuf::from),
                );
                directories = engine_layout_directories(prefixes.iter().map(PathBuf::as_path));
            }
            directories
        })
        .clone()
}

/// Ask the engine for its function path without any startup file.
fn engine_fpath(zsh: &Path) -> Vec<PathBuf> {
    let mut command = std::process::Command::new(zsh);
    shell_args(&mut command, ["-f", "-c", "print -rl -- $fpath"]);
    command.env("TERM", "dumb");
    let Some(output) = shucked_command::process::capture(
        &mut command,
        std::time::Duration::from_secs(2),
        &|| false,
        false,
    ) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .collect()
}

/// Conventional engine layouts under an installation prefix: a versioned
/// `share/zsh/<version>/functions` directory (macOS, Homebrew) and the nested
/// `share/zsh/functions/**` tree (Debian, Arch), plus `share/zsh/site-functions`.
pub(in super::super) fn engine_layout_directories<'a>(
    prefixes: impl Iterator<Item = &'a Path>,
) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut push = |directory: PathBuf| {
        if directory.is_dir() && !directories.contains(&directory) {
            directories.push(directory);
        }
    };
    for prefix in prefixes {
        let zsh = prefix.join("share/zsh");
        push(zsh.join("site-functions"));
        if let Ok(entries) = std::fs::read_dir(&zsh) {
            let mut versions = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(|ch: char| ch.is_ascii_digit()))
                })
                .collect::<Vec<_>>();
            versions.sort();
            for version in versions {
                push(version.join("functions"));
            }
        }
        let mut pending = vec![(zsh.join("functions"), 0usize)];
        while let Some((directory, depth)) = pending.pop() {
            if !directory.is_dir() {
                continue;
            }
            push(directory.clone());
            if depth >= 4 {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&directory) {
                let mut children = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect::<Vec<_>>();
                children.sort();
                children.reverse();
                pending.extend(children.into_iter().map(|child| (child, depth + 1)));
            }
        }
    }
    directories
}

/// Directories scanned for installed completion definitions: the standard
/// installation directories on the execution host plus, for Zsh, the engine's
/// own function directories. The engine directories are for discovery only;
/// the worker already has them on its default function path, and adding them
/// ahead of the bundled packs could mix core functions across versions.
pub(in super::super) fn discovery_directories(
    execution_path: Option<&std::ffi::OsStr>,
    engine: &str,
) -> Vec<PathBuf> {
    let mut directories = completion_directories(execution_path, engine);
    if engine == "zsh" {
        let root = root();
        for directory in engine_function_directories() {
            if root
                .as_ref()
                .is_some_and(|root| directory.starts_with(root))
            {
                continue;
            }
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
    }
    directories
}

fn shell_candidates(directory: &Path, name: &str, windows: bool) -> Vec<PathBuf> {
    if windows {
        vec![directory.join(format!("{name}.exe")), directory.join(name)]
    } else {
        vec![directory.join(name)]
    }
}

fn managed_shell(root: &Path, name: &str, windows: bool) -> Option<PathBuf> {
    let directories = if windows {
        vec![root.join("runtime/msys/usr/bin"), root.join("runtime/bin")]
    } else {
        vec![root.join("runtime/bin")]
    };
    directories
        .into_iter()
        .flat_map(|path| shell_candidates(&path, name, windows))
        .find(|path| path.is_file())
}

fn helper_directories(root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.join("runtime/helpers/bin"), root.join("runtime/bin")];
    if cfg!(windows) {
        directories.push(root.join("runtime/msys/usr/bin"));
    }
    directories
}

/// Filesystem values consumed by the managed POSIX shell, not target discovery.
pub(in super::super) fn shell_path(path: &Path) -> std::ffi::OsString {
    if cfg!(windows) {
        windows_shell_path(&path.to_string_lossy()).into()
    } else {
        path.as_os_str().to_owned()
    }
}

fn windows_shell_path(path: &str) -> String {
    let path = path
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!("//{rest}"))
        .unwrap_or_else(|| path.strip_prefix(r"\\?\").unwrap_or(path).to_owned())
        .replace('\\', "/");
    if path.as_bytes().get(1) == Some(&b':')
        && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && path.as_bytes().get(2) == Some(&b'/')
    {
        format!("/{}{}", path[..1].to_ascii_lowercase(), &path[2..])
    } else {
        path
    }
}

#[cfg(test)]
#[path = "../../../tests/completion/native_assets.rs"]
mod tests;

// The private primary binding keeps this basename pinned to the resolved host
// command. Completion packs conventionally register names without .exe.
pub(in super::super) fn primary_word(word: &str) -> String {
    if cfg!(windows) && Path::new(word).is_absolute() {
        let name = Path::new(word)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        without_exe_suffix(&name).to_owned()
    } else {
        word.to_owned()
    }
}

/// MSYS expands unquoted wildcards even when the parent supplied argv directly.
pub(in super::super) fn shell_args<I, S>(command: &mut std::process::Command, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    for arg in args {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.raw_arg(quote_windows_argument(arg.as_ref()));
        }
        #[cfg(not(windows))]
        command.arg(arg.as_ref());
    }
}

#[cfg(any(windows, test))]
fn quote_windows_argument(argument: &str) -> String {
    let mut quoted = String::from("\"");
    let mut slashes = 0;
    for character in argument.chars() {
        if character == '\\' {
            slashes += 1;
        } else {
            quoted.extend(std::iter::repeat_n(
                '\\',
                if character == '\"' {
                    2 * slashes + 1
                } else {
                    slashes
                },
            ));
            slashes = 0;
            quoted.push(character);
        }
    }
    quoted.extend(std::iter::repeat_n('\\', 2 * slashes));
    quoted.push('\"');
    quoted
}

fn without_exe_suffix(name: &str) -> &str {
    name.get(name.len().saturating_sub(4)..)
        .filter(|suffix| suffix.eq_ignore_ascii_case(".exe"))
        .map_or(name, |_| &name[..name.len() - 4])
}

/// Standard completion installation directories on the selected execution host.
/// Only absolute PATH prefixes are considered; personal startup files stay out.
pub(in super::super) fn completion_directories(
    execution_path: Option<&std::ffi::OsStr>,
    engine: &str,
) -> Vec<PathBuf> {
    let path = execution_path
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let relative = match engine {
        "zsh" => "share/zsh/site-functions",
        "bash" => "share/bash-completion/completions",
        _ => "share/fish/vendor_completions.d",
    };
    let mut directories = Vec::new();
    for prefix in std::env::split_paths(&path)
        .filter(|path| path.is_absolute())
        .filter_map(|path| path.parent().map(Path::to_owned))
    {
        let directory = prefix.join(relative);
        if directory.is_dir() && !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    for prefix in ["/usr/local", "/usr", "/opt/homebrew"] {
        let directory = Path::new(prefix).join(relative);
        if directory.is_dir() && !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    if engine == "zsh" {
        let directory = PathBuf::from("/usr/share/zsh/vendor-completions");
        if directory.is_dir() && !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

pub(in super::super) fn joined_completion_paths(directories: &[PathBuf]) -> String {
    directories
        .iter()
        .map(|path| shell_path(path).to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":")
}
