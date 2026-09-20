use shucked_indexer::Indexer;
use shucked_parser::parser::Parser;
use shucked_semantic::{
    SemanticBuildOptions, SemanticModel, SourcePathAnalyzer, SourcePathFileProvider, SourceRefKind,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Files(BTreeMap<PathBuf, String>);
impl SourcePathFileProvider for Files {
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        vec![from.parent().unwrap().join(candidate)]
    }
    fn is_file(&self, path: &Path) -> bool {
        self.0.contains_key(path)
    }
    fn read_source(&self, path: &Path) -> Option<String> {
        self.0.get(path).cloned()
    }
}

fn candidates(main: &str, helpers: &[(&str, &str)]) -> Vec<Option<PathBuf>> {
    let source = format!("#!/usr/bin/env bash\n{main}");
    let parse = Parser::new(&source).parse();
    assert!(!parse.is_err(), "{source}");
    let indexer = Indexer::new(&source, &parse);
    let path = Path::new("/workspace/main.sh");
    let model = SemanticModel::build_with_options(
        &parse.file,
        &source,
        &indexer,
        SemanticBuildOptions {
            source_path: Some(path),
            resolve_source_closure: false,
            ..Default::default()
        },
    );
    let files = Files(
        helpers
            .iter()
            .map(|(path, source)| {
                (
                    PathBuf::from(path),
                    format!("#!/usr/bin/env bash\n{source}"),
                )
            })
            .collect(),
    );
    let resolved = SourcePathAnalyzer::default().resolve(&model, path, &files);
    model
        .source_refs()
        .iter()
        .filter(|reference| {
            matches!(
                reference.kind,
                SourceRefKind::Dynamic | SourceRefKind::SingleVariableStaticTail { .. }
            )
        })
        .map(|reference| resolved.candidate(reference).flatten().map(PathBuf::from))
        .collect()
}

#[test]
fn imported_values_keep_the_helper_directory_and_support_caller_derivations() {
    assert_eq!(
        candidates(
            "source config/paths.sh\nLIB_DIR=\"$ROOT_DIR/lib one\"\nsource \"${LIB_DIR}/values.sh\"\n",
            &[(
                "/workspace/config/paths.sh",
                "SCRIPT_DIR=\"$(cd -- \"$(dirname -- \"${BASH_SOURCE[0]}\")\" && pwd)\"\nROOT_DIR=\"$(cd -- \"$SCRIPT_DIR/..\" && pwd)\"\n"
            )],
        ),
        vec![Some(PathBuf::from("/workspace/lib one/values.sh"))]
    );
}

#[test]
fn transitive_imports_and_caller_values_share_the_source_environment() {
    assert_eq!(
        candidates(
            "BASE=/workspace\nsource bridge.sh\nsource \"$LIB_DIR/values.sh\"\n",
            &[
                ("/workspace/bridge.sh", "source config/paths.sh\n"),
                ("/workspace/config/paths.sh", "LIB_DIR=\"$BASE/lib\"\n")
            ],
        ),
        vec![Some(PathBuf::from("/workspace/lib/values.sh"))]
    );
}

#[test]
fn later_imports_and_assignments_replace_previous_path_values() {
    assert_eq!(
        candidates(
            "ROOT_DIR=/old\nsource paths.sh\nsource \"$ROOT_DIR/first.sh\"\nROOT_DIR=/local\nsource \"$ROOT_DIR/second.sh\"\n",
            &[
                ("/workspace/paths.sh", "ROOT_DIR=/imported\n"),
                ("/imported/first.sh", "# unchanged\n")
            ],
        ),
        vec![
            Some(PathBuf::from("/imported/first.sh")),
            Some(PathBuf::from("/local/second.sh"))
        ]
    );
}

#[test]
fn uncertain_imports_and_writes_do_not_leak_stale_path_values() {
    for body in [
        "ROOT_DIR=\"$runtime\"\n",
        "unset ROOT_DIR\n",
        "if enabled; then ROOT_DIR=/other; fi\n",
        "if enabled; then unset ROOT_DIR; fi\n",
        "source \"$dynamic\"\n",
        "source missing.sh\n",
        "source paths.sh\n",
        "eval \"$code\"\n",
        "return\nROOT_DIR=/late\n",
    ] {
        assert_eq!(
            candidates(
                "ROOT_DIR=/old\nsource paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
                &[("/workspace/paths.sh", body)]
            ),
            vec![None],
            "{body}"
        );
    }
    assert_eq!(
        candidates(
            "if enabled; then source paths.sh; fi\nsource \"$ROOT_DIR/values.sh\"\n",
            &[("/workspace/paths.sh", "ROOT_DIR=/workspace\n")]
        ),
        vec![None]
    );
}

#[test]
fn helper_locals_and_subshell_assignments_do_not_escape() {
    assert_eq!(
        candidates(
            "source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
            &[(
                "/workspace/paths.sh",
                "ROOT_DIR=/workspace\nf() { local ROOT_DIR=/other; }\n(ROOT_DIR=/subshell)\n"
            )]
        ),
        vec![Some(PathBuf::from("/workspace/values.sh"))]
    );
}

#[test]
fn runtime_dependent_directory_commands_are_not_executed_or_guessed() {
    for body in [
        "ROOT_DIR=\"$(pwd)\"\n",
        "ROOT_DIR=\"$(arbitrary-command)\"\n",
        "ROOT_DIR=relative\n",
    ] {
        assert_eq!(
            candidates(
                "source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
                &[("/workspace/paths.sh", body)]
            ),
            vec![None]
        );
    }
}

#[test]
fn source_order_and_uncertain_execution_prevent_path_inference() {
    for main in [
        "source \"$ROOT_DIR/values.sh\"\nsource paths.sh\n",
        "ROOT_DIR=/old\nTEMP=value source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
        "source paths.sh\nsource $ROOT_DIR/values.sh\n",
    ] {
        assert_eq!(
            candidates(
                main,
                &[("/workspace/paths.sh", "ROOT_DIR='/workspace/with spaces'\n")]
            ),
            vec![None],
            "{main}"
        );
    }
    assert_eq!(
        candidates(
            "source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
            &[(
                "/workspace/paths.sh",
                "source paths.sh\nROOT_DIR=/unreachable\n"
            )]
        ),
        vec![None]
    );
}

#[test]
fn deep_import_chains_and_growing_path_values_are_bounded() {
    let owned = (0..40)
        .map(|index| {
            (
                format!("/workspace/path{index}.sh"),
                format!("source path{}.sh\nROOT_DIR=/unreachable\n", index + 1),
            )
        })
        .collect::<Vec<_>>();
    let helpers = owned
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        candidates(
            "source path0.sh\nsource \"$ROOT_DIR/values.sh\"\n",
            &helpers
        ),
        vec![None]
    );
    let mut body = String::from("source paths.sh\n");
    for _ in 0..40 {
        body.push_str("ROOT_DIR=\"$ROOT_DIR$ROOT_DIR\"\n");
    }
    body.push_str("source \"$ROOT_DIR/values.sh\"\n");
    assert_eq!(
        candidates(&body, &[("/workspace/paths.sh", "ROOT_DIR=/root\n")]),
        vec![None]
    );
}

#[test]
fn calls_to_imported_functions_invalidate_values_they_may_change() {
    assert_eq!(
        candidates(
            "source paths.sh\nchange_root\nsource \"$ROOT_DIR/values.sh\"\n",
            &[(
                "/workspace/paths.sh",
                "ROOT_DIR=/workspace\nchange_root() { ROOT_DIR=/runtime; }\n"
            )],
        ),
        vec![None]
    );
}

#[test]
fn exporting_a_known_path_without_reassigning_preserves_its_value() {
    assert_eq!(
        candidates(
            "source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
            &[(
                "/workspace/paths.sh",
                "ROOT_DIR=/workspace\nexport ROOT_DIR\n"
            )],
        ),
        vec![Some(PathBuf::from("/workspace/values.sh"))]
    );
}
