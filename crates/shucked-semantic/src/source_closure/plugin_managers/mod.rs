//! Zsh plugin manager adapters.
//!
//! A manager recognizes one family of zsh plugin/framework syntax and converts
//! it into source-closure data: logical plugin requests and deferred runtime
//! entrypoints. The source-closure engine consumes those common outputs without
//! knowing whether they came from Oh My Zsh, a custom manager, or generic zsh
//! runtime APIs.

mod generic_zsh_runtime;
mod oh_my_zsh;
mod prezto;
mod zdot;
mod zinit;

use super::*;
use crate::ZshPluginFramework;

pub(super) use oh_my_zsh::{dedup_plugin_requests, sorted_dependency_paths};

pub(super) struct PluginManagerContext<'a> {
    pub(super) model: &'a SemanticModel,
    pub(super) file: &'a File,
    pub(super) source: &'a str,
    pub(super) source_path: &'a Path,
    pub(super) plugin_resolver: &'a dyn PluginResolver,
    /// Home directory used to expand `~` and `$HOME` in framework paths.
    pub(super) home_dir: Option<PathBuf>,
}

pub(super) struct DeferredPluginRuntimeContext<'a> {
    pub(super) semantic: &'a SemanticModel,
    pub(super) analysis: &'a crate::SemanticAnalysis<'a>,
    pub(super) facts: &'a AstFacts,
    pub(super) source: &'a str,
    pub(super) scope: ScopeId,
    pub(super) synthetic_reads: &'a [SyntheticRead],
}

trait ZshPluginManager: ZshPluginFramework {
    fn is_active(&self, context: &PluginManagerContext<'_>) -> bool {
        context.model.shell_profile().dialect == ParseShellDialect::Zsh
    }

    fn collect_plugin_requests(&self, context: &PluginManagerContext<'_>) -> Vec<PluginRequest> {
        let _ = context;
        Vec::new()
    }
}

trait ZshDeferredRuntimeManager {
    fn collect_deferred_required_reads(
        &self,
        context: &DeferredPluginRuntimeContext<'_>,
    ) -> Vec<Name> {
        let _ = context;
        Vec::new()
    }
}

static OH_MY_ZSH_PLUGIN_MANAGER: oh_my_zsh::OhMyZshPluginManager = oh_my_zsh::OhMyZshPluginManager;
static PREZTO_PLUGIN_MANAGER: prezto::PreztoPluginManager = prezto::PreztoPluginManager;
static ZDOT_PLUGIN_MANAGER: zdot::ZdotPluginManager = zdot::ZdotPluginManager;
static ZINIT_PLUGIN_MANAGER: zinit::ZinitPluginManager = zinit::ZinitPluginManager;

static ZSH_PLUGIN_MANAGERS: [&dyn ZshPluginManager; 4] = [
    &OH_MY_ZSH_PLUGIN_MANAGER,
    &PREZTO_PLUGIN_MANAGER,
    &ZDOT_PLUGIN_MANAGER,
    &ZINIT_PLUGIN_MANAGER,
];

static ZSH_PLUGIN_FRAMEWORKS: [&dyn ZshPluginFramework; 4] = [
    &OH_MY_ZSH_PLUGIN_MANAGER,
    &PREZTO_PLUGIN_MANAGER,
    &ZDOT_PLUGIN_MANAGER,
    &ZINIT_PLUGIN_MANAGER,
];

/// Returns all built-in zsh plugin framework implementations.
pub fn zsh_plugin_frameworks() -> &'static [&'static dyn ZshPluginFramework] {
    &ZSH_PLUGIN_FRAMEWORKS
}

/// Returns the built-in implementation for a framework, when Shucked knows one.
pub fn layout_for_plugin_framework(
    framework: &PluginFramework,
) -> Option<&'static dyn ZshPluginFramework> {
    zsh_plugin_frameworks()
        .iter()
        .copied()
        .find(|layout| &layout.framework() == framework)
}

pub(super) fn collect_plugin_requests(
    model: &SemanticModel,
    file: &File,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
    plugin_resolver: &dyn PluginResolver,
) -> Vec<PluginRequest> {
    if model.shell_profile().dialect != ParseShellDialect::Zsh {
        return Vec::new();
    }

    let context = PluginManagerContext {
        model,
        file,
        source,
        source_path,
        plugin_resolver,
        home_dir: home_dir.map(Path::to_path_buf),
    };
    let mut requests = Vec::new();
    for manager in ZSH_PLUGIN_MANAGERS {
        if manager.is_active(&context) {
            requests.extend(manager.collect_plugin_requests(&context));
        }
    }
    let mut seen = requests
        .iter()
        .map(plugin_request_dependency_key)
        .collect::<FxHashSet<_>>();
    let mut index = 0;
    while index < requests.len() {
        let request = requests[index].clone();
        if let Some(manager) = manager_for_plugin_framework(&request.framework) {
            for dependency in manager.dependent_plugin_requests(&request) {
                if seen.insert(plugin_request_dependency_key(&dependency)) {
                    requests.push(dependency);
                }
            }
        }
        index += 1;
    }
    dedup_plugin_requests(requests)
}

/// The zsh plugin and framework loads a file requests, for consumers that
/// index files outside the linter's source closure (the language server).
///
/// Requests are anchored at the semantic model's recorded command spans, so a
/// consumer can attach the resolved entrypoints to the same statement the
/// model's source references and command sites use. `home_dir` expands `~`
/// and `$HOME` in framework paths; the resolver contributes configured loads
/// and the layout roots.
pub fn zsh_plugin_requests(
    model: &SemanticModel,
    file: &File,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
    plugin_resolver: &dyn PluginResolver,
) -> Vec<PluginRequest> {
    let mut requests =
        collect_plugin_requests(model, file, source, source_path, home_dir, plugin_resolver);
    for request in &mut requests {
        request.span = recorded_linear_span(model, request.span);
    }
    requests
}

/// A statement that loads a plugin framework's bootstrap file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZshFrameworkBootstrap {
    /// Framework whose bootstrap the statement sources.
    pub framework: PluginFramework,
    /// Recorded span of the `source` statement.
    pub span: Span,
    /// Framework root inferred from the file's own assignments, if any.
    pub root_hint: Option<PathBuf>,
}

/// The framework bootstrap statements of `file` (`source $ZSH/oh-my-zsh.sh`
/// and its spellings), anchored like [`zsh_plugin_requests`], whether or not
/// the file also selects plugins or a theme.
pub fn zsh_framework_bootstraps(
    model: &SemanticModel,
    file: &File,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
) -> Vec<ZshFrameworkBootstrap> {
    if model.shell_profile().dialect != ParseShellDialect::Zsh {
        return Vec::new();
    }
    let mut bootstraps = oh_my_zsh::bootstraps(file, source, source_path, home_dir);
    for bootstrap in &mut bootstraps {
        bootstrap.span = recorded_linear_span(model, bootstrap.span);
    }
    bootstraps
}

/// The recorded span of the simple command starting where `span` starts,
/// or `span` itself when the model recorded no such command.
fn recorded_linear_span(model: &SemanticModel, span: Span) -> Span {
    model
        .recorded_program()
        .commands()
        .iter()
        .find(|command| {
            matches!(command.kind, crate::cfg::RecordedCommandKind::Linear)
                && command.span.start.offset() == span.start.offset()
        })
        .map_or(span, |command| command.span)
}

/// Static values of framework path variables (`ZSH`, `ZSH_CUSTOM`,
/// `ZPREZTODIR`, ...) assigned at the top level of `file`, rendered against
/// `home_dir` and the file's location, in the order they are assigned.
///
/// Only assignments whose value is a literal path, a `~`/`$HOME` anchored
/// path, or a template over variables assigned earlier in the same file are
/// rendered; anything else leaves the variable out.
pub fn zsh_framework_path_variables(
    file: &File,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
    names: &[&str],
) -> std::collections::BTreeMap<String, PathBuf> {
    oh_my_zsh::static_path_variables(file, source, source_path, home_dir, names)
}

fn plugin_request_dependency_key(
    request: &PluginRequest,
) -> (
    PluginFramework,
    PluginRequestKind,
    String,
    usize,
    Option<PathBuf>,
) {
    (
        request.framework.clone(),
        request.kind,
        request.name.clone(),
        request.span.start.offset(),
        request.root_hint.clone(),
    )
}

fn manager_for_plugin_framework(
    framework: &PluginFramework,
) -> Option<&'static dyn ZshPluginManager> {
    ZSH_PLUGIN_MANAGERS
        .iter()
        .copied()
        .find(|manager| &manager.framework() == framework)
}

pub(super) fn deferred_zsh_entrypoint_required_reads(
    semantic: &SemanticModel,
    analysis: &crate::SemanticAnalysis<'_>,
    facts: &AstFacts,
    source: &str,
    scope: ScopeId,
    synthetic_reads: &[SyntheticRead],
) -> Vec<Name> {
    if semantic.shell_profile().dialect != ParseShellDialect::Zsh {
        return Vec::new();
    }

    let context = DeferredPluginRuntimeContext {
        semantic,
        analysis,
        facts,
        source,
        scope,
        synthetic_reads,
    };
    let managers: [&dyn ZshDeferredRuntimeManager; 1] =
        [&generic_zsh_runtime::GenericZshRuntimeManager];

    let mut reads = Vec::new();
    for manager in managers {
        reads.extend(manager.collect_deferred_required_reads(&context));
    }
    reads.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    reads.dedup();
    reads
}

fn static_command_args(command: &SimpleCommand, source: &str) -> Option<Vec<String>> {
    command
        .args
        .iter()
        .map(|arg| static_word_text(arg, source).map(|text| text.into_owned()))
        .collect()
}

fn static_plugin_names<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut plugins = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() || name.contains('/') || plugins.iter().any(|plugin| plugin == name) {
            continue;
        }
        plugins.push(name.to_owned());
    }
    plugins
}

fn suffix_after_last_marker<const N: usize>(path: &str, markers: [&str; N]) -> Option<PathBuf> {
    for marker in markers {
        if let Some(index) = path.rfind(marker) {
            let suffix = &path[index + 1..];
            if !suffix.is_empty() {
                return Some(PathBuf::from(suffix));
            }
        }
    }
    None
}

fn path_text_starts_with_path(path: &str, root: &Path) -> bool {
    let root_text = root.to_string_lossy().replace('\\', "/");
    path == root_text
        || path
            .strip_prefix(&root_text)
            .is_some_and(|tail| tail.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoPlugins;

    impl PluginResolver for NoPlugins {
        fn resolve_plugin_request(
            &self,
            _source_path: &Path,
            _request: &PluginRequest,
        ) -> crate::PluginResolution {
            crate::PluginResolution::default()
        }
    }

    #[test]
    fn bootstraps_and_requests_are_anchored_at_recorded_command_spans() {
        let source = "export ZSH=\"$HOME/.oh-my-zsh\"\nplugins=(git fzf)\nZSH_THEME=agnoster\nsource $ZSH/oh-my-zsh.sh;\n";
        let profile = ShellProfile::native(ParseShellDialect::Zsh);
        let parsed = Parser::with_profile(source, profile.clone()).parse();
        let indexer = Indexer::new(source, &parsed);
        let model = crate::SemanticModel::build_with_options(
            &parsed.file,
            source,
            &indexer,
            crate::SemanticBuildOptions {
                source_path: Some(Path::new("/tmp/project/.zshrc")),
                shell_profile: Some(profile),
                resolve_source_closure: false,
                ..Default::default()
            },
        );
        let home = Path::new("/home/me");
        let bootstrap_span = model.source_refs()[0].span;

        let bootstraps = zsh_framework_bootstraps(
            &model,
            &parsed.file,
            source,
            Path::new("/tmp/project/.zshrc"),
            Some(home),
        );
        assert_eq!(bootstraps.len(), 1);
        assert_eq!(bootstraps[0].framework, PluginFramework::OhMyZsh);
        assert_eq!(bootstraps[0].span, bootstrap_span);
        assert_eq!(
            bootstraps[0].root_hint.as_deref(),
            Some(Path::new("/home/me/.oh-my-zsh"))
        );

        let requests = zsh_plugin_requests(
            &model,
            &parsed.file,
            source,
            Path::new("/tmp/project/.zshrc"),
            Some(home),
            &NoPlugins,
        );
        let mut names = requests
            .iter()
            .map(|request| (request.name.as_str(), request.kind))
            .collect::<Vec<_>>();
        names.sort_by_key(|(name, _)| *name);
        assert_eq!(
            names,
            vec![
                ("agnoster", PluginRequestKind::Theme),
                ("fzf", PluginRequestKind::Plugin),
                ("git", PluginRequestKind::Plugin),
            ]
        );
        assert!(
            requests
                .iter()
                .all(|request| request.span == bootstrap_span)
        );

        let variables = zsh_framework_path_variables(
            &parsed.file,
            source,
            Path::new("/tmp/project/.zshrc"),
            Some(home),
            &["ZSH", "ZSH_CUSTOM"],
        );
        assert_eq!(
            variables.get("ZSH").map(PathBuf::as_path),
            Some(Path::new("/home/me/.oh-my-zsh"))
        );
        assert!(!variables.contains_key("ZSH_CUSTOM"));
    }

    #[test]
    fn dependency_dedup_key_preserves_request_anchor() {
        let mut first = PluginRequest {
            framework: PluginFramework::Other("zsh-autosuggestions".to_owned()),
            kind: PluginRequestKind::Plugin,
            name: "zsh-autosuggestions".to_owned(),
            span: Span::new(),
            explicit: false,
            root_hint: None,
        };
        let mut second = first.clone();
        first.span.start =
            shucked_ast::Position::at(first.span.start.line(), first.span.start.column(), 10);
        second.span.start =
            shucked_ast::Position::at(second.span.start.line(), second.span.start.column(), 20);

        assert_ne!(
            plugin_request_dependency_key(&first),
            plugin_request_dependency_key(&second)
        );
    }
}
