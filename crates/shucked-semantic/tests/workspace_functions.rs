use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shucked_indexer::Indexer;
use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};
use shucked_semantic::{
    CallFactSourceEdge, FileCallFacts, FileFunctionEffects, SemanticBuildOptions, SemanticModel,
    SourcePathAnalyzer, SourcePathFileProvider, WorkspaceFunctionIndex,
};

struct Files;
impl SourcePathFileProvider for Files {
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        vec![from.parent().unwrap().join(candidate)]
    }
    fn home_dir(&self) -> Option<PathBuf> {
        None
    }
}

fn workspace(root: &Path, sources: &[(&str, &str)]) -> WorkspaceFunctionIndex {
    for (name, source) in sources {
        std::fs::write(
            root.join(name),
            source.replace("ROOT", &root.display().to_string()),
        )
        .unwrap();
    }
    let mut files = BTreeMap::new();
    let mut analyzer = SourcePathAnalyzer::default();
    for (name, _) in sources {
        let path = root.join(name).canonicalize().unwrap();
        let source = std::fs::read_to_string(&path).unwrap();
        let profile = ShellProfile::native(ShellDialect::Zsh);
        let parsed = Parser::with_profile(&source, profile.clone()).parse();
        assert!(!parsed.is_err(), "{name}: {source}");
        let indexer = Indexer::new(&source, &parsed);
        let model = SemanticModel::build_with_options(
            &parsed.file,
            &source,
            &indexer,
            SemanticBuildOptions {
                source_path: Some(&path),
                shell_profile: Some(profile),
                resolve_source_closure: false,
                ..Default::default()
            },
        );
        let paths = analyzer.resolve(&model, &path, &Files);
        let edges = model
            .source_refs()
            .iter()
            .flat_map(|r| {
                let targets = paths.sequence(r).map(|p| p.to_vec()).unwrap_or_else(|| {
                    paths
                        .candidate(r)
                        .flatten()
                        .map(Path::to_path_buf)
                        .or_else(|| match &r.kind {
                            shucked_semantic::SourceRefKind::Literal(p)
                            | shucked_semantic::SourceRefKind::Directive(p) => {
                                Some(root.join(p.as_str()))
                            }
                            _ => None,
                        })
                        .into_iter()
                        .collect()
                });
                targets.into_iter().map(move |path| CallFactSourceEdge {
                    path: path.canonicalize().unwrap_or(path),
                    span: r.span,
                    conditional: r.conditionally_executed,
                    completion_visible: true,
                })
            })
            .collect();
        let calls = FileCallFacts::project_with_source_edges(&model, edges);
        files.insert(
            path,
            Arc::new(FileFunctionEffects::project(&model, &calls, &paths)),
        );
    }
    WorkspaceFunctionIndex::build(files, &|| false).unwrap()
}
fn calls<'a>(
    index: &'a WorkspaceFunctionIndex,
    file: &str,
) -> Vec<&'a shucked_semantic::WorkspaceFunctionCall> {
    index.calls().filter(|c| c.path.ends_with(file)).collect()
}

#[test]
fn ordered_modules_inherit_helpers_in_both_startup_branches() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "init.zsh",
                "if [[ -n $AGENT ]]; then source ROOT/02-utils.zsh; source ROOT/03-paths.zsh; return; fi\nfor module in ROOT/{0[2-9],[1-9][0-9]}-*.zsh(N); do source \"$module\"; done\n",
            ),
            ("02-utils.zsh", "add_path() { :; }\nhas_cmd() { :; }\n"),
            ("03-paths.zsh", "add_path /opt/tools\n"),
            ("30-aliases.zsh", "has_cmd editor && alias e=editor\n"),
        ],
    );
    for file in ["03-paths.zsh", "30-aliases.zsh"] {
        let sites = calls(&index, file);
        let target = sites[0]
            .resolution
            .exact()
            .expect("module must inherit the helper");
        assert!(target.path.ends_with("02-utils.zsh"));
        assert!(
            sites[0]
                .resolution
                .loaders
                .iter()
                .any(|p| p.ends_with("init.zsh"))
        );
    }
    let visible = index.visible(&root.path().join("03-paths.zsh").canonicalize().unwrap(), 2);
    assert!(visible.iter().any(|(name, _)| name == "add_path"));
}

#[test]
fn conditional_sources_keep_both_definitions_and_unknown_sources_keep_candidates() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "main.zsh",
                "if ready; then source ROOT/a.zsh; else source ROOT/b.zsh; fi\nhelper\nsource \"$PLUGIN\"\nhelper\nmissing_tool\n",
            ),
            ("a.zsh", "helper() { :; }\n"),
            ("b.zsh", "helper() { :; }\n"),
        ],
    );
    let sites = calls(&index, "main.zsh");
    let helpers = sites
        .iter()
        .filter(|c| !c.resolution.definitions.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(helpers.len(), 2);
    assert_eq!(helpers[0].resolution.definitions.len(), 2);
    assert!(!helpers[0].resolution.may_be_absent);
    assert!(!helpers[0].resolution.incomplete);
    assert!(helpers[1].resolution.incomplete);
    assert_eq!(helpers[1].resolution.definitions.len(), 2);
    assert!(sites.last().unwrap().resolution.incomplete);
}

#[test]
fn called_loader_functions_export_effects_but_uncalled_functions_do_not() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "main.zsh",
                "load() { source ROOT/helper.zsh; }\nhelper\nload\nhelper\n",
            ),
            ("helper.zsh", "helper() { :; }\n"),
        ],
    );
    let sites = calls(&index, "main.zsh");
    let before = sites.iter().find(|c| c.span.start.line() == 2).unwrap();
    assert!(before.resolution.definitions.is_empty());
    assert!(!before.resolution.incomplete);
    let after = sites.iter().find(|c| c.span.start.line() == 4).unwrap();
    assert!(after.resolution.exact().is_some());
}

#[test]
fn unrelated_definitions_clear_operations_and_command_bypasses_do_not_bind() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "main.zsh",
                "helper\nhelper() { :; }\ncommand helper\nenv helper\nbuiltin helper\nhelper\nunset -f helper\nhelper\n",
            ),
            ("unrelated.zsh", "helper() { :; }\n"),
        ],
    );
    for call in calls(&index, "main.zsh") {
        if call.span.start.line() == 6 {
            assert!(call.resolution.exact().is_some());
        } else {
            assert!(call.resolution.definitions.is_empty(), "{:?}", call);
        }
    }
}

#[test]
fn early_return_and_subshell_do_not_export_later_or_isolated_definitions() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "main.zsh",
                "source ROOT/helper.zsh\nhelper\n(isolated() { :; })\nisolated\n",
            ),
            ("helper.zsh", "return\nhelper() { :; }\n"),
        ],
    );
    assert!(
        calls(&index, "main.zsh")
            .iter()
            .all(|call| call.resolution.definitions.is_empty()),
        "{:?}",
        calls(&index, "main.zsh")
    );
}

#[test]
fn later_definitions_replace_earlier_bindings_and_cycles_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "main.zsh",
                "source ROOT/a.zsh\nsource ROOT/b.zsh\nhelper\nsource ROOT/main.zsh\nhelper\n",
            ),
            ("a.zsh", "helper() { :; }\n"),
            ("b.zsh", "helper() { :; }\n"),
        ],
    );
    assert!(
        calls(&index, "main.zsh")
            .last()
            .unwrap()
            .resolution
            .incomplete
    );
    for call in calls(&index, "main.zsh")
        .iter()
        .filter(|call| !call.resolution.definitions.is_empty())
    {
        assert!(
            call.resolution
                .definitions
                .iter()
                .all(|target| target.path.ends_with("b.zsh"))
        );
    }
}

#[test]
fn utility_loops_preserve_functions_and_short_circuit_loaders_preserve_context() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            (
                "main.zsh",
                "ready && source ROOT/a.zsh && source ROOT/b.zsh\n",
            ),
            (
                "a.zsh",
                "add_path() { local dir; for dir in \"$@\"; do [[ -d $dir ]] && path+=(\"$dir\"); done; }\nhelper() { :; }\n",
            ),
            ("b.zsh", "add_path /bin\nhelper\n"),
        ],
    );
    for call in calls(&index, "b.zsh") {
        assert!(call.resolution.exact().is_some(), "{call:?}");
    }
}

#[test]
fn multiple_loader_contexts_keep_distinct_bindings_without_importing_unrelated_names() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            ("one.zsh", "source ROOT/a.zsh\nsource ROOT/module.zsh\n"),
            ("two.zsh", "source ROOT/b.zsh\nsource ROOT/module.zsh\n"),
            ("a.zsh", "helper() { :; }\n"),
            ("b.zsh", "helper() { :; }\n"),
            ("module.zsh", "helper\n"),
            ("unrelated.zsh", "helper() { :; }\n"),
        ],
    );
    let sites = calls(&index, "module.zsh");
    assert_eq!(sites[0].resolution.definitions.len(), 2);
    assert!(!sites[0].resolution.may_be_absent);
    assert!(!sites[0].resolution.incomplete);
    assert!(
        sites[0]
            .resolution
            .definitions
            .iter()
            .all(|d| !d.path.ends_with("unrelated.zsh"))
    );
}

#[test]
fn source_null_glob_and_cancellation_do_not_invent_imports() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[(
            "main.zsh",
            "for file in ROOT/missing-*.zsh(N); do source \"$file\"; done\nmissing_helper\n",
        )],
    );
    let sites = calls(&index, "main.zsh");
    assert_eq!(sites.len(), 1);
    assert!(sites[0].resolution.definitions.is_empty());
    assert!(!sites[0].resolution.incomplete);
    assert!(WorkspaceFunctionIndex::build(BTreeMap::new(), &|| true).is_none());
}

#[test]
fn zsh_always_blocks_apply_function_effects_before_returning_to_the_caller() {
    let root = tempfile::tempdir().unwrap();
    let index = workspace(
        root.path(),
        &[
            ("main.zsh", "source ROOT/helper.zsh\nhelper\n"),
            ("helper.zsh", "{ return; } always { helper() { :; } }\n"),
        ],
    );
    assert!(calls(&index, "main.zsh")[0].resolution.exact().is_some());
}
