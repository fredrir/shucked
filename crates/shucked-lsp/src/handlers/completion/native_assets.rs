use std::path::{Path, PathBuf};

pub(in super::super) fn root() -> Option<PathBuf> {
    std::env::var_os("SHUCKED_PROVIDER_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.join("packs/manifest.json").is_file())
        .or_else(|| {
            let path = std::env::current_exe().ok()?.parent()?.join("providers");
            path.join("packs/manifest.json").is_file().then_some(path)
        })
        .or_else(|| {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tooling/providers");
            path.join("packs/manifest.json").is_file().then_some(path)
        })
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
    let current_path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value.map(ToOwned::to_owned))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let mut paths = vec![directory.path().to_path_buf()];
    paths.extend(std::env::split_paths(&current_path));
    let path = std::env::join_paths(paths)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    command.env("PATH", path);
    Ok(Some(directory))
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
