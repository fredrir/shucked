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
#[test]
fn negative_checks_guard_else_and_termination_successors_only() {
    let sites = analyze(
        "if ! command -v optional; then optional; else optional; fi\noptional\nif ! command -v other; then exit 1; fi\nother\nunrelated\n",
        ShellDialect::Bash,
    );
    let optional = sites
        .iter()
        .filter(|s| s.name() == Some("optional"))
        .map(|s| s.guarded_available)
        .collect::<Vec<_>>();
    assert_eq!(optional, [false, true, false]);
    assert!(
        sites
            .iter()
            .find(|s| s.name() == Some("other"))
            .unwrap()
            .guarded_available
    );
    assert!(
        !sites
            .iter()
            .find(|s| s.name() == Some("unrelated"))
            .unwrap()
            .guarded_available
    );
}
#[test]
fn terminating_boolean_guards_are_scoped_to_their_sequence() {
    let sites = analyze(
        "command -v optional || exit 1\noptional\nif test x; then command -v branch || exit; branch; fi\nbranch\n",
        ShellDialect::Bash,
    );
    assert!(
        sites
            .iter()
            .find(|s| s.name() == Some("optional"))
            .unwrap()
            .guarded_available
    );
    let branch = sites
        .iter()
        .filter(|s| s.name() == Some("branch"))
        .map(|s| s.guarded_available)
        .collect::<Vec<_>>();
    assert_eq!(branch, [true, false]);
}
#[test]
fn optional_check_negation_cannot_prove_availability_in_missing_branch() {
    let sites = analyze(
        "! command -v optional && optional\n! command -v optional || optional\ncommand -v optional || echo unavailable\noptional\n",
        ShellDialect::Bash,
    );
    assert_eq!(
        sites
            .iter()
            .filter(|s| s.name() == Some("optional"))
            .map(|s| s.guarded_available)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
}
#[test]
fn shadowed_checks_or_exits_and_background_checks_do_not_prove_availability() {
    for source in [
        "command() { :; }\nif command -v optional; then optional; fi\n",
        "exit() { :; }\ncommand -v optional || exit\noptional\n",
        "if ! command -v optional; then exit & fi\noptional\n",
        "if PATH=/elsewhere command -v optional; then optional; fi\n",
    ] {
        let sites = analyze(source, ShellDialect::Bash);
        assert!(
            !sites
                .iter()
                .find(|s| s.name() == Some("optional"))
                .unwrap()
                .guarded_available,
            "{source}"
        );
    }
}
#[test]
fn alias_options_and_definitions_obey_parse_unit_boundaries() {
    let sites = analyze(
        "alias ls=eza\nshopt -s expand_aliases; ls\nls\n",
        ShellDialect::Bash,
    );
    let names = sites.iter().filter_map(|s| s.name()).collect::<Vec<_>>();
    assert_eq!(names, ["alias", "shopt", "ls", "eza"]);
    let sites = analyze("alias ls=eza;\nfunction f {\n ls\n}\n", ShellDialect::Zsh);
    assert_eq!(sites.last().unwrap().name(), Some("eza"));
}
#[test]
fn alias_cycles_and_opaque_builtin_aliases_do_not_claim_builtin_identity() {
    let sites = analyze(
        "alias printf='echo data | cat'\nprintf hello\n",
        ShellDialect::Zsh,
    );
    assert!(sites.last().unwrap().name().is_none());
    let sites = analyze("alias a=b\nalias b=a\na\n", ShellDialect::Zsh);
    assert!(sites.last().unwrap().environment_uncertain.is_some());
    let sites = analyze("alias ls='ls --color'\nls\n", ShellDialect::Zsh);
    assert_eq!(sites.last().unwrap().name(), Some("ls"));
    assert_eq!(sites.last().unwrap().effective_words.len(), 2);
}
#[test]
fn double_quote_backslashes_preserve_literal_command_identity() {
    let sites = analyze("\"literal\\name\"\n", ShellDialect::Bash);
    assert_eq!(sites[0].name(), Some("literal\\name"));
}
#[test]
fn alias_builtins_respect_wrappers_and_function_shadowing() {
    let sites = analyze("builtin alias ls=eza\nls\n", ShellDialect::Zsh);
    assert_eq!(sites.last().unwrap().name(), Some("eza"));
    let sites = analyze("alias() { :; }\nalias ls=eza\nls\n", ShellDialect::Bash);
    assert_eq!(sites.last().unwrap().name(), Some("ls"));
    let sites = analyze("alias \"$name=echo\"\nunknown\n", ShellDialect::Zsh);
    assert!(sites.last().unwrap().environment_uncertain.is_some());
}

fn visible_aliases(source: &str, dialect: ShellDialect, cursor: usize) -> Vec<String> {
    let parsed = Parser::with_dialect(source, dialect)
        .without_alias_expansion()
        .parse();
    let indexer = Indexer::new(source, &parsed);
    let model = SemanticModel::build_with_options(
        &parsed.file,
        source,
        &indexer,
        SemanticBuildOptions {
            shell_profile: Some(ShellProfile::native(dialect)),
            ..Default::default()
        },
    );
    let line = source[..cursor]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1;
    model
        .visible_aliases_at(shucked_ast::Position::at(line, 1, cursor))
        .into_iter()
        .map(|alias| alias.name)
        .collect()
}
#[test]
fn alias_name_completions_follow_source_order_options_and_removals() {
    let source = "alias myls=eza\nmy\nunalias myls\nmy\n";
    assert_eq!(
        visible_aliases(source, ShellDialect::Zsh, source.find("\nmy").unwrap() + 3),
        ["myls"]
    );
    assert!(visible_aliases(source, ShellDialect::Zsh, source.len()).is_empty());
    assert!(
        visible_aliases(source, ShellDialect::Bash, source.find("\nmy").unwrap() + 3).is_empty()
    );
    let source = "shopt -s expand_aliases\nalias myls=eza\nmy";
    assert_eq!(
        visible_aliases(source, ShellDialect::Bash, source.len()),
        ["myls"]
    );
    let source = "alias myls=eza\nunsetopt aliases\nmy";
    assert!(visible_aliases(source, ShellDialect::Zsh, source.len()).is_empty());
    let source = "alias myls=eza\nsetopt no_aliases\nsetopt aliases\nmy";
    assert_eq!(
        visible_aliases(source, ShellDialect::Zsh, source.len()),
        ["myls"]
    );
    let source = "alias myls=eza; my";
    assert!(visible_aliases(source, ShellDialect::Zsh, source.len()).is_empty());
    let source = "if true; then alias myls=eza; fi\nmy";
    assert!(visible_aliases(source, ShellDialect::Zsh, source.len()).is_empty());
    let source = "alias myls=eza\nsource \"$UNKNOWN\"\nmy";
    assert!(visible_aliases(source, ShellDialect::Zsh, source.len()).is_empty());
}

#[test]
fn alias_removal_and_redefinition_take_effect_after_current_parse_unit() {
    for dialect in [ShellDialect::Zsh, ShellDialect::Bash] {
        let prefix = if dialect == ShellDialect::Bash {
            "shopt -s expand_aliases\n"
        } else {
            ""
        };
        let source = format!("{prefix}alias mycmd=printf\nunalias mycmd; mycmd\nmycmd\n");
        let sites = analyze(&source, dialect);
        let names = sites
            .iter()
            .filter(|site| {
                site.words.first().and_then(|word| word.text.as_deref()) == Some("mycmd")
            })
            .map(|site| site.name())
            .collect::<Vec<_>>();
        assert_eq!(names, [Some("printf"), Some("mycmd")]);
        let cursor = source.find("; mycmd").unwrap() + "; mycmd".len();
        assert_eq!(visible_aliases(&source, dialect, cursor), ["mycmd"]);
        let source = format!("{prefix}alias mycmd=printf\nalias mycmd=echo; mycmd\nmycmd\n");
        let sites = analyze(&source, dialect);
        let names = sites
            .iter()
            .filter(|site| {
                site.words.first().and_then(|word| word.text.as_deref()) == Some("mycmd")
            })
            .map(|site| site.name())
            .collect::<Vec<_>>();
        assert_eq!(names, [Some("printf"), Some("echo")]);
    }
}
