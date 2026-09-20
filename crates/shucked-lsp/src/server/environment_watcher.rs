//! Host-side dependency watches; polling remains the fallback for unsupported filesystems.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam::channel::{Sender, bounded};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::session::Client;

pub(crate) struct EnvironmentWatcher {
    updates: Sender<()>,
    targets: Arc<Mutex<BTreeSet<PathBuf>>>,
}

impl EnvironmentWatcher {
    pub(crate) fn new(client: Client) -> Self {
        let (updates, receiver) = bounded::<()>(1);
        let targets = Arc::new(Mutex::new(BTreeSet::<PathBuf>::new()));
        let thread_targets = targets.clone();
        std::thread::Builder::new()
            .name("shucked-environment-watch".into())
            .spawn(move || {
                let (events, changes) = bounded(1);
                let effective_targets = Arc::new(Mutex::new(BTreeSet::<PathBuf>::new()));
                let callback_targets = effective_targets.clone();
                let Ok(mut watcher) =
                    notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                        if event.as_ref().map_or(true, |event| {
                            !matches!(event.kind, EventKind::Access(_))
                                && (event.paths.is_empty()
                                    || event.paths.iter().any(|path| {
                                        callback_targets
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .iter()
                                            .any(|target| {
                                                path.starts_with(target) || target.starts_with(path)
                                            })
                                    }))
                        }) {
                            let _ = events.try_send(());
                        }
                    })
                else {
                    tracing::warn!("host filesystem watches unavailable; using periodic refresh");
                    return;
                };
                let mut watched = BTreeSet::new();
                let mut observed = None;
                let mut pending = None;
                let mut revalidate = false;
                let mut last_registration = Instant::now() - Duration::from_secs(30);
                loop {
                    match receiver.recv_timeout(Duration::from_millis(100)) {
                        Ok(()) => {
                            last_registration = Instant::now() - Duration::from_secs(2);
                            revalidate = true;
                        }
                        Err(crossbeam::channel::RecvTimeoutError::Disconnected) => return,
                        Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
                    }
                    if changes.try_recv().is_ok() {
                        pending.get_or_insert_with(Instant::now);
                    }
                    if last_registration.elapsed() >= Duration::from_secs(2) {
                        let mut desired = thread_targets
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone();
                        desired.extend(configuration_directories());
                        let canonical = desired
                            .iter()
                            .filter_map(|path| path.canonicalize().ok())
                            .collect::<Vec<_>>();
                        desired.extend(canonical);
                        *effective_targets
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = desired.clone();
                        let targets = desired
                            .iter()
                            .filter_map(|path| existing_directory(path))
                            .collect::<BTreeSet<_>>();
                        if targets != watched {
                            // A batch avoids restarting the entire native event stream
                            // for every newly discovered provider directory.
                            let mut changes = watcher.paths_mut();
                            for path in watched.difference(&targets) {
                                let _ = changes.remove(path);
                            }
                            let mut next = watched
                                .intersection(&targets)
                                .cloned()
                                .collect::<BTreeSet<_>>();
                            for path in targets.difference(&watched) {
                                if changes.add(path, RecursiveMode::NonRecursive).is_ok() {
                                    next.insert(path.clone());
                                }
                            }
                            if changes.commit().is_ok() {
                                watched = next;
                            } else {
                                watched.clear();
                            }
                        }
                        let fingerprint = fingerprints(&desired);
                        let metadata_changed = observed
                            .as_ref()
                            .is_some_and(|previous| previous != &fingerprint);
                        observed = Some(fingerprint);
                        last_registration = Instant::now();
                        // Inputs can change between analysis and native watch registration.
                        let registration_changed = std::mem::take(&mut revalidate);
                        if (registration_changed || metadata_changed)
                            && client.environment_changed().is_err()
                        {
                            return;
                        }
                    }
                    if pending.is_some_and(|start| start.elapsed() >= Duration::from_millis(250)) {
                        pending = None;
                        if client.environment_changed().is_err() {
                            return;
                        }
                    }
                }
            })
            .expect("environment watcher thread");
        Self { updates, targets }
    }

    pub(crate) fn update(&self, paths: Vec<PathBuf>) {
        let paths = paths.into_iter().take(4096).collect();
        let mut current = self
            .targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == paths {
            return;
        }
        *current = paths;
        let _ = self.updates.try_send(());
    }
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    directory: bool,
    length: u64,
    modified: Option<std::time::SystemTime>,
    permissions: u32,
}

fn fingerprints(paths: &BTreeSet<PathBuf>) -> BTreeMap<PathBuf, Option<Fingerprint>> {
    paths
        .iter()
        .map(|path| {
            let fingerprint = path.metadata().ok().map(|metadata| {
                #[cfg(unix)]
                let permissions = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode()
                };
                #[cfg(not(unix))]
                let permissions = u32::from(metadata.permissions().readonly());
                Fingerprint {
                    directory: metadata.is_dir(),
                    length: metadata.len(),
                    modified: metadata.modified().ok(),
                    permissions,
                }
            });
            (path.clone(), fingerprint)
        })
        .collect()
}

fn existing_directory(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    while !candidate.is_dir() {
        candidate = candidate.parent()?;
    }
    // Never watch an entire filesystem just because a configured path is missing.
    candidate.parent()?;
    Some(candidate.to_path_buf())
}

fn configuration_directories() -> Vec<PathBuf> {
    let mut paths = [
        "/var/lib/pacman/local",
        "/var/lib/pacman/sync",
        "/var/lib/dpkg",
        "/var/lib/rpm",
        "/usr/local/Cellar",
        "/opt/homebrew/Cellar",
        "/opt/homebrew/Library/Taps",
        "/usr/local/Library/Taps",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.exists())
    .collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for suffix in [
            ".gitconfig",
            ".zshrc",
            ".zshenv",
            ".bashrc",
            ".bash_profile",
            ".config/git",
            ".config/fish/config.fish",
            ".config/fish/completions",
            ".docker/config.json",
            ".docker/cli-plugins",
            ".kube/config",
            ".kube/kuberc",
            ".local/share/fish/vendor_completions.d",
        ] {
            let path = home.join(suffix);
            paths.push(path);
        }
    }
    for key in [
        "XDG_CONFIG_HOME",
        "DOCKER_CONFIG",
        "ZDOTDIR",
        "HOMEBREW_PREFIX",
    ] {
        if let Some(path) = std::env::var_os(key) {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

#[cfg(test)]
#[path = "../../tests/server/environment_watcher.rs"]
mod tests;
