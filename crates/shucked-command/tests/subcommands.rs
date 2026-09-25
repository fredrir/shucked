#![cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use shucked_command::subcommands::*;
use shucked_command::*;

fn executable(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Distinct modification times keep identities apart within one test.
    std::thread::sleep(std::time::Duration::from_millis(15));
}

fn context(root: &Path) -> ExecutionContext {
    ExecutionContext {
        target_id: "fixture-host".into(),
        cwd: Some(root.into()),
        cwd_known: true,
        native_execution_allowed: true,
        ..Default::default()
    }
}

fn identity(environment: &EnvironmentSnapshot, name: &str) -> ExecutableIdentity {
    match environment.lookup(name) {
        LookupEvidence::Present(executable) => executable.identity,
        other => panic!("{name} not present: {other:?}"),
    }
}

fn names(inventory: &SubcommandInventory) -> Vec<&str> {
    inventory
        .commands
        .iter()
        .map(|entry| entry.name.as_str())
        .collect()
}

fn description<'a>(inventory: &'a SubcommandInventory, name: &str) -> &'a str {
    &inventory
        .commands
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} missing from {inventory:?}"))
        .description
}

#[test]
fn brew_and_git_listings_are_names_with_bundled_summaries() {
    let brew =
        parse_brew("==> Built-in commands\ninstall list\nupgrade\n==> Aliases\nls\n").unwrap();
    let inventory = SubcommandInventory {
        schema_version: 1,
        tool: "brew".into(),
        executable: "/x/brew".into(),
        size: None,
        modified_unix_ms: None,
        captured_unix_ms: 0,
        commands: brew,
    };
    assert_eq!(names(&inventory), ["install", "list", "ls", "upgrade"]);
    assert_eq!(
        description(&inventory, "install"),
        "Install a formula or cask"
    );
    assert!(parse_brew("list\nupgrade\n").is_none(), "no install anchor");
    assert!(parse_brew("").is_none());

    let git = parse_git_list("add\ncommit\nmy-alias\n--bogus\n").unwrap();
    assert_eq!(
        git.iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["add", "commit", "my-alias"]
    );
    assert_eq!(
        git[1].description,
        "Record staged changes to the repository"
    );
    assert_eq!(git[2].description, "");
}

#[test]
fn git_help_sections_keep_the_printed_summaries() {
    let text = "See 'git help <command>' to read about a specific subcommand\n\n\
                Main Porcelain Commands\n   add                  Stage it (fixture wording)\n   \
                commit               Record it. \n\nAncillary Commands / Manipulators\n   \
                config               Tweak settings\n\nCommand aliases\n   co     checkout\n";
    let git = parse_git_help(text).unwrap();
    let inventory = SubcommandInventory {
        schema_version: 1,
        tool: "git".into(),
        executable: "/x/git".into(),
        size: None,
        modified_unix_ms: None,
        captured_unix_ms: 0,
        commands: git,
    };
    assert_eq!(names(&inventory), ["add", "co", "commit", "config"]);
    assert_eq!(description(&inventory, "add"), "Stage it (fixture wording)");
    assert_eq!(description(&inventory, "commit"), "Record it");
    assert_eq!(description(&inventory, "co"), "checkout");
}

#[test]
fn docker_and_kubectl_help_sections_are_parsed_by_heading() {
    let docker = "\nUsage:  docker [OPTIONS] COMMAND\n\nA fixture runtime\n\nCommon Commands:\n  \
                  run         Start a fixture container\n  exec        Run inside a container\n\n\
                  Management Commands:\n  builder     Manage builds\n  buildx*     Extended builds\n\n\
                  Commands:\n  attach      Attach to a container\n\nGlobal Options:\n      \
                  --config string      Location of client config files\n\nRun 'docker COMMAND --help'\n";
    let inventory = parse_docker_help(docker).unwrap();
    let docker = SubcommandInventory {
        schema_version: 1,
        tool: "docker".into(),
        executable: "/x/docker".into(),
        size: None,
        modified_unix_ms: None,
        captured_unix_ms: 0,
        commands: inventory,
    };
    assert_eq!(
        names(&docker),
        ["attach", "builder", "buildx", "exec", "run"]
    );
    assert_eq!(description(&docker, "buildx"), "Extended builds");
    assert!(parse_docker_help("Global Options:\n  --config string   x\n").is_none());

    let kubectl = "kubectl controls the cluster.\n\n Find more information at: https://example.test/\n\n\
                   Basic Commands (Beginner):\n  create          Create a resource\n  \
                   run             Run an image\n\nDeploy Commands:\n  rollout         Manage rollouts\n\n\
                   Other Commands:\n  get             Display resources\n\nUsage:\n  kubectl [flags] [options]\n";
    let inventory = parse_kubectl_help(kubectl).unwrap();
    let kubectl = SubcommandInventory {
        schema_version: 1,
        tool: "kubectl".into(),
        executable: "/x/kubectl".into(),
        size: None,
        modified_unix_ms: None,
        captured_unix_ms: 0,
        commands: inventory,
    };
    assert_eq!(names(&kubectl), ["create", "get", "rollout", "run"]);
    assert_eq!(description(&kubectl, "rollout"), "Manage rollouts");
    assert!(
        parse_kubectl_help("Usage:\n  kubectl [flags]\n").is_none(),
        "no get anchor"
    );
}

#[test]
fn inventories_are_cached_by_identity_until_the_snapshot_changes() {
    invalidate();
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    let tool = root.path().join("brew");
    let runs = root.path().join("runs");
    executable(
        &tool,
        &format!(
            "printf x >> '{}'\n[ \"$*\" = 'commands --quiet --include-aliases' ] || exit 9\nprintf 'install\\nlist\\n'",
            runs.display()
        ),
    );
    let context = context(root.path());
    let environment = host::capture(&context, vec![root.path().into()], 1);
    let first = identity(&environment, "brew");
    assert!(cached(&first, Some(&cache)).is_none());
    let inventory = acquire(&context, &environment, &first, Some(&cache), &|| false).unwrap();
    assert_eq!(names(&inventory), ["install", "list"]);
    assert_eq!(std::fs::read(&runs).unwrap().len(), 1);
    // Served from memory, then from disk once memory is cleared.
    assert!(acquire(&context, &environment, &first, Some(&cache), &|| false).is_some());
    invalidate();
    assert_eq!(
        names(&cached(&first, Some(&cache)).unwrap()),
        ["install", "list"]
    );
    assert_eq!(std::fs::read(&runs).unwrap().len(), 1, "no second run");

    // A changed executable is invisible to the old identity ...
    executable(
        &tool,
        &format!(
            "printf x >> '{}'\nprintf 'install\\nlist\\nupgrade\\n'",
            runs.display()
        ),
    );
    assert_eq!(
        names(&acquire(&context, &environment, &first, Some(&cache), &|| false).unwrap()),
        ["install", "list"]
    );
    assert_eq!(std::fs::read(&runs).unwrap().len(), 1);
    // ... and yields a new inventory once the snapshot is refreshed.
    let refreshed = host::capture(&context, vec![root.path().into()], 2);
    let second = identity(&refreshed, "brew");
    assert_ne!(first.modified_unix_ms, second.modified_unix_ms);
    assert!(cached(&second, Some(&cache)).is_none());
    let inventory = acquire(&context, &refreshed, &second, Some(&cache), &|| false).unwrap();
    assert_eq!(names(&inventory), ["install", "list", "upgrade"]);
    assert_eq!(std::fs::read(&runs).unwrap().len(), 2);
    let files: Vec<_> = std::fs::read_dir(cache.join("subcommands"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].starts_with("brew-") && files[0].ends_with(".json"));
    invalidate();
}

#[test]
fn untrusted_or_frozen_contexts_never_run_the_tool() {
    invalidate();
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("executed");
    executable(
        &root.path().join("git"),
        &format!("printf x > '{}'\nprintf 'commit\\n'", marker.display()),
    );
    for (allowed, policy, fresh) in [
        (false, ValidationPolicy::Workspace, true),
        (true, ValidationPolicy::Captured, true),
        (true, ValidationPolicy::Portable, true),
        (true, ValidationPolicy::Workspace, false),
    ] {
        let mut context = context(root.path());
        let mut environment = host::capture(&context, vec![root.path().into()], 0);
        let identity = identity(&environment, "git");
        context.native_execution_allowed = allowed;
        context.policy = policy;
        environment.fresh = fresh;
        assert!(!execution_permitted(&context, &environment));
        assert!(acquire(&context, &environment, &identity, None, &|| false).is_none());
    }
    assert!(!marker.exists());
    let context = context(root.path());
    let environment = host::capture(&context, vec![root.path().into()], 0);
    assert!(execution_permitted(&context, &environment));
    let mut other = context.clone();
    other.target_id = "elsewhere".into();
    assert!(!execution_permitted(&other, &environment));
    // Unsupported tools are never queried either.
    assert_eq!(tool_name(Path::new("/usr/bin/git")), Some("git"));
    assert_eq!(
        tool_name(Path::new("C:/tools/kubectl.exe")),
        Some("kubectl")
    );
    assert_eq!(tool_name(Path::new("/usr/bin/ls")), None);
    invalidate();
}

#[test]
fn git_falls_back_to_help_listing_and_queries_run_without_inherited_environment() {
    invalidate();
    let root = tempfile::tempdir().unwrap();
    let seen = root.path().join("environment");
    executable(
        &root.path().join("git"),
        &format!(
            "/usr/bin/env > '{seen}'\ncase \"$1\" in\n  help) printf 'Main Porcelain Commands\\n   commit    Record it\\n   push      Send it\\n' ;;\n  *) exit 3 ;;\nesac",
            seen = seen.display()
        ),
    );
    let context = context(root.path());
    let environment = host::capture(&context, vec![root.path().into()], 0);
    let identity = identity(&environment, "git");
    unsafe {
        std::env::set_var("SHUCKED_SUBCOMMAND_TEST_SECRET", "must-not-leak");
    }
    let inventory = acquire(&context, &environment, &identity, None, &|| false).unwrap();
    unsafe {
        std::env::remove_var("SHUCKED_SUBCOMMAND_TEST_SECRET");
    }
    assert_eq!(names(&inventory), ["commit", "push"]);
    assert_eq!(description(&inventory, "push"), "Send it");
    let observed = std::fs::read_to_string(&seen).unwrap();
    assert!(!observed.contains("must-not-leak"), "{observed}");
    assert!(observed.contains("GIT_TERMINAL_PROMPT=0"), "{observed}");
    assert!(observed.contains("TERM=dumb"), "{observed}");
    invalidate();
}

#[test]
fn slow_and_oversized_listings_are_discarded() {
    invalidate();
    let root = tempfile::tempdir().unwrap();
    executable(&root.path().join("docker"), "/bin/sleep 30");
    let context = context(root.path());
    let environment = host::capture(&context, vec![root.path().into()], 0);
    let identity = identity(&environment, "docker");
    let started = std::time::Instant::now();
    let cancelled = || started.elapsed() > std::time::Duration::from_millis(200);
    assert!(acquire(&context, &environment, &identity, None, &cancelled).is_none());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let huge = format!(
        "Commands:\n  run  x\n{}",
        (0..6000)
            .map(|index| format!("  cmd{index}  x\n"))
            .collect::<String>()
    );
    assert!(parse_docker_help(&huge).is_none(), "command count cap");
    invalidate();
}
