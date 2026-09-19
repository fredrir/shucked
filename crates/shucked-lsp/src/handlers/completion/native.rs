use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::environment::Environment;
use super::native_process::capture;
use super::native_zsh::{Candidate, NativeZsh};
use crate::session::RequestCancellationToken;

const TTL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(super) struct Native {
    cache: Mutex<VecDeque<Cached>>,
    running: Mutex<()>,
    zsh: Option<NativeZsh>,
    generation: AtomicU64,
}

struct Cached {
    executable: PathBuf,
    arguments: Vec<&'static str>,
    directory: PathBuf,
    created: Instant,
    result: Option<Arc<Vec<Candidate>>>,
}

impl Native {
    pub(super) fn detect() -> Self {
        Self {
            zsh: NativeZsh::detect(),
            ..Self::default()
        }
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        if let Some(zsh) = &self.zsh {
            zsh.invalidate();
        }
    }

    pub(super) fn complete(
        &self,
        environment: &Environment,
        words: &[String],
        prefix: &str,
        directory: &Path,
        cancellation: &RequestCancellationToken,
        personal: bool,
    ) -> Option<Arc<Vec<Candidate>>> {
        let command = words.first()?;
        let name = command.rsplit('/').next()?;
        if personal
            && prefix.starts_with('-')
            && !words.iter().any(|word| word == "--")
            && let Some(zsh) = &self.zsh
            && let Some(entries) = zsh.complete(words, prefix, directory, 1500, true, cancellation)
            && !entries.is_empty()
        {
            return Some(entries);
        }
        let executable = environment.executable_path(command, directory)?;
        if let Some(queries) = package_queries(name, words, prefix) {
            let mut result = Vec::new();
            for arguments in queries {
                let entries =
                    self.query(&executable, &arguments, directory, false, cancellation)?;
                result.extend(entries.iter().cloned());
            }
            result.sort_by(|a, b| a.text.cmp(&b.text));
            result.dedup_by(|a, b| a.text == b.text);
            return Some(Arc::new(result));
        }
        if !prefix.starts_with('-') || words.iter().any(|word| word == "--") {
            return None;
        }
        // Only known read-only help interfaces may be invoked. Never forward the
        // edited command's arguments to an executable.
        if matches!(
            name,
            "ls" | "gls" | "eza" | "exa" | "rg" | "fd" | "fdfind" | "bat" | "batcat"
        ) && let Some(entries) =
            self.query(&executable, &["--help"], directory, true, cancellation)
            && !entries.is_empty()
        {
            return Some(entries);
        }
        self.zsh
            .as_ref()?
            .complete(words, prefix, directory, 1200, false, cancellation)
            .filter(|entries| !entries.is_empty())
    }

    fn query(
        &self,
        executable: &Path,
        arguments: &[&'static str],
        directory: &Path,
        help: bool,
        cancellation: &RequestCancellationToken,
    ) -> Option<Arc<Vec<Candidate>>> {
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = cache.iter().find(|entry| {
                entry.executable == executable
                    && entry.arguments == arguments
                    && entry.directory == directory
                    && entry.created.elapsed() < TTL
            }) {
                return entry.result.clone();
            }
        }
        let _running = self.running.try_lock().ok()?;
        let generation = self.generation.load(Ordering::Acquire);
        let mut command = std::process::Command::new(executable);
        command
            .args(arguments)
            .current_dir(directory)
            .env("LC_ALL", "C")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .env("HOMEBREW_NO_ANALYTICS", "1");
        let result = capture(
            &mut command,
            Duration::from_millis(1500),
            cancellation,
            false,
        )
        .and_then(|output| String::from_utf8(output).ok())
        .map(|output| {
            Arc::new(if help {
                help_candidates(&output)
            } else {
                package_candidates(&output)
            })
        });
        if cancellation.is_cancelled() {
            return None;
        }
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation != self.generation.load(Ordering::Acquire) {
            return result;
        }
        cache.retain(|entry| entry.created.elapsed() < TTL);
        cache.push_back(Cached {
            executable: executable.to_owned(),
            arguments: arguments.to_vec(),
            directory: directory.to_owned(),
            created: Instant::now(),
            result: result.clone(),
        });
        while cache.len() > 16
            || cache
                .iter()
                .filter_map(|entry| entry.result.as_ref())
                .map(|entries| entries.len())
                .sum::<usize>()
                > 100_000
        {
            cache.pop_front();
        }
        result
    }
}

// Query package databases only. No document text is used as an executable argument.
fn package_queries(name: &str, words: &[String], prefix: &str) -> Option<Vec<Vec<&'static str>>> {
    if prefix.starts_with(['-', '/', '.', '~']) {
        return None;
    }
    match name {
        "brew" => {
            let subcommand = words.get(1)?.as_str();
            if words
                .iter()
                .any(|word| matches!(word.as_str(), "--prefix" | "--cache" | "--repository"))
            {
                return None;
            }
            let installed = match subcommand {
                "install" | "info" | "home" => false,
                "uninstall" | "remove" | "rm" | "reinstall" | "upgrade" | "pin" | "unpin"
                | "link" | "unlink" => true,
                _ => return None,
            };
            let formula = !words.iter().any(|word| word == "--cask");
            let cask = !words.iter().any(|word| word == "--formula")
                && !matches!(subcommand, "pin" | "unpin" | "link" | "unlink");
            let mut queries = Vec::new();
            if formula {
                queries.push(if installed {
                    vec!["list", "--formula", "-1"]
                } else {
                    vec!["formulae"]
                });
            }
            if cask {
                queries.push(if installed {
                    vec!["list", "--cask", "-1"]
                } else {
                    vec!["casks"]
                });
            }
            Some(queries)
        }
        "pacman" => {
            if words.iter().any(|word| {
                matches!(
                    word.as_str(),
                    "--root" | "--dbpath" | "--config" | "--sysroot" | "-r" | "-b"
                ) || word.starts_with("--root=")
                    || word.starts_with("--dbpath=")
                    || word.starts_with("--config=")
                    || word.starts_with("--sysroot=")
            }) {
                return None;
            }
            let flags: Vec<_> = words
                .iter()
                .skip(1)
                .take_while(|word| word.as_str() != "--")
                .filter(|word| word.starts_with('-'))
                .collect();
            let has = |short: char, long: &str| {
                flags.iter().any(|word| {
                    word.as_str() == long || (!word.starts_with("--") && word.contains(short))
                })
            };
            if has('o', "--owns")
                || has('p', "--file")
                || has('g', "--groups")
                || (has('S', "--sync") && has('l', "--list"))
            {
                return None;
            }
            for word in flags {
                if word == "--sync"
                    || (word.starts_with('-') && !word.starts_with("--") && word.contains('S'))
                {
                    return Some(vec![vec!["-Slq"]]);
                }
                if matches!(word.as_str(), "--remove" | "--query")
                    || (word.starts_with('-')
                        && !word.starts_with("--")
                        && (word.contains('R') || word.contains('Q')))
                {
                    return Some(vec![vec!["-Qq"]]);
                }
            }
            None
        }
        _ => None,
    }
}

fn package_candidates(output: &str) -> Vec<Candidate> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && line.len() < 512
                && line
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"@+._/-".contains(&byte))
        })
        .take(50_000)
        .map(|text| Candidate {
            text: text.to_owned(),
            description: "Package on workspace host".to_owned(),
        })
        .collect()
}

fn help_candidates(output: &str) -> Vec<Candidate> {
    let mut result: Vec<Candidate> = Vec::new();
    let mut pending: Vec<usize> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut flag_indent = 0;
    'lines: for line in output.lines() {
        let indent = line.len() - line.trim_start().len();
        let line = line.trim_start();
        if !line.starts_with('-') {
            if !line.is_empty() && !pending.is_empty() {
                if indent > flag_indent {
                    let description: String = line
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(4096)
                        .collect();
                    for index in pending.drain(..) {
                        result[index].description.clone_from(&description);
                    }
                } else {
                    pending.clear();
                }
            }
            continue;
        }
        pending.clear();
        flag_indent = indent;
        let boundary = line
            .find("  ")
            .or_else(|| line.find('\t'))
            .unwrap_or(line.len());
        let (flags, description) = line.split_at(boundary);
        let description = description.trim();
        for token in flags.split([',', ' ', '|']) {
            let flag = token.split(['=', '[', '<']).next().unwrap_or("");
            if flag.len() < 2
                || !flag.starts_with('-')
                || !flag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-?.".contains(&byte))
                || !seen.insert(flag.to_owned())
            {
                continue;
            }
            if result.len() >= 2000 {
                break 'lines;
            }
            if description.is_empty() {
                pending.push(result.len());
            }
            result.push(Candidate {
                text: flag.to_owned(),
                description: description
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(4096)
                    .collect(),
            });
        }
    }
    result
}

#[cfg(all(test, unix))]
#[path = "../../../tests/completion/native.rs"]
mod tests;
