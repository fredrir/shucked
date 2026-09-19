use shucked_config::{CommandRequirement, ShuckConfig, apply_config_overrides};

#[test]
fn project_command_declarations_retain_file_and_target_scopes() {
    let config: ShuckConfig = toml::from_str(r#"
[environment.commands.codegen]
kind = "generated"
files = ["scripts/**"]
targets = ["deployment"]
[environment.commands.optional-tool]
kind = "optional"
"#).unwrap();
    let generated = &config.environment.commands["codegen"];
    assert_eq!(generated.kind, CommandRequirement::Generated);
    assert_eq!(generated.files, ["scripts/**"]);
    assert_eq!(generated.targets, ["deployment"]);
    assert_eq!(config.environment.commands["optional-tool"].kind, CommandRequirement::Optional);
}

#[test]
fn declaration_overrides_merge_by_command_and_reject_unknown_kinds() {
    let mut base: ShuckConfig = toml::from_str("[environment.commands.first]\nkind = 'required'\n").unwrap();
    let overrides: ShuckConfig = toml::from_str("[environment.commands.second]\nkind = 'deployment'\n").unwrap();
    apply_config_overrides(&mut base, overrides);
    assert_eq!(base.environment.commands.len(), 2);
    assert!(toml::from_str::<ShuckConfig>("[environment.commands.first]\nkind = 'installed'\n").is_err());
}
