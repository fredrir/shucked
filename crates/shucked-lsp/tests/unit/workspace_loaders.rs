//! Startup files outside the workspace roots that load workspace files
//! ("loaders") join the index, bounded and only when they actually source
//! into the workspace.

use super::*;

fn context(workspace: &Path) -> WorkspaceFunctionContext {
    WorkspaceFunctionContext {
        workspace_roots: vec![workspace.to_path_buf()],
        settings_workspace_roots: vec![workspace.to_path_buf()],
        workspace_settings: Vec::new(),
        global_options: ClientOptions::default(),
        open_documents: Vec::new(),
        encoding: PositionEncoding::UTF16,
        max_files: 1000,
        cache: Arc::new(WorkspaceFunctionIndexCache::default()),
        epoch: 0,
        cancellation: RequestCancellationToken::default(),
    }
}

fn call_span(index: &WorkspaceFunctionIndex, path: &Path, name: &str) -> Span {
    index
        .graph
        .files()
        .find_map(|(file, facts)| (file == path).then_some(facts))
        .unwrap_or_else(|| panic!("{} should be indexed", path.display()))
        .call_sites
        .iter()
        .find(|call| call.callee.as_str() == name)
        .unwrap_or_else(|| panic!("{} should call {name}", path.display()))
        .name_span
}

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(dir.path()).unwrap();
    (dir, path)
}

#[test]
fn startup_files_that_load_a_workspace_file_join_the_index_as_loaders() {
    let (_home_dir, home) = canonical_tempdir();
    let (_workspace_dir, workspace) = canonical_tempdir();
    let lib = workspace.join("lib.zsh");
    let other = workspace.join("other.zsh");
    std::fs::write(&lib, "myfn\n").unwrap();
    std::fs::write(&other, "myfn\n").unwrap();
    let zshrc = home.join(".zshrc");
    std::fs::write(
        &zshrc,
        format!(
            "myfn() {{ :; }}\nsource {}/lib.zsh\nmyfn\n",
            workspace.display()
        ),
    )
    .unwrap();
    // A startup file that never sources the workspace stays out of the index.
    let bashrc = home.join(".bashrc");
    std::fs::write(&bashrc, "unrelated() { :; }\nunrelated\n").unwrap();

    with_test_home_dir(&home, || {
        let built = WorkspaceFunctionIndex::build(&context(&workspace)).unwrap();
        assert!(built.contains(&zshrc), "the loader should be indexed");
        assert!(
            !built.contains(&bashrc),
            "a non-loader startup file is not indexed"
        );
        assert!(built.is_complete());

        let resolution = built.function_resolution(&lib, call_span(&built, &lib, "myfn"));
        let definition = resolution
            .exact()
            .expect("the loader's definition should reach the sourced file exactly");
        assert_eq!(definition.path, zshrc);
        let (references, incomplete) = built.function_references(&resolution.definitions);
        assert!(!incomplete);
        let mut reference_files = references
            .iter()
            .map(|location| location.uri.to_file_path().unwrap())
            .collect::<Vec<_>>();
        reference_files.sort();
        let mut expected = vec![zshrc.clone(), lib.clone()];
        expected.sort();
        assert_eq!(reference_files, expected);

        // A workspace file nobody sources does not inherit the definition.
        let resolution = built.function_resolution(&other, call_span(&built, &other, "myfn"));
        assert!(resolution.definitions.is_empty(), "{resolution:?}");
    });
}

#[test]
fn zdotdir_assigned_in_zshenv_locates_the_loader() {
    let (_home_dir, home) = canonical_tempdir();
    let (_workspace_dir, workspace) = canonical_tempdir();
    let lib = workspace.join("lib.zsh");
    std::fs::write(&lib, "myfn\n").unwrap();
    std::fs::write(
        home.join(".zshenv"),
        "export ZDOTDIR=\"$HOME/.config/zsh\"\n",
    )
    .unwrap();
    let zdotdir = home.join(".config/zsh");
    std::fs::create_dir_all(&zdotdir).unwrap();
    let zshrc = zdotdir.join(".zshrc");
    std::fs::write(
        &zshrc,
        format!("myfn() {{ :; }}\nsource {}/lib.zsh\n", workspace.display()),
    )
    .unwrap();

    with_test_home_dir(&home, || {
        let built = WorkspaceFunctionIndex::build(&context(&workspace)).unwrap();
        assert!(
            built.contains(&zshrc),
            "the ZDOTDIR loader should be indexed"
        );
        let resolution = built.function_resolution(&lib, call_span(&built, &lib, "myfn"));
        assert_eq!(resolution.exact().unwrap().path, zshrc);
    });
}

#[test]
fn loader_closure_is_bounded_without_marking_the_workspace_incomplete() {
    let (_home_dir, home) = canonical_tempdir();
    let (_workspace_dir, workspace) = canonical_tempdir();
    let lib = workspace.join("lib.zsh");
    std::fs::write(&lib, "myfn\n").unwrap();
    let chain = home.join("chain");
    std::fs::create_dir_all(&chain).unwrap();
    let chain_length = MAX_LOADER_FILES + 16;
    for index in 0..chain_length {
        let next = chain.join(format!("{}.zsh", index + 1));
        std::fs::write(
            chain.join(format!("{index}.zsh")),
            format!("chain_{index}() {{ :; }}\nsource {}\n", next.display()),
        )
        .unwrap();
    }
    let zshrc = home.join(".zshrc");
    std::fs::write(
        &zshrc,
        format!(
            "myfn() {{ :; }}\nsource {}/lib.zsh\nsource {}/0.zsh\n",
            workspace.display(),
            chain.display()
        ),
    )
    .unwrap();

    with_test_home_dir(&home, || {
        let built = WorkspaceFunctionIndex::build(&context(&workspace)).unwrap();
        assert!(built.contains(&zshrc));
        assert!(built.contains(&chain.join("0.zsh")));
        assert!(!built.contains(&chain.join(format!("{chain_length}.zsh"))));
        let loader_files = built
            .files
            .keys()
            .filter(|path| path.starts_with(&home))
            .count();
        assert!(loader_files <= MAX_LOADER_FILES, "{loader_files}");
        // The cap bounds work; it is not a workspace discovery failure.
        assert!(built.is_complete());
        assert!(built.incomplete_reason().is_none());
        let resolution = built.function_resolution(&lib, call_span(&built, &lib, "myfn"));
        assert_eq!(resolution.exact().unwrap().path, zshrc);
    });
}

#[test]
fn oversized_startup_files_are_not_inspected() {
    let (_home_dir, home) = canonical_tempdir();
    let (_workspace_dir, workspace) = canonical_tempdir();
    let lib = workspace.join("lib.zsh");
    std::fs::write(&lib, "myfn\n").unwrap();
    let zshrc = home.join(".zshrc");
    let mut source = format!("myfn() {{ :; }}\nsource {}/lib.zsh\n", workspace.display());
    let padding = "# ".to_owned() + &"x".repeat(1022) + "\n";
    while u64::try_from(source.len()).unwrap() <= MAX_LOADER_FILE_BYTES {
        source.push_str(&padding);
    }
    std::fs::write(&zshrc, source).unwrap();

    with_test_home_dir(&home, || {
        let built = WorkspaceFunctionIndex::build(&context(&workspace)).unwrap();
        assert!(!built.contains(&zshrc));
        assert!(built.is_complete());
    });
}
