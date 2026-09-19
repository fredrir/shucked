use super::*;
use shucked_ast::Position;
use shucked_command::{CommandSite, host, resolve};
use shucked_semantic::{CommandNamespace, CommandWord};
use std::os::unix::fs::PermissionsExt;

fn fixture(tool: &str) -> (tempfile::TempDir, ExecutionContext, EnvironmentSnapshot) {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join(tool);
    let repository = root.path().join("repository");
    for dir in ["cmd", "dev-cmd"] {
        std::fs::create_dir_all(repository.join("Library/Homebrew").join(dir)).unwrap();
    }
    std::fs::write(
        repository.join("Library/Homebrew/cmd/internal-hidden.rb"),
        "# never execute this file\n",
    )
    .unwrap();
    let body = if tool == "brew" {
        format!(
            "case \"$*\" in\n'--version') printf 'Homebrew 5.0.0\\n';;\n'commands --quiet --include-aliases') printf 'install\\nlist\\ncommands\\n';;\n'--repository') printf '%s\\n' '{}';;\n*) exit 9;;\nesac\n",
            repository.display()
        )
    } else {
        "case \"$*\" in\n'--version') printf 'git version 2.50.0\\n';;\n'--list-cmds=builtins,main,others,alias') printf 'add\\ncommit\\nhelp\\nmy-extension\\nmy-alias\\n';;\n*) exit 9;;\nesac\n".into()
    };
    std::fs::write(&binary, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let context = ExecutionContext {
        target_id: root.path().display().to_string(),
        cwd: Some(root.path().into()),
        cwd_known: true,
        native_execution_allowed: true,
        ..Default::default()
    };
    let environment = host::capture(&context, vec![root.path().into()], 1);
    (root, context, environment)
}

fn site(
    words: &[&str],
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
) -> (CommandSiteFacts, CommandResolution) {
    let mut offset = 0;
    let original: Vec<_> = words
        .iter()
        .map(|word| {
            let start = offset;
            offset += word.len() + 1;
            CommandWord {
                text: Some((*word).into()),
                span: Span::from_positions(
                    Position::at(1, start + 1, start),
                    Position::at(1, offset, offset - 1),
                ),
                alias_eligible: true,
                injected: false,
            }
        })
        .collect();
    let facts = CommandSiteFacts {
        span: Span::from_positions(Position::new(), Position::at(1, offset, offset - 1)),
        words: original.clone(),
        effective_words: original,
        aliases: Vec::new(),
        visible_function: None,
        namespace: CommandNamespace::Shell,
        environment_uncertain: None,
        guarded_available: false,
    };
    let command = CommandSite {
        arguments: words.iter().skip(1).map(|word| (*word).into()).collect(),
        ..CommandSite::literal(words[0])
    };
    let resolution = resolve(context, environment, &command);
    (facts, resolution)
}

#[test]
fn brew_typo_warns_but_hidden_and_path_extension_commands_do_not() {
    let (root, context, mut environment) = fixture("brew");
    let external = root.path().join("brew-external-command");
    std::fs::write(&external, "#!/bin/sh\nexit 99\n").unwrap();
    std::fs::set_permissions(external, std::fs::Permissions::from_mode(0o755)).unwrap();
    environment = host::capture(
        &context,
        vec![root.path().into()],
        environment.generation + 1,
    );
    let sites = vec![
        site(&["brew", "instll"], &context, &environment),
        site(&["brew", "internal-hidden"], &context, &environment),
        site(&["brew", "external-command"], &context, &environment),
    ];
    let diagnostics = validate(
        &context,
        &environment,
        &sites,
        &RequestCancellationToken::default(),
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "ENV002");
    assert_eq!(diagnostics[0].span.start.offset(), 5);
    assert_eq!(diagnostics[0].suggestions, ["install"]);
}

#[test]
fn git_inventory_includes_extensions_and_aliases() {
    let (_root, context, environment) = fixture("git");
    let sites = vec![
        site(&["git", "my-extension"], &context, &environment),
        site(&["git", "my-alias"], &context, &environment),
        site(&["git", "commti"], &context, &environment),
    ];
    let diagnostics = validate(
        &context,
        &environment,
        &sites,
        &RequestCancellationToken::default(),
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].suggestions, ["commit"]);
}

#[test]
fn trust_portable_incomplete_path_and_failed_metadata_never_warn() {
    let (root, mut context, mut environment) = fixture("brew");
    let sites = vec![site(&["brew", "not-a-command"], &context, &environment)];
    context.native_execution_allowed = false;
    assert!(
        validate(
            &context,
            &environment,
            &sites,
            &RequestCancellationToken::default()
        )
        .is_empty()
    );
    context.native_execution_allowed = true;
    context.policy = ValidationPolicy::Portable;
    assert!(
        validate(
            &context,
            &environment,
            &sites,
            &RequestCancellationToken::default()
        )
        .is_empty()
    );
    context.policy = ValidationPolicy::Workspace;
    environment.path_known = false;
    assert!(
        validate(
            &context,
            &environment,
            &sites,
            &RequestCancellationToken::default()
        )
        .is_empty()
    );
    environment.path_known = true;
    std::fs::write(root.path().join("brew"), "#!/bin/sh\nexit 1\n").unwrap();
    environment = host::capture(&context, vec![root.path().into()], 2);
    let sites = vec![site(&["brew", "not-a-command"], &context, &environment)];
    assert!(
        validate(
            &context,
            &environment,
            &sites,
            &RequestCancellationToken::default()
        )
        .is_empty()
    );
}

#[test]
fn metadata_never_receives_or_executes_editor_arguments() {
    let (root, context, environment) = fixture("brew");
    let sites = vec![site(&["brew", "$(touch marker)"], &context, &environment)];
    let diagnostics = validate(
        &context,
        &environment,
        &sites,
        &RequestCancellationToken::default(),
    );
    assert_eq!(diagnostics.len(), 1);
    assert!(!root.path().join("marker").exists());
}

#[test]
fn unknown_global_options_and_child_arguments_do_not_become_strict_grammar() {
    let (_root, context, environment) = fixture("git");
    let sites = vec![
        site(
            &["git", "-C", "another-dir", "missing"],
            &context,
            &environment,
        ),
        site(
            &["git", "commit", "--some-future-flag"],
            &context,
            &environment,
        ),
    ];
    assert!(
        validate(
            &context,
            &environment,
            &sites,
            &RequestCancellationToken::default()
        )
        .is_empty()
    );
}

#[test]
fn cancelling_validation_does_not_start_metadata_queries() {
    let (_root, context, environment) = fixture("brew");
    let sites = vec![site(&["brew", "missing"], &context, &environment)];
    let cancellation = RequestCancellationToken::default();
    cancellation.cancel();
    assert!(validate(&context, &environment, &sites, &cancellation).is_empty());
}

#[test]
fn identified_flag_manifests_warn_on_typos_and_ignore_uncovered_versions() {
    let root = tempfile::tempdir().unwrap();
    for (name, output) in [
        (
            "eza",
            "eza eza - A modern, maintained replacement for ls\nv0.23.5 [+git]\n",
        ),
        ("rg", "ripgrep 15.2.0\n"),
        ("pacman", "Pacman v7.1.0 - libalpm v16.0.0\n"),
    ] {
        let path = root.path().join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\n[ \"$*\" = '--version' ] || exit 9\nprintf '%s' '{}'\n",
                output
            ),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let context = ExecutionContext {
        target_id: root.path().display().to_string(),
        cwd: Some(root.path().into()),
        cwd_known: true,
        native_execution_allowed: true,
        ..Default::default()
    };
    let environment = host::capture(&context, vec![root.path().into()], 1);
    let sites = vec![
        site(&["eza", "--icnos"], &context, &environment),
        site(&["rg", "--shucked-fixture-typo"], &context, &environment),
        site(
            &["pacman", "-S", "--shucked-fixture-typo"],
            &context,
            &environment,
        ),
    ];
    let diagnostics = validate(
        &context,
        &environment,
        &sites,
        &RequestCancellationToken::default(),
    );
    assert_eq!(diagnostics.len(), 3);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code == "ENV003")
    );
    assert!(diagnostics[0].suggestions.contains(&"--icons".into()));
    std::fs::write(
        root.path().join("eza"),
        "#!/bin/sh\nprintf 'eza - replacement\\nv999.0.0\\n'\n",
    )
    .unwrap();
    let environment = host::capture(&context, vec![root.path().into()], 2);
    let sites = vec![site(&["eza", "--new-future-flag"], &context, &environment)];
    assert!(
        validate(
            &context,
            &environment,
            &sites,
            &RequestCancellationToken::default()
        )
        .is_empty()
    );
}
