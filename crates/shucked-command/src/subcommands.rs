//! Cached subcommand inventories for tools whose first argument selects a
//! command: brew, git, docker and kubectl.
//!
//! Each installed executable is queried once with a fixed listing command,
//! under a bounded deadline, a capped output size, a clean environment and no
//! controlling terminal. The result is kept in memory and under the shucked
//! cache directory, keyed by the executable's path, size and modification
//! time, so a later server start answers from disk without running the tool.
//! The owner of the environment snapshot decides when a changed executable is
//! noticed: a new identity from a refreshed snapshot yields a new inventory.
//! Inventories are suggestions for completion only; they never justify a
//! rejection.
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{EnvironmentSnapshot, ExecutableIdentity, ExecutionContext, ValidationPolicy};

/// Tools with a supported inventory query, by executable basename.
pub const SUBCOMMAND_TOOLS: &[&str] = &["brew", "git", "docker", "kubectl"];
/// Deadline for one listing query.
pub const INVENTORY_TIMEOUT: Duration = Duration::from_secs(3);
/// Listing output beyond this size is discarded instead of parsed.
pub const MAX_INVENTORY_OUTPUT: usize = 1024 * 1024;
const MAX_COMMANDS: usize = 5000;
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 120;
const MAX_MEMORY_ENTRIES: usize = 16;
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubcommandEntry {
    pub name: String,
    /// The tool's own one-line summary when it prints one, otherwise a summary
    /// written for this repository, otherwise empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubcommandInventory {
    pub schema_version: u32,
    pub tool: String,
    pub executable: PathBuf,
    pub size: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    pub captured_unix_ms: u64,
    pub commands: Vec<SubcommandEntry>,
}

impl SubcommandInventory {
    fn matches(&self, identity: &ExecutableIdentity) -> bool {
        self.schema_version == SCHEMA_VERSION
            && self.executable == identity.path
            && self.size == identity.size
            && self.modified_unix_ms == identity.modified_unix_ms
    }
}

static MEMORY: OnceLock<Mutex<VecDeque<Arc<SubcommandInventory>>>> = OnceLock::new();

fn memory() -> std::sync::MutexGuard<'static, VecDeque<Arc<SubcommandInventory>>> {
    MEMORY
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The supported tool an executable stands for, by basename.
pub fn tool_name(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    let name = name.strip_suffix(".exe").unwrap_or(name);
    SUBCOMMAND_TOOLS.iter().copied().find(|tool| *tool == name)
}

/// The same gate that permits every native query: explicit client authority,
/// a live host policy and a fresh snapshot for this target.
pub fn execution_permitted(context: &ExecutionContext, environment: &EnvironmentSnapshot) -> bool {
    context.native_execution_allowed
        && !matches!(
            context.policy,
            ValidationPolicy::Captured | ValidationPolicy::Portable
        )
        && environment.fresh
        && context.target_id == environment.target_id
}

/// Forget every inventory held in memory. Disk entries stay and are
/// re-validated against the executable identity on the next lookup.
pub fn invalidate() {
    memory().clear();
}

/// An inventory for exactly this executable identity without running anything:
/// memory first, then the cache directory.
pub fn cached(
    identity: &ExecutableIdentity,
    cache_directory: Option<&Path>,
) -> Option<Arc<SubcommandInventory>> {
    tool_name(&identity.path)?;
    if let Some(inventory) = memory()
        .iter()
        .find(|inventory| inventory.matches(identity))
    {
        return Some(inventory.clone());
    }
    let inventory = Arc::new(read_disk(identity, cache_directory?)?);
    remember(inventory.clone());
    Some(inventory)
}

/// A cached inventory, or a fresh one from a single bounded query when the
/// context permits native execution.
pub fn acquire(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    identity: &ExecutableIdentity,
    cache_directory: Option<&Path>,
    cancellation: &dyn Fn() -> bool,
) -> Option<Arc<SubcommandInventory>> {
    let tool = tool_name(&identity.path)?;
    if let Some(inventory) = cached(identity, cache_directory) {
        return Some(inventory);
    }
    if !execution_permitted(context, environment) || cancellation() {
        return None;
    }
    // Query the file the snapshot described, never a replacement that appeared
    // since the snapshot was taken.
    let crate::LookupEvidence::Present(current) =
        crate::host::exact_lookup(context, environment, identity.path.to_str()?)
    else {
        return None;
    };
    if current.identity.path != identity.path
        || current.identity.size != identity.size
        || current.identity.modified_unix_ms != identity.modified_unix_ms
    {
        return None;
    }
    let commands = match tool {
        "brew" => parse_brew(&run(
            context,
            environment,
            &identity.path,
            &["commands", "--quiet", "--include-aliases"],
            cancellation,
        )?),
        "git" => run(
            context,
            environment,
            &identity.path,
            &["--list-cmds=main,others,alias,nohelpers"],
            cancellation,
        )
        .and_then(|text| parse_git_list(&text))
        .or_else(|| {
            parse_git_help(&run(
                context,
                environment,
                &identity.path,
                &["help", "-a"],
                cancellation,
            )?)
        }),
        "docker" => parse_docker_help(&run(
            context,
            environment,
            &identity.path,
            &["--help"],
            cancellation,
        )?),
        "kubectl" => parse_kubectl_help(&run(
            context,
            environment,
            &identity.path,
            &["--help"],
            cancellation,
        )?),
        _ => None,
    }?;
    // A listing that completed is kept even when the request that asked for
    // it was cancelled meanwhile: the tool ran once, and the next request
    // should not run it again.
    let inventory = Arc::new(SubcommandInventory {
        schema_version: SCHEMA_VERSION,
        tool: tool.into(),
        executable: identity.path.clone(),
        size: identity.size,
        modified_unix_ms: identity.modified_unix_ms,
        captured_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or_default(),
        commands,
    });
    remember(inventory.clone());
    if let Some(directory) = cache_directory {
        write_disk(&inventory, directory);
    }
    Some(inventory)
}

fn remember(inventory: Arc<SubcommandInventory>) {
    let mut memory = memory();
    memory.retain(|old| old.executable != inventory.executable);
    memory.push_back(inventory);
    while memory.len() > MAX_MEMORY_ENTRIES {
        memory.pop_front();
    }
}

fn disk_path(identity: &ExecutableIdentity, directory: &Path) -> Option<PathBuf> {
    use sha2::Digest;
    let tool = tool_name(&identity.path)?;
    let digest = sha2::Sha256::digest(identity.path.as_os_str().as_encoded_bytes());
    let key = digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Some(
        directory
            .join("subcommands")
            .join(format!("{tool}-{key}.json")),
    )
}

fn read_disk(identity: &ExecutableIdentity, directory: &Path) -> Option<SubcommandInventory> {
    use std::io::Read;
    let path = disk_path(identity, directory)?;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_INVENTORY_OUTPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_INVENTORY_OUTPUT {
        return None;
    }
    let inventory: SubcommandInventory = serde_json::from_slice(&bytes).ok()?;
    (inventory.matches(identity)
        && inventory.tool == tool_name(&identity.path)?
        && inventory.commands.len() <= MAX_COMMANDS)
        .then_some(inventory)
}

fn write_disk(inventory: &SubcommandInventory, directory: &Path) {
    let Some(path) = disk_path(
        &ExecutableIdentity {
            path: inventory.executable.clone(),
            size: inventory.size,
            modified_unix_ms: inventory.modified_unix_ms,
            version: None,
            vendor: None,
        },
        directory,
    ) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(json) = serde_json::to_vec_pretty(inventory) else {
        return;
    };
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    if std::fs::write(&temporary, json).is_ok() && std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Run one fixed listing query with a clean environment: only the target's
/// PATH, locale and non-interactive settings, no inherited variables, no
/// standard input and no controlling terminal.
fn run(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    executable: &Path,
    arguments: &[&str],
    cancellation: &dyn Fn() -> bool,
) -> Option<String> {
    let path = std::env::join_paths(
        environment
            .search_path
            .iter()
            .map(|directory| &directory.path),
    )
    .ok()?;
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .env_clear()
        .env("PATH", path)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .env("HOMEBREW_NO_ENV_HINTS", "1")
        .env("NONINTERACTIVE", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "")
        .env("PAGER", "cat");
    // Tools locate their own configuration and temporary space through these;
    // nothing else from the server's environment is visible.
    for name in [
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "SYSTEMROOT",
        "SystemRoot",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(cwd) = &context.cwd {
        command.current_dir(cwd);
    }
    detach_terminal(&mut command);
    let output = crate::process::capture(&mut command, INVENTORY_TIMEOUT, cancellation, false)?;
    if output.len() > MAX_INVENTORY_OUTPUT {
        return None;
    }
    String::from_utf8(output).ok()
}

#[cfg(unix)]
fn detach_terminal(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs in the forked child before exec and performs
    // only async-signal-safe calls (open, ioctl, close) on its own descriptor.
    unsafe {
        command.pre_exec(|| {
            let tty = libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if tty >= 0 {
                libc::ioctl(tty, libc::TIOCNOTTY);
                libc::close(tty);
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_terminal(_command: &mut Command) {}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+:".contains(&byte))
        && !name.starts_with('-')
}

fn clean_description(text: &str) -> String {
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_DESCRIPTION)
        .collect::<String>();
    text.trim_end_matches('.').trim().to_owned()
}

/// Build the inventory from name lines, keeping the tool's own description
/// when it printed one and otherwise the bundled one-liner.
fn collect(
    tool: &str,
    entries: impl IntoIterator<Item = (String, String)>,
    anchor: &str,
) -> Option<Vec<SubcommandEntry>> {
    let mut seen = BTreeSet::new();
    let mut commands = Vec::new();
    for (name, description) in entries {
        if !valid_name(&name) || !seen.insert(name.clone()) {
            continue;
        }
        if commands.len() >= MAX_COMMANDS {
            return None;
        }
        let description = if description.is_empty() {
            bundled_description(tool, &name)
                .map(str::to_owned)
                .unwrap_or_default()
        } else {
            description
        };
        commands.push(SubcommandEntry { name, description });
    }
    if !seen.contains(anchor) {
        return None;
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    Some(commands)
}

/// One name per whitespace-separated token; `==>` headers are skipped.
pub fn parse_brew(text: &str) -> Option<Vec<SubcommandEntry>> {
    let names = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("==>"))
        .flat_map(str::split_whitespace)
        .map(|name| (name.to_owned(), String::new()));
    collect("brew", names, "install")
}

/// `git --list-cmds=...` prints one command per line without descriptions.
pub fn parse_git_list(text: &str) -> Option<Vec<SubcommandEntry>> {
    let names = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|name| (name.to_owned(), String::new()));
    collect("git", names, "commit")
}

/// `git help -a` groups indented `name  description` lines under headings.
pub fn parse_git_help(text: &str) -> Option<Vec<SubcommandEntry>> {
    collect("git", indented_entries(text, |_| true), "commit")
}

/// `docker --help` lists `name  description` lines under headings that end in
/// `Commands:`; a trailing `*` marks a plugin.
pub fn parse_docker_help(text: &str) -> Option<Vec<SubcommandEntry>> {
    collect(
        "docker",
        indented_entries(text, |heading| heading.ends_with("Commands:")),
        "run",
    )
}

/// `kubectl --help` lists `name  description` lines under headings such as
/// `Basic Commands (Beginner):` and `Other Commands:`.
pub fn parse_kubectl_help(text: &str) -> Option<Vec<SubcommandEntry>> {
    collect(
        "kubectl",
        indented_entries(text, |heading| {
            heading.ends_with(':') && heading.to_ascii_lowercase().contains("commands")
        }),
        "get",
    )
}

/// Indented `name  description` lines under the headings the filter accepts.
/// A blank or unindented line closes the current section.
fn indented_entries(text: &str, section: impl Fn(&str) -> bool) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    let mut inside = false;
    for line in text.lines().take(50_000) {
        if line.trim().is_empty() {
            inside = false;
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            inside = section(line.trim_end());
            continue;
        }
        if !inside {
            continue;
        }
        let mut parts = line.trim().splitn(2, char::is_whitespace);
        let Some(name) = parts.next() else { continue };
        let name = name.trim_end_matches('*');
        let description = parts.next().map(clean_description).unwrap_or_default();
        entries.push((name.to_owned(), description));
    }
    entries
}

/// One-line summaries for the commands people reach for most, written for
/// this repository. Tools that print their own summary take precedence.
pub fn bundled_description(tool: &str, name: &str) -> Option<&'static str> {
    let table: &[(&str, &str)] = match tool {
        "brew" => BREW,
        "git" => GIT,
        _ => return None,
    };
    table
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, description)| *description)
}

const BREW: &[(&str, &str)] = &[
    ("abv", "Show information about a formula or cask"),
    ("alias", "Define a shortcut for a Homebrew command"),
    ("analytics", "Control Homebrew's anonymous analytics"),
    (
        "autoremove",
        "Uninstall formulae that were only installed as dependencies",
    ),
    ("bundle", "Install or dump a Brewfile of dependencies"),
    ("cask", "Manage macOS applications distributed as casks"),
    ("casks", "List all casks"),
    ("cat", "Print the source of a formula or cask"),
    ("cleanup", "Remove old versions and stale downloads"),
    ("commands", "List built-in and external commands"),
    (
        "completions",
        "Control whether Homebrew links shell completions",
    ),
    ("config", "Show Homebrew and system configuration"),
    ("create", "Generate a formula or cask for a URL"),
    ("deps", "Show the dependencies of a formula"),
    ("desc", "Show the description of a formula or cask"),
    ("developer", "Control developer mode"),
    ("docs", "Open Homebrew's documentation"),
    ("doctor", "Check the system for potential problems"),
    ("dr", "Check the system for potential problems"),
    ("edit", "Open a formula or cask in the editor"),
    ("env", "Show the build environment used by Homebrew"),
    (
        "fetch",
        "Download the source or bottle of a formula or cask",
    ),
    ("formulae", "List all formulae"),
    ("gist-logs", "Upload the logs of a failed build as a gist"),
    ("help", "Show usage for Homebrew or a command"),
    ("home", "Open the homepage of a formula or cask"),
    ("homepage", "Open the homepage of a formula or cask"),
    ("info", "Show information about a formula or cask"),
    ("install", "Install a formula or cask"),
    (
        "leaves",
        "List installed formulae that nothing else depends on",
    ),
    ("link", "Symlink a keg's files into the Homebrew prefix"),
    ("list", "List installed formulae and casks"),
    ("ln", "Symlink a keg's files into the Homebrew prefix"),
    ("log", "Show the git log of a formula or cask"),
    ("ls", "List installed formulae and casks"),
    ("mcp-server", "Run Homebrew's MCP server"),
    ("migrate", "Move installed kegs after a formula rename"),
    (
        "missing",
        "Check for missing dependencies of installed formulae",
    ),
    ("nodenv-sync", "Link Homebrew Node versions into nodenv"),
    ("options", "Show install options for a formula"),
    (
        "outdated",
        "List formulae and casks with newer versions available",
    ),
    (
        "pin",
        "Keep a formula at its installed version during upgrades",
    ),
    ("postinstall", "Rerun the post-install steps of a formula"),
    ("pyenv-sync", "Link Homebrew Python versions into pyenv"),
    ("rbenv-sync", "Link Homebrew Ruby versions into rbenv"),
    ("readall", "Load every formula or cask to check for errors"),
    ("reinstall", "Uninstall and install a formula or cask again"),
    ("remove", "Uninstall a formula or cask"),
    ("rm", "Uninstall a formula or cask"),
    (
        "search",
        "Search for formulae and casks by name or description",
    ),
    (
        "services",
        "Manage background services with launchd or systemd",
    ),
    ("setup-ruby", "Install the Ruby that Homebrew uses"),
    (
        "shellenv",
        "Print shell commands that set up the Homebrew environment",
    ),
    ("tab", "Edit the tab of an installed formula or cask"),
    ("tap", "Add a tap of additional formulae, or list taps"),
    ("tap-info", "Show information about a tap"),
    ("tap-new", "Create a new tap skeleton"),
    ("unalias", "Remove a Homebrew command shortcut"),
    ("uninstall", "Uninstall a formula or cask"),
    ("unlink", "Remove a keg's symlinks from the Homebrew prefix"),
    ("unpin", "Allow a pinned formula to be upgraded again"),
    ("untap", "Remove a tap"),
    (
        "update",
        "Fetch the newest version of Homebrew and its taps",
    ),
    (
        "update-reset",
        "Reset the Homebrew repositories to the latest origin",
    ),
    ("upgrade", "Upgrade outdated formulae and casks"),
    ("uses", "Show the formulae that depend on a formula"),
    ("vendor-install", "Install Homebrew's vendored tools"),
    ("which-formula", "Show which formula provides a command"),
    ("which-update", "Update the database used by which-formula"),
];

const GIT: &[(&str, &str)] = &[
    ("add", "Stage file contents for the next commit"),
    ("am", "Apply a series of patches from a mailbox"),
    ("annotate", "Annotate file lines with commit information"),
    ("apply", "Apply a patch to files and/or the index"),
    ("archive", "Create an archive of files from a tree"),
    (
        "bisect",
        "Find the commit that introduced a bug by binary search",
    ),
    ("blame", "Show who last changed each line of a file"),
    ("branch", "List, create or delete branches"),
    ("bugreport", "Collect information for a bug report"),
    ("bundle", "Move objects and refs by archive"),
    (
        "cat-file",
        "Show the content, type or size of repository objects",
    ),
    ("check-attr", "Show gitattributes information"),
    ("check-ignore", "Debug gitignore and exclude files"),
    ("checkout", "Switch branches or restore working tree files"),
    ("cherry", "Find commits not merged upstream"),
    (
        "cherry-pick",
        "Apply the changes introduced by existing commits",
    ),
    ("citool", "Graphical alternative to git commit"),
    ("clean", "Remove untracked files from the working tree"),
    ("clone", "Copy a repository into a new directory"),
    ("commit", "Record staged changes to the repository"),
    ("config", "Get and set repository or global options"),
    (
        "count-objects",
        "Count unpacked objects and their disk usage",
    ),
    ("credential", "Retrieve and store user credentials"),
    ("daemon", "Serve repositories over the Git protocol"),
    ("describe", "Name a commit using the nearest reachable tag"),
    (
        "diagnose",
        "Generate a zip archive of diagnostic information",
    ),
    (
        "diff",
        "Show changes between commits, the index and the working tree",
    ),
    ("difftool", "Show changes using an external diff tool"),
    (
        "fast-export",
        "Export the history as a stream for importers",
    ),
    ("fast-import", "Import a history stream into the repository"),
    ("fetch", "Download objects and refs from another repository"),
    ("filter-branch", "Rewrite branches by applying filters"),
    ("flow", "Use the git-flow branching model"),
    ("for-each-ref", "Output information on each ref"),
    ("format-patch", "Prepare patches for e-mail submission"),
    ("fsck", "Verify the connectivity and validity of objects"),
    (
        "gc",
        "Clean up unnecessary files and optimize the repository",
    ),
    ("gitk", "Browse the history in a graphical window"),
    ("grep", "Search the tracked files for a pattern"),
    ("gui", "Graphical interface for Git"),
    (
        "hash-object",
        "Compute the object id of a file and optionally store it",
    ),
    ("help", "Show help for Git or a command"),
    ("init", "Create an empty repository or reinitialize one"),
    ("instaweb", "Browse the repository in a web browser"),
    (
        "interpret-trailers",
        "Add or parse trailer lines in commit messages",
    ),
    ("lfs", "Manage large files with Git LFS"),
    ("log", "Show the commit history"),
    (
        "ls-files",
        "Show information about files in the index and working tree",
    ),
    ("ls-remote", "List references in a remote repository"),
    ("ls-tree", "List the contents of a tree object"),
    (
        "mailinfo",
        "Extract patch and authorship from a single e-mail message",
    ),
    ("maintenance", "Run tasks that keep the repository healthy"),
    ("merge", "Join two or more development histories together"),
    ("merge-base", "Find the best common ancestor for a merge"),
    ("mergetool", "Resolve merge conflicts with an external tool"),
    ("mv", "Move or rename a file, directory or symlink"),
    ("name-rev", "Find symbolic names for given revisions"),
    ("notes", "Add or inspect notes attached to objects"),
    ("pack-refs", "Pack heads and tags for efficient access"),
    (
        "prune",
        "Remove unreachable objects from the object database",
    ),
    (
        "pull",
        "Fetch from another repository and integrate the changes",
    ),
    (
        "push",
        "Update remote refs along with the associated objects",
    ),
    ("range-diff", "Compare two commit ranges"),
    ("rebase", "Reapply commits on top of another base"),
    ("reflog", "Manage the reference logs"),
    ("remote", "Manage the set of tracked repositories"),
    ("repack", "Pack unpacked objects in the repository"),
    (
        "replace",
        "Create, list or delete refs that replace objects",
    ),
    ("request-pull", "Generate a summary of pending changes"),
    ("rerere", "Reuse recorded resolutions of conflicted merges"),
    ("reset", "Reset the current HEAD to a given state"),
    ("restore", "Restore working tree files"),
    (
        "rev-list",
        "List commit objects in reverse chronological order",
    ),
    ("rev-parse", "Pick out and massage parameters"),
    ("revert", "Create commits that undo existing commits"),
    ("rm", "Remove files from the working tree and the index"),
    ("scalar", "Manage large Git repositories"),
    ("send-email", "Send a collection of patches as e-mails"),
    ("shortlog", "Summarize the commit history by author"),
    ("show", "Show objects such as commits, tags and blobs"),
    ("show-branch", "Show branches and their commits"),
    ("show-ref", "List references in a local repository"),
    (
        "sparse-checkout",
        "Limit the working tree to a subset of tracked files",
    ),
    ("stash", "Shelve changes in a dirty working tree"),
    ("status", "Show the state of the working tree"),
    ("stripspace", "Remove unnecessary whitespace from text"),
    ("submodule", "Initialize, update or inspect submodules"),
    ("switch", "Switch branches"),
    ("symbolic-ref", "Read, modify or delete symbolic refs"),
    ("tag", "Create, list, delete or verify tags"),
    (
        "update-index",
        "Register file contents in the working tree to the index",
    ),
    (
        "update-ref",
        "Update the object name stored in a ref safely",
    ),
    ("var", "Show a Git logical variable"),
    ("verify-commit", "Check the GPG signature of commits"),
    ("verify-tag", "Check the GPG signature of tags"),
    ("version", "Show the Git version"),
    (
        "whatchanged",
        "Show logs with the differences each commit introduces",
    ),
    ("worktree", "Manage multiple working trees"),
    ("write-tree", "Create a tree object from the current index"),
];
