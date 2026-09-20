use super::*;
use crate::TextDocument;

fn context(root: &Path) -> WorkspaceFunctionContext {
    WorkspaceFunctionContext {
        workspace_roots: vec![root.to_path_buf()],
        settings_workspace_roots: vec![root.to_path_buf()],
        workspace_settings: Vec::new(),
        global_options: ClientOptions::default(),
        open_documents: Vec::new(),
        encoding: PositionEncoding::UTF16,
        max_files: 100,
        cache: Arc::new(WorkspaceFunctionIndexCache::default()),
        epoch: 0,
        cancellation: RequestCancellationToken::default(),
    }
}

fn refresh(context: &mut WorkspaceFunctionContext) -> Arc<WorkspaceFunctionIndex> {
    context.cache.invalidate();
    context.epoch = context.cache.current_epoch();
    workspace_function_index(context).unwrap()
}

fn overlay(context: &mut WorkspaceFunctionContext, path: &Path, source: &str, version: i32) {
    let uri = types::Url::from_file_path(path).unwrap();
    context.open_documents.retain(|open| open.uri != uri);
    context.open_documents.push(WorkspaceOpenDocument {
        uri,
        document: Arc::new(
            TextDocument::new(source.into(), version).with_language_id("shellscript"),
        ),
    });
}

fn file<'a>(index: &'a WorkspaceFunctionIndex, path: &Path) -> &'a IndexedWorkspaceFile {
    index.files.get(&canonical_path(path)).unwrap()
}

fn uses(index: &WorkspaceFunctionIndex, path: &Path, name: &str) -> bool {
    index
        .variable_usage(&|| false)
        .unwrap()
        .consumed_names(path)
        .iter()
        .any(|used| used == name)
}

fn target(index: &WorkspaceFunctionIndex, path: &Path) -> Vec<PathBuf> {
    file(index, path)
        .projection
        .calls
        .source_edges
        .iter()
        .map(|edge| edge.path.clone())
        .collect()
}

#[test]
fn unchanged_files_reuse_analysis_and_projection_across_invalidation() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper.sh");
    let main = root.path().join("main.sh");
    std::fs::write(&helper, "VALUE=shared\n").unwrap();
    std::fs::write(&main, "source helper.sh\necho \"$VALUE\"\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    let after = refresh(&mut context);
    assert!(!Arc::ptr_eq(&before, &after));
    for path in [&helper, &main] {
        assert!(Arc::ptr_eq(
            &file(&before, path).analysis,
            &file(&after, path).analysis
        ));
        assert!(Arc::ptr_eq(
            &file(&before, path).projection,
            &file(&after, path).projection
        ));
    }
    assert!(uses(&after, &helper, "VALUE"));
}

#[test]
fn unsaved_consumer_edits_reuse_helpers_but_refresh_cross_file_usage() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper.sh");
    let main = root.path().join("main.sh");
    let unrelated = root.path().join("unrelated.sh");
    std::fs::write(&helper, "VALUE=shared\n").unwrap();
    std::fs::write(&main, "source helper.sh\necho \"$VALUE\"\n").unwrap();
    std::fs::write(&unrelated, "printf unrelated\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    overlay(&mut context, &main, "source helper.sh\n", 2);
    let after = refresh(&mut context);
    assert!(!uses(&after, &helper, "VALUE"));
    assert!(!Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&after, &main).analysis
    ));
    for path in [&helper, &unrelated] {
        assert!(Arc::ptr_eq(
            &file(&before, path).analysis,
            &file(&after, path).analysis
        ));
        assert!(Arc::ptr_eq(
            &file(&before, path).projection,
            &file(&after, path).projection
        ));
    }
    context.open_documents.clear();
    let closed = refresh(&mut context);
    assert!(uses(&closed, &helper, "VALUE"));
    assert_eq!(file(&closed, &main).version(), None);
}

#[test]
fn helper_edits_refresh_transitive_path_dependents_without_reparsing_them() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("paths.sh");
    let bridge = root.path().join("bridge.sh");
    let main = root.path().join("main.sh");
    let one = root.path().join("one.sh");
    let two = root.path().join("two.sh");
    std::fs::write(&helper, format!("ROOT='{}'\n", one.display())).unwrap();
    std::fs::write(&bridge, "source paths.sh\n").unwrap();
    std::fs::write(
        &main,
        "source bridge.sh\nsource \"$ROOT\"\necho \"$VALUE\"\n",
    )
    .unwrap();
    std::fs::write(&one, "VALUE=first\n").unwrap();
    std::fs::write(&two, "VALUE=second\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    assert!(uses(&before, &one, "VALUE"));
    overlay(
        &mut context,
        &helper,
        &format!("ROOT='{}'\n", two.display()),
        2,
    );
    let after = refresh(&mut context);
    assert!(!uses(&after, &one, "VALUE"));
    assert!(uses(&after, &two, "VALUE"));
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&after, &main).analysis
    ));
    assert!(!Arc::ptr_eq(
        &file(&before, &main).projection,
        &file(&after, &main).projection
    ));
    assert!(Arc::ptr_eq(
        &file(&before, &one).projection,
        &file(&after, &one).projection
    ));
    assert!(target(&after, &main).contains(&canonical_path(&two)));
}

#[test]
fn missing_targets_creation_removal_and_move_refresh_source_edges() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    let helper = root.path().join("helper");
    let moved = root.path().join("moved");
    std::fs::write(&main, "source ./helper\necho \"$VALUE\"\n").unwrap();
    let mut context = context(root.path());
    let missing = workspace_function_index(&context).unwrap();
    assert!(target(&missing, &main).is_empty());
    std::fs::write(&helper, "VALUE=shared\n").unwrap();
    let created = refresh(&mut context);
    assert!(uses(&created, &helper, "VALUE"));
    assert!(Arc::ptr_eq(
        &file(&missing, &main).analysis,
        &file(&created, &main).analysis
    ));
    assert!(!Arc::ptr_eq(
        &file(&missing, &main).projection,
        &file(&created, &main).projection
    ));
    std::fs::rename(&helper, &moved).unwrap();
    let removed = refresh(&mut context);
    assert!(target(&removed, &main).is_empty());
    assert!(!removed.contains(&canonical_path(&helper)));
    overlay(&mut context, &main, "source ./moved\necho \"$VALUE\"\n", 2);
    let renamed = refresh(&mut context);
    assert!(uses(&renamed, &moved, "VALUE"));
    assert_eq!(target(&renamed, &main), vec![canonical_path(&moved)]);
    assert!(!renamed.files.contains_key(&canonical_path(&helper)));
}

#[test]
fn new_and_deleted_consumers_refresh_usage_without_rebuilding_helper_facts() {
    let root = tempfile::tempdir().unwrap();
    let helper = root.path().join("helper.sh");
    let main = root.path().join("main.sh");
    let moved = root.path().join("moved.sh");
    std::fs::write(&helper, "VALUE=shared\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    assert!(!uses(&before, &helper, "VALUE"));
    std::fs::write(&main, "source helper.sh\necho \"$VALUE\"\n").unwrap();
    let created = refresh(&mut context);
    assert!(uses(&created, &helper, "VALUE"));
    std::fs::rename(&main, &moved).unwrap();
    let renamed = refresh(&mut context);
    assert!(uses(&renamed, &helper, "VALUE"));
    assert!(!renamed.contains(&canonical_path(&main)));
    std::fs::remove_file(&moved).unwrap();
    let deleted = refresh(&mut context);
    assert!(!uses(&deleted, &helper, "VALUE"));
    assert!(Arc::ptr_eq(
        &file(&before, &helper).projection,
        &file(&deleted, &helper).projection
    ));
}

#[test]
fn earlier_search_candidates_and_source_path_settings_invalidate_resolution() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    let one = root.path().join("one/helper.sh");
    let two = root.path().join("two/helper.sh");
    std::fs::create_dir_all(one.parent().unwrap()).unwrap();
    std::fs::create_dir_all(two.parent().unwrap()).unwrap();
    std::fs::write(&two, "VALUE=two\n").unwrap();
    std::fs::write(&main, "source helper.sh\necho \"$VALUE\"\n").unwrap();
    let config = root.path().join(".shucked.toml");
    std::fs::write(&config, "[lint]\nsource-paths = ['one', 'two']\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    assert_eq!(target(&before, &main), vec![canonical_path(&two)]);
    std::fs::write(&one, "VALUE=one\n").unwrap();
    let created = refresh(&mut context);
    assert_eq!(target(&created, &main), vec![canonical_path(&one)]);
    std::fs::write(&config, "[lint]\nsource-paths = ['two', 'one']\n").unwrap();
    let configured = refresh(&mut context);
    assert_eq!(target(&configured, &main), vec![canonical_path(&two)]);
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&configured, &main).analysis
    ));
}

#[test]
fn fresh_mutation_indexes_recheck_disk_and_preserve_open_versions() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    let helper = root.path().join("helper.sh");
    let original = "source helper.sh\necho \"$VALUE\"\n";
    std::fs::write(&helper, "VALUE=shared\n").unwrap();
    std::fs::write(&main, original).unwrap();
    let mut context = context(root.path());
    overlay(&mut context, &main, original, 1);
    let before = workspace_function_index(&context).unwrap();
    overlay(&mut context, &main, original, 2);
    let versioned = refresh(&mut context);
    assert_eq!(file(&versioned, &main).version(), Some(2));
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&versioned, &main).analysis
    ));
    assert!(Arc::ptr_eq(
        &file(&before, &main).projection,
        &file(&versioned, &main).projection
    ));
    // No watcher event: edit requests must still see the disk change.
    std::fs::write(&helper, "OTHER=changed\n").unwrap();
    let fresh = fresh_workspace_function_index(&context).unwrap();
    assert_eq!(file(&fresh, &helper).source(), "OTHER=changed\n");
    assert!(!uses(&fresh, &helper, "VALUE"));
    assert!(!Arc::ptr_eq(
        &file(&before, &helper).analysis,
        &file(&fresh, &helper).analysis
    ));
}

#[test]
fn stale_and_cancelled_builds_do_not_replace_the_reusable_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    std::fs::write(&main, "source missing.sh\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    let stale = context.clone();
    context.cache.invalidate();
    context.epoch = context.cache.current_epoch();
    assert!(workspace_function_index(&stale).is_none());
    assert!(fresh_workspace_function_index(&stale).is_none());
    context.cache.store(stale.epoch, before.clone());
    assert!(context.cache.get(context.epoch).is_none());
    assert!(!context.cache.dependency_paths().is_empty());
    context.cancellation.cancel();
    assert!(workspace_function_index(&context).is_none());
    context.cancellation = RequestCancellationToken::default();
    let after = workspace_function_index(&context).unwrap();
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&after, &main).analysis
    ));
}

#[cfg(unix)]
#[test]
fn symlink_retargeting_refreshes_edges_even_when_helper_contents_match() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    let alias = root.path().join("selected");
    let one = root.path().join("one.sh");
    let two = root.path().join("two.sh");
    std::fs::write(&main, "source ./selected\necho \"$VALUE\"\n").unwrap();
    std::fs::write(&one, "VALUE=shared\n").unwrap();
    std::fs::write(&two, "VALUE=shared\n").unwrap();
    std::os::unix::fs::symlink(&one, &alias).unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&two, &alias).unwrap();
    let after = refresh(&mut context);
    assert_eq!(target(&before, &main), vec![canonical_path(&one)]);
    assert_eq!(target(&after, &main), vec![canonical_path(&two)]);
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&after, &main).analysis
    ));
    assert!(!Arc::ptr_eq(
        &file(&before, &main).projection,
        &file(&after, &main).projection
    ));
}

#[test]
fn opening_and_closing_a_missing_helper_refreshes_unchanged_consumers() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    let helper = root.path().join("helper.sh");
    std::fs::write(&main, "source helper.sh\necho \"$VALUE\"\n").unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    assert!(target(&before, &main).is_empty());
    overlay(&mut context, &helper, "VALUE=unsaved\n", 1);
    let opened = refresh(&mut context);
    assert!(uses(&opened, &helper, "VALUE"));
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&opened, &main).analysis
    ));
    assert_eq!(target(&opened, &main), vec![canonical_path(&helper)]);
    context.open_documents.clear();
    let closed = refresh(&mut context);
    assert!(target(&closed, &main).is_empty());
    assert!(!closed.contains(&canonical_path(&helper)));
}

#[test]
fn helper_project_settings_refresh_transitive_paths() {
    let root = tempfile::tempdir().unwrap();
    let main = root.path().join("main.sh");
    let helper = root.path().join("config/paths.sh");
    let config = root.path().join("config/.shucked.toml");
    let one = root.path().join("one.sh");
    let two = root.path().join("two.sh");
    std::fs::create_dir_all(root.path().join("config/one")).unwrap();
    std::fs::create_dir_all(root.path().join("config/two")).unwrap();
    std::fs::write(root.path().join(".shucked.toml"), "[lint]\n").unwrap();
    std::fs::write(&config, "[lint]\nsource-paths = ['one']\n").unwrap();
    std::fs::write(&helper, "source selected.sh\n").unwrap();
    std::fs::write(
        root.path().join("config/one/selected.sh"),
        format!("ROOT='{}'\n", one.display()),
    )
    .unwrap();
    std::fs::write(
        root.path().join("config/two/selected.sh"),
        format!("ROOT='{}'\n", two.display()),
    )
    .unwrap();
    std::fs::write(&one, "VALUE=first\n").unwrap();
    std::fs::write(&two, "VALUE=second\n").unwrap();
    std::fs::write(
        &main,
        "source config/paths.sh\nsource \"$ROOT\"\necho \"$VALUE\"\n",
    )
    .unwrap();
    let mut context = context(root.path());
    let before = workspace_function_index(&context).unwrap();
    assert!(uses(&before, &one, "VALUE"));
    std::fs::write(&config, "[lint]\nsource-paths = ['two']\n").unwrap();
    let after = refresh(&mut context);
    assert!(!uses(&after, &one, "VALUE"));
    assert!(uses(&after, &two, "VALUE"));
    assert!(Arc::ptr_eq(
        &file(&before, &main).analysis,
        &file(&after, &main).analysis
    ));
    assert!(!Arc::ptr_eq(
        &file(&before, &main).projection,
        &file(&after, &main).projection
    ));
}

#[test]
fn model_retention_is_bounded_without_losing_facts_or_dependency_refreshes() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..=MAX_RETAINED_MODELS {
        std::fs::write(
            root.path().join(format!("consumer_{index:03}.sh")),
            "source helper.sh\necho \"$VALUE\"\n",
        )
        .unwrap();
    }
    let mut context = context(root.path());
    context.max_files = MAX_RETAINED_MODELS + 10;
    let before = workspace_function_index(&context).unwrap();
    assert!(before.complete);
    assert_eq!(
        before
            .files
            .values()
            .filter(|file| file.analysis.model.is_some())
            .count(),
        MAX_RETAINED_MODELS
    );
    let evicted = root
        .path()
        .join(format!("consumer_{MAX_RETAINED_MODELS:03}.sh"));
    assert!(file(&before, &evicted).analysis.model.is_none());
    let unchanged = refresh(&mut context);
    assert!(Arc::ptr_eq(
        &file(&before, &evicted).projection,
        &file(&unchanged, &evicted).projection
    ));
    let helper = root.path().join("helper.sh");
    std::fs::write(&helper, "VALUE=shared\n").unwrap();
    let after = refresh(&mut context);
    assert!(after.complete);
    assert_eq!(target(&after, &evicted), vec![canonical_path(&helper)]);
    assert!(!Arc::ptr_eq(
        &file(&before, &evicted).projection,
        &file(&after, &evicted).projection
    ));
}
