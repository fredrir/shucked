use super::*;

#[test]
fn managed_windows_shells_use_the_msys_closure_before_path_fallback() {
    let root = tempfile::tempdir().unwrap();
    let binaries = root.path().join("runtime/msys/usr/bin");
    std::fs::create_dir_all(&binaries).unwrap();
    for name in ["bash", "zsh", "fish"] {
        let executable = binaries.join(format!("{name}.exe"));
        std::fs::write(&executable, b"fixture").unwrap();
        assert_eq!(managed_shell(root.path(), name, true), Some(executable));
        assert_eq!(managed_shell(root.path(), name, false), None);
    }
}

#[test]
fn windows_worker_paths_support_drives_unc_and_extended_paths() {
    for (native, posix) in [
        (
            r"C:\Users\Fixture Space\providers",
            "/c/Users/Fixture Space/providers",
        ),
        (r"D:/tools/zsh.exe", "/d/tools/zsh.exe"),
        (r"\\server\share\tool.exe", "//server/share/tool.exe"),
        (r"\\?\C:\tools\git.exe", "/c/tools/git.exe"),
        (r"\\?\UNC\server\share\tool.exe", "//server/share/tool.exe"),
        ("/usr/bin/git", "/usr/bin/git"),
    ] {
        assert_eq!(windows_shell_path(native), posix);
    }
}

#[test]
fn windows_shell_arguments_quote_wildcards_and_preserve_escaping() {
    for (argument, quoted) in [
        ("*?[x]", "\"*?[x]\""),
        ("", "\"\""),
        ("a b", "\"a b\""),
        ("a\"b", "\"a\\\"b\""),
        ("a\\", "\"a\\\\\""),
        ("a\\\"b", "\"a\\\\\\\"b\""),
    ] {
        assert_eq!(quote_windows_argument(argument), quoted);
    }
}

#[test]
fn windows_primary_completion_names_ignore_executable_suffix_case() {
    assert_eq!(without_exe_suffix("git.EXE"), "git");
    assert_eq!(without_exe_suffix("git.exe"), "git");
    assert_eq!(without_exe_suffix("git"), "git");
    assert_eq!(without_exe_suffix("éx"), "éx");
}

#[test]
fn engine_layouts_cover_versioned_and_nested_function_trees() {
    let root = tempfile::tempdir().unwrap();
    let versioned = root.path().join("share/zsh/5.9/functions");
    let nested = root.path().join("share/zsh/functions/Completion/Unix");
    let site = root.path().join("share/zsh/site-functions");
    for directory in [&versioned, &nested, &site] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(versioned.join("_ls"), "#compdef ls\n").unwrap();
    std::fs::write(nested.join("_eza"), "#compdef eza\n").unwrap();
    std::fs::create_dir_all(root.path().join("share/zsh/help")).unwrap();
    let directories = engine_layout_directories([root.path()].into_iter());
    assert_eq!(
        directories,
        [
            site,
            versioned,
            root.path().join("share/zsh/functions"),
            root.path().join("share/zsh/functions/Completion"),
            nested,
        ]
    );
    assert!(engine_layout_directories([Path::new("/nonexistent-prefix")].into_iter()).is_empty());
}

#[cfg(unix)]
#[test]
fn primary_binding_is_skipped_when_the_worker_path_already_resolves_the_same_file() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    let shadow = root.path().join("shadow");
    for directory in [&bin, &shadow] {
        std::fs::create_dir_all(directory).unwrap();
        let tool = directory.join("brew");
        std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let primary = bin.join("brew");
    let mut command = std::process::Command::new("unused");
    command.env("PATH", std::env::join_paths([&bin, &shadow]).unwrap());
    assert!(
        bind_primary(&mut command, Some(primary.to_str().unwrap()))
            .unwrap()
            .is_none(),
        "the tool keeps its real location"
    );
    let mut command = std::process::Command::new("unused");
    command.env("PATH", std::env::join_paths([&shadow, &bin]).unwrap());
    let binding = bind_primary(&mut command, Some(primary.to_str().unwrap()))
        .unwrap()
        .expect("a shadowed primary is pinned");
    let worker_path = command
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .unwrap()
        .1
        .unwrap()
        .to_owned();
    assert_eq!(
        std::env::split_paths(&worker_path).next().unwrap(),
        binding.path()
    );
    assert_eq!(
        std::fs::read_link(binding.path().join("brew")).unwrap(),
        primary
    );
}

#[test]
fn completion_dumps_are_keyed_by_engine_and_definition_directories() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("cache");
    let previous = std::env::var_os("SHUCKED_CACHE_DIR");
    // The variable is process-wide; keep the window short and restore it.
    unsafe { std::env::set_var("SHUCKED_CACHE_DIR", &cache) };
    let shell = root.path().join("zsh");
    std::fs::write(&shell, "engine").unwrap();
    let installed = root.path().join("share/zsh/site-functions");
    std::fs::create_dir_all(&installed).unwrap();
    let first = completion_dump(&shell, None, std::slice::from_ref(&installed)).unwrap();
    let same = completion_dump(&shell, None, std::slice::from_ref(&installed)).unwrap();
    let other_set = completion_dump(&shell, None, &[]).unwrap();
    match previous {
        Some(value) => unsafe { std::env::set_var("SHUCKED_CACHE_DIR", value) },
        None => unsafe { std::env::remove_var("SHUCKED_CACHE_DIR") },
    }
    assert_eq!(first, same);
    assert_ne!(first, other_set);
    assert!(first.starts_with(cache.join("completion/zsh")));
    assert!(
        first
            .extension()
            .is_some_and(|extension| extension == "zcompdump")
    );
}
