//! Provider eligibility under environment uncertainty.
//!
//! Diagnostics keep treating every uncertainty as a reason not to report a
//! missing command. Completion distinguishes the reasons: a name keeps its
//! provider grammar through a function body, an earlier `source`, an extended
//! PATH, a relative or unreadable PATH entry, and the portable policy, while a
//! known function or alias for that exact name, a replaced PATH, or a dynamic
//! name withholds it.
use super::*;
use shucked_command::{
    CommandResolution, CommandSite, EnvironmentSnapshot, ExecutionContext, LookupMode,
    ValidationPolicy,
};
use shucked_parser::ShellDialect;
use shucked_semantic::{CommandNamespace, CommandSiteFacts, EnvironmentUncertainty};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn executable(directory: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn last_site(source: &str, dialect: ShellDialect) -> CommandSiteFacts {
    let parsed = shucked_parser::parser::Parser::with_dialect(source, dialect)
        .without_alias_expansion()
        .parse();
    let indexer = shucked_indexer::Indexer::new(source, &parsed);
    shucked_semantic::SemanticModel::build_with_options(
        &parsed.file,
        source,
        &indexer,
        shucked_semantic::SemanticBuildOptions {
            shell_profile: Some(shucked_parser::ShellProfile::native(dialect)),
            ..Default::default()
        },
    )
    .command_site_facts()
    .pop()
    .expect("a command site")
}

/// The site the command handler derives from semantic facts.
fn command_site(facts: &CommandSiteFacts) -> CommandSite {
    let name = facts.name().map(str::to_owned);
    CommandSite {
        name: name.clone(),
        arguments: facts
            .effective_words
            .iter()
            .skip(1)
            .map(|word| word.text.clone().unwrap_or_default())
            .collect(),
        alias_eligible: facts.aliases.is_empty()
            && facts.words.first().is_some_and(|word| word.alias_eligible),
        lookup: match facts.namespace {
            CommandNamespace::Shell => LookupMode::Normal,
            CommandNamespace::Builtin => LookupMode::BuiltinOnly,
            CommandNamespace::ExternalOrBuiltin => LookupMode::Command,
            CommandNamespace::External => LookupMode::ExternalOnly,
        },
        functions: if facts.visible_function.is_some() {
            name.into_iter().collect()
        } else {
            BTreeSet::new()
        },
        environment_uncertain: facts.environment_uncertain.is_some(),
        ..Default::default()
    }
}

struct Host {
    context: ExecutionContext,
    environment: EnvironmentSnapshot,
}

impl Host {
    /// A workspace host whose launch directory is assumed, as for an editor
    /// without an explicit cwd setting.
    fn new(root: &Path, paths: Vec<PathBuf>) -> Self {
        let context = ExecutionContext {
            cwd: Some(root.to_owned()),
            cwd_known: false,
            ..ExecutionContext::default()
        };
        let environment = shucked_command::host::capture(&context, paths, 1);
        Self {
            context,
            environment,
        }
    }

    fn resolve(&mut self, facts: &CommandSiteFacts) -> CommandResolution {
        if self.context.policy == ValidationPolicy::Workspace
            && let Some(name) = facts.name()
        {
            shucked_command::host::refresh_exact(
                &self.context,
                &mut self.environment,
                &[name.to_owned()],
            );
        }
        shucked_command::resolve(&self.context, &self.environment, &command_site(facts))
    }

    fn allowed(&mut self, source: &str, dialect: ShellDialect) -> bool {
        let facts = last_site(source, dialect);
        let resolution = self.resolve(&facts);
        completion_allowed(&facts, &resolution, &self.environment)
    }
}

#[test]
fn host_grammar_survives_functions_sources_directory_changes_and_path_extensions() {
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("bin"), "git");
    let mut host = Host::new(root.path(), vec![root.path().join("bin")]);
    for (source, dialect, reason) in [
        (
            "source ./env.sh\ngit ",
            ShellDialect::Bash,
            EnvironmentUncertainty::SourceOrEval,
        ),
        (
            "eval \"$(brew shellenv)\"\ngit ",
            ShellDialect::Zsh,
            EnvironmentUncertainty::SourceOrEval,
        ),
        (
            "autoload -Uz compinit\ncompinit\ngit ",
            ShellDialect::Zsh,
            EnvironmentUncertainty::Autoload,
        ),
        (
            "cd /tmp\ngit ",
            ShellDialect::Bash,
            EnvironmentUncertainty::WorkingDirectoryChange,
        ),
        (
            "deploy() {\n  git \n}\n",
            ShellDialect::Bash,
            EnvironmentUncertainty::InFunction,
        ),
        (
            "export PATH=\"$HOME/bin:$PATH\"\ngit ",
            ShellDialect::Bash,
            EnvironmentUncertainty::PathExtended,
        ),
        (
            "path+=(/opt/bin)\ngit ",
            ShellDialect::Zsh,
            EnvironmentUncertainty::PathExtended,
        ),
        (
            "typeset -U path\npath=(~/bin $path)\ngit ",
            ShellDialect::Zsh,
            EnvironmentUncertainty::PathExtended,
        ),
        (
            "PATH=/toolchain/bin make\ngit ",
            ShellDialect::Bash,
            EnvironmentUncertainty::PathExtended,
        ),
        (
            "PATH=$PATH:/toolchain/bin git ",
            ShellDialect::Bash,
            EnvironmentUncertainty::PathExtended,
        ),
    ] {
        let facts = last_site(source, dialect);
        assert_eq!(facts.uncertainty(), Some(reason), "{source:?}");
        // Diagnostics still cannot claim absence at such a site.
        assert!(
            matches!(host.resolve(&facts), CommandResolution::Unknown(_)),
            "{source:?}"
        );
        assert!(host.allowed(source, dialect), "{source:?}");
    }
}

#[test]
fn replaced_path_known_shadows_and_dynamic_names_withhold_host_grammar() {
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("bin"), "git");
    let mut host = Host::new(root.path(), vec![root.path().join("bin")]);
    for (source, dialect) in [
        ("export PATH=/usr/bin\ngit ", ShellDialect::Bash),
        ("PATH=/toolchain/bin git ", ShellDialect::Bash),
        (
            "export PATH=/usr/bin\ndeploy() {\n  git \n}\n",
            ShellDialect::Bash,
        ),
        ("git() { :; }\ngit ", ShellDialect::Bash),
        ("alias git='hub | cat'\ngit ", ShellDialect::Zsh),
        ("$tool ", ShellDialect::Bash),
        ("env -i git ", ShellDialect::Bash),
        // A directory change only matters for a relative executable path.
        ("cd /tmp\n./git ", ShellDialect::Bash),
    ] {
        assert!(!host.allowed(source, dialect), "{source:?}");
    }
}

#[test]
fn builtins_keep_their_identity_through_path_replacement() {
    let root = tempfile::tempdir().unwrap();
    let mut host = Host::new(root.path(), vec![root.path().join("bin")]);
    assert!(host.allowed("export PATH=/usr/bin\ncd ", ShellDialect::Bash));
    assert!(host.allowed("deploy() {\n  cd \n}\n", ShellDialect::Bash));
    let facts = last_site("export PATH=/usr/bin\ncd ", ShellDialect::Bash);
    assert!(!uncertainty_blocks(&facts, true));
    assert!(uncertainty_blocks(&facts, false));
}

#[test]
fn relative_empty_and_unreadable_path_entries_do_not_withhold_grammar() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    executable(&bin, "shucked-fixture");
    std::fs::create_dir_all(root.path().join("rel")).unwrap();
    let locked = root.path().join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    for paths in [
        vec![PathBuf::from("rel"), bin.clone()],
        vec![PathBuf::new(), bin.clone()],
        vec![PathBuf::from("~/bin"), bin.clone()],
        vec![locked.clone(), bin.clone()],
        vec![PathBuf::from("node_modules/.bin"), bin.clone()],
    ] {
        let mut host = Host::new(root.path(), paths.clone());
        let facts = last_site("shucked-fixture --abs", ShellDialect::Bash);
        let resolution = host.resolve(&facts);
        assert!(
            completion_allowed(&facts, &resolution, &host.environment),
            "{paths:?}: {resolution:?}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn portable_policy_suppresses_diagnostics_but_not_provider_grammar() {
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("bin"), "shucked-fixture");
    let mut host = Host::new(root.path(), vec![root.path().join("bin")]);
    host.context.policy = ValidationPolicy::Portable;
    let facts = last_site("shucked-fixture --abs", ShellDialect::Bash);
    let resolution = host.resolve(&facts);
    assert!(matches!(
        &resolution,
        CommandResolution::Unknown(unknown)
            if unknown.reason == shucked_command::UnknownReason::PortablePolicy
    ));
    assert!(completion_allowed(&facts, &resolution, &host.environment));
    assert!(!host.allowed(
        "shucked-fixture() { :; }\nshucked-fixture --abs",
        ShellDialect::Bash
    ));
}

#[test]
fn workspace_function_bindings_and_unregistered_names_follow_the_command_handler() {
    let root = tempfile::tempdir().unwrap();
    let mut host = Host::new(root.path(), vec![root.path().join("bin")]);
    // A missing name without shadows still lets bundled definitions apply.
    assert!(host.allowed("unregistered --", ShellDialect::Bash));
    // The command handler marks an ambiguous workspace function binding with
    // its own detail; that remains a possible shadow.
    let facts = last_site("git ", ShellDialect::Bash);
    let ambiguous = CommandResolution::Unknown(shucked_command::UnknownCommand {
        name: Some("git".into()),
        reason: shucked_command::UnknownReason::DynamicEnvironment,
        detail: "Workspace function binding depends on source or execution context".into(),
    });
    assert!(!completion_allowed(&facts, &ambiguous, &host.environment));
    let resolution = host.resolve(&last_site("source x\ngit ", ShellDialect::Bash));
    assert!(matches!(
        &resolution,
        CommandResolution::Unknown(unknown)
            if unknown.detail == shucked_command::DYNAMIC_ENVIRONMENT_DETAIL
    ));
}

#[test]
fn live_and_directory_gates_share_the_uncertainty_classification() {
    for (source, dialect, blocked) in [
        ("source x\nls ", ShellDialect::Zsh, false),
        ("f() { ls ; }", ShellDialect::Bash, false),
        ("PATH=/x\nls ", ShellDialect::Bash, true),
        ("alias ls='eza | cat'\nls ", ShellDialect::Zsh, true),
    ] {
        let facts = last_site(source, dialect);
        assert_eq!(uncertainty_blocks(&facts, false), blocked, "{source:?}");
    }
}
