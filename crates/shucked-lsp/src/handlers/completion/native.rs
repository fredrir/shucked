#[path = "native_assets.rs"]
pub(super) mod assets;
#[path = "native_shell.rs"]
mod shell;

use super::environment::Environment;
use super::native_zsh::{Candidate, NativeZsh};
use crate::session::RequestCancellationToken;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Native {
    zsh: Option<NativeZsh>,
    bash: Option<shell::ManagedShell>,
    fish: Option<shell::ManagedShell>,
    registry: Mutex<std::collections::BTreeMap<String, Vec<Registration>>>,
    installed: Mutex<std::collections::VecDeque<Installed>>,
}

#[derive(Clone, serde::Deserialize)]
struct Registration {
    engine: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    path: std::path::PathBuf,
}

impl Native {
    pub(super) fn detect() -> Self {
        Self {
            zsh: NativeZsh::detect(),
            bash: shell::ManagedShell::detect("bash"),
            fish: shell::ManagedShell::detect("fish"),
            registry: Mutex::new(load_registry()),
            installed: Mutex::default(),
        }
    }

    pub(super) fn invalidate(&self) {
        for index in self
            .installed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter_mut()
        {
            index.created = Instant::now() - Duration::from_secs(3);
        }
        *self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = load_registry();
        for shell in [&self.bash, &self.fish].into_iter().flatten() {
            shell.invalidate();
        }
        if let Some(zsh) = &self.zsh {
            zsh.invalidate();
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(super) fn complete(
        &self,
        environment: &Environment,
        words: &[String],
        prefix: &str,
        directory: &Path,
        cancellation: &RequestCancellationToken,
        personal: bool,
        dialect: &str,
    ) -> Option<Arc<Vec<Candidate>>> {
        self.complete_at(
            environment,
            words,
            prefix,
            directory,
            cancellation,
            personal,
            dialect,
            "",
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn complete_at(
        &self,
        environment: &Environment,
        words: &[String],
        prefix: &str,
        directory: &Path,
        cancellation: &RequestCancellationToken,
        personal: bool,
        dialect: &str,
        suffix: &str,
    ) -> Option<Arc<Vec<Candidate>>> {
        tracing::debug!(
            word_count = words.len(),
            prefix_bytes = prefix.len(),
            suffix_bytes = suffix.len(),
            "native completion dispatch"
        );
        let execution_path = environment.execution_path();
        let execution_path = execution_path.as_deref();
        let command = words.first()?;
        let primary = assets::primary_word(command);
        let name = Path::new(&primary).file_name()?.to_str()?;
        let mut bound_words = words.to_vec();
        if let Some(executable) = environment.executable_path(command, directory) {
            bound_words[0] = executable.to_string_lossy().into_owned();
        }
        if personal
            && dialect == "zsh"
            && let Some(zsh) = &self.zsh
        {
            return zsh.complete_at(
                &bound_words,
                prefix,
                suffix,
                directory,
                1500,
                true,
                cancellation,
                execution_path,
            );
        }
        let mut providers = Vec::new();
        // Installed definitions track the selected host's tool versions. Registry
        // registrations provide the always-available bundled fallback.
        providers.extend(self.installed_providers(name, execution_path));
        let registry_count = {
            let registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(registrations) = registry.get(name) {
                providers.extend(registrations.iter().cloned());
            }
            registry.len()
        };
        tracing::debug!(
            provider_count = providers.len(),
            registry_commands = registry_count,
            "registered completion providers"
        );
        let mut engines = std::collections::BTreeSet::new();
        for registration in providers {
            if cancellation.is_cancelled() {
                return None;
            }
            if !engines.insert(registration.engine.clone()) {
                continue;
            }
            tracing::debug!(
                engine = registration.engine.as_str(),
                source = registration.source.as_str(),
                "selected completion provider"
            );
            let run = |suffix: &str| match registration.engine.as_str() {
                "zsh" => self.zsh.as_ref().and_then(|shell| {
                    shell.complete_at(
                        &bound_words,
                        prefix,
                        suffix,
                        directory,
                        1500,
                        false,
                        cancellation,
                        execution_path,
                    )
                }),
                "bash" => self.bash.as_ref().and_then(|shell| {
                    shell.complete_at(
                        &bound_words,
                        prefix,
                        suffix,
                        directory,
                        cancellation,
                        execution_path,
                    )
                }),
                "fish" => self.fish.as_ref().and_then(|shell| {
                    shell.complete_at(
                        &bound_words,
                        prefix,
                        suffix,
                        directory,
                        cancellation,
                        execution_path,
                    )
                }),
                _ => None,
            };
            let mut entries = run(suffix);
            if !suffix.is_empty() && entries.as_ref().is_some_and(|items| items.is_empty()) {
                // The editor replaces the entire token. An unmatched suffix must
                // not hide otherwise valid prefix candidates.
                entries = run("");
            }
            let Some(entries) = entries else {
                tracing::debug!(
                    engine = registration.engine.as_str(),
                    "completion provider unavailable or failed"
                );
                continue;
            };
            let mut filtered = if let Some(root) = assets::root() {
                filter_private_candidates(
                    &root,
                    environment,
                    directory,
                    dialect,
                    wrapper_command_position(words),
                    &entries,
                )
            } else {
                entries.as_ref().clone()
            };
            for candidate in &mut filtered {
                candidate.provider = if registration.source == registration.engine
                    || registration.source.is_empty()
                {
                    registration.engine.clone()
                } else {
                    format!("{} · {}", registration.source, registration.engine)
                };
                candidate.no_space |= candidate.text.ends_with(['/', '=']);
            }
            // Empty is a successful contextual answer, not permission to mix a
            // second engine's grammar into the same command position.
            tracing::debug!(
                engine = registration.engine.as_str(),
                candidates = filtered.len(),
                "native completion result"
            );
            return Some(Arc::new(filtered));
        }
        None
    }

    pub(super) fn watch_directories(
        &self,
        execution_path: Option<&std::ffi::OsStr>,
    ) -> Vec<std::path::PathBuf> {
        let mut paths = std::collections::BTreeSet::new();
        if let Some(root) = assets::root() {
            let packs = root.join("packs");
            paths.insert(packs.clone());
            paths.insert(packs.join("registry.json"));
            let registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for registration in registry.values().flatten() {
                if let Some(parent) = registration.path.parent() {
                    paths.insert(packs.join(parent));
                }
            }
        }
        let mut environments = self
            .installed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|index| index.path.clone())
            .collect::<Vec<_>>();
        environments.push(execution_path.map(ToOwned::to_owned));
        for path in environments {
            for engine in ["zsh", "bash", "fish"] {
                paths.extend(assets::completion_directories(path.as_deref(), engine));
            }
        }
        paths.into_iter().collect()
    }

    fn installed_providers(
        &self,
        name: &str,
        execution_path: Option<&std::ffi::OsStr>,
    ) -> Vec<Registration> {
        let mut cache = self
            .installed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(index) = cache.iter().find(|index| {
            index.path.as_deref() == execution_path
                && index.created.elapsed() < Duration::from_secs(2)
        }) {
            return index.commands.get(name).cloned().unwrap_or_default();
        }
        let mut commands: std::collections::BTreeMap<String, Vec<Registration>> =
            std::collections::BTreeMap::new();
        let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
        for engine in ["zsh", "bash", "fish"] {
            for directory in assets::completion_directories(execution_path, engine) {
                directory.hash(&mut fingerprint);
                let Ok(entries) = std::fs::read_dir(&directory) else {
                    continue;
                };
                let mut paths = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                paths.sort();
                for path in paths {
                    let Ok(metadata) = path.metadata() else {
                        continue;
                    };
                    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
                        continue;
                    }
                    path.hash(&mut fingerprint);
                    metadata.len().hash(&mut fingerprint);
                    metadata.modified().ok().hash(&mut fingerprint);
                    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let registered = if engine == "zsh" {
                        use std::io::BufRead;
                        let Ok(file) = std::fs::File::open(&path) else {
                            continue;
                        };
                        let mut line = String::new();
                        if std::io::BufReader::new(file).read_line(&mut line).is_err() {
                            continue;
                        }
                        let Some(header) = line.strip_prefix("#compdef ") else {
                            continue;
                        };
                        header
                            .split_whitespace()
                            .take_while(|word| !word.starts_with('#'))
                            .filter(|word| !word.starts_with('-'))
                            .map(|word| {
                                word.split_once('=')
                                    .map_or(word, |(name, _)| name)
                                    .to_owned()
                            })
                            .collect::<Vec<_>>()
                    } else if engine == "fish" {
                        filename
                            .strip_suffix(".fish")
                            .map(|name| vec![name.to_owned()])
                            .unwrap_or_default()
                    } else {
                        vec![filename.to_owned()]
                    };
                    for command in registered {
                        let registrations = commands.entry(command).or_default();
                        if !registrations.iter().any(|entry| entry.engine == engine) {
                            registrations.push(Registration {
                                engine: engine.into(),
                                source: "installed".into(),
                                path: path.clone(),
                            });
                        }
                    }
                }
            }
        }
        let fingerprint = fingerprint.finish();
        if cache.iter().any(|index| {
            index.path.as_deref() == execution_path && index.fingerprint != fingerprint
        }) {
            for shell in [&self.bash, &self.fish].into_iter().flatten() {
                shell.invalidate();
            }
            if let Some(shell) = &self.zsh {
                shell.invalidate();
            }
        }
        cache.retain(|index| index.path.as_deref() != execution_path);
        let result = commands.get(name).cloned().unwrap_or_default();
        cache.push_back(Installed {
            path: execution_path.map(ToOwned::to_owned),
            created: Instant::now(),
            fingerprint,
            commands,
        });
        while cache.len() > 16 {
            cache.pop_front();
        }
        result
    }
}

struct Installed {
    path: Option<std::ffi::OsString>,
    created: Instant,
    fingerprint: u64,
    commands: std::collections::BTreeMap<String, Vec<Registration>>,
}

fn load_registry() -> std::collections::BTreeMap<String, Vec<Registration>> {
    assets::root()
        .and_then(|root| std::fs::read(root.join("packs/registry.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Registry>(&bytes).ok())
        .map(|registry| registry.commands)
        .unwrap_or_else(|| {
            tracing::debug!("completion pack registry unavailable");
            Default::default()
        })
}

#[derive(serde::Deserialize)]
struct Registry {
    commands: std::collections::BTreeMap<String, Vec<Registration>>,
}

fn wrapper_command_position(words: &[String]) -> bool {
    let Some(command) = words.first().and_then(|word| word.rsplit('/').next()) else {
        return false;
    };
    let (switches, values): (&[&str], &[&str]) = match command {
        "env" => (
            &["-i", "--ignore-environment", "-0", "--null"],
            &["-u", "--unset", "-C", "--chdir", "-a", "--argv0"],
        ),
        "sudo" => (
            &[
                "-A",
                "-b",
                "-E",
                "-H",
                "-k",
                "-n",
                "-S",
                "--askpass",
                "--background",
                "--non-interactive",
                "--stdin",
            ],
            &[
                "-u",
                "--user",
                "-g",
                "--group",
                "-p",
                "--prompt",
                "-C",
                "--close-from",
                "-T",
                "--command-timeout",
                "-r",
                "--role",
                "-t",
                "--type",
            ],
        ),
        "command" => (&["-p", "-v", "-V"], &[]),
        "exec" => (&["-c", "-l"], &["-a"]),
        "nohup" => (&[], &[]),
        "time" => (&["-p"], &[]),
        "xargs" => (
            &[
                "-0",
                "--null",
                "-r",
                "--no-run-if-empty",
                "-t",
                "--verbose",
                "-p",
                "--interactive",
                "-x",
                "--exit",
            ],
            &[
                "-n",
                "--max-args",
                "-s",
                "--max-chars",
                "-P",
                "--max-procs",
                "-I",
                "--replace",
                "-E",
                "--eof",
                "-L",
                "--max-lines",
                "-d",
                "--delimiter",
            ],
        ),
        _ => return false,
    };
    let mut arguments = words.iter().skip(1);
    while let Some(word) = arguments.next() {
        if word == "--" {
            return arguments.next().is_none();
        }
        if command == "env" && !word.starts_with('-') && word.contains('=') {
            continue;
        }
        if switches.contains(&word.as_str()) {
            continue;
        }
        if values.contains(&word.as_str()) {
            if arguments.next().is_none() {
                return false;
            }
            continue;
        }
        if word
            .split_once('=')
            .is_some_and(|(name, _)| values.contains(&name))
        {
            continue;
        }
        // A command is already present, or an unknown option changes the position.
        return false;
    }
    true
}

fn filter_private_candidates(
    root: &Path,
    environment: &Environment,
    directory: &Path,
    dialect: &str,
    command_position: bool,
    entries: &[Candidate],
) -> Vec<Candidate> {
    let shell = match dialect {
        "fish" => shucked_command::ShellDialect::Fish,
        "zsh" => shucked_command::ShellDialect::Zsh,
        "sh" => shucked_command::ShellDialect::Posix,
        "ksh" => shucked_command::ShellDialect::Mksh,
        _ => shucked_command::ShellDialect::Bash,
    };
    let builtins = shucked_command::builtins(shell);
    entries
        .iter()
        .filter(|candidate| {
            let name = candidate.text.trim_end();
            if Path::new(name).is_absolute() && Path::new(name).starts_with(root.join("runtime")) {
                return false;
            }
            !command_position
                || !assets::private_command(root, name)
                || builtins.contains(&name)
                || directory.join(name).exists()
                || environment.executable_path(name, directory).is_some()
        })
        .cloned()
        .collect()
}

#[cfg(all(test, unix))]
#[path = "../../../tests/completion/native.rs"]
mod tests;
