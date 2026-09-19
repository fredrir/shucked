use shucked_indexer::Indexer;
use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};
use shucked_semantic::{CommandNamespace, CommandSiteFacts, SemanticBuildOptions, SemanticModel};
fn analyze(source: &str, dialect: ShellDialect) -> Vec<CommandSiteFacts> {
    let parsed = Parser::with_dialect(source, dialect)
        .without_alias_expansion()
        .parse();
    let indexer = Indexer::new(source, &parsed);
    SemanticModel::build_with_options(
        &parsed.file,
        source,
        &indexer,
        SemanticBuildOptions {
            shell_profile: Some(ShellProfile::native(dialect)),
            ..Default::default()
        },
    )
    .command_site_facts()
}
#[test]
fn aliases_expand_with_injected_arguments_and_original_name_span() {
    let source = "alias ls='eza --icons'\nls --long\n";
    let sites = analyze(source, ShellDialect::Zsh);
    let site = sites.last().unwrap();
    assert_eq!(site.name(), Some("eza"));
    assert_eq!(
        site.effective_words
            .iter()
            .map(|w| w.text.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["eza", "--icons", "--long"]
    );
    assert_eq!(
        &source[site.name_span().start.offset()..site.name_span().end.offset()],
        "ls"
    );
    assert!(site.effective_words[1].injected);
}
#[test]
fn alias_definitions_do_not_rewrite_quoted_or_same_line_commands() {
    let sites = analyze(
        "alias ls=eza; ls\n'ls'\n\\ls\nunalias ls\nls\n",
        ShellDialect::Zsh,
    );
    assert_eq!(sites.iter().filter(|s| s.name() == Some("ls")).count(), 4);
    assert!(sites.iter().all(|s| s.aliases.is_empty()));
}
#[test]
fn bash_script_aliases_require_expand_aliases() {
    assert_eq!(
        analyze("alias ls=eza\nls\n", ShellDialect::Bash)
            .last()
            .unwrap()
            .name(),
        Some("ls")
    );
    assert_eq!(
        analyze(
            "shopt -s expand_aliases\nalias ls=eza\nls\n",
            ShellDialect::Bash
        )
        .last()
        .unwrap()
        .name(),
        Some("eza")
    );
}
#[test]
fn functions_shadow_executables_but_command_wrapper_bypasses_functions() {
    let sites = analyze("ls() { :; }\nls\ncommand ls\n", ShellDialect::Bash);
    let direct = sites
        .iter()
        .find(|s| s.name() == Some("ls") && s.namespace == CommandNamespace::Shell)
        .unwrap();
    assert!(direct.visible_function.is_some());
    let wrapped = sites.last().unwrap();
    assert_eq!(wrapped.name(), Some("ls"));
    assert_eq!(wrapped.namespace, CommandNamespace::ExternalOrBuiltin);
    assert!(wrapped.visible_function.is_none());
}
#[test]
fn only_dominated_uses_of_the_checked_command_are_guarded() {
    let sites = analyze(
        "if command -v optional; then optional; unrelated; fi\noptional\ncommand -v other && other\nother\n",
        ShellDialect::Bash,
    );
    let optional = sites
        .iter()
        .filter(|s| s.name() == Some("optional"))
        .collect::<Vec<_>>();
    assert!(optional[0].guarded_available);
    assert!(!optional[1].guarded_available);
    assert!(
        !sites
            .iter()
            .find(|s| s.name() == Some("unrelated"))
            .unwrap()
            .guarded_available
    );
    let other = sites
        .iter()
        .filter(|s| s.name() == Some("other"))
        .collect::<Vec<_>>();
    assert!(other[0].guarded_available);
    assert!(!other[1].guarded_available);
}
#[test]
fn dynamic_names_and_environment_mutations_weaken_absence_evidence() {
    for source in [
        "$cmd\n",
        "PATH=$target\nmissing\n",
        "cd somewhere\nmissing\n",
        "source unknown.sh\nmissing\n",
        "PATH=/other missing\n",
        "env -i missing\n",
    ] {
        let sites = analyze(source, ShellDialect::Bash);
        assert!(
            sites.last().unwrap().environment_uncertain.is_some(),
            "{source}"
        );
    }
}
#[test]
fn opaque_aliases_do_not_adopt_external_tool_grammar() {
    let sites = analyze("alias ls='eza | cat'\nls -x\n", ShellDialect::Zsh);
    assert!(sites.last().unwrap().environment_uncertain.is_some());
    assert!(sites.last().unwrap().aliases.is_empty());
}
