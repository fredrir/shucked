//! Login-shell environment capture.
//!
//! The "Login shell" execution context runs the user's login shell once per
//! (shell path, startup-file fingerprint) and reads back the same NUL-framed
//! records the terminal hooks in `editors/vscode/shell-integration` report:
//! working directory, PATH, aliases, function names and alias options. The
//! result feeds the resolver exactly like an attached terminal, so aliases,
//! functions, PATH and `aliases=on` semantics apply identically.
//!
//! Running startup files is real code execution. It only happens when the
//! workspace is trusted (`nativeExecutionAllowed`), never for editor buffers,
//! and never more often than the startup-file fingerprint changes.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::commands::ShellSessionState;
use crate::session::Client;

/// Hard limit for one capture, including startup-file execution.
pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum accepted output; a larger report is rejected as unparsable.
pub(crate) const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Session identifier used for login-shell state in analysis contexts.
pub(crate) const SESSION_ID: &str = "login-shell";
/// Environment policy value that selects login-shell capture.
pub(crate) const POLICY: &str = "login-shell";
/// Environment variable set for the captured shell so users can guard slow startup code.
pub(crate) const CAPTURE_VARIABLE: &str = "SHUCKED_CAPTURE";
const END_KEY: &str = "end";
const END_VALUE: &str = "shucked-login-shell";
const RETRY_BASE: Duration = Duration::from_secs(30);
const RETRY_CAP: Duration = Duration::from_secs(600);

/// Shell family, decided by the executable name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind {
    Zsh,
    Bash,
    Fish,
    Posix,
}

impl ShellKind {
    pub(crate) fn from_path(shell: &Path) -> Self {
        let name = shell
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let name = name.strip_suffix(".exe").unwrap_or(&name);
        match name {
            "zsh" => Self::Zsh,
            "bash" => Self::Bash,
            "fish" => Self::Fish,
            _ => Self::Posix,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
            Self::Posix => "sh",
        }
    }

    /// The capture script prints the same record stream as the terminal hooks.
    fn script(self) -> &'static str {
        match self {
            Self::Zsh => {
                r#"builtin printf 'cwd\0%s\0' "$PWD"
for __shucked_part in "${path[@]}"; do builtin printf 'path\0%s\0' "$__shucked_part"; done
for __shucked_name in "${(@k)aliases}"; do builtin printf 'alias\0%s=%s\0' "$__shucked_name" "${aliases[$__shucked_name]}"; done
for __shucked_name in "${(@k)functions}"; do builtin printf 'function\0%s\0' "$__shucked_name"; done
builtin printf 'option\0aliases=%s\0' "${options[aliases]}"
builtin printf 'end\0shucked-login-shell\0'
"#
            }
            Self::Bash => {
                r#"builtin printf 'cwd\0%s\0' "$PWD"
builtin printf 'searchpath\0%s\0' "$PATH"
while IFS= read -r __shucked_name; do builtin printf 'alias\0%s\0' "$__shucked_name"; done < <(builtin alias -p)
while IFS= read -r __shucked_name; do builtin printf 'function\0%s\0' "$__shucked_name"; done < <(builtin compgen -A function)
builtin printf 'option\0expand_aliases=%s\0' "$(builtin shopt -q expand_aliases && builtin printf 1 || builtin printf 0)"
builtin printf 'end\0shucked-login-shell\0'
"#
            }
            Self::Fish => {
                r#"builtin printf 'cwd\0%s\0' "$PWD"
for entry in $PATH; builtin printf 'path\0%s\0' "$entry"; end
functions --names | while read -l name; builtin printf 'function\0%s\0' "$name"; end
builtin printf 'end\0shucked-login-shell\0'
"#
            }
            Self::Posix => {
                r#"printf 'cwd\0%s\0' "$PWD"
printf 'searchpath\0%s\0' "$PATH"
alias | while IFS= read -r __shucked_name; do printf 'alias\0%s\0' "$__shucked_name"; done
printf 'end\0shucked-login-shell\0'
"#
            }
        }
    }
}

/// Resolve the login shell: explicit setting, then `$SHELL`, then the platform default.
pub(crate) fn resolve_shell(explicit: Option<&Path>, os_shell: Option<&Path>) -> PathBuf {
    explicit
        .filter(|path| !path.as_os_str().is_empty())
        .or(os_shell)
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/bash"
            })
        })
}

/// Directories that decide which startup files a login shell reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupLocations {
    pub home: Option<PathBuf>,
    pub zdotdir: Option<PathBuf>,
    pub config_home: Option<PathBuf>,
}

impl StartupLocations {
    pub(crate) fn from_environment() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self {
            zdotdir: std::env::var_os("ZDOTDIR")
                .map(PathBuf::from)
                .or_else(|| home.clone()),
            config_home: std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(".config"))),
            home,
        }
    }

    #[cfg(test)]
    pub(crate) fn private(home: &Path) -> Self {
        Self {
            home: Some(home.to_path_buf()),
            zdotdir: Some(home.to_path_buf()),
            config_home: Some(home.join(".config")),
        }
    }
}

/// Startup files and directories whose changes invalidate a capture.
pub(crate) fn startup_files(kind: ShellKind, locations: &StartupLocations) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let home = |name: &str| locations.home.as_ref().map(|home| home.join(name));
    match kind {
        ShellKind::Zsh => {
            for name in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
                files.extend(locations.zdotdir.as_ref().map(|dir| dir.join(name)));
                files.extend(home(name));
            }
            files.extend(
                [
                    "/etc/zshenv",
                    "/etc/zprofile",
                    "/etc/zshrc",
                    "/etc/zlogin",
                    "/etc/zsh/zshenv",
                    "/etc/zsh/zprofile",
                    "/etc/zsh/zshrc",
                    "/etc/zsh/zlogin",
                ]
                .map(PathBuf::from),
            );
        }
        ShellKind::Bash => {
            for name in [".bash_profile", ".bash_login", ".profile", ".bashrc"] {
                files.extend(home(name));
            }
            files.extend(
                [
                    "/etc/profile",
                    "/etc/bash.bashrc",
                    "/etc/bashrc",
                    "/etc/profile.d",
                ]
                .map(PathBuf::from),
            );
        }
        ShellKind::Fish => {
            if let Some(config) = &locations.config_home {
                files.push(config.join("fish/config.fish"));
                files.push(config.join("fish/conf.d"));
                files.push(config.join("fish/functions"));
            }
            files.extend(["/etc/fish/config.fish", "/etc/fish/conf.d"].map(PathBuf::from));
        }
        ShellKind::Posix => {
            files.extend(home(".profile"));
            files.push(PathBuf::from("/etc/profile"));
        }
    }
    files.sort();
    files.dedup();
    files
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
    directory: bool,
}

/// Metadata of the shell executable and its startup files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint(BTreeMap<PathBuf, Option<FileStamp>>);

impl Fingerprint {
    pub(crate) fn capture(shell: &Path, kind: ShellKind, locations: &StartupLocations) -> Self {
        let mut paths = startup_files(kind, locations);
        paths.push(shell.to_path_buf());
        Self(
            paths
                .into_iter()
                .map(|path| {
                    let stamp = path.metadata().ok().map(|metadata| FileStamp {
                        length: metadata.len(),
                        modified: metadata.modified().ok(),
                        directory: metadata.is_dir(),
                    });
                    (path, stamp)
                })
                .collect(),
        )
    }
}

/// Records parsed from a capture report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParsedCapture {
    pub cwd: Option<PathBuf>,
    pub path: Vec<PathBuf>,
    pub aliases: BTreeMap<String, Vec<String>>,
    pub functions: BTreeSet<String>,
    pub options: BTreeMap<String, String>,
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

/// Split a simple alias value into words; anything with shell syntax is opaque.
pub(crate) fn simple_alias(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if ch == '\'' || ch == '"' {
            if quote == Some(ch) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(ch);
            } else {
                word.push(ch);
            }
            continue;
        }
        if matches!(
            ch,
            '`' | '$' | ';' | '|' | '&' | '<' | '>' | '(' | ')' | '\n'
        ) {
            return None;
        }
        if quote.is_none() && ch.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if escaped || quote.is_some() || text.ends_with(char::is_whitespace) {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    let command_word = |word: &str| {
        !word.is_empty()
            && word.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'/' | b'+' | b'-')
            })
    };
    let switch = |word: &str| {
        let body = word
            .strip_prefix("--")
            .or_else(|| word.strip_prefix('-'))
            .unwrap_or("");
        !body.is_empty()
            && body
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    };
    // Only command mappings and simple switches are exported, never argument values.
    (!words.is_empty()
        && words.len() <= 32
        && command_word(&words[0])
        && words[1..].iter().all(|word| switch(word)))
    .then_some(words)
}

/// Parse a NUL-framed `key\0value\0` record stream. The trailing end marker
/// proves the capture script itself ran to completion.
pub(crate) fn parse_records(bytes: &[u8]) -> Result<ParsedCapture, String> {
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(format!(
            "the shell report exceeded {} KiB",
            MAX_OUTPUT_BYTES / 1024
        ));
    }
    let text = String::from_utf8_lossy(bytes);
    let mut fields = text.split('\0');
    let mut parsed = ParsedCapture::default();
    let mut complete = false;
    while let (Some(key), Some(value)) = (fields.next(), fields.next()) {
        match key {
            "cwd" => parsed.cwd = Some(PathBuf::from(value)),
            "searchpath" => parsed.path = value.split(':').map(PathBuf::from).collect(),
            "path" => parsed.path.push(PathBuf::from(value)),
            "function" => {
                if valid_name(value) && !value.starts_with("__shucked") {
                    parsed.functions.insert(value.to_owned());
                }
            }
            "alias" => {
                let line = value.strip_prefix("alias ").unwrap_or(value);
                let Some((name, expansion)) = line.split_once('=') else {
                    continue;
                };
                if !valid_name(name) {
                    continue;
                }
                let expansion = expansion
                    .strip_prefix('\'')
                    .and_then(|rest| rest.strip_suffix('\''))
                    .map(|inner| inner.replace("'\\''", "'"))
                    .unwrap_or_else(|| expansion.to_owned());
                match simple_alias(&expansion) {
                    Some(words) => {
                        parsed.aliases.insert(name.to_owned(), words);
                    }
                    None => {
                        parsed.functions.insert(name.to_owned());
                    }
                }
            }
            "option" => {
                if let Some((key, value)) = value.split_once('=') {
                    parsed.options.insert(key.to_owned(), value.to_owned());
                }
            }
            END_KEY if value == END_VALUE => complete = true,
            _ => {}
        }
    }
    if !complete {
        return Err(
            "the capture script did not finish; a startup file may have replaced or exited the shell"
                .into(),
        );
    }
    Ok(parsed)
}

/// Why a capture produced no usable state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureError {
    Spawn(String),
    Timeout(Duration),
    Exit(String),
    Report(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "could not start the shell: {error}"),
            Self::Timeout(limit) => write!(f, "timed out after {} s", limit.as_secs()),
            Self::Exit(status) => write!(f, "the shell exited with {status}"),
            Self::Report(reason) => write!(f, "{reason}"),
        }
    }
}

/// Run `shell -l -i -c <script>` with no stdin, a bounded stdout and a hard deadline.
pub(crate) fn run_capture(
    shell: &Path,
    kind: ShellKind,
    timeout: Duration,
    cwd: Option<&Path>,
) -> Result<ParsedCapture, CaptureError> {
    #[cfg(not(unix))]
    {
        let _ = (shell, kind, timeout, cwd);
        Err(CaptureError::Spawn(
            "login shell capture is only supported on Unix hosts".into(),
        ))
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut command = Command::new(shell);
        command
            .args(["-l", "-i", "-c", kind.script()])
            .env("TERM", "dumb")
            .env(CAPTURE_VARIABLE, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        if let Some(home) = cwd.filter(|home| home.is_dir()) {
            // A login shell starts in the home directory that owns its startup files.
            command.current_dir(home).env("HOME", home);
        }
        let mut child = command
            .spawn()
            .map_err(|error| CaptureError::Spawn(error.to_string()))?;
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err(CaptureError::Spawn("stdout unavailable".into()));
        };
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_buffer = buffer.clone();
        let reader_finished = finished.clone();
        // A startup file may leave background children holding the pipe open,
        // so the reader is never joined; it ends when the last writer closes.
        std::thread::Builder::new()
            .name("shucked-login-shell-read".into())
            .spawn(move || {
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => {
                            let mut output = reader_buffer
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if output.len() + count > MAX_OUTPUT_BYTES + 1 {
                                let room = (MAX_OUTPUT_BYTES + 1).saturating_sub(output.len());
                                output.extend_from_slice(&chunk[..room]);
                                break;
                            }
                            output.extend_from_slice(&chunk[..count]);
                        }
                    }
                }
                reader_finished.store(true, Ordering::Release);
            })
            .map_err(|error| CaptureError::Spawn(error.to_string()))?;
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if started.elapsed() >= timeout => {
                    // SAFETY: the child was placed in its own process group, so
                    // the group id equals its pid and only its own tree is signalled.
                    unsafe {
                        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(CaptureError::Timeout(timeout));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    break Err(CaptureError::Spawn(error.to_string()));
                }
            }
        };
        let status = status?;
        // Drain what the shell wrote before it exited; stray grandchildren
        // keeping the pipe open must not stall the capture.
        let grace = Instant::now();
        while !finished.load(Ordering::Acquire) && grace.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !status.success() {
            return Err(CaptureError::Exit(status.to_string()));
        }
        parse_records(&output).map_err(CaptureError::Report)
    }
}

/// Current knowledge about one login shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoginShellStatus {
    /// Capture cannot run: untrusted workspace or unsupported host.
    Disabled(String),
    /// A capture is running; callers fall back to the workspace snapshot.
    Pending,
    /// Captured state, used like an attached terminal.
    Ready(Arc<ShellSessionState>),
    /// The last capture failed; the workspace snapshot stays in use.
    Failed(String),
}

impl LoginShellStatus {
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Disabled(reason) => format!("disabled ({reason})"),
            Self::Pending => "capture in progress".into(),
            Self::Ready(state) => format!(
                "captured ({} PATH entries, {} aliases, {} functions)",
                state.path.len(),
                state.aliases.len(),
                state.functions.len()
            ),
            Self::Failed(reason) => format!("failed ({reason})"),
        }
    }
}

struct Entry {
    kind: ShellKind,
    fingerprint: Fingerprint,
    status: LoginShellStatus,
    attempts: u32,
    next_retry: Option<Instant>,
    captured: Option<Instant>,
}

/// Runs and caches login-shell captures for the whole session.
pub(crate) struct LoginShellService {
    native_allowed: bool,
    client: Option<Client>,
    timeout: Duration,
    locations: StartupLocations,
    os_shell: Option<PathBuf>,
    entries: Mutex<BTreeMap<PathBuf, Entry>>,
    generation: AtomicU64,
    captures_started: AtomicU64,
}

impl LoginShellService {
    pub(crate) fn new(native_allowed: bool, client: Option<Client>) -> Self {
        Self {
            native_allowed,
            client,
            timeout: CAPTURE_TIMEOUT,
            locations: StartupLocations::from_environment(),
            os_shell: std::env::var_os("SHELL").map(PathBuf::from),
            entries: Mutex::default(),
            generation: AtomicU64::new(0),
            captures_started: AtomicU64::new(0),
        }
    }

    /// Test hook: private startup locations, `$SHELL` and deadline.
    #[cfg(test)]
    pub(crate) fn with_host(
        mut self,
        locations: StartupLocations,
        os_shell: Option<PathBuf>,
        timeout: Duration,
    ) -> Self {
        self.locations = locations;
        self.os_shell = os_shell;
        self.timeout = timeout;
        self
    }

    /// Number of shell processes started so far; tests use it to prove restraint.
    #[cfg(test)]
    pub(crate) fn captures_started(&self) -> u64 {
        self.captures_started.load(Ordering::Acquire)
    }

    pub(crate) fn resolve_shell(&self, explicit: Option<&Path>) -> PathBuf {
        resolve_shell(explicit, self.os_shell.as_deref())
    }

    /// Current status without starting a capture.
    pub(crate) fn status(&self, shell: &Path) -> LoginShellStatus {
        if let Some(reason) = self.disabled_reason() {
            return LoginShellStatus::Disabled(reason);
        }
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(shell)
            .map(|entry| entry.status.clone())
            .unwrap_or(LoginShellStatus::Pending)
    }

    /// Age of the current capture, if one succeeded.
    pub(crate) fn captured_age(&self, shell: &Path) -> Option<Duration> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(shell)
            .and_then(|entry| entry.captured)
            .map(|captured| captured.elapsed())
    }

    fn disabled_reason(&self) -> Option<String> {
        if !self.native_allowed {
            return Some("the workspace is not trusted".into());
        }
        if !cfg!(unix) {
            return Some("login shell capture is only supported on Unix hosts".into());
        }
        None
    }

    /// Status for `shell`, starting the first capture when nothing is known yet.
    pub(crate) fn lookup(self: &Arc<Self>, shell: &Path) -> LoginShellStatus {
        if let Some(reason) = self.disabled_reason() {
            return LoginShellStatus::Disabled(reason);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = entries.get(shell) {
            return entry.status.clone();
        }
        if entries.len() >= 8 {
            return LoginShellStatus::Failed("too many distinct login shells requested".into());
        }
        let kind = ShellKind::from_path(shell);
        let fingerprint = Fingerprint::capture(shell, kind, &self.locations);
        entries.insert(
            shell.to_path_buf(),
            Entry {
                kind,
                fingerprint,
                status: LoginShellStatus::Pending,
                attempts: 0,
                next_retry: None,
                captured: None,
            },
        );
        drop(entries);
        self.spawn(shell.to_path_buf(), kind);
        LoginShellStatus::Pending
    }

    /// Re-capture shells whose startup files changed, and retry failures with backoff.
    pub(crate) fn refresh(self: &Arc<Self>) {
        if self.disabled_reason().is_some() {
            return;
        }
        let mut restart = Vec::new();
        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (shell, entry) in entries.iter_mut() {
                if entry.status == LoginShellStatus::Pending {
                    continue;
                }
                let fingerprint = Fingerprint::capture(shell, entry.kind, &self.locations);
                let changed = fingerprint != entry.fingerprint;
                let retry_due = matches!(entry.status, LoginShellStatus::Failed(_))
                    && entry.next_retry.is_some_and(|at| Instant::now() >= at);
                if changed {
                    entry.fingerprint = fingerprint;
                    entry.attempts = 0;
                }
                if changed || retry_due {
                    entry.status = LoginShellStatus::Pending;
                    entry.next_retry = None;
                    restart.push((shell.clone(), entry.kind));
                }
            }
        }
        for (shell, kind) in restart {
            self.spawn(shell, kind);
        }
    }

    /// Retry failed captures now; used when the user selects the context again.
    pub(crate) fn retry_failed(self: &Arc<Self>) {
        if self.disabled_reason().is_some() {
            return;
        }
        let mut restart = Vec::new();
        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (shell, entry) in entries.iter_mut() {
                if matches!(entry.status, LoginShellStatus::Failed(_)) {
                    entry.fingerprint = Fingerprint::capture(shell, entry.kind, &self.locations);
                    entry.status = LoginShellStatus::Pending;
                    entry.next_retry = None;
                    restart.push((shell.clone(), entry.kind));
                }
            }
        }
        for (shell, kind) in restart {
            self.spawn(shell, kind);
        }
    }

    /// PATH entries of captured shells, for install/removal watches.
    pub(crate) fn watch_directories(&self) -> Vec<PathBuf> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut paths = Vec::new();
        for (shell, entry) in entries.iter() {
            paths.extend(startup_files(entry.kind, &self.locations));
            paths.push(shell.clone());
            if let LoginShellStatus::Ready(state) = &entry.status {
                paths.extend(state.path.iter().map(|path| state.cwd.join(path)));
            }
        }
        paths
    }

    fn spawn(self: &Arc<Self>, shell: PathBuf, kind: ShellKind) {
        self.captures_started.fetch_add(1, Ordering::AcqRel);
        let service = self.clone();
        let thread_shell = shell.clone();
        let spawned = std::thread::Builder::new()
            .name("shucked-login-shell".into())
            .spawn(move || service.capture(thread_shell, kind));
        if let Err(error) = spawned {
            self.finish(&shell, Err(CaptureError::Spawn(error.to_string())), kind);
        }
    }

    fn capture(self: Arc<Self>, shell: PathBuf, kind: ShellKind) {
        tracing::info!(shell = %shell.display(), kind = kind.label(), "capturing login shell environment");
        let result = run_capture(&shell, kind, self.timeout, self.locations.home.as_deref());
        self.finish(&shell, result, kind);
    }

    fn finish(&self, shell: &Path, result: Result<ParsedCapture, CaptureError>, kind: ShellKind) {
        let status = match result {
            Ok(parsed) => {
                let cwd = parsed
                    .cwd
                    .filter(|cwd| cwd.is_absolute())
                    .or_else(|| self.locations.home.clone())
                    .unwrap_or_else(|| PathBuf::from("/"));
                let mut state = ShellSessionState {
                    id: SESSION_ID.into(),
                    generation: self.generation.fetch_add(1, Ordering::AcqRel) + 1,
                    cwd,
                    path: parsed.path,
                    aliases: parsed.aliases,
                    functions: parsed.functions,
                    connected: true,
                    live_completion: false,
                    shell: Some(kind.label().into()),
                    options: parsed.options,
                };
                super::commands::truncate_session_state(&mut state);
                tracing::info!(
                    shell = %shell.display(),
                    path_entries = state.path.len(),
                    aliases = state.aliases.len(),
                    functions = state.functions.len(),
                    "login shell environment captured"
                );
                LoginShellStatus::Ready(Arc::new(state))
            }
            Err(error) => {
                tracing::warn!(shell = %shell.display(), %error, "login shell capture failed");
                LoginShellStatus::Failed(error.to_string())
            }
        };
        let notice = {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = entries.get_mut(shell) else {
                return;
            };
            match &status {
                LoginShellStatus::Ready(_) => {
                    entry.attempts = 0;
                    entry.next_retry = None;
                    entry.captured = Some(Instant::now());
                }
                LoginShellStatus::Failed(_) => {
                    entry.attempts = entry.attempts.saturating_add(1);
                    let backoff = RETRY_BASE
                        .saturating_mul(1u32 << entry.attempts.saturating_sub(1).min(5))
                        .min(RETRY_CAP);
                    entry.next_retry = Some(Instant::now() + backoff);
                }
                _ => {}
            }
            let notice = match &status {
                LoginShellStatus::Failed(reason) => Some(format!(
                    "Shucked could not capture the login shell {} ({reason}). The workspace environment stays in use; set SHUCKED_CAPTURE guards in slow startup files or change shucked.environment.loginShell.",
                    shell.display()
                )),
                _ => None,
            };
            entry.status = status;
            notice
        };
        if let Some(client) = &self.client {
            if let Some(notice) = notice {
                let _ = client.show_message_once(
                    format!("login-shell:{}", shell.display()),
                    notice,
                    lsp_types::MessageType::INFO,
                );
            }
            let _ = client.environment_changed();
        }
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/server/login_shell.rs"]
mod tests;
