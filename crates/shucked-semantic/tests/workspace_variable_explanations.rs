use std::path::{Path, PathBuf};

use shucked_indexer::Indexer;
use shucked_parser::parser::Parser;
use shucked_semantic::{
    CallFactSourceEdge, FileCallFacts, SemanticBuildOptions, SemanticModel, SourceRefKind,
    WorkspaceVariableExplanation, WorkspaceVariableIndex, variable_target,
};

fn model(source: &str) -> SemanticModel {
    let parsed = Parser::new(source).parse();
    assert!(!parsed.is_err(), "{source}");
    let indexer = Indexer::new(source, &parsed);
    SemanticModel::build_with_options(
        &parsed.file,
        source,
        &indexer,
        SemanticBuildOptions {
            resolve_source_closure: false,
            ..Default::default()
        },
    )
}

fn explain(files: &[(&str, &str)], selected: usize, offset: usize) -> WorkspaceVariableExplanation {
    let mut index = WorkspaceVariableIndex::default();
    for (name, source) in files {
        let model = model(source);
        let edges = model
            .source_refs()
            .iter()
            .filter_map(|reference| {
                let SourceRefKind::Literal(path) = &reference.kind else {
                    return None;
                };
                Some(CallFactSourceEdge {
                    path: PathBuf::from("/workspace").join(path.as_str()),
                    span: reference.span,
                    conditional: reference.conditionally_executed,
                    completion_visible: false,
                })
            })
            .collect();
        let calls = FileCallFacts::project_with_source_edges(&model, edges);
        index.insert(
            Path::new("/workspace").join(name),
            &model,
            &calls.source_effects,
        );
    }
    let model = model(files[selected].1);
    let target = model.editor_query().target_at_offset(offset).unwrap();
    let target = variable_target(&model, &target).unwrap();
    assert!(
        index
            .explain(
                &Path::new("/workspace").join(files[selected].0),
                &target,
                &|| true
            )
            .is_none()
    );
    index
        .explain(
            &Path::new("/workspace").join(files[selected].0),
            &target,
            &|| false,
        )
        .unwrap()
}

#[test]
fn selected_assignment_excludes_overwritten_values_and_unrelated_names() {
    let files = [
        ("helper.sh", "VALUE=old\nVALUE=current\n"),
        (
            "main.sh",
            "source helper.sh\necho \"$VALUE\"\nVALUE=own\necho \"$VALUE\"\n",
        ),
        ("unrelated.sh", "VALUE=other\necho \"$VALUE\"\n"),
    ];
    let old = explain(&files, 0, 1);
    assert!(old.references.is_empty());
    let current = explain(&files, 0, 11);
    assert_eq!(current.definitions.len(), 1);
    assert_eq!(current.definitions[0].span.start.offset(), 10);
    assert_eq!(current.references.len(), 1);
    assert_eq!(current.references[0].path, Path::new("/workspace/main.sh"));
    assert_eq!(
        current.references[0].span.start.offset(),
        files[1].1.find("$VALUE").unwrap() + 1
    );
    assert!(!current.incomplete);
}

#[test]
fn read_shows_both_possible_conditional_origins() {
    let files = [
        ("a.sh", "VALUE=a\n"),
        ("b.sh", "VALUE=b\n"),
        (
            "main.sh",
            "source a.sh\nif ready; then source b.sh; fi\necho \"$VALUE\"\n",
        ),
    ];
    let details = explain(&files, 2, files[2].1.find("$VALUE").unwrap() + 2);
    assert_eq!(details.definitions.len(), 2);
    assert_eq!(details.references.len(), 1);
    assert!(details.conditional);
    assert!(!details.incomplete);
}

#[test]
fn called_loader_provides_origin_but_function_locals_do_not_consume_it() {
    let files = [
        ("helper.sh", "VALUE=shared\n"),
        (
            "main.sh",
            "load() { source helper.sh; }\nload\necho \"$VALUE\"\nf() { local VALUE=private; echo \"$VALUE\"; }\nf\n",
        ),
    ];
    let details = explain(&files, 0, 1);
    assert_eq!(details.references.len(), 1);
    assert_eq!(
        details.references[0].span.start.offset(),
        files[1].1.find("$VALUE").unwrap() + 1
    );
}

#[test]
fn unknown_source_keeps_possible_reads_and_marks_partial_results() {
    let files = [
        ("helper.sh", "VALUE=shared\n"),
        (
            "main.sh",
            "source helper.sh\necho \"$VALUE\"\nsource \"$UNKNOWN\"\necho \"$VALUE\"\n",
        ),
    ];
    let details = explain(&files, 0, 1);
    assert_eq!(details.references.len(), 2);
    assert!(details.incomplete);
}

#[test]
fn initializer_read_belongs_to_the_previous_assignment() {
    let files = [(
        "main.sh",
        "VALUE=first\nVALUE=\"$VALUE/next\"\necho \"$VALUE\"\n",
    )];
    let details = explain(&files, 0, files[0].1.find("$VALUE").unwrap() + 2);
    assert_eq!(details.definitions.len(), 1);
    assert_eq!(details.definitions[0].span.start.offset(), 0);
    assert_eq!(details.references.len(), 1);
    assert_eq!(
        details.references[0].span.start.offset(),
        files[0].1.find("$VALUE").unwrap() + 1
    );
}

#[test]
fn inherited_reads_identify_the_callers_assignment() {
    let files = [
        ("helper.sh", "echo \"$VALUE\"\n"),
        ("main.sh", "VALUE=shared\nsource helper.sh\n"),
    ];
    let details = explain(&files, 0, files[0].1.find("$VALUE").unwrap() + 2);
    assert_eq!(details.definitions.len(), 1);
    assert_eq!(details.definitions[0].path, Path::new("/workspace/main.sh"));
    assert_eq!(details.references.len(), 1);
    assert!(!details.incomplete);
}
