use super::*;

fn context(root: &Path, source: &str, max_files: usize) -> WorkspaceFunctionContext {
    WorkspaceFunctionContext {
        workspace_roots: vec![root.to_owned()],
        settings_workspace_roots: vec![root.to_owned()],
        workspace_settings: Vec::new(),
        global_options: ClientOptions::default(),
        open_documents: vec![WorkspaceOpenDocument {
            uri: types::Url::from_file_path(root.join("current.sh")).unwrap(),
            document: Arc::new(
                crate::TextDocument::new(source.into(), 1).with_language_id("shellscript"),
            ),
        }],
        encoding: PositionEncoding::UTF16,
        max_files,
        cache: Arc::default(),
        epoch: 0,
        cancellation: RequestCancellationToken::default(),
    }
}

fn call(index: &WorkspaceFunctionIndex, path: &Path, name: &str) -> Span {
    index
        .graph
        .files()
        .find(|(file, _)| *file == path)
        .unwrap()
        .1
        .call_sites
        .iter()
        .find(|call| call.callee.as_str() == name)
        .unwrap()
        .name_span
}

#[test]
fn unrelated_workspace_limit_does_not_poison_complete_command_component() {
    let root = tempfile::tempdir().unwrap();
    let root = canonical_path(root.path());
    std::fs::write(root.join("other.sh"), "source missing.sh\n").unwrap();
    let context = context(&root, "ls -\n", 1);
    let index = completion_workspace_function_index(&context).unwrap();
    assert!(
        !index.is_complete(),
        "whole-workspace safety limit remains in effect"
    );
    let path = root.join("current.sh");
    let span = call(&index, &path, "ls");
    let resolution = index.completion_function_resolution(&path, span);
    assert!(!resolution.incomplete);
    assert!(
        resolution.definitions.is_empty(),
        "ls is not a sourced function"
    );
    assert!(
        index.functions.get().is_none(),
        "completion must not evaluate unrelated roots"
    );
    assert!(
        index.function_resolution(&path, span).incomplete,
        "whole-workspace queries remain conservative"
    );
}

#[test]
fn invalid_component_configuration_keeps_completion_resolution_incomplete() {
    let root = tempfile::tempdir().unwrap();
    let root = canonical_path(root.path());
    std::fs::write(root.join(".shucked.toml"), "invalid = [").unwrap();
    let context = context(&root, "ls -\n", 20);
    let index = completion_workspace_function_index(&context).unwrap();
    let path = root.join("current.sh");
    assert!(
        index
            .completion_function_resolution(&path, call(&index, &path, "ls"))
            .incomplete
    );
}

#[test]
fn completion_component_keeps_incoming_loader_and_ordered_sibling_modules() {
    let root = tempfile::tempdir().unwrap();
    let root = canonical_path(root.path());
    std::fs::write(root.join("helpers.sh"), "helper() { :; }\n").unwrap();
    std::fs::write(
        root.join("loader.sh"),
        "source helpers.sh\nsource current.sh\n",
    )
    .unwrap();
    std::fs::write(root.join("unrelated.sh"), "source missing.sh\n").unwrap();
    let context = context(&root, "helper\n", 20);
    let index = completion_workspace_function_index(&context).unwrap();
    let path = root.join("current.sh");
    let resolution = index.completion_function_resolution(&path, call(&index, &path, "helper"));
    assert_eq!(resolution.exact().unwrap().path, root.join("helpers.sh"));
    assert!(
        index
            .function_completions(&path, 0)
            .iter()
            .any(|item| item.name.as_str() == "helper" && !item.possible)
    );
}

#[test]
fn interrupted_index_progress_reuses_validated_projections_and_refreshes_changes() {
    let root = tempfile::tempdir().unwrap();
    let root = canonical_path(root.path());
    std::fs::write(root.join("helper.sh"), "before() { :; }\n").unwrap();
    let mut context = context(&root, "source helper.sh\nbefore\n", 20);
    // A projection build can finish before its obsolete epoch is published.
    let first = WorkspaceFunctionIndex::build_projections(&context).unwrap();
    let helper = root.join("helper.sh");
    context.cache.invalidate();
    context.epoch = context.cache.current_epoch();
    let second = completion_workspace_function_index(&context).unwrap();
    assert!(
        Arc::ptr_eq(
            &first.files[&helper].projection,
            &second.files[&helper].projection
        ),
        "completed per-file work survives an unpublished index"
    );
    std::fs::write(&helper, "after() { :; }\n").unwrap();
    context.cache.invalidate();
    context.epoch = context.cache.current_epoch();
    let third = completion_workspace_function_index(&context).unwrap();
    let path = root.join("current.sh");
    let names: Vec<_> = third
        .function_completions(&path, 18)
        .into_iter()
        .map(|item| item.name.to_string())
        .collect();
    assert!(names.contains(&"after".to_owned()));
    assert!(!names.contains(&"before".to_owned()));
}
