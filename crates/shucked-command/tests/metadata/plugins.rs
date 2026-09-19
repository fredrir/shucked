use super::*;

#[test]
fn docker_extra_directories_are_data_and_incomplete_filesystems_remain_unknown() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    std::fs::write(
        &config,
        r#"{"auths":{"secret":"never-export"},"cliPluginsExtraDirs":["plugins"]}"#,
    )
    .unwrap();
    let context = ExecutionContext {
        cwd: Some(root.path().into()),
        cwd_known: true,
        ..Default::default()
    };
    let paths = extra_docker_directories(&context, &config).unwrap();
    assert_eq!(paths, [root.path().join("plugins")]);
    std::fs::create_dir(&paths[0]).unwrap();
    std::fs::write(paths[0].join("docker-compose"), "never executed").unwrap();
    assert!(docker_directories(&paths).unwrap().contains("compose"));
    std::os::unix::fs::symlink("docker-compose", paths[0].join("docker-linked")).unwrap();
    assert!(docker_directories(&paths).unwrap().contains("linked"));
    std::os::unix::fs::symlink("docker-loop", paths[0].join("docker-loop")).unwrap();
    assert!(docker_directories(&paths).is_none());
    std::fs::write(&config, "{invalid").unwrap();
    assert!(extra_docker_directories(&context, &config).is_none());
}

#[test]
fn kubectl_plugin_names_keep_underscore_encoding_and_require_complete_path() {
    let root = tempfile::tempdir().unwrap();
    let context = ExecutionContext::default();
    let mut environment = crate::host::capture(&context, vec![root.path().into()], 0);
    let identity = crate::ExecutableIdentity {
        path: root.path().join("kubectl-foo_bar-sub"),
        size: None,
        modified_unix_ms: None,
        version: None,
        vendor: None,
    };
    environment.search_path[0].commands.insert(
        "kubectl-foo_bar-sub".into(),
        crate::Executable {
            identity,
            provenance: crate::Provenance::new("fixture"),
        },
    );
    assert_eq!(
        kubectl(&environment).unwrap(),
        BTreeSet::from(["foo-bar".into()])
    );
    environment.search_path[0].complete = false;
    assert!(kubectl(&environment).is_none());
}
