#[cfg(test)]
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
#[cfg(test)]
use std::time::{Duration, Instant};

use crate::session::RequestCancellationToken;

const MAX_DIRECTORY_ENTRIES: usize = 20_000;
#[cfg(test)]
const MAX_CACHED_ENTRIES: usize = 40_000;
#[cfg(test)]
const MAX_CACHED_DIRECTORIES: usize = 64;
#[cfg(test)]
const CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct Environment {
    pub(super) cwd: PathBuf,
    pub(super) native: Arc<super::native::Native>,
    pub(super) native_allowed: bool,
    pub(super) home: Option<PathBuf>,
    pub(super) variables: BTreeSet<String>,
    pub(super) path_variables: BTreeMap<String, PathBuf>,
    path: Vec<PathBuf>,
    executable_extensions: Vec<String>,
    #[cfg(test)]
    cache: Arc<Mutex<VecDeque<CachedDirectory>>>,
    pub(super) service: Arc<super::service::Service>,
    #[cfg(test)]
    pub(super) synchronous: bool,
}

#[cfg(test)]
struct CachedDirectory {
    path: PathBuf,
    scanned: Instant,
    listing: Arc<Directory>,
}

#[derive(Clone, Default)]
pub(super) struct Directory {
    pub entries: Vec<Entry>,
    pub incomplete: bool,
}

#[derive(Clone)]
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
            native: Arc::new(super::native::Native::default()),
            native_allowed: false,
            home: Some(root.to_owned()),
            variables: BTreeSet::from(["SHUCKED_TEST_VARIABLE".to_owned()]),
            path_variables: BTreeMap::from([("HOME".to_owned(), root.to_owned())]),
            path: vec![root.join("bin")],
            executable_extensions: vec![".exe".to_owned(), ".cmd".to_owned()],
            cache: Arc::default(),
            service: Arc::default(),
            synchronous: true,
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
        let path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        let executable_extensions = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|extension| extension.starts_with('.') && extension.len() > 1)
            .map(str::to_ascii_lowercase)
            .collect();
        Self {
            cwd,
            native: Arc::new(super::native::Native::detect()),
            native_allowed,
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            variables,
            path_variables,
            path,
            executable_extensions,
            #[cfg(test)]
            cache: Arc::default(),
            service: Arc::default(),
            #[cfg(test)]
            synchronous: false,
        }
    }

    pub(super) fn scoped(
        &self,
        context: &shucked_command::ExecutionContext,
        snapshot: &shucked_command::EnvironmentSnapshot,
    ) -> Self {
        Self {
            cwd: context.cwd.clone().unwrap_or_else(|| self.cwd.clone()),
            native: self.native.clone(),
            native_allowed: self.native_allowed && context.native_execution_allowed,
            home: self.home.clone(),
            variables: self.variables.clone(),
            path_variables: self.path_variables.clone(),
            path: snapshot
                .search_path
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
            executable_extensions: snapshot.executable_extensions.clone(),
            #[cfg(test)]
            cache: self.cache.clone(),
            service: self.service.clone(),
            #[cfg(test)]
            synchronous: self.synchronous,
        }
    }

    pub(super) fn execution_path(&self) -> Option<std::ffi::OsString> {
        std::env::join_paths(&self.path).ok()
    }

    pub(crate) fn invalidate(&self) {
        self.service.invalidate();
        self.native.invalidate();
        super::offline::invalidate();
        #[cfg(test)]
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(crate) fn prewarm_recent(self: &Arc<Self>, session: &crate::session::Session) {
        for notice in self.service.recent() {
            if let Some(snapshot) = session.take_snapshot(notice.uri)
                && snapshot.query().document().version() == notice.version
            {
                super::background::refresh(self.clone(), snapshot, notice.client, notice.position);
            }
        }
    }

    pub(crate) fn cancel_document(&self, uri: &lsp_types::Url) {
        self.service.cancel_document(uri, None);
    }

    pub(super) fn directory_cached(
        &self,
        path: &Path,
        notice: Option<super::service::Notice>,
    ) -> Arc<Directory> {
        #[cfg(test)]
        if self.synchronous {
            return self.directory(path, &RequestCancellationToken::default());
        }
        let environment = self.clone();
        let directory = path.to_owned();
        let (result, pending) = self.service.query(
            super::service::Key::Directory(directory.clone()),
            notice,
            move |cancel| {
                let listing = environment.read_directory(&directory, cancel);
                (!cancel.is_cancelled())
                    .then(|| super::service::Output::Directory(Arc::new(listing)))
            },
        );
        match result {
            Some(super::service::Output::Directory(listing)) if pending => Arc::new(Directory {
                entries: listing.entries.clone(),
                incomplete: true,
            }),
            Some(super::service::Output::Directory(listing)) => listing,
            _ => Arc::new(Directory {
                entries: Vec::new(),
                incomplete: pending,
            }),
        }
    }

    pub(crate) fn watch_directories(&self) -> Vec<PathBuf> {
        let mut directories = self.service.watch_directories();
        directories.extend(
            self.native
                .watch_directories(self.execution_path().as_deref()),
        );
        directories.sort();
        directories.dedup();
        directories
    }

    pub(super) fn executable_path(&self, command: &str, directory: &Path) -> Option<PathBuf> {
        let candidates = if command.contains('/') {
            vec![directory.join(command)]
        } else {
            self.path
                .iter()
                .map(|entry| {
                    if entry.is_absolute() {
                        entry.join(command)
                    } else {
                        directory.join(entry).join(command)
                    }
                })
                .collect()
        };
        candidates.into_iter().find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && self.executable(command, &metadata))
        })
    }

    #[cfg(test)]
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
            let path = if path.is_absolute() {
                path.clone()
            } else {
                self.cwd.join(path)
            };
            let directory = self.directory(&path, cancellation);
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

    #[cfg(test)]
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
            result.incomplete = true;
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
