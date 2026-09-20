use std::path::{Path, PathBuf};

use shucked_indexer::Indexer;
use shucked_parser::parser::Parser;
use shucked_semantic::{
    BindingAttributes, CallFactSourceEdge, FileCallFacts, SemanticBuildOptions, SemanticModel,
    SourceRefKind, WorkspaceVariableIndex, WorkspaceVariableUsage,
};

fn model(source: &str, usage: Option<&WorkspaceVariableUsage>) -> SemanticModel {
    let parse = Parser::new(source).parse();
    assert!(!parse.is_err(), "{source}");
    let indexer = Indexer::new(source, &parse);
    SemanticModel::build_with_options(
        &parse.file,
        source,
        &indexer,
        SemanticBuildOptions {
            source_path: Some(Path::new("/workspace/values.sh")),
            workspace_variable_usage: usage,
            resolve_source_closure: false,
            ..Default::default()
        },
    )
}

fn usage(files: &[(&str, &str)]) -> WorkspaceVariableUsage {
    let mut index = WorkspaceVariableIndex::default();
    for (name, source) in files {
        let model = model(source, None);
        let edges = model
            .source_refs()
            .iter()
            .filter_map(|reference| {
                let SourceRefKind::Literal(target) = &reference.kind else {
                    return None;
                };
                Some(CallFactSourceEdge {
                    path: PathBuf::from("/workspace").join(target.as_str()),
                    span: reference.span,
                    conditional: reference.conditionally_executed,
                    completion_visible: false,
                })
            })
            .collect();
        let calls = FileCallFacts::project_with_source_edges(&model, edges);
        index.insert(
            PathBuf::from("/workspace").join(name),
            &model,
            &calls.source_effects,
        );
    }
    index.usage(&|| false).unwrap()
}

fn consumed_offsets(usage: &WorkspaceVariableUsage, name: &str) -> Vec<usize> {
    usage
        .consumed_bindings(&PathBuf::from("/workspace").join(name))
        .into_iter()
        .map(|binding| binding.start)
        .collect()
}

#[test]
fn usage_marks_the_exact_assignment_in_the_linted_model() {
    let source = "VALUE=old\nVALUE=current\n";
    let usage = usage(&[
        ("values.sh", source),
        ("main.sh", "source values.sh\necho \"$VALUE\"\n"),
    ]);
    assert_eq!(consumed_offsets(&usage, "values.sh"), vec![10]);
    let model = model(source, Some(&usage));
    let bindings = model
        .bindings()
        .iter()
        .filter(|binding| binding.name == "VALUE")
        .collect::<Vec<_>>();
    assert_eq!(bindings.len(), 2);
    assert!(
        !bindings[0]
            .attributes
            .contains(BindingAttributes::WORKSPACE_CONSUMED)
    );
    assert!(
        bindings[1]
            .attributes
            .contains(BindingAttributes::WORKSPACE_CONSUMED)
    );
    assert!(
        model
            .analysis()
            .unused_assignments()
            .contains(&bindings[0].id)
    );
    assert!(
        !model
            .analysis()
            .unused_assignments()
            .contains(&bindings[1].id)
    );
}

#[test]
fn reads_at_different_source_sites_can_consume_both_assignments() {
    let source = "VALUE=first\nsource reader.sh\nVALUE=last\nsource reader.sh\n";
    let usage = usage(&[("values.sh", source), ("reader.sh", "echo \"$VALUE\"\n")]);
    assert_eq!(
        consumed_offsets(&usage, "values.sh"),
        vec![0, source.find("VALUE=last").unwrap()]
    );
}

#[test]
fn a_sourced_unset_is_distinct_from_a_file_that_leaves_the_value_unchanged() {
    for (reset, expected) in [("# unchanged\n", vec![0]), ("unset VALUE\n", vec![])] {
        let usage = usage(&[
            ("values.sh", "VALUE=old\n"),
            ("reset.sh", reset),
            ("bridge.sh", "source reset.sh\n"),
            (
                "main.sh",
                "source values.sh\nsource bridge.sh\necho \"$VALUE\"\n",
            ),
        ]);
        assert_eq!(consumed_offsets(&usage, "values.sh"), expected, "{reset}");
    }
}

#[test]
fn cleared_inherited_values_do_not_reach_a_sourced_reader() {
    let usage = usage(&[
        ("main.sh", "VALUE=old\nunset VALUE\nsource reader.sh\n"),
        ("reader.sh", "echo \"$VALUE\"\n"),
    ]);
    assert!(consumed_offsets(&usage, "main.sh").is_empty());
}

#[test]
fn conditional_writes_retain_each_possible_reaching_assignment() {
    let source = "VALUE=default\nif enabled; then VALUE=other; fi\n";
    let usage = usage(&[
        ("values.sh", source),
        ("main.sh", "source values.sh\necho \"$VALUE\"\n"),
    ]);
    assert_eq!(
        consumed_offsets(&usage, "values.sh"),
        vec![0, source.find("VALUE=other").unwrap()]
    );
}

#[test]
fn a_later_unconditional_write_replaces_conditional_values() {
    let source = "VALUE=default\nif enabled; then VALUE=other; fi\nVALUE=final\n";
    let usage = usage(&[
        ("values.sh", source),
        ("main.sh", "source values.sh\necho \"$VALUE\"\n"),
    ]);
    assert_eq!(
        consumed_offsets(&usage, "values.sh"),
        vec![source.find("VALUE=final").unwrap()]
    );
}

#[test]
fn assignment_initializers_consume_the_value_before_the_new_write() {
    let usage = usage(&[
        ("values.sh", "VALUE=old\nVALUE=current\n"),
        (
            "main.sh",
            "source values.sh\nVALUE=\"$VALUE/suffix\"\necho \"$VALUE\"\n",
        ),
    ]);
    assert_eq!(consumed_offsets(&usage, "values.sh"), vec![10]);
}

#[test]
fn early_returns_preserve_values_that_can_reach_the_caller() {
    for source in [
        "VALUE=first\nif enabled; then return; fi\nVALUE=last\n",
        "VALUE=first\nif enabled; then return; fi\nunset VALUE\n",
    ] {
        let usage = usage(&[
            ("values.sh", source),
            ("main.sh", "source values.sh\necho \"$VALUE\"\n"),
        ]);
        assert!(
            consumed_offsets(&usage, "values.sh").contains(&0),
            "{source}"
        );
    }
}

#[test]
fn conditional_sources_keep_each_possible_assignment_in_use() {
    for main in [
        "if enabled; then source values.sh; fi\necho \"$VALUE\"\n",
        "enabled && source values.sh\necho \"$VALUE\"\n",
        "if enabled; then source values.sh; else source other.sh; fi\necho \"$VALUE\"\n",
    ] {
        let usage = usage(&[
            ("values.sh", "VALUE=one\n"),
            ("other.sh", "VALUE=two\n"),
            ("main.sh", main),
        ]);
        assert_eq!(consumed_offsets(&usage, "values.sh"), vec![0], "{main}");
    }
}

#[test]
fn named_loaders_export_sources_only_when_called_and_persistent() {
    for (main, expected) in [
        (
            "load() { source values.sh; }; load; echo \"$VALUE\"\n",
            vec![0],
        ),
        (
            "load() { source values.sh; }; enabled && load; echo \"$VALUE\"\n",
            vec![0],
        ),
        (
            "load() { source values.sh; }; outer() { load; }; outer; echo \"$VALUE\"\n",
            vec![0],
        ),
        ("load() { source values.sh; }; echo \"$VALUE\"\n", vec![]),
        (
            "load() { source values.sh; }; echo \"$VALUE\"; load\n",
            vec![],
        ),
        (
            "load() { source values.sh; }; (load); echo \"$VALUE\"\n",
            vec![],
        ),
        (
            "load() { local VALUE=private; source values.sh; }; load; echo \"$VALUE\"\n",
            vec![],
        ),
    ] {
        let usage = usage(&[("values.sh", "VALUE=one\n"), ("main.sh", main)]);
        assert_eq!(consumed_offsets(&usage, "values.sh"), expected, "{main}");
    }
}

#[test]
fn guarded_imports_do_not_consume_an_overwritten_value() {
    let usage = usage(&[
        ("values.sh", "VALUE=one\n"),
        (
            "main.sh",
            "enabled && source values.sh\nVALUE=local\necho \"$VALUE\"\n",
        ),
    ]);
    assert!(consumed_offsets(&usage, "values.sh").is_empty());
}

#[test]
fn sources_within_a_loader_keep_their_execution_order() {
    for (body, expected) in [
        ("source values.sh; source reader.sh", vec![0]),
        ("source reader.sh; source values.sh", vec![]),
        (
            "source values.sh; source replacement.sh; source reader.sh",
            vec![],
        ),
    ] {
        let main = format!("load() {{ {body}; }}; load\n");
        let usage = usage(&[
            ("values.sh", "VALUE=one\n"),
            ("replacement.sh", "VALUE=two\n"),
            ("reader.sh", "echo \"$VALUE\"\n"),
            ("main.sh", &main),
        ]);
        assert_eq!(consumed_offsets(&usage, "values.sh"), expected, "{body}");
    }
}
