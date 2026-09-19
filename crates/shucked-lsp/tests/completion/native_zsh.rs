use super::*;

#[test]
fn managed_zsh_completes_described_flags_without_personal_startup_files() {
    let Some(mut provider) = NativeZsh::detect() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    // Loading any personal startup file would prevent completion entirely.
    for file in [".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zcompdump"] {
        std::fs::write(root.path().join(file), "print broken-startup; exit 1\n").unwrap();
    }
    provider.zdotdir = Some(root.path().to_owned());
    let entries = provider
        .complete(
            &["ls".to_owned()],
            "-",
            root.path(),
            5000,
            false,
            &RequestCancellationToken::default(),
            None,
        )
        .expect("managed Zsh completion");
    assert!(
        entries
            .iter()
            .any(|entry| entry.text == "-a" && !entry.description.is_empty()),
        "{entries:?}"
    );
}

#[test]
fn edited_shell_syntax_is_never_executed() {
    let Some(mut provider) = NativeZsh::detect() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    provider.zdotdir = Some(root.path().to_owned());
    let marker = root.path().join("must-not-exist");
    let text = format!("$(touch {})", marker.display());
    provider.complete(
        &["ls".to_owned(), text],
        "-",
        root.path(),
        5000,
        false,
        &RequestCancellationToken::default(),
        None,
    );
    assert!(!marker.exists());
}

#[test]
fn rejects_partial_protocol_and_merges_candidate_descriptions() {
    assert!(parse_output(b"P\0").is_none());
    assert!(parse_output(b"P\x00123\0M\0--long\0description\0").is_none());
    let entries =
        parse_output(b"P\x00123\0M\0--long\0\0M\0--long\0--long -- Extended output\0E\0").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].text, "--long");
    assert_eq!(entries[0].description, "Extended output");
}

#[test]
fn personal_aliases_and_completers_require_explicit_opt_in() {
    let Some(mut provider) = NativeZsh::detect() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join(".zshrc"),
        r#"
autoload -Uz compinit
compinit -i -D
alias ls='shucked_listing --icons'
_shucked_listing() { _arguments '--absolute[Use full entry paths]' '--icons[Show entry icons]' }
compdef _shucked_listing shucked_listing
"#,
    )
    .unwrap();
    provider.zdotdir = Some(root.path().to_owned());
    let words = ["ls".to_owned()];
    let token = RequestCancellationToken::default();
    let managed = provider
        .complete(&words, "-", root.path(), 5000, false, &token, None)
        .unwrap();
    assert!(!managed.iter().any(|entry| entry.text == "--absolute"));
    let personal = provider
        .complete(&words, "-", root.path(), 5000, true, &token, None)
        .unwrap();
    assert!(
        personal
            .iter()
            .any(|entry| entry.text == "--absolute"
                && entry.description.contains("full entry paths")),
        "{personal:?}"
    );
}
