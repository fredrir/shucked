use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use shucked_command::*;

fn context() -> ExecutionContext {
    ExecutionContext {
        target_id: "test-host".into(),
        ..ExecutionContext::default()
    }
}

fn executable(name: &str) -> Executable {
    Executable {
        identity: ExecutableIdentity {
            path: PathBuf::from(format!("/tools/{name}")),
            size: Some(100),
            modified_unix_ms: Some(1),
            version: None,
            vendor: None,
        },
        provenance: Provenance::new("test inventory"),
    }
}

fn snapshot() -> EnvironmentSnapshot {
    let mut snapshot = EnvironmentSnapshot::empty(&context());
    snapshot.path_known = true;
    snapshot.search_path.push(SearchDirectory {
        path: "/tools".into(),
        commands: BTreeMap::from([
            ("eza".into(), executable("eza")),
            ("brew".into(), executable("brew")),
        ]),
        complete: true,
        failure: None,
    });
    snapshot
}

#[test]
fn windows_inventory_needs_exact_evidence_to_choose_an_executable() {
    let mut snapshot = snapshot();
    snapshot.platform = "windows".into();
    snapshot.case_sensitive = false;
    assert!(matches!(
        snapshot.lookup("brew"),
        LookupEvidence::Unknown(_)
    ));
    snapshot.exact_lookups.insert(
        "brew".into(),
        LookupEvidence::Present(executable("brew.cmd")),
    );
    let LookupEvidence::Present(found) = snapshot.lookup("brew") else {
        panic!("exact query must win")
    };
    assert_eq!(found.identity.path, PathBuf::from("/tools/brew.cmd"));
}

#[test]
fn differently_cased_filename_requires_filesystem_evidence() {
    let mut snapshot = snapshot();
    assert!(matches!(
        snapshot.lookup("BREW"),
        LookupEvidence::Unknown(_)
    ));
    snapshot
        .exact_lookups
        .insert("BREW".into(), LookupEvidence::Missing);
    assert!(matches!(snapshot.lookup("BREW"), LookupEvidence::Missing));
}

#[test]
fn hash_is_a_builtin_without_any_host_executable() {
    for dialect in [ShellDialect::Bash, ShellDialect::Posix] {
        let context = ExecutionContext {
            dialect,
            ..context()
        };
        assert_eq!(
            resolve(
                &context,
                &EnvironmentSnapshot::empty(&context),
                &CommandSite::literal("hash")
            )
            .resolved()
            .expect("hash builtin")
            .kind,
            CommandKind::Builtin
        );
    }
}

#[test]
fn alias_injected_arguments_and_executable_identity_are_shared() {
    let mut site = CommandSite::literal("ls");
    site.arguments.push("--all".into());
    site.aliases.insert(
        "ls".into(),
        Alias {
            words: vec!["eza".into(), "--icons".into()],
            ..Alias::default()
        },
    );
    let resolved = resolve(&context(), &snapshot(), &site);
    let CommandResolution::Resolved(resolved) = resolved else {
        panic!("alias must resolve")
    };
    assert_eq!(resolved.name, "eza");
    assert_eq!(resolved.effective_words, ["eza", "--icons", "--all"]);
    assert_eq!(resolved.alias_chain, ["ls"]);
    assert_eq!(
        resolved.executable.expect("executable").path,
        PathBuf::from("/tools/eza")
    );
}

#[test]
fn quoted_command_bypasses_alias_expansion() {
    let mut site = CommandSite::literal("ls");
    site.alias_eligible = false;
    site.aliases.insert(
        "ls".into(),
        Alias {
            words: vec!["eza".into()],
            ..Alias::default()
        },
    );
    assert!(matches!(
        resolve(&context(), &snapshot(), &site),
        CommandResolution::Missing(_)
    ));
}

#[test]
fn self_alias_reaches_original_executable_once() {
    let mut site = CommandSite::literal("eza");
    site.aliases.insert(
        "eza".into(),
        Alias {
            words: vec!["eza".into(), "--icons".into()],
            ..Alias::default()
        },
    );
    let resolution = resolve(&context(), &snapshot(), &site);
    assert_eq!(
        resolution.resolved().expect("self alias").effective_words,
        ["eza", "--icons"]
    );
}

#[test]
fn opaque_and_recursive_aliases_are_unknown() {
    let mut site = CommandSite::literal("a");
    site.aliases.insert(
        "a".into(),
        Alias {
            opaque: true,
            ..Alias::default()
        },
    );
    assert!(matches!(
        resolve(&context(), &snapshot(), &site),
        CommandResolution::Unknown(UnknownCommand {
            reason: UnknownReason::OpaqueAlias,
            ..
        })
    ));
    site.aliases.insert(
        "a".into(),
        Alias {
            words: vec!["b".into()],
            ..Alias::default()
        },
    );
    site.aliases.insert(
        "b".into(),
        Alias {
            words: vec!["a".into()],
            ..Alias::default()
        },
    );
    assert!(matches!(
        resolve(&context(), &snapshot(), &site),
        CommandResolution::Unknown(UnknownCommand {
            reason: UnknownReason::RecursiveAlias,
            ..
        })
    ));
}

#[test]
fn scripts_do_not_inherit_live_session_aliases_or_functions() {
    let mut snapshot = snapshot();
    snapshot.functions.insert("private-function".into());
    snapshot.aliases.insert(
        "ls".into(),
        Alias {
            words: vec!["eza".into()],
            ..Alias::default()
        },
    );
    for name in ["ls", "private-function"] {
        assert!(matches!(
            resolve(&context(), &snapshot, &CommandSite::literal(name)),
            CommandResolution::Missing(_)
        ));
    }
    let mut context = context();
    context.mode = ExecutionMode::InteractiveSession;
    for name in ["ls", "private-function"] {
        assert!(matches!(
            resolve(&context, &snapshot, &CommandSite::literal(name)),
            CommandResolution::Resolved(_)
        ));
    }
}

#[test]
fn stale_session_alias_cannot_resolve_to_a_builtin() {
    let mut snapshot = snapshot();
    snapshot.fresh = false;
    snapshot.aliases.insert(
        "ls".into(),
        Alias {
            words: vec!["echo".into()],
            ..Alias::default()
        },
    );
    let mut context = context();
    context.mode = ExecutionMode::InteractiveSession;
    assert!(matches!(
        resolve(&context, &snapshot, &CommandSite::literal("ls")),
        CommandResolution::Unknown(UnknownCommand {
            reason: UnknownReason::StaleEnvironment,
            ..
        })
    ));
}

#[test]
fn portable_policy_preserves_source_and_builtin_resolution() {
    let mut context = context();
    context.policy = ValidationPolicy::Portable;
    assert!(matches!(
        resolve(&context, &snapshot(), &CommandSite::literal("brew")),
        CommandResolution::Unknown(UnknownCommand {
            reason: UnknownReason::PortablePolicy,
            ..
        })
    ));
    assert_eq!(
        resolve(&context, &snapshot(), &CommandSite::literal("printf"))
            .resolved()
            .expect("builtin")
            .kind,
        CommandKind::Builtin
    );
    let mut site = CommandSite::literal("project_function");
    site.functions.insert("project_function".into());
    assert_eq!(
        resolve(&context, &snapshot(), &site)
            .resolved()
            .expect("function")
            .kind,
        CommandKind::Function
    );
}

#[test]
fn function_shadows_external_tool_but_command_wrapper_bypasses_it() {
    let mut site = CommandSite::literal("brew");
    site.functions.insert("brew".into());
    assert_eq!(
        resolve(&context(), &snapshot(), &site)
            .resolved()
            .expect("function")
            .kind,
        CommandKind::Function
    );
    site.lookup = LookupMode::Command;
    assert_eq!(
        resolve(&context(), &snapshot(), &site)
            .resolved()
            .expect("executable")
            .kind,
        CommandKind::Executable
    );
}

#[test]
fn incomplete_earlier_path_directory_cannot_prove_later_identity() {
    let mut snapshot = snapshot();
    snapshot.search_path.insert(
        0,
        SearchDirectory {
            path: "/unreadable".into(),
            commands: BTreeMap::new(),
            complete: false,
            failure: Some("unreadable directory".into()),
        },
    );
    assert!(matches!(
        resolve(&context(), &snapshot, &CommandSite::literal("brew")),
        CommandResolution::Unknown(_)
    ));
    snapshot
        .exact_lookups
        .insert("brew".into(), LookupEvidence::Present(executable("brew")));
    assert!(matches!(
        resolve(&context(), &snapshot, &CommandSite::literal("brew")),
        CommandResolution::Resolved(_)
    ));
}

#[test]
fn uncertain_path_and_wrong_target_never_claim_absence() {
    let mut site = CommandSite::literal("not-installed");
    site.environment_uncertain = true;
    assert!(matches!(
        resolve(&context(), &snapshot(), &site),
        CommandResolution::Unknown(_)
    ));
    site.environment_uncertain = false;
    let mut other_context = context();
    other_context.target_id = "other-host".into();
    assert!(matches!(
        resolve(&other_context, &snapshot(), &site),
        CommandResolution::Unknown(_)
    ));
}

#[test]
fn missing_expected_dependency_is_distinct_from_typo() {
    let mut site = CommandSite::literal("generated-tool");
    site.declared.insert(
        "generated-tool".into(),
        CommandDeclaration {
            kind: DeclarationKind::Generated,
            ..CommandDeclaration::default()
        },
    );
    let CommandResolution::Missing(missing) = resolve(&context(), &snapshot(), &site) else {
        panic!("missing dependency")
    };
    assert_eq!(
        missing.declaration.expect("declaration").kind,
        DeclarationKind::Generated
    );
}

#[test]
fn only_the_guarded_site_suppresses_an_optional_dependency() {
    let mut site = CommandSite::literal("optional-tool");
    site.guarded = BTreeSet::from(["optional-tool".into()]);
    assert!(matches!(
        resolve(&context(), &snapshot(), &site),
        CommandResolution::Unknown(UnknownCommand {
            reason: UnknownReason::GuardedDependency,
            ..
        })
    ));
    site.guarded.clear();
    assert!(matches!(
        resolve(&context(), &snapshot(), &site),
        CommandResolution::Missing(_)
    ));
}

#[test]
fn typo_suggestions_are_bounded_deterministic_and_handle_transpositions() {
    assert_eq!(
        typo_candidates("brwe", ["brew", "echo", "brwe"], 3),
        ["brew"]
    );
    assert!(typo_candidates("x", ["y"], 3).is_empty());
    assert!(typo_candidates(&"a".repeat(1000), ["a"], 3).is_empty());
}
