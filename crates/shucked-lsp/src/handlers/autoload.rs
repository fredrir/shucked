//! Autoload-aware navigation facts.
//!
//! A zsh `autoload -Uz name` declares `name` and reads its body from the
//! first file called `name` on `$fpath` when it is first invoked. The
//! semantic model records the declaration; this module projects, per indexed
//! file, the directories the file adds to `$fpath` and the `zle -N` widget
//! registrations and `bindkey` references, so the index can offer the
//! function file for a call, an `autoload` operand or a bound widget. The
//! host's `$fpath` is not captured, so the search covers the directories the
//! file family declares plus the conventional installation directories that
//! exist on this machine. Nothing is executed.

use std::path::{Path, PathBuf};

use shucked_ast::Span;
use shucked_parser::parser::Parser;
use shucked_semantic::{
    SemanticModel, SourcePathFileProvider, ZshWidgetFacts, zsh_function_path_assignments,
    zsh_widget_facts,
};

/// Upper bound on conventional function directories probed on the host.
const MAX_DEFAULT_DIRECTORIES: usize = 256;

/// A function declared by `autoload` in one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AutoloadDeclaration {
    pub(crate) name: String,
    /// The operand naming the function.
    pub(crate) name_span: Span,
    /// The whole `autoload` command.
    pub(crate) declaration_span: Span,
}

/// Zsh navigation facts projected from one file.
#[derive(Clone, Debug, Default)]
pub(crate) struct ZshNavigationFacts {
    pub(crate) autoloads: Vec<AutoloadDeclaration>,
    /// Directories the file names in `fpath`/`FPATH` assignments, in the
    /// order they appear, rendered statically.
    pub(crate) function_path: Vec<PathBuf>,
    pub(crate) widgets: ZshWidgetFacts,
}

fn mentions_navigation_statements(source: &str) -> bool {
    ["fpath", "FPATH", "zle ", "bindkey"]
        .iter()
        .any(|marker| source.contains(marker))
}

/// Projects the autoload declarations, `fpath` directories and widget facts
/// of `source`. `zdotdir` is the directory zsh reads startup files from for
/// this file, which seeds `$ZDOTDIR` in path templates.
pub(crate) fn project(
    model: &SemanticModel,
    source: &str,
    path: &Path,
    provider: &dyn SourcePathFileProvider,
    zdotdir: Option<&Path>,
) -> ZshNavigationFacts {
    let mut facts = ZshNavigationFacts::default();
    if model.shell_profile().dialect != shucked_parser::ShellDialect::Zsh {
        return facts;
    }
    facts.autoloads = model
        .autoloaded_function_bindings()
        .map(|binding| AutoloadDeclaration {
            name: binding.name.to_string(),
            name_span: binding.span,
            declaration_span: match binding.origin {
                shucked_semantic::BindingOrigin::FunctionDefinition { definition_span } => {
                    definition_span
                }
                _ => binding.span,
            },
        })
        .collect();
    if !mentions_navigation_statements(source) {
        return facts;
    }
    let parsed = Parser::with_profile(source, model.shell_profile().clone()).parse();
    let home = provider.home_dir();
    let mut seeds = Vec::new();
    if let Some(home) = &home {
        seeds.push(("HOME", home.as_path()));
    }
    if let Some(zdotdir) = zdotdir {
        seeds.push(("ZDOTDIR", zdotdir));
    }
    for assignment in
        zsh_function_path_assignments(&parsed.file, source, path, home.as_deref(), &seeds)
    {
        for directory in assignment.directories {
            if !facts.function_path.contains(&directory) {
                facts.function_path.push(directory);
            }
        }
    }
    facts.widgets = zsh_widget_facts(&parsed.file, source);
    facts
}

/// The conventional zsh function directories present on this host: the
/// site-function and vendor directories and the engine's own function trees
/// under the usual installation prefixes. Only directories that exist are
/// returned, so a machine without zsh contributes nothing.
pub(crate) fn default_function_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut push = |directory: PathBuf| {
        if directories.len() < MAX_DEFAULT_DIRECTORIES
            && directory.is_dir()
            && !directories.contains(&directory)
        {
            directories.push(directory);
        }
    };
    for prefix in [
        "/usr/local",
        "/opt/homebrew",
        "/opt/local",
        "/usr/pkg",
        "/usr",
    ] {
        let zsh = Path::new(prefix).join("share/zsh");
        push(zsh.join("site-functions"));
        push(zsh.join("vendor-functions"));
        push(zsh.join("vendor-completions"));
        if let Ok(entries) = std::fs::read_dir(&zsh) {
            let mut versions = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(|ch: char| ch.is_ascii_digit()))
                })
                .collect::<Vec<_>>();
            versions.sort();
            for version in versions {
                push(version.join("functions"));
            }
        }
        let mut pending = vec![(zsh.join("functions"), 0usize)];
        while let Some((directory, depth)) = pending.pop() {
            if !directory.is_dir() {
                continue;
            }
            push(directory.clone());
            if depth >= 4 {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&directory) {
                let mut children = entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect::<Vec<_>>();
                children.sort();
                pending.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
            }
        }
    }
    directories
}

/// The files named `name` in `directories`, in search order.
pub(crate) fn function_files(directories: &[PathBuf], name: &str) -> Vec<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return Vec::new();
    }
    directories
        .iter()
        .map(|directory| directory.join(name))
        .filter(|file| file.is_file())
        .collect()
}
