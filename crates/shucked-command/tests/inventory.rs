use shucked_command::*;

fn target(id: &str, complete: bool) -> TargetInventory {
    let context = ExecutionContext {
        target_id: id.into(),
        native_execution_allowed: true,
        ..ExecutionContext::default()
    };
    let mut snapshot = EnvironmentSnapshot::empty(&context);
    snapshot.path_known = complete;
    snapshot.captured_unix_ms = 1000;
    snapshot.aliases.insert(
        "secret".into(),
        Alias {
            words: vec!["tool".into(), "--token=private".into()],
            ..Alias::default()
        },
    );
    snapshot.functions.insert("private_name".into());
    TargetInventory::capture(id, &context, &snapshot)
}

#[test]
fn export_is_deterministic_and_removes_personal_state_and_execution_authority() {
    let target = target("offline-target", true);
    let first = target.to_json().expect("inventory export");
    assert_eq!(first, target.to_json().expect("second export"));
    assert!(!first.contains("private"));
    let restored = TargetInventory::from_json(&first).expect("inventory import");
    assert!(!restored.context.native_execution_allowed);
    assert_eq!(restored.context.policy, ValidationPolicy::Captured);
    assert!(restored.snapshot.aliases.is_empty());
    assert_eq!(restored.age_ms(2500), 1500);
}

#[test]
fn corrupted_inventory_and_future_schema_are_rejected() {
    let json = target("offline-target", true).to_json().expect("export");
    let corrupted = json.replace("offline-target", "tampered-target");
    assert!(matches!(
        TargetInventory::from_json(&corrupted),
        Err(InventoryError::Checksum)
    ));
    let mut value: serde_json::Value = serde_json::from_str(&json).expect("JSON fixture");
    value["schemaVersion"] = 900.into();
    assert!(matches!(
        TargetInventory::from_json(&value.to_string()),
        Err(InventoryError::UnsupportedVersion(900))
    ));
}

#[test]
fn target_comparison_distinguishes_missing_unknown_and_builtin_without_host_access() {
    let comparison = compare_targets(
        &[target("known", true), target("partial", false)],
        &[
            CommandSite::literal("not-captured"),
            CommandSite::literal("printf"),
        ],
    );
    assert!(matches!(
        comparison.commands[0].results[0],
        CommandResolution::Missing(_)
    ));
    assert!(matches!(
        comparison.commands[0].results[1],
        CommandResolution::Unknown(_)
    ));
    assert!(matches!(
        comparison.commands[1].results[0],
        CommandResolution::Resolved(_)
    ));
    assert!(matches!(
        comparison.commands[1].results[1],
        CommandResolution::Resolved(_)
    ));
    assert_eq!(comparison.targets[0].target_id, "known");
}

#[test]
fn provider_cursor_requires_valid_unicode_boundary_and_target_identity() {
    let context = ExecutionContext::default();
    let mut request = ProviderRequest {
        context: context.clone(),
        key: ResolutionSnapshotKey {
            document_uri: "file:///test.sh".into(),
            document_version: 1,
            analysis_generation: 0,
            target_id: context.target_id.clone(),
            environment_generation: 0,
            provider_generation: 0,
        },
        words: vec![ProviderWord {
            value: "é".into(),
            source_range: Some(SourceRange { start: 0, end: 2 }),
        }],
        word_index: 0,
        byte_offset_in_word: 1,
        pack_id: "test".into(),
        runtime_id: "test".into(),
    };
    assert!(request.validate().is_err());
    request.byte_offset_in_word = 2;
    assert!(request.validate().is_ok());
    request.key.target_id = "different".into();
    assert!(request.validate().is_err());
}
