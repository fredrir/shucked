use std::fs;

use shucked_config::{
    ConfigArguments, FormatConfig, ShuckConfig, SingleConfigArgument,
    discovered_config_path_for_root, load_project_config, resolve_project_root_for_file,
    resolve_project_root_for_input,
};
use tempfile::tempdir;

#[test]
fn public_api_layers_discovered_config_and_inline_overrides() {
    let tempdir = tempdir().unwrap();
    fs::write(
        tempdir.path().join("shucked.toml"),
        "[format]\nfunction-next-line = true\n",
    )
    .unwrap();

    let override_config = ShuckConfig {
        format: FormatConfig {
            indent_width: Some(2),
            ..FormatConfig::default()
        },
        ..ShuckConfig::default()
    };

    let config = ConfigArguments::from_cli(
        vec![SingleConfigArgument::SettingsOverride(Box::new(
            override_config,
        ))],
        false,
    )
    .unwrap();

    let loaded = load_project_config(tempdir.path(), &config).unwrap();
    assert_eq!(loaded.format.function_next_line, Some(true));
    assert_eq!(loaded.format.indent_width, Some(2));
}

#[test]
fn public_api_loads_shared_per_file_shell_config() {
    let tempdir = tempdir().unwrap();
    fs::write(
        tempdir.path().join("shucked.toml"),
        "[per-file-shell]\n'dot_z*' = 'zsh'\n",
    )
    .unwrap();

    let loaded = load_project_config(tempdir.path(), &ConfigArguments::default()).unwrap();

    assert_eq!(
        loaded
            .per_file_shell
            .as_ref()
            .and_then(|entries| entries.get("dot_z*"))
            .map(String::as_str),
        Some("zsh")
    );
}

#[test]
fn public_api_prefers_explicit_config_file_over_discovered_file() {
    let tempdir = tempdir().unwrap();
    fs::write(
        tempdir.path().join("shucked.toml"),
        "[format]\nfunction-next-line = false\n",
    )
    .unwrap();

    let explicit = tempdir.path().join("override.toml");
    fs::write(&explicit, "[format]\nfunction-next-line = true\n").unwrap();

    let config =
        ConfigArguments::from_cli(vec![SingleConfigArgument::FilePath(explicit)], false).unwrap();

    let loaded = load_project_config(tempdir.path(), &config).unwrap();
    assert_eq!(loaded.format.function_next_line, Some(true));
}

#[test]
fn public_api_resolves_project_roots_and_discovered_config_paths() {
    let tempdir = tempdir().unwrap();
    let nested = tempdir.path().join("nested");
    let file = nested.join("script.sh");

    fs::create_dir_all(&nested).unwrap();
    fs::write(tempdir.path().join(".shucked.toml"), "[format]\n").unwrap();
    fs::write(&file, "#!/bin/sh\necho hi\n").unwrap();

    assert_eq!(
        resolve_project_root_for_input(&nested, true).unwrap(),
        tempdir.path()
    );
    assert_eq!(
        resolve_project_root_for_file(&file, &nested, true).unwrap(),
        tempdir.path()
    );
    assert_eq!(
        discovered_config_path_for_root(tempdir.path()).unwrap(),
        Some(tempdir.path().join(".shucked.toml"))
    );
}
