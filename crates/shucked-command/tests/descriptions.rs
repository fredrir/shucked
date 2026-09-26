use shucked_command::*;

const COMPLETION_TOOLS: &[&str] = &[
    "apple-ls", "gnu-ls", "eza", "docker", "ripgrep", "fd", "bat", "curl", "openssh", "kubectl",
];

#[test]
fn every_bundled_grammar_loads_with_or_without_descriptions() {
    let mut loaded = 0;
    for (tool, version) in known_tool_versions() {
        let grammar = known_tool_grammar(tool, version)
            .unwrap_or_else(|| panic!("{tool} {version} must load"));
        assert!(
            !grammar.grammar.flags.is_empty() || !grammar.grammar.subcommands.is_empty(),
            "{tool} {version} records nothing"
        );
        loaded += 1;
    }
    assert!(loaded >= 34, "{loaded} grammars loaded");
    // Bare arity entries keep loading and simply carry no description.
    let pacman = known_tool_grammar("pacman", "7.1.0").unwrap();
    assert!(
        pacman
            .grammar
            .flags
            .values()
            .all(|flag| flag.description.is_none())
    );
    assert_eq!(pacman.grammar.flags["--sync"].value, FlagValue::None);
    // Object entries carry the arity and the text.
    let ls = known_tool_grammar("gnu-ls", "9.7").unwrap();
    assert_eq!(ls.grammar.flags["--width"].value, FlagValue::Required);
    assert_eq!(
        ls.grammar.flags["-l"].description.as_deref(),
        Some("List in long format")
    );
    assert_eq!(
        ls.grammar.flags["--color"].value,
        FlagValue::OptionalAttached
    );
}

#[test]
fn completion_tools_describe_every_option() {
    let mut described = 0;
    for (tool, version) in known_tool_versions() {
        if !COMPLETION_TOOLS.contains(&tool) {
            continue;
        }
        let grammar = known_tool_grammar(tool, version).unwrap();
        for (name, flag) in &grammar.grammar.flags {
            let description = flag
                .description
                .as_deref()
                .unwrap_or_else(|| panic!("{tool} {version} {name} has no description"));
            assert!(
                !description.trim().is_empty() && description.trim() == description,
                "{tool} {version} {name}: untrimmed or empty"
            );
            assert!(
                !description.ends_with('.') || description.ends_with(".."),
                "{tool} {version} {name}: trailing period"
            );
            assert!(
                description.chars().count() <= 120,
                "{tool} {version} {name}: too long"
            );
            assert!(
                description
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_uppercase()),
                "{tool} {version} {name}: not a capitalized imperative line"
            );
            assert!(
                !description.chars().any(char::is_control),
                "{tool} {version} {name}: control characters"
            );
            described += 1;
        }
    }
    assert!(described > 3000, "{described} options described");
    for version in ["457.140.3", "475", "479"] {
        let apple = known_tool_grammar("apple-ls", version).unwrap();
        assert!(apple.grammar.flags["-l"].description.is_some(), "{version}");
        assert!(apple.grammar.flags["-@"].description.is_some(), "{version}");
    }
}

#[test]
fn descriptions_travel_with_captured_validation_evidence() {
    let grammar = known_tool_grammar("gnu-ls", "9.7").unwrap();
    let evidence = ValidationEvidence {
        executable: ExecutableIdentity {
            path: "/fixtures/ls".into(),
            size: Some(1),
            modified_unix_ms: Some(1),
            version: Some("9.7".into()),
            vendor: Some("gnu-ls".into()),
        },
        platform: "fixture".into(),
        kind: EvidenceKind::VersionedManifest,
        grammar: grammar.grammar,
        extensions: Default::default(),
        extensions_complete: true,
        plugin_extensible: false,
        fresh: true,
        provenance: Provenance::new("fixture"),
    };
    let json = serde_json::to_string(&evidence).unwrap();
    assert!(json.contains("\"description\":\"List in long format\""));
    let restored: ValidationEvidence = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, evidence);
    // Records written before descriptions existed still load.
    let legacy: FlagSpec =
        serde_json::from_str(r#"{"value":"none","values":[],"valuesComplete":false}"#).unwrap();
    assert_eq!(legacy.description, None);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("description")
    );
}

#[test]
fn newest_grammar_orders_versions_numerically() {
    assert_eq!(newest_tool_grammar("openssh").unwrap().0, "10.5p1");
    assert_eq!(newest_tool_grammar("apple-ls").unwrap().0, "479");
    assert_eq!(newest_tool_grammar("curl").unwrap().0, "8.15.0");
    assert_eq!(newest_tool_grammar("eza").unwrap().0, "0.23.5");
    assert_eq!(newest_tool_grammar("kubectl").unwrap().0, "1.34.0");
    assert!(newest_tool_grammar("unknown-tool").is_none());
}
