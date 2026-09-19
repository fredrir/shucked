use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::session::RequestCancellationToken;

const MAX_DIRECTORY_ENTRIES: usize = 20_000;
const MAX_CACHED_ENTRIES: usize = 40_000;
const MAX_CACHED_DIRECTORIES: usize = 64;
const CACHE_TTL: Duration = Duration::from_secs(2);

pub(crate) struct Environment {
    pub(super) cwd: PathBuf,
    pub(super) native: super::native::Native,
    pub(super) native_allowed: bool,
    pub(super) home: Option<PathBuf>,
    pub(super) variables: BTreeSet<String>,
    pub(super) path_variables: BTreeMap<String, PathBuf>,
    path: Vec<PathBuf>,
    executable_extensions: Vec<String>,
    cache: Mutex<VecDeque<CachedDirectory>>,
}

struct CachedDirectory {
    path: PathBuf,
    scanned: Instant,
    listing: Arc<Directory>,
}

#[derive(Default)]
pub(super) struct Directory {
    pub entries: Vec<Entry>,
    pub incomplete: bool,
}

pub(super) struct Entry {
    pub name: String,
    pub directory: bool,
    pub executable: bool,
}

impl Environment {
    #[cfg(test)]
    pub(super) fn fixture(root: &Path) -> Self {
        Self {
            cwd: root.to_owned(),
            native: super::native::Native::default(),
            native_allowed: false,
            home: Some(root.to_owned()),
            variables: BTreeSet::from(["SHUCKED_TEST_VARIABLE".to_owned()]),
            path_variables: BTreeMap::from([("HOME".to_owned(), root.to_owned())]),
            path: vec![root.join("bin")],
            executable_extensions: vec![".exe".to_owned(), ".cmd".to_owned()],
            cache: Mutex::default(),
        }
    }

    pub(crate) fn detect(native_allowed: bool) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let variables = std::env::vars_os()
            .filter_map(|(name, _)| name.into_string().ok())
            .filter(|name| super::context::identifier(name))
            .collect();
        let path_variables = std::env::vars_os()
            .filter_map(|(name, value)| {
                let name = name.into_string().ok()?;
                let path = PathBuf::from(value);
                path.is_absolute().then_some((name, path))
            })
            .collect();
        let mut seen = BTreeSet::new();
        #[allow(unused_mut)] // Standard Unix directories are appended below.
        let mut path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|path| {
                        if path.is_absolute() {
                            path
                        } else {
                            cwd.join(path)
                        }
                    })
                    .filter(|path| seen.insert(path.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // Desktop launches often omit package-manager bins from PATH.
        #[cfg(unix)]
        for directory in [
            "/opt/homebrew/bin",
            "/home/linuxbrew/.linuxbrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
        ] {
            let directory = PathBuf::from(directory);
            if directory.is_dir() && seen.insert(directory.clone()) {
                path.push(directory);
            }
        }
        let executable_extensions = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|extension| extension.starts_with('.') && extension.len() > 1)
            .map(str::to_ascii_lowercase)
            .collect();
        Self {
            cwd,
            native: super::native::Native::detect(),
            native_allowed,
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            variables,
            path_variables,
            path,
            executable_extensions,
            cache: Mutex::default(),
        }
    }

    pub(crate) fn invalidate(&self) {
        self.native.invalidate();
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(super) fn executable_path(&self, command: &str, directory: &Path) -> Option<PathBuf> {
        let candidates = if command.contains('/') {
            vec![directory.join(command)]
        } else {
            self.path
                .iter()
                .map(|directory| directory.join(command))
                .collect()
        };
        candidates.into_iter().find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && self.executable(command, &metadata))
        })
    }

    pub(super) fn commands(
        &self,
        prefix: &str,
        cancellation: &RequestCancellationToken,
    ) -> (BTreeMap<String, PathBuf>, bool) {
        let mut commands = BTreeMap::new();
        let mut incomplete = self.path.len() > 128;
        let mut scanned = 0;
        for path in self.path.iter().take(128) {
            if cancellation.is_cancelled() {
                return (commands, true);
            }
            let directory = self.directory(path, cancellation);
            incomplete |= directory.incomplete;
            scanned += directory.entries.len();
            for entry in &directory.entries {
                if entry.executable && super::matches(&entry.name, prefix) {
                    commands
                        .entry(entry.name.clone())
                        .or_insert_with(|| path.join(&entry.name));
                }
            }
            if scanned >= MAX_CACHED_ENTRIES {
                incomplete = true;
                break;
            }
        }
        (commands, incomplete)
    }

    pub(super) fn directory(
        &self,
        path: &Path,
        cancellation: &RequestCancellationToken,
    ) -> Arc<Directory> {
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = cache
                .iter()
                .find(|entry| entry.path == path && entry.scanned.elapsed() < CACHE_TTL)
            {
                return entry.listing.clone();
            }
        }
        let listing = Arc::new(self.read_directory(path, cancellation));
        if cancellation.is_cancelled() {
            return listing;
        }
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|entry| entry.path != path && entry.scanned.elapsed() < CACHE_TTL);
        cache.push_back(CachedDirectory {
            path: path.to_owned(),
            scanned: Instant::now(),
            listing: listing.clone(),
        });
        while cache.len() > MAX_CACHED_DIRECTORIES
            || cache
                .iter()
                .map(|entry| entry.listing.entries.len())
                .sum::<usize>()
                > MAX_CACHED_ENTRIES
        {
            cache.pop_front();
        }
        listing
    }

    fn read_directory(&self, path: &Path, cancellation: &RequestCancellationToken) -> Directory {
        let mut result = Directory::default();
        let Ok(entries) = std::fs::read_dir(path) else {
            return result;
        };
        for (index, entry) in entries.enumerate() {
            if index >= MAX_DIRECTORY_ENTRIES || cancellation.is_cancelled() {
                result.incomplete = true;
                break;
            }
            let Ok(entry) = entry else { continue };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(metadata) = std::fs::metadata(entry.path()) else {
                continue;
            };
            let executable = metadata.is_file() && self.executable(&name, &metadata);
            result.entries.push(Entry {
                name,
                directory: metadata.is_dir(),
                executable,
            });
        }
        result.entries.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    fn executable(&self, name: &str, metadata: &std::fs::Metadata) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = (name, &self.executable_extensions);
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            let name = name.to_ascii_lowercase();
            self.executable_extensions
                .iter()
                .any(|extension| name.ends_with(extension))
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/completion/environment.rs"]
mod tests;
