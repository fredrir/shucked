//! Filesystem-only extension inventories for audited CLI versions.
use crate::{EnvironmentSnapshot, ExecutionContext};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) fn kubectl_configuration_is_plain(context: &ExecutionContext) -> bool {
    if std::env::var("KUBERC").is_ok_and(|value| value == "off")
        || std::env::var("KUBECTL_KUBERC").is_ok_and(|value| value == "false")
    {
        return true;
    }
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let mut paths = vec![PathBuf::from(home).join(".kube/kuberc")];
    if let Some(path) = std::env::var_os("KUBERC").filter(|path| !path.is_empty()) {
        let Some(path) = absolute(context, Path::new(&path)) else {
            return false;
        };
        paths.push(path);
    }
    // Preferences can define command aliases/defaults; presence weakens coverage.
    paths.iter().all(|path| {
        std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

pub(super) fn kubectl(environment: &EnvironmentSnapshot) -> Option<BTreeSet<String>> {
    if !environment.is_complete() {
        return None;
    }
    let mut names = BTreeSet::new();
    for candidate in environment
        .search_path
        .iter()
        .flat_map(|directory| directory.commands.keys())
    {
        if let Some(plugin) = candidate.strip_prefix("kubectl-") {
            // Hyphens delimit subcommands; underscores encode hyphens in a name.
            if let Some(root) = plugin.split('-').next().filter(|name| !name.is_empty()) {
                names.insert(root.replace('_', "-"));
            }
        }
    }
    Some(names)
}

pub(super) fn docker(context: &ExecutionContext) -> Option<BTreeSet<String>> {
    if !cfg!(unix) {
        return None;
    }
    let directory = std::env::var_os("DOCKER_CONFIG")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".docker")))?;
    let directory = absolute(context, &directory)?;
    let mut directories = vec![directory.join("cli-plugins")];
    directories.extend(
        [
            "/usr/local/lib/docker/cli-plugins",
            "/usr/local/libexec/docker/cli-plugins",
            "/usr/lib/docker/cli-plugins",
            "/usr/libexec/docker/cli-plugins",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    directories.extend(extra_docker_directories(
        context,
        &directory.join("config.json"),
    )?);
    docker_directories(&directories)
}

fn extra_docker_directories(context: &ExecutionContext, path: &Path) -> Option<Vec<PathBuf>> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let Some(directories) = json.get("cliPluginsExtraDirs") else {
        return Some(Vec::new());
    };
    let directories = directories.as_array()?;
    if directories.len() > 128 {
        return None;
    }
    directories
        .iter()
        .map(|path| absolute(context, Path::new(path.as_str()?)))
        .collect()
}

fn absolute(context: &ExecutionContext, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_owned());
    }
    context
        .cwd
        .as_ref()
        .filter(|cwd| context.cwd_known && cwd.is_absolute())
        .map(|cwd| cwd.join(path))
}

fn docker_directories(directories: &[PathBuf]) -> Option<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let mut count = 0;
    for directory in directories {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        for entry in entries {
            count += 1;
            if count > 20_000 {
                return None;
            }
            let entry = entry.ok()?;
            let filename = entry.file_name();
            let Some(name) = filename.to_str()?.strip_prefix("docker-") else {
                continue;
            };
            // Plugin bodies and metadata entry points are never executed.
            if std::fs::metadata(entry.path()).ok()?.is_file() && !name.is_empty() {
                names.insert(name.into());
            }
        }
    }
    Some(names)
}

#[cfg(all(test, unix))]
#[path = "../tests/metadata/plugins.rs"]
mod tests;
