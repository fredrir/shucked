use shucked_command::*;
use std::collections::{BTreeMap, BTreeSet};

fn evidence() -> ValidationEvidence {
    let mut grammar = CommandGrammar {
        requires_subcommand: true,
        subcommands_complete: true,
        ..CommandGrammar::default()
    };
    let child = CommandGrammar {
        flags: BTreeMap::from([
            ("--verbose".into(), FlagSpec::default()),
            (
                "--format".into(),
                FlagSpec {
                    value: FlagValue::Required,
                    values: BTreeSet::from(["json".into(), "text".into()]),
                    values_complete: true,
                    description: None,
                },
            ),
            ("-a".into(), FlagSpec::default()),
            ("-v".into(), FlagSpec::default()),
        ]),
        flags_complete: true,
        short_flag_clusters: true,
        positional_arguments: true,
        ..CommandGrammar::default()
    };
    grammar.subcommands.insert("install".into(), child);
    ValidationEvidence {
        executable: ExecutableIdentity {
            path: "/tools/fixture".into(),
            size: Some(4),
            modified_unix_ms: Some(1),
            version: Some("1.0".into()),
            vendor: Some("fixture".into()),
        },
        platform: "fixture-host".into(),
        kind: EvidenceKind::StructuredInterface,
        grammar,
        extensions: BTreeSet::new(),
        extensions_complete: true,
        plugin_extensible: true,
        fresh: true,
        provenance: Provenance::new("fixture metadata endpoint"),
    }
}

fn command(words: &[&str], evidence: &ValidationEvidence) -> ResolvedCommand {
    ResolvedCommand {
        name: "fixture".into(),
        kind: CommandKind::Executable,
        executable: Some(evidence.executable.clone()),
        effective_words: words.iter().map(|s| (*s).into()).collect(),
        alias_chain: Vec::new(),
        provenance: Vec::new(),
    }
}

#[test]
fn complete_authority_can_reject_subcommand_typo_with_an_explicit_candidate() {
    let evidence = evidence();
    let result = validate_invocation(
        &command(&["fixture", "instal"], &evidence),
        &evidence,
        "fixture-host",
    );
    let ValidationResult::Invalid(issues) = result else {
        panic!("invalid subcommand")
    };
    assert_eq!(issues[0].word_index, 1);
    assert_eq!(issues[0].suggestions, ["install"]);
}

#[test]
fn incomplete_plugin_inventory_cannot_establish_subcommand_invalidity() {
    let mut evidence = evidence();
    evidence.extensions_complete = false;
    assert!(matches!(
        validate_invocation(
            &command(&["fixture", "external-plugin"], &evidence),
            &evidence,
            "fixture-host"
        ),
        ValidationResult::Unknown(_)
    ));
}

#[test]
fn flags_values_and_end_of_options_obey_context_grammar() {
    let evidence = evidence();
    for words in [
        vec!["fixture", "install", "-av"],
        vec!["fixture", "install", "--format=json"],
        vec!["fixture", "install", "--format", "text"],
        vec!["fixture", "install", "--", "--not-a-flag"],
    ] {
        assert_eq!(
            validate_invocation(&command(&words, &evidence), &evidence, "fixture-host"),
            ValidationResult::Valid
        );
    }
    assert!(matches!(
        validate_invocation(
            &command(&["fixture", "install", "--verbse"], &evidence),
            &evidence,
            "fixture-host"
        ),
        ValidationResult::Invalid(_)
    ));
    assert!(matches!(
        validate_invocation(
            &command(&["fixture", "install", "--format", "xml"], &evidence),
            &evidence,
            "fixture-host"
        ),
        ValidationResult::Invalid(_)
    ));
}

#[test]
fn executable_replacement_platform_mismatch_and_function_shadowing_invalidate_grammar() {
    let evidence = evidence();
    let mut invocation = command(&["fixture", "unknown"], &evidence);
    invocation
        .executable
        .as_mut()
        .expect("identity")
        .modified_unix_ms = Some(2);
    assert!(matches!(
        validate_invocation(&invocation, &evidence, "fixture-host"),
        ValidationResult::Unknown(_)
    ));
    invocation = command(&["fixture", "unknown"], &evidence);
    assert!(matches!(
        validate_invocation(&invocation, &evidence, "different-platform"),
        ValidationResult::Unknown(_)
    ));
    invocation.kind = CommandKind::Function;
    assert!(matches!(
        validate_invocation(&invocation, &evidence, "fixture-host"),
        ValidationResult::Unknown(_)
    ));
}

#[test]
fn unfinished_invocations_do_not_warn_while_typing() {
    let evidence = evidence();
    for words in [vec!["fixture"], vec!["fixture", "install", "--format"]] {
        assert_eq!(
            validate_invocation(&command(&words, &evidence), &evidence, "fixture-host"),
            ValidationResult::Valid
        );
    }
}
