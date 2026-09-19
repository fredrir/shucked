use shucked_command::*;

fn validate(tool: &str, version: &str, arguments: &[&str]) -> ValidationResult {
    let manifest = known_tool_grammar(tool, version).expect("supported fixture version");
    let identity = ExecutableIdentity {
        path: format!("/fixtures/{tool}").into(),
        size: Some(1),
        modified_unix_ms: Some(1),
        version: Some(version.into()),
        vendor: Some(tool.into()),
    };
    let evidence = ValidationEvidence {
        executable: identity.clone(),
        platform: "fixture".into(),
        kind: EvidenceKind::VersionedManifest,
        grammar: manifest.grammar,
        extensions: Default::default(),
        extensions_complete: true,
        plugin_extensible: false,
        fresh: true,
        provenance: Provenance::new("fixture version"),
    };
    let command = ResolvedCommand {
        name: tool.into(),
        kind: CommandKind::Executable,
        executable: Some(identity),
        effective_words: std::iter::once(tool.to_owned())
            .chain(arguments.iter().map(|s| (*s).into()))
            .collect(),
        alias_chain: Vec::new(),
        provenance: Vec::new(),
    };
    validate_invocation(&command, &evidence, "fixture")
}

#[test]
fn exact_version_manifests_reject_typos_without_assuming_future_versions() {
    for (tool, version) in [
        ("eza", "0.23.0"),
        ("eza", "0.23.5"),
        ("ripgrep", "14.1.1"),
        ("ripgrep", "15.2.0"),
        ("fd", "10.3.0"),
        ("bat", "0.25.0"),
        ("gnu-ls", "9.7"),
        ("pacman", "7.0.0"),
        ("pacman", "7.1.0"),
    ] {
        assert!(
            matches!(
                validate(tool, version, &["--shucked-fixture-unknown"]),
                ValidationResult::Invalid(_)
            ),
            "{tool} {version}"
        );
        assert!(known_tool_grammar(tool, "999.0.0").is_none());
    }
}

#[test]
fn hidden_negations_aliases_and_long_flag_values_are_covered() {
    assert_eq!(
        validate(
            "ripgrep",
            "14.1.1",
            &["--no-hidden", "--no-encoding", "--maxdepth", "4", "needle"]
        ),
        ValidationResult::Valid
    );
    assert_eq!(
        validate(
            "fd",
            "10.3.0",
            &["--no-hidden", "--literal", "--newer", "1d", "needle"]
        ),
        ValidationResult::Valid
    );
    assert_eq!(
        validate(
            "bat",
            "0.25.0",
            &["--no-config", "--no-pager", "--theme", "a-theme"]
        ),
        ValidationResult::Valid
    );
    assert_eq!(
        validate(
            "eza",
            "0.23.0",
            &["--colour=always", "-la", "--sort", "size"]
        ),
        ValidationResult::Valid
    );
    assert_eq!(
        validate(
            "eza",
            "0.23.5",
            &["--colour=always", "--code=both", "--short-nix"]
        ),
        ValidationResult::Valid
    );
}

#[test]
fn delegated_arguments_and_abbreviated_gnu_options_stay_unknown() {
    assert!(matches!(
        validate("fd", "10.3.0", &["-x", "external", "--its-own-flag"]),
        ValidationResult::Unknown(_)
    ));
    assert!(matches!(
        validate("fd", "10.3.0", &["-Hx", "external", "--its-own-flag"]),
        ValidationResult::Unknown(_)
    ));
    assert!(matches!(
        validate("bat", "0.25.0", &["cache", "--build"]),
        ValidationResult::Unknown(_)
    ));
    assert!(matches!(
        validate("gnu-ls", "9.7", &["--colo=always"]),
        ValidationResult::Unknown(_)
    ));
}

#[test]
fn options_after_end_marker_are_filenames_and_short_values_are_consumed() {
    assert_eq!(
        validate("eza", "0.23.0", &["-L4", "--", "--filename"]),
        ValidationResult::Valid
    );
    assert_eq!(
        validate("gnu-ls", "9.7", &["-w80", "--", "--filename"]),
        ValidationResult::Valid
    );
    assert_eq!(
        validate(
            "pacman",
            "7.0.0",
            &["-Syu", "--cachedir", "/fixture/cache", "package"]
        ),
        ValidationResult::Valid
    );
}
