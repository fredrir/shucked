use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::session::RequestCancellationToken;

const TTL: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct Candidate {
    pub text: String,
    pub description: String,
}

pub(super) struct NativeZsh {
    shell: PathBuf,
    cache: Mutex<VecDeque<Cached>>,
    running: Mutex<()>,
    generation: AtomicU64,
    #[cfg(test)]
    zdotdir: Option<PathBuf>,
}

struct Cached {
    directory: PathBuf,
    buffer: String,
    personal: bool,
    created: Instant,
    result: Option<Arc<Vec<Candidate>>>,
}

impl NativeZsh {
    pub(super) fn detect() -> Option<Self> {
        if !cfg!(unix) {
            return None;
        }
        let shell = std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(|path| {
                path.file_name().is_some_and(|name| name == "zsh")
                    && path.is_absolute()
                    && path.is_file()
            })
            .or_else(|| {
                std::env::var_os("PATH").and_then(|path| {
                    std::env::split_paths(&path)
                        .filter(|path| path.is_absolute())
                        .map(|path| path.join("zsh"))
                        .find(|path| path.is_file())
                })
            })
            .or_else(|| {
                Path::new("/bin/zsh")
                    .is_file()
                    .then(|| PathBuf::from("/bin/zsh"))
            })?;
        Some(Self {
            shell,
            cache: Mutex::default(),
            running: Mutex::default(),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            zdotdir: None,
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
        timeout_ms: usize,
        personal: bool,
        cancellation: &RequestCancellationToken,
    ) -> Option<Arc<Vec<Candidate>>> {
        // A single option request supplies all flags, so further typing uses the cache.
        let query_prefix = if prefix.starts_with('-') { "-" } else { prefix };
        let buffer = words
            .iter()
            .map(|word| quote_word(word))
            .chain(std::iter::once(quote_word(query_prefix)))
            .collect::<Vec<_>>()
            .join(" ");
        if buffer.len() > 8192 || buffer.contains('\0') || cancellation.is_cancelled() {
            return None;
        }
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = cache.iter().find(|entry| {
                entry.directory == directory
                    && entry.buffer == buffer
                    && entry.personal == personal
                    && entry.created.elapsed() < TTL
            }) {
                return entry.result.clone();
            }
        }
        let _running = self.running.try_lock().ok()?;
        let generation = self.generation.load(Ordering::Acquire);
        let result = self
            .run(
                &buffer,
                directory,
                Duration::from_millis(timeout_ms.clamp(100, 5000) as u64),
                cancellation,
                personal,
            )
            .map(Arc::new);
        if cancellation.is_cancelled() {
            return None;
        }
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.generation.load(Ordering::Acquire) == generation {
            cache.retain(|entry| entry.created.elapsed() < TTL);
            cache.push_back(Cached {
                directory: directory.to_owned(),
                buffer,
                personal,
                created: Instant::now(),
                result: result.clone(),
            });
            while cache.len() > 16 {
                cache.pop_front();
            }
        }
        result
    }

    fn run(
        &self,
        buffer: &str,
        directory: &Path,
        timeout: Duration,
        cancellation: &RequestCancellationToken,
        personal: bool,
    ) -> Option<Vec<Candidate>> {
        let mut command = std::process::Command::new(&self.shell);
        command
            .args(["-f", "-c", include_str!("zsh_supervisor.zsh")])
            .env("SHUCKED_NATIVE_SHELL", &self.shell)
            .env("SHUCKED_NATIVE_SCRIPT", include_str!("zsh_worker.zsh"))
            .env("SHUCKED_NATIVE_BUFFER", buffer)
            .env("SHUCKED_NATIVE_PERSONAL", if personal { "1" } else { "0" })
            .env("TERM", "dumb")
            .current_dir(directory);
        if !personal {
            command.env_remove("FPATH");
        }
        #[cfg(test)]
        if let Some(zdotdir) = &self.zdotdir {
            command.env("ZDOTDIR", zdotdir);
        }
        let output = super::native_process::capture(&mut command, timeout, cancellation, true)?;
        parse_output(&output)
    }
}

fn quote_word(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./-:@=+".contains(&byte))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

fn parse_output(output: &[u8]) -> Option<Vec<Candidate>> {
    if output.len() > MAX_OUTPUT {
        return None;
    }
    let mut fields = output.split(|byte| *byte == 0);
    if fields.next()? != b"P" {
        return None;
    }
    fields.next()?;
    let mut items: Vec<Candidate> = Vec::new();
    loop {
        match fields.next()? {
            b"E" => return Some(items),
            b"M" => {
                let text = std::str::from_utf8(fields.next()?).ok()?;
                let display = std::str::from_utf8(fields.next()?).ok()?;
                if text.is_empty()
                    || text.len() > 8192
                    || text.chars().any(char::is_control)
                    || items.len() >= 2000
                {
                    continue;
                }
                let description = display
                    .strip_prefix(text)
                    .unwrap_or(display)
                    .trim()
                    .trim_start_matches("--")
                    .trim();
                let description = description
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(4096)
                    .collect::<String>();
                if let Some(existing) = items.iter_mut().find(|item| item.text == text) {
                    if existing.description.is_empty() {
                        existing.description = description;
                    }
                } else {
                    items.push(Candidate {
                        text: text.to_owned(),
                        description,
                    });
                }
            }
            _ => return None,
        }
    }
}

#[cfg(all(test, unix))]
#[path = "../../../tests/completion/native_zsh.rs"]
mod tests;
