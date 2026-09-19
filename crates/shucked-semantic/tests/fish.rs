use shucked_semantic::{CommandNamespace, analyze_fish};
#[test]
fn fish_blocks_functions_and_namespaces_are_independent_of_bash() {
    let doc = analyze_fish(
        "function greet --argument-names person\n echo $person\nend\ngreet world\ncommand greet\n",
    );
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
    assert_eq!(doc.functions[0].name, "greet");
    assert_eq!(doc.function_calls.len(), 1);
    assert_eq!(
        doc.commands.last().unwrap().namespace,
        CommandNamespace::External
    );
}
#[test]
fn fish_recovers_missing_quotes_and_block_ends() {
    let doc = analyze_fish("if true\n echo 'unfinished\n");
    assert!(
        doc.diagnostics
            .iter()
            .any(|d| d.message.contains("closing quote"))
    );
    assert!(
        doc.diagnostics
            .iter()
            .any(|d| d.message.contains("closing end"))
    );
}
#[test]
fn fish_substitutions_keep_original_unicode_offsets() {
    let source = "echo 'ø' (missing --flag)\n";
    let doc = analyze_fish(source);
    assert!(doc.diagnostics.is_empty());
    let nested = doc
        .commands
        .iter()
        .find(|s| s.name() == Some("missing"))
        .unwrap();
    assert_eq!(
        &source[nested.name_span().start.offset()..nested.name_span().end.offset()],
        "missing"
    );
    assert!(
        doc.commands[0]
            .effective_words
            .last()
            .unwrap()
            .text
            .is_none()
    );
}
#[test]
fn fish_guard_is_limited_to_positive_branch_and_matching_name() {
    let doc = analyze_fish(
        "if command -q optional\n optional\n unrelated\nelse\n optional\nend\noptional\n",
    );
    let sites = doc
        .commands
        .iter()
        .filter(|s| s.name() == Some("optional"))
        .collect::<Vec<_>>();
    assert_eq!(
        sites
            .iter()
            .map(|s| s.guarded_available)
            .collect::<Vec<_>>(),
        [true, false, false]
    );
    assert!(
        !doc.commands
            .iter()
            .find(|s| s.name() == Some("unrelated"))
            .unwrap()
            .guarded_available
    );
}
#[test]
fn fish_dynamic_lookup_and_environment_changes_remain_uncertain() {
    let doc = analyze_fish("set -gx PATH $other\nmissing\n$command\n");
    assert!(doc.commands[1].environment_uncertain.is_some());
    assert!(doc.commands[2].environment_uncertain.is_some());
}
#[test]
fn fish_stray_end_and_dangling_pipeline_are_diagnosed() {
    let doc = analyze_fish("end\necho |\n");
    assert!(
        doc.diagnostics
            .iter()
            .any(|d| d.message.contains("open block"))
    );
    assert!(
        doc.diagnostics
            .iter()
            .any(|d| d.message.contains("following command"))
    );
}
#[test]
fn fish_negative_and_terminating_guards_keep_branch_scope() {
    let doc = analyze_fish(
        "if not command -q optional\n optional\nelse\n optional\nend\noptional\nif not command -q other\n exit 1\nend\nother\n",
    );
    assert_eq!(
        doc.commands
            .iter()
            .filter(|s| s.name() == Some("optional"))
            .map(|s| s.guarded_available)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    assert!(
        doc.commands
            .iter()
            .find(|s| s.name() == Some("other"))
            .unwrap()
            .guarded_available
    );
}
