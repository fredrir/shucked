use shucked_indexer::Indexer;
use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};
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
    candidates_with_dialect(ShellDialect::Bash, main, helpers)
}

fn candidates_with_dialect(
    dialect: ShellDialect,
    main: &str,
    helpers: &[(&str, &str)],
) -> Vec<Option<PathBuf>> {
    let source = format!("#!/usr/bin/env bash\n{main}");
    let profile = ShellProfile::native(dialect);
    let parse = Parser::with_profile(&source, profile.clone()).parse();
    assert!(!parse.is_err(), "{source}");
    let indexer = Indexer::new(&source, &parse);
    let path = Path::new("/workspace/main.sh");
    let model = SemanticModel::build_with_options(
        &parse.file,
        &source,
        &indexer,
        SemanticBuildOptions {
            source_path: Some(path),
            shell_profile: Some(profile),
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
    let mut references = model.source_refs().iter().collect::<Vec<_>>();
    references.sort_by_key(|reference| reference.span.start.offset());
    references
        .into_iter()
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
        // Bash field-splits and globs an unquoted expansion, so an unquoted
        // `$ROOT_DIR/values.sh` is not one path even when the value is known
        // (here it contains a space). Zsh keeps such operands whole; see
        // `zsh_unquoted_operands_resolve_like_quoted_ones`.
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
fn zsh_unquoted_operands_resolve_like_quoted_ones() {
    // Zsh performs no field splitting or globbing on an unquoted parameter
    // expansion by default, so `$VAR/tail` names the same single file as
    // `"$VAR/tail"`, even when the value contains a space.
    let helpers = [("/workspace/paths.sh", "ROOT_DIR='/workspace/with spaces'\n")];
    for main in [
        "source paths.sh\nsource $ROOT_DIR/values.sh\n",
        "source paths.sh\nsource ${ROOT_DIR}/values.sh\n",
        "source paths.sh\n. $ROOT_DIR/values.sh\n",
        "source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
    ] {
        assert_eq!(
            candidates_with_dialect(ShellDialect::Zsh, main, &helpers),
            vec![Some(PathBuf::from("/workspace/with spaces/values.sh"))],
            "{main}"
        );
        assert_eq!(
            candidates_with_dialect(ShellDialect::Bash, main, &helpers),
            vec![
                Some(PathBuf::from("/workspace/with spaces/values.sh"))
                    .filter(|_| main.contains("\"$ROOT_DIR"))
            ],
            "{main}"
        );
    }
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
fn calls_to_imported_functions_update_known_path_values() {
    assert_eq!(
        candidates(
            "source paths.sh\nchange_root\nsource \"$ROOT_DIR/values.sh\"\n",
            &[(
                "/workspace/paths.sh",
                "ROOT_DIR=/workspace\nchange_root() { ROOT_DIR=/runtime; }\n"
            )],
        ),
        vec![Some(PathBuf::from("/runtime/values.sh"))]
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

#[test]
fn conditional_imports_resolve_inside_the_guard_without_exporting_uncertain_values() {
    for main in [
        "if enabled; then source paths.sh; source \"$ROOT_DIR/values.sh\"; fi\nsource \"$ROOT_DIR/after.sh\"\n",
        "enabled && { source paths.sh; source \"$ROOT_DIR/values.sh\"; }\nsource \"$ROOT_DIR/after.sh\"\n",
    ] {
        assert_eq!(
            candidates(
                main,
                &[
                    ("/workspace/paths.sh", "ROOT_DIR=/workspace\n"),
                    ("/workspace/values.sh", "# unchanged\n")
                ]
            ),
            vec![Some(PathBuf::from("/workspace/values.sh")), None],
            "{main}"
        );
    }
}

#[test]
fn branches_preserve_only_agreed_values_and_leave_unrelated_paths_intact() {
    for body in [
        "if enabled; then source paths.sh; else ROOT_DIR=/workspace; fi",
        "ROOT_DIR=/workspace; if enabled; then source paths.sh; fi",
        "ROOT_DIR=/workspace; if enabled; then UNRELATED=/other; fi; source paths.sh",
    ] {
        assert_eq!(
            candidates(
                &format!("{body}\nsource \"$ROOT_DIR/values.sh\"\n"),
                &[("/workspace/paths.sh", "ROOT_DIR=/workspace\n")]
            ),
            vec![Some(PathBuf::from("/workspace/values.sh"))],
            "{body}"
        );
    }
    assert_eq!(
        candidates(
            "ROOT_DIR=/old\nif enabled; then source paths.sh; fi\nsource \"$ROOT_DIR/values.sh\"\n",
            &[("/workspace/paths.sh", "ROOT_DIR=/new\n")]
        ),
        vec![None]
    );
}

#[test]
fn called_loaders_share_globals_and_restore_function_locals() {
    assert_eq!(
        candidates(
            "ROOT_DIR=/outside\nload() { local ROOT_DIR=/inside; source paths.sh; source \"$ROOT_DIR/local.sh\"; }\nload\nsource \"$ROOT_DIR/global.sh\"\nsource \"$LIB_DIR/values.sh\"\n",
            &[
                (
                    "/workspace/paths.sh",
                    "ROOT_DIR=/changed\nLIB_DIR=/library\n"
                ),
                ("/changed/local.sh", "# unchanged\n"),
                ("/outside/global.sh", "# unchanged\n")
            ]
        ),
        vec![
            Some(PathBuf::from("/changed/local.sh")),
            Some(PathBuf::from("/outside/global.sh")),
            Some(PathBuf::from("/library/values.sh"))
        ]
    );
}

#[test]
fn loader_arguments_and_nested_calls_resolve_at_the_call_site() {
    assert_eq!(
        candidates(
            "inner() { local ROOT_DIR=\"$1\"; source \"$ROOT_DIR/paths.sh\"; }\nouter() { inner /workspace; }\nouter\nsource \"$LIB_DIR/values.sh\"\n",
            &[("/workspace/paths.sh", "LIB_DIR=/library\n")]
        ),
        vec![
            Some(PathBuf::from("/workspace/paths.sh")),
            Some(PathBuf::from("/library/values.sh"))
        ]
    );
}

#[test]
fn conflicting_loader_calls_do_not_choose_one_source_target() {
    assert_eq!(
        candidates(
            "load() { source \"$1/paths.sh\"; }\nload /one\nload /two\nsource \"$ROOT_DIR/values.sh\"\n",
            &[
                ("/one/paths.sh", "ROOT_DIR=/one\n"),
                ("/two/paths.sh", "ROOT_DIR=/two\n")
            ]
        ),
        vec![None, Some(PathBuf::from("/two/values.sh"))]
    );
}

#[test]
fn uncalled_guarded_and_recursive_loaders_do_not_prove_exports() {
    for main in [
        "load() { source paths.sh; }; source \"$ROOT_DIR/values.sh\"\n",
        "load() { source paths.sh; }; enabled && load; source \"$ROOT_DIR/values.sh\"\n",
        "load() { source paths.sh; load; }; load; source \"$ROOT_DIR/values.sh\"\n",
    ] {
        assert_eq!(
            candidates(main, &[("/workspace/paths.sh", "ROOT_DIR=/workspace\n")]),
            vec![None],
            "{main}"
        );
    }
}

#[test]
fn guarded_source_chains_keep_values_within_the_executed_chain() {
    assert_eq!(
        candidates(
            "enabled && source paths.sh && source \"$ROOT_DIR/values.sh\"\nsource \"$ROOT_DIR/after.sh\"\n",
            &[
                ("/workspace/paths.sh", "ROOT_DIR=/workspace\n"),
                ("/workspace/values.sh", "# unchanged\n")
            ]
        ),
        vec![Some(PathBuf::from("/workspace/values.sh")), None]
    );
}

#[test]
fn uncertain_function_definitions_and_early_returns_do_not_leave_stale_exports() {
    for body in [
        "if enabled; then change() { ROOT_DIR=/other; }; fi\nROOT_DIR=/old\nchange\n",
        "change() { ROOT_DIR=/other; }\nunset -f change\nROOT_DIR=/old\nchange\n",
        "if enabled; then return; fi\nROOT_DIR=/old\n",
    ] {
        assert_eq!(
            candidates(
                "source paths.sh\nsource \"$ROOT_DIR/values.sh\"\n",
                &[("/workspace/paths.sh", body)]
            ),
            vec![None],
            "{body}"
        );
    }
}

#[test]
fn source_arguments_override_function_arguments_and_shift_invalidates_them() {
    assert_eq!(
        candidates(
            "load() { source paths.sh /actual; }; load /wrong\nsource \"$ROOT_DIR/values.sh\"\n",
            &[("/workspace/paths.sh", "ROOT_DIR=\"$1\"\n")]
        ),
        vec![Some(PathBuf::from("/actual/values.sh"))]
    );
    assert_eq!(
        candidates(
            "load() { shift; source \"$1/paths.sh\"; }; load /wrong /actual\nsource \"$ROOT_DIR/values.sh\"\n",
            &[("/wrong/paths.sh", "ROOT_DIR=/wrong\n")]
        ),
        vec![None, None]
    );
}

#[test]
fn case_imports_join_values_without_assuming_a_pattern_matches() {
    for (body, expected) in [
        (
            "case $mode in first) source paths.sh;; *) ROOT_DIR=/workspace;; esac",
            Some(PathBuf::from("/workspace/values.sh")),
        ),
        ("case $mode in first) source paths.sh;; esac", None),
        (
            "case $mode in first) source paths.sh;; *) ROOT_DIR=/other;; esac",
            None,
        ),
    ] {
        assert_eq!(
            candidates(
                &format!("{body}\nsource \"$ROOT_DIR/values.sh\"\n"),
                &[("/workspace/paths.sh", "ROOT_DIR=/workspace\n")]
            ),
            vec![expected],
            "{body}"
        );
    }
}

#[test]
fn conditional_local_declarations_do_not_hide_possible_global_writes() {
    assert_eq!(
        candidates(
            "ROOT_DIR=/old\nload() { if enabled; then local ROOT_DIR=/private; else ROOT_DIR=/new; fi; }\nload\nsource \"$ROOT_DIR/values.sh\"\n",
            &[]
        ),
        vec![None]
    );
}

#[test]
fn sourced_variables_shadowed_by_loader_locals_are_not_imported_at_file_scope() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("main.sh");
    std::fs::write(root.path().join("values.sh"), "value=shared\n").unwrap();
    for (local, expected) in [("", true), ("local value=private;", false)] {
        let source = format!("load() {{ {local} source ./values.sh; }}\nload\necho \"$value\"\n");
        let parse = Parser::new(&source).parse();
        assert!(!parse.is_err());
        let indexer = Indexer::new(&source, &parse);
        let model = SemanticModel::build_with_options(
            &parse.file,
            &source,
            &indexer,
            SemanticBuildOptions {
                source_path: Some(&path),
                ..Default::default()
            },
        );
        assert_eq!(
            model
                .bindings()
                .iter()
                .any(|binding| binding.name == "value"
                    && matches!(binding.kind, shucked_semantic::BindingKind::Imported)
                    && binding.scope == model.scope_at(0)),
            expected,
            "{source}"
        );
    }
}
