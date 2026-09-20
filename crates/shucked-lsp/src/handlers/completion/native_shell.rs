use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::super::native_zsh::{Candidate, parse_output};
use super::assets;
use crate::session::RequestCancellationToken;

pub(super) struct ManagedShell {
    name: &'static str,
    executable: PathBuf,
    root: PathBuf,
    cache: Mutex<VecDeque<Cached>>,
    running: Mutex<()>,
    generation: AtomicU64,
    #[cfg(test)]
    home: Option<PathBuf>,
}

struct Cached {
    words: Vec<String>,
    directory: PathBuf,
    execution_path: Option<std::ffi::OsString>,
    created: Instant,
    result: Arc<Vec<Candidate>>,
}

impl ManagedShell {
    pub(super) fn detect(name: &'static str) -> Option<Self> {
        Some(Self {
            name,
            executable: assets::shell(name)?,
            root: assets::root()?,
            cache: Mutex::default(),
            running: Mutex::default(),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            home: None,
        })
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub(super) fn complete(
        &self,
        words: &[String],
        prefix: &str,
        directory: &Path,
        cancellation: &RequestCancellationToken,
        execution_path: Option<&std::ffi::OsStr>,
    ) -> Option<Arc<Vec<Candidate>>> {
        if words.is_empty()
            || words.len() > 256
            || words.iter().map(String::len).sum::<usize>() + prefix.len() > 8192
        {
            return None;
        }
        let mut input = words.to_vec();
        input.push(prefix.to_owned());
        if let Some(entry) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|entry| {
                entry.words == input
                    && entry.execution_path.as_deref() == execution_path
                    && entry.directory == directory
                    && entry.created.elapsed() < Duration::from_secs(30)
            })
        {
            return Some(Arc::clone(&entry.result));
        }
        let _running = self.running.try_lock().ok()?;
        let generation = self.generation.load(Ordering::Acquire);
        let mut command = std::process::Command::new(&self.executable);
        if self.name == "fish" {
            assets::shell_args(
                &mut command,
                [
                    "--no-config",
                    "--private",
                    "-c",
                    include_str!("fish_worker.fish"),
                    "--",
                ],
            );
        } else {
            assets::shell_args(
                &mut command,
                [
                    "--noprofile",
                    "--norc",
                    "-c",
                    include_str!("bash_worker.bash"),
                    "shucked-complete",
                ],
            );
        }
        assets::shell_args(
            &mut command,
            std::iter::once(assets::primary_word(&input[0])).chain(input[1..].iter().cloned()),
        );
        command
            .current_dir(directory)
            .env("SHUCKED_PROVIDER_ROOT", assets::shell_path(&self.root))
            .env("LC_ALL", "C")
            .env("TERM", "dumb")
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .env("HOMEBREW_NO_ANALYTICS", "1")
            .env_remove("BASH_ENV")
            .env_remove("ENV");
        assets::configure_worker_path(&mut command, &self.root, execution_path);
        let _primary =
            assets::bind_primary(&mut command, words.first().map(String::as_str)).ok()?;
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("BASH_FUNC_") {
                command.env_remove(name);
            }
        }
        #[cfg(test)]
        if let Some(home) = &self.home {
            command
                .env("HOME", home)
                .env("XDG_CONFIG_HOME", home.join(".config"));
        }
        let output = super::super::native_process::capture(
            &mut command,
            Duration::from_millis(1500),
            cancellation,
            false,
        )?;
        let result = Arc::new(parse_output(&output)?);
        if cancellation.is_cancelled() {
            return None;
        }
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.generation.load(Ordering::Acquire) != generation {
            return Some(result);
        }
        cache.push_back(Cached {
            words: input,
            directory: directory.to_owned(),
            execution_path: execution_path.map(ToOwned::to_owned),
            created: Instant::now(),
            result: Arc::clone(&result),
        });
        while cache.len() > 16 {
            cache.pop_front();
        }
        Some(result)
    }
}

#[cfg(test)]
#[path = "../../../tests/completion/native_shell.rs"]
mod tests;
