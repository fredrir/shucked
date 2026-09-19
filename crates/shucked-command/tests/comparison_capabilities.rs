use shucked_command::*;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn comparison_distinguishes_recorded_flag_incompatibility_from_unknown_coverage() {
    let context = ExecutionContext::default();
    let mut snapshot = EnvironmentSnapshot::empty(&context);
    let executable = ExecutableIdentity { path: "/captured/tool".into(), size: Some(10), modified_unix_ms: Some(1), version: Some("1".into()), vendor: None };
    snapshot.exact_lookups.insert("tool".into(), LookupEvidence::Present(Executable { identity: executable.clone(), provenance: Provenance::new("captured") }));
    let unknown = TargetInventory::capture("unknown grammar", &context, &snapshot);
    snapshot.validators.insert("tool".into(), ValidationEvidence {
        executable, platform: snapshot.platform.clone(), kind: EvidenceKind::VersionedManifest,
        grammar: CommandGrammar { flags: BTreeMap::from([("--old".into(), FlagSpec::default())]), flags_complete: true, positional_arguments: true, ..Default::default() },
        extensions: BTreeSet::new(), extensions_complete: true, plugin_extensible: false, fresh: true, provenance: Provenance::new("fixture version grammar"),
    });
    let known = TargetInventory::capture("known grammar", &context, &snapshot);
    let site = CommandSite { arguments: vec!["--new".into()], ..CommandSite::literal("tool") };
    let comparison = compare_targets(&[known, unknown], &[site]);
    assert!(matches!(comparison.commands[0].validation[0], ValidationResult::Invalid(_)));
    assert!(matches!(comparison.commands[0].validation[1], ValidationResult::Unknown(_)));
    assert!(comparison.commands[0].results.iter().all(|result| matches!(result, CommandResolution::Resolved(_))));
}
