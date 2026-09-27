//! Plugin-framework loads for the workspace index.
//!
//! `source $ZSH/oh-my-zsh.sh`, `plugins=(git ...)`, `ZSH_THEME=...`,
//! `zstyle ':prezto:load' pmodule ...` and `pmodload ...` bring files into
//! the shell that no `source` operand names. The semantic crate recognises
//! those statements and knows each framework's layout; this module resolves
//! them against the installation on disk so the index can follow them like
//! any other source edge. Nothing is executed: only file and directory
//! metadata is consulted, through the same provider the rest of the index
//! reads with, and every load is bounded in file count and file size.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use shucked_ast::Span;
use shucked_parser::parser::Parser;
use shucked_semantic::{
    PluginFramework, PluginRequest, PluginRequestKind, PluginResolution, PluginResolver,
    SemanticModel, SourcePathFileProvider, layout_for_plugin_framework, zsh_framework_bootstraps,
    zsh_framework_path_variables, zsh_plugin_requests,
};

/// Upper bound on files one framework load contributes to the index.
pub(crate) const MAX_FRAMEWORK_FILES: usize = 64;
/// Framework files larger than this are left out of a load.
pub(crate) const MAX_FRAMEWORK_FILE_BYTES: u64 = 1024 * 1024;

/// Files a statement loads through a plugin framework.
#[derive(Debug, Clone)]
pub(crate) struct FrameworkLoad {
    pub(crate) framework: PluginFramework,
    /// Recorded span of the loading statement.
    pub(crate) span: Span,
    /// Files in load order; a framework bootstrap file comes first.
    pub(crate) files: Vec<PathBuf>,
    /// Paths and directories whose state decided `files`, for cache
    /// invalidation.
    pub(crate) dependencies: Vec<PathBuf>,
    /// Whether the sequence was cut at [`MAX_FRAMEWORK_FILES`].
    pub(crate) truncated: bool,
}

/// Cheap textual gate: parsing for framework loads only pays off when the
/// file spells one of the recognised statements.
fn mentions_framework(source: &str) -> bool {
    [
        "oh-my-zsh.sh",
        "pmodload",
        ":prezto:load",
        "zdot_load_module",
        "zdot_use_plugin",
    ]
    .iter()
    .any(|marker| source.contains(marker))
}

/// The framework whose own bootstrap file `path` is.
///
/// A bootstrap's internal loads iterate over `$plugins` and glob the
/// framework's directories, which no static analysis can follow; the files
/// they bring in are attached to the statement that sources the bootstrap
/// instead (see [`framework_loads`]). Detection is by layout rather than by
/// configuration so the answer is the same across builds.
pub(crate) fn bootstrap_file_framework(path: &Path) -> Option<PluginFramework> {
    let name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    match name {
        "oh-my-zsh.sh" if parent.join("lib").is_dir() && parent.join("plugins").is_dir() => {
            Some(PluginFramework::OhMyZsh)
        }
        "init.zsh" if parent.join("modules").is_dir() && parent.join("runcoms").is_dir() => {
            Some(PluginFramework::Prezto)
        }
        _ => None,
    }
}

/// Human-readable framework name for hover text.
pub(crate) fn framework_label(framework: &PluginFramework) -> String {
    match framework {
        PluginFramework::OhMyZsh => "oh-my-zsh".into(),
        PluginFramework::Prezto => "prezto".into(),
        PluginFramework::Zdot => "zdot".into(),
        PluginFramework::Zinit => "zinit".into(),
        PluginFramework::ExplicitFilesystem => "configured entrypoint".into(),
        PluginFramework::Other(name) => name.clone(),
    }
}

/// Every framework load in `source`, resolved against the installation the
/// provider sees. Empty for non-zsh files and for files without framework
/// statements.
pub(crate) fn framework_loads(
    model: &SemanticModel,
    source: &str,
    path: &Path,
    provider: &dyn SourcePathFileProvider,
) -> Vec<FrameworkLoad> {
    if model.shell_profile().dialect != shucked_parser::ShellDialect::Zsh
        || !mentions_framework(source)
    {
        return Vec::new();
    }
    let parsed = Parser::with_profile(source, model.shell_profile().clone()).parse();
    let home = provider.home_dir();
    let variables = zsh_framework_path_variables(
        &parsed.file,
        source,
        path,
        home.as_deref(),
        &["ZSH", "ZSH_CUSTOM", "ZPREZTODIR", "ZDOTDIR"],
    );
    let resolver = FrameworkResolver {
        provider,
        home,
        variables,
    };
    let mut requests = BTreeMap::<usize, Vec<PluginRequest>>::new();
    for request in zsh_plugin_requests(
        model,
        &parsed.file,
        source,
        path,
        resolver.home.as_deref(),
        &resolver,
    ) {
        requests
            .entry(request.span.start.offset())
            .or_default()
            .push(request);
    }

    let mut loads = Vec::new();
    for bootstrap in
        zsh_framework_bootstraps(model, &parsed.file, source, path, resolver.home.as_deref())
    {
        let anchored = requests
            .remove(&bootstrap.span.start.offset())
            .unwrap_or_default();
        let Some(mut load) = resolver.bootstrap_load(
            &bootstrap.framework,
            bootstrap.root_hint.as_deref(),
            bootstrap.span,
        ) else {
            continue;
        };
        for request in &anchored {
            let (entrypoint, candidates) = resolver.entrypoint(request);
            load.dependencies.extend(candidates);
            load.files.extend(entrypoint);
        }
        // `$ZSH_CUSTOM/*.zsh` runs after plugins and the theme.
        if bootstrap.framework == PluginFramework::OhMyZsh
            && let Some(custom) = resolver
                .oh_my_zsh_root(bootstrap.root_hint.as_deref())
                .map(|root| resolver.oh_my_zsh_custom(&root))
        {
            load.dependencies.push(custom.clone());
            load.files.extend(
                resolver
                    .zsh_files_in(&custom)
                    .into_iter()
                    .filter(|file| file.parent() == Some(custom.as_path())),
            );
        }
        loads.push(load);
    }
    for (_, anchored) in requests {
        let mut load = None::<FrameworkLoad>;
        for request in &anchored {
            let (entrypoint, candidates) = resolver.entrypoint(request);
            if entrypoint.is_none() && candidates.is_empty() {
                continue;
            }
            let load = load.get_or_insert_with(|| FrameworkLoad {
                framework: request.framework.clone(),
                span: request.span,
                files: Vec::new(),
                dependencies: Vec::new(),
                truncated: false,
            });
            load.dependencies.extend(candidates);
            load.files.extend(entrypoint);
        }
        if let Some(load) = load.filter(|load| !load.files.is_empty()) {
            loads.push(load);
        }
    }
    for load in &mut loads {
        resolver.finish(load);
    }
    loads.sort_by_key(|load| load.span.start.offset());
    loads
}

struct FrameworkResolver<'a> {
    provider: &'a dyn SourcePathFileProvider,
    home: Option<PathBuf>,
    variables: BTreeMap<String, PathBuf>,
}

impl FrameworkResolver<'_> {
    fn environment_path(&self, name: &str) -> Option<PathBuf> {
        self.provider
            .environment_variable(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    fn zdotdir(&self) -> Option<PathBuf> {
        self.environment_path("ZDOTDIR")
            .or_else(|| self.variables.get("ZDOTDIR").cloned())
            .or_else(|| self.home.clone())
    }

    /// The oh-my-zsh installation: the file's own `ZSH`, then the
    /// environment's, then `~/.oh-my-zsh`; only a root that holds the
    /// bootstrap counts.
    fn oh_my_zsh_root(&self, hint: Option<&Path>) -> Option<PathBuf> {
        [
            hint.map(Path::to_path_buf),
            self.variables.get("ZSH").cloned(),
            self.environment_path("ZSH"),
            self.home.as_ref().map(|home| home.join(".oh-my-zsh")),
        ]
        .into_iter()
        .flatten()
        .find(|root| self.provider.is_file(&root.join("oh-my-zsh.sh")))
    }

    fn oh_my_zsh_custom(&self, root: &Path) -> PathBuf {
        self.variables
            .get("ZSH_CUSTOM")
            .cloned()
            .or_else(|| self.environment_path("ZSH_CUSTOM"))
            .unwrap_or_else(|| root.join("custom"))
    }

    fn prezto_root(&self, hint: Option<&Path>) -> Option<PathBuf> {
        [
            hint.map(Path::to_path_buf),
            self.variables.get("ZPREZTODIR").cloned(),
            self.environment_path("ZPREZTODIR"),
            self.zdotdir().map(|directory| directory.join(".zprezto")),
            self.home.as_ref().map(|home| home.join(".zprezto")),
        ]
        .into_iter()
        .flatten()
        .find(|root| self.provider.is_file(&root.join("init.zsh")))
    }

    /// `*.zsh` files directly in `directory`, sorted; also files in its
    /// subdirectories when the provider lists them (it never recurses).
    fn zsh_files_in(&self, directory: &Path) -> Vec<PathBuf> {
        let mut files = self
            .provider
            .directory_entries(directory)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| {
                entry
                    .extension()
                    .is_some_and(|extension| extension == "zsh")
                    && self.provider.is_file(entry)
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    /// The files a framework bootstrap brings in before plugins and themes.
    fn bootstrap_load(
        &self,
        framework: &PluginFramework,
        hint: Option<&Path>,
        span: Span,
    ) -> Option<FrameworkLoad> {
        match framework {
            PluginFramework::OhMyZsh => {
                let root = self.oh_my_zsh_root(hint)?;
                let custom = self.oh_my_zsh_custom(&root);
                let lib = root.join("lib");
                let custom_lib = custom.join("lib");
                let mut files = vec![root.join("oh-my-zsh.sh")];
                let mut dependencies =
                    vec![root.join("oh-my-zsh.sh"), lib.clone(), custom_lib.clone()];
                // Each `lib/*.zsh` is replaced by a same-named file under
                // `$ZSH_CUSTOM/lib` when one exists.
                for file in self.zsh_files_in(&lib) {
                    let Some(name) = file.file_name() else {
                        continue;
                    };
                    let custom_file = custom_lib.join(name);
                    dependencies.push(custom_file.clone());
                    files.push(if self.provider.is_file(&custom_file) {
                        custom_file
                    } else {
                        file
                    });
                }
                Some(FrameworkLoad {
                    framework: framework.clone(),
                    span,
                    files,
                    dependencies,
                    truncated: false,
                })
            }
            _ => None,
        }
    }

    /// The entrypoint a plugin or theme request resolves to, plus every
    /// candidate consulted (first match wins, so a candidate appearing later
    /// changes the answer).
    fn entrypoint(&self, request: &PluginRequest) -> (Option<PathBuf>, Vec<PathBuf>) {
        let candidates: Vec<PathBuf> = match (&request.framework, request.kind) {
            (PluginFramework::OhMyZsh, PluginRequestKind::Plugin) => {
                let Some(root) = self.oh_my_zsh_root(request.root_hint.as_deref()) else {
                    return (None, Vec::new());
                };
                let custom = self.oh_my_zsh_custom(&root);
                let layout = layout_for_plugin_framework(&request.framework);
                [custom, root]
                    .iter()
                    .filter_map(|root| {
                        layout.and_then(|layout| {
                            layout.resolve_entrypoint(root, request.kind, &request.name)
                        })
                    })
                    .collect()
            }
            (PluginFramework::OhMyZsh, PluginRequestKind::Theme) => {
                let Some(root) = self.oh_my_zsh_root(request.root_hint.as_deref()) else {
                    return (None, Vec::new());
                };
                let custom = self.oh_my_zsh_custom(&root);
                let theme = format!("{}.zsh-theme", request.name);
                vec![
                    custom.join("themes").join(&theme),
                    custom.join(&theme),
                    root.join("themes").join(&theme),
                ]
            }
            (PluginFramework::Prezto, PluginRequestKind::Plugin) => {
                let Some(root) = self.prezto_root(request.root_hint.as_deref()) else {
                    return (None, Vec::new());
                };
                layout_for_plugin_framework(&request.framework)
                    .and_then(|layout| {
                        layout.resolve_entrypoint(&root, request.kind, &request.name)
                    })
                    .into_iter()
                    .collect()
            }
            (PluginFramework::ExplicitFilesystem, PluginRequestKind::Entrypoint) => {
                vec![PathBuf::from(&request.name)]
            }
            // zinit and zdot keep their plugins under manager-specific stores
            // that the layouts do not describe, and custom frameworks have no
            // known root.
            _ => Vec::new(),
        };
        let candidates = candidates
            .into_iter()
            .filter(|candidate| candidate.is_absolute())
            .collect::<Vec<_>>();
        let entrypoint = candidates
            .iter()
            .find(|candidate| self.provider.is_file(candidate))
            .cloned();
        (entrypoint, candidates)
    }

    /// Applies the file-count and file-size bounds and removes repeats.
    fn finish(&self, load: &mut FrameworkLoad) {
        let mut seen = std::collections::BTreeSet::new();
        let mut kept = Vec::new();
        for file in std::mem::take(&mut load.files) {
            if !seen.insert(file.clone()) {
                continue;
            }
            if std::fs::metadata(&file)
                .is_ok_and(|metadata| metadata.len() > MAX_FRAMEWORK_FILE_BYTES)
            {
                tracing::debug!(
                    "workspace functions: framework file {} exceeds the size limit",
                    file.display()
                );
                continue;
            }
            if kept.len() >= MAX_FRAMEWORK_FILES {
                load.truncated = true;
                break;
            }
            kept.push(file);
        }
        if load.truncated {
            tracing::debug!(
                "workspace functions: framework load at offset {} cut at {MAX_FRAMEWORK_FILES} files",
                load.span.start.offset()
            );
        }
        load.files = kept;
        load.dependencies.sort();
        load.dependencies.dedup();
    }
}

impl PluginResolver for FrameworkResolver<'_> {
    fn resolve_plugin_request(
        &self,
        _source_path: &Path,
        request: &PluginRequest,
    ) -> PluginResolution {
        PluginResolution {
            entrypoints: self.entrypoint(request).0.into_iter().collect(),
            ..PluginResolution::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_files_are_recognised_by_layout() {
        let root = tempfile::tempdir().unwrap();
        let omz = root.path().join(".oh-my-zsh");
        std::fs::create_dir_all(omz.join("lib")).unwrap();
        std::fs::create_dir_all(omz.join("plugins")).unwrap();
        std::fs::write(omz.join("oh-my-zsh.sh"), "").unwrap();
        assert_eq!(
            bootstrap_file_framework(&omz.join("oh-my-zsh.sh")),
            Some(PluginFramework::OhMyZsh)
        );
        let prezto = root.path().join(".zprezto");
        std::fs::create_dir_all(prezto.join("modules")).unwrap();
        std::fs::create_dir_all(prezto.join("runcoms")).unwrap();
        std::fs::write(prezto.join("init.zsh"), "").unwrap();
        assert_eq!(
            bootstrap_file_framework(&prezto.join("init.zsh")),
            Some(PluginFramework::Prezto)
        );
        // A same-named file outside a framework tree is an ordinary script.
        std::fs::write(root.path().join("init.zsh"), "").unwrap();
        assert_eq!(
            bootstrap_file_framework(&root.path().join("init.zsh")),
            None
        );
        std::fs::write(root.path().join("oh-my-zsh.sh"), "").unwrap();
        assert_eq!(
            bootstrap_file_framework(&root.path().join("oh-my-zsh.sh")),
            None
        );
    }
}
