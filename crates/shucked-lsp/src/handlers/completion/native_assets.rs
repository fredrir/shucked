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
        .map(|path| path.join("runtime/bin").join(name))
        .filter(|path| path.is_file())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(name))
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
    for helper in [root.join("runtime/helpers/bin"), root.join("runtime/bin")] {
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
    ["runtime/helpers/bin", "runtime/bin"]
        .iter()
        .any(|directory| {
            let path = root.join(directory).join(name);
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
    let alias = directory.path().join(name);
    #[cfg(unix)]
    std::os::unix::fs::symlink(primary, &alias)?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(primary, &alias)?;
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
