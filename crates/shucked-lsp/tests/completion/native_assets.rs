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
