use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::session::RequestCancellationToken;

const TTL: Duration = Duration::from_secs(2);
const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct Candidate {
    pub text: String,
    pub description: String,
    pub kind: Option<lsp_types::CompletionItemKind>,
    pub no_space: bool,
    pub provider: String,
}

pub(super) struct NativeZsh {
    shell: PathBuf,
    cache: Mutex<VecDeque<Cached>>,
    worker: super::native_process::Persistent,
    generation: AtomicU64,
    #[cfg(test)]
    zdotdir: Option<PathBuf>,
}

struct Cached {
    directory: PathBuf,
    buffer: String,
    cursor: usize,
    personal: bool,
    execution_path: Option<std::ffi::OsString>,
    created: Instant,
    result: Option<Arc<Vec<Candidate>>>,
}

impl NativeZsh {
    pub(super) fn detect() -> Option<Self> {
        let shell = super::native::assets::shell("zsh")?;
        Some(Self {
            shell,
            cache: Mutex::default(),
            worker: super::native_process::Persistent::default(),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            zdotdir: None,
        })
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.worker.invalidate();
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(super) fn complete(
        &self,
        words: &[String],
        prefix: &str,
        directory: &Path,
        timeout_ms: usize,
        personal: bool,
        cancellation: &RequestCancellationToken,
        execution_path: Option<&std::ffi::OsStr>,
    ) -> Option<Arc<Vec<Candidate>>> {
        self.complete_at(
            words,
            prefix,
            "",
            directory,
            timeout_ms,
            personal,
            cancellation,
            execution_path,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn complete_at(
        &self,
        words: &[String],
        prefix: &str,
        suffix: &str,
        directory: &Path,
        timeout_ms: usize,
        personal: bool,
        cancellation: &RequestCancellationToken,
        execution_path: Option<&std::ffi::OsStr>,
    ) -> Option<Arc<Vec<Candidate>>> {
        let current = format!("{prefix}{suffix}");
        let mut parts = words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                if index == 0 {
                    quote_word(&super::native::assets::primary_word(word))
                } else {
                    quote_word(word)
                }
            })
            .collect::<Vec<_>>();
        parts.push(quote_word(&current));
        let buffer = parts.join(" ");
        let tail = if quote_word(&current).starts_with('\'') {
            suffix.replace('\'', "'\\''").chars().count() + 1
        } else {
            suffix.chars().count()
        };
        let cursor = if suffix.is_empty() {
            buffer.chars().count()
        } else {
            buffer.chars().count().saturating_sub(tail)
        };
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
                    && entry.cursor == cursor
                    && entry.personal == personal
                    && entry.execution_path.as_deref() == execution_path
                    && entry.created.elapsed() < TTL
            }) {
                return entry.result.clone();
            }
        }
        let generation = self.generation.load(Ordering::Acquire);
        let result = self
            .run(
                &buffer,
                cursor,
                directory,
                Duration::from_millis(timeout_ms.clamp(100, 5000) as u64),
                cancellation,
                personal,
                execution_path,
                words.first().map(String::as_str),
            )
            .map(Arc::new);
        if cancellation.is_cancelled() || result.is_none() {
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
                cursor,
                personal,
                execution_path: execution_path.map(ToOwned::to_owned),
                created: Instant::now(),
                result: result.clone(),
            });
            while cache.len() > 16 {
                cache.pop_front();
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        buffer: &str,
        cursor: usize,
        directory: &Path,
        timeout: Duration,
        cancellation: &RequestCancellationToken,
        personal: bool,
        execution_path: Option<&std::ffi::OsStr>,
        primary: Option<&str>,
    ) -> Option<Vec<Candidate>> {
        let mut command = std::process::Command::new(&self.shell);
        super::native::assets::shell_args(
            &mut command,
            ["-f", "-c", include_str!("zsh_supervisor.zsh")],
        );
        command
            .env(
                "SHUCKED_NATIVE_SHELL",
                super::native::assets::shell_path(&self.shell),
            )
            .env("SHUCKED_NATIVE_SCRIPT", include_str!("zsh_worker.zsh"))
            .env("SHUCKED_NATIVE_PERSONAL", if personal { "1" } else { "0" })
            .env("TERM", "dumb")
            .current_dir(directory);
        if let Some(path) = execution_path {
            command.env("PATH", path);
        }
        if !personal {
            command.env_remove("FPATH");
        }
        if let Some(root) = super::native::assets::root() {
            super::native::assets::configure_worker_path(&mut command, &root, execution_path);
            command.env(
                "SHUCKED_PROVIDER_ROOT",
                super::native::assets::shell_path(&root),
            );
        }
        let _primary = super::native::assets::bind_primary(&mut command, primary).ok()?;
        #[cfg(test)]
        if let Some(zdotdir) = &self.zdotdir {
            command.env("ZDOTDIR", zdotdir);
        }
        let installed = super::native::assets::completion_directories(execution_path, "zsh");
        command.env(
            "SHUCKED_COMPLETION_PATHS",
            super::native::assets::joined_completion_paths(&installed),
        );
        let path = command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
            .or_else(|| execution_path.map(|value| value.to_string_lossy().into_owned()))
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        let key = format!(
            "{:?}:{:?}:{personal}:{}:{:?}:{:?}",
            self.shell,
            execution_path,
            self.generation.load(Ordering::Acquire),
            super::native::assets::root(),
            installed
        );
        let output = self.worker.request(
            &mut command,
            key,
            &[
                super::native::assets::shell_path(directory)
                    .to_string_lossy()
                    .into_owned(),
                path,
                buffer.to_owned(),
                cursor.to_string(),
            ],
            timeout,
            Duration::from_secs(5),
            cancellation,
        )?;
        let mut result = parse_output(&output)?;
        for candidate in &mut result {
            candidate.provider = "zsh".into();
        }
        Some(result)
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

pub(super) fn parse_output(output: &[u8]) -> Option<Vec<Candidate>> {
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
            kind @ (b"M" | b"B" | b"C" | b"Q") => {
                let raw_text = std::str::from_utf8(fields.next()?).ok()?;
                let display = std::str::from_utf8(fields.next()?).ok()?;
                let (candidate_kind, mut no_space) = if matches!(kind, b"C" | b"Q") {
                    let candidate_kind = match fields.next()? {
                        b"file" => Some(lsp_types::CompletionItemKind::FILE),
                        b"directory" => Some(lsp_types::CompletionItemKind::FOLDER),
                        _ => None,
                    };
                    (candidate_kind, fields.next()? == b"1")
                } else {
                    (None, false)
                };
                if raw_text.len() > 8192 {
                    continue;
                }
                let text = if matches!(kind, b"B" | b"Q") {
                    let Some((text, delimiter)) = decode_insertion_candidate(raw_text) else {
                        continue;
                    };
                    if delimiter {
                        no_space = false;
                    }
                    std::borrow::Cow::Owned(text)
                } else {
                    std::borrow::Cow::Borrowed(raw_text)
                };
                if text.is_empty()
                    || text.len() > 8192
                    || text.chars().any(char::is_control)
                    || items.len() >= 2000
                {
                    continue;
                }
                let description = display
                    .strip_prefix(text.as_ref())
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
                    existing.no_space |= no_space;
                    existing.kind = existing.kind.or(candidate_kind);
                    if existing.description.is_empty() {
                        existing.description = description;
                    }
                } else {
                    items.push(Candidate {
                        text: text.into_owned(),
                        description,
                        kind: candidate_kind,
                        no_space,
                        provider: String::new(),
                    });
                }
            }
            _ => return None,
        }
    }
}

/// Decode a callback's single shell-word insertion without evaluating it.
/// Filename-mode callbacks bypass this: Readline would quote those raw names.
pub(crate) fn decode_bash_candidate(candidate: &str) -> Option<String> {
    decode_insertion_candidate(candidate).map(|(text, _)| text)
}

fn decode_insertion_candidate(candidate: &str) -> Option<(String, bool)> {
    const PREFIX: &str = "__shucked_completion__ ";
    let source = format!("{PREFIX}{candidate}");
    let parsed = shucked_parser::parser::Parser::new(&source).parse();
    if !parsed.diagnostics.is_empty() || parsed.file.body.len() != 1 {
        return None;
    }
    let statement = parsed.file.body.first()?;
    if statement.negated
        || !statement.redirects.is_empty()
        || statement.terminator.is_some()
        || statement.inline_comment.is_some()
    {
        return None;
    }
    let shucked_ast::Command::Simple(command) = &statement.command else {
        return None;
    };
    let [word] = command.args.as_slice() else {
        return None;
    };
    if word.span.start.offset() != PREFIX.len()
        || !source[word.span.end.offset()..]
            .chars()
            .all(|character| matches!(character, ' ' | '\t'))
    {
        return None;
    }
    shucked_ast::static_command_name_text(word, &source)
        .map(|text| (text.into_owned(), word.span.end.offset() < source.len()))
}

#[cfg(all(test, unix))]
#[path = "../../../tests/completion/native_zsh.rs"]
mod tests;
