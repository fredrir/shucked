//! Source operands anchored at the home directory: `~/x`, `$HOME/x`,
//! variables derived from `HOME`, and the `${VAR:-default}` spellings that
//! every zshrc/bashrc uses.
//!
//! The provider pins the home directory and hides the process environment so
//! the expectations do not depend on the machine running the tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use shucked_indexer::Indexer;
use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};
use shucked_semantic::{
    ResolvedSourcePaths, SemanticBuildOptions, SemanticModel, SourcePathAnalyzer,
    SourcePathFileProvider, SourceRefKind,
};

const HOME: &str = "/home/me";

struct Files(BTreeMap<PathBuf, String>);

impl Files {
    fn new(files: &[(&str, &str)]) -> Self {
        Self(
            files
                .iter()
                .map(|(path, source)| (PathBuf::from(path), (*source).to_owned()))
                .collect(),
        )
    }
}

impl SourcePathFileProvider for Files {
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        vec![from.parent().unwrap().join(candidate)]
    }
    fn is_file(&self, path: &Path) -> bool {
        self.0.contains_key(path)
    }
    fn read_source(&self, path: &Path) -> Option<String> {
        self.0.get(path).cloned()
    }
    fn home_dir(&self) -> Option<PathBuf> {
        Some(PathBuf::from(HOME))
    }
    fn environment_variable(&self, _name: &str) -> Option<String> {
        None
    }
}

fn analyze(
    dialect: ShellDialect,
    path: &Path,
    source: &str,
    files: &Files,
) -> (SemanticModel, ResolvedSourcePaths) {
    let profile = ShellProfile::native(dialect);
    let parsed = Parser::with_profile(source, profile.clone()).parse();
    assert!(!parsed.is_err(), "{source}");
    let indexer = Indexer::new(source, &parsed);
    let model = SemanticModel::build_with_options(
        &parsed.file,
        source,
        &indexer,
        SemanticBuildOptions {
            source_path: Some(path),
            shell_profile: Some(profile),
            resolve_source_closure: false,
            ..Default::default()
        },
    );
    let resolved = SourcePathAnalyzer::default().resolve(&model, path, files);
    (model, resolved)
}

/// The analyzer's candidate for every dynamic source operand, in file order.
fn dynamic_candidates(
    dialect: ShellDialect,
    path: &str,
    source: &str,
    files: &[(&str, &str)],
) -> Vec<Option<PathBuf>> {
    let (model, resolved) = analyze(dialect, Path::new(path), source, &Files::new(files));
    let mut references = model.source_refs().iter().collect::<Vec<_>>();
    references.sort_by_key(|reference| reference.span.start.offset());
    references
        .into_iter()
        .filter(|reference| {
            matches!(
                reference.kind,
                SourceRefKind::Dynamic | SourceRefKind::SingleVariableStaticTail { .. }
            )
        })
        .map(|reference| resolved.candidate(reference).flatten().map(PathBuf::from))
        .collect()
}

fn zsh_candidates(source: &str, files: &[(&str, &str)]) -> Vec<Option<PathBuf>> {
    dynamic_candidates(
        ShellDialect::Zsh,
        "/home/me/scripts/init.zsh",
        source,
        files,
    )
}

fn home(tail: &str) -> Option<PathBuf> {
    Some(PathBuf::from(HOME).join(tail))
}

#[test]
fn literal_home_tilde_operands_name_one_file_under_the_home_directory() {
    let files = Files::new(&[]);
    let source = "[[ -f ~/.fzf.zsh ]] && source ~/.fzf.zsh\nsource ~\nsource ~bob/.zshrc\n";
    let (model, _) = analyze(
        ShellDialect::Zsh,
        Path::new("/home/me/.zshrc"),
        source,
        &files,
    );
    let candidates = model
        .source_refs()
        .iter()
        .map(|reference| files.source_ref_candidates(Path::new("/home/me/.zshrc"), reference))
        .collect::<Vec<_>>();
    assert_eq!(
        candidates,
        vec![
            vec![PathBuf::from("/home/me/.fzf.zsh")],
            vec![PathBuf::from("/home/me")],
            // Another user's tilde needs a password lookup: it keeps the
            // ordinary relative search so the miss stays visible.
            vec![PathBuf::from("/home/me/~bob/.zshrc")],
        ]
    );
}

#[test]
fn tilde_sources_are_followed_so_their_path_values_reach_later_sources() {
    assert_eq!(
        zsh_candidates(
            "source ~/.zsh/paths.zsh\nsource \"$ZSH_LIB/prompt.zsh\"\n",
            &[("/home/me/.zsh/paths.zsh", "ZSH_LIB=\"$HOME/.zsh/lib\"\n")],
        ),
        vec![home(".zsh/lib/prompt.zsh")]
    );
}

#[test]
fn zsh_unquoted_home_variables_resolve_like_quoted_ones() {
    for source in [
        "source $HOME/.aliases\n",
        "source ${HOME}/.aliases\n",
        ". $HOME/.aliases\n",
        "source \"$HOME/.aliases\"\n",
        "source ~/.aliases$EMPTY\n",
    ] {
        let expected = if source.contains("$EMPTY") {
            None
        } else {
            home(".aliases")
        };
        assert_eq!(zsh_candidates(source, &[]), vec![expected], "{source}");
    }
}

#[test]
fn variables_derived_from_home_feed_unquoted_and_quoted_sources() {
    for source in [
        "export ZSH=\"$HOME/.oh-my-zsh\"\nsource \"$ZSH/oh-my-zsh.sh\"\n",
        "ZSH=$HOME/.oh-my-zsh\nsource $ZSH/oh-my-zsh.sh\n",
        "ZSH=~/.oh-my-zsh\nsource $ZSH/oh-my-zsh.sh\n",
        "export ZSH=\"${HOME}/.oh-my-zsh\"\nsource ${ZSH}/oh-my-zsh.sh\n",
    ] {
        assert_eq!(
            zsh_candidates(source, &[]),
            vec![home(".oh-my-zsh/oh-my-zsh.sh")],
            "{source}"
        );
    }
    // A quoted tilde is literal in every shell.
    assert_eq!(
        zsh_candidates("ZSH=\"~/.oh-my-zsh\"\nsource \"$ZSH/oh-my-zsh.sh\"\n", &[]),
        vec![None]
    );
}

#[test]
fn zdotdir_defaults_to_home_and_matching_default_expansions_agree() {
    for source in [
        "source \"${ZDOTDIR:-$HOME}/.zshrc.local\"\n",
        "source ${ZDOTDIR:-$HOME}/.zshrc.local\n",
        "source ${ZDOTDIR:-~}/.zshrc.local\n",
        "source \"${ZDOTDIR-$HOME}/.zshrc.local\"\n",
        "source \"$ZDOTDIR/.zshrc.local\"\n",
    ] {
        assert_eq!(
            zsh_candidates(source, &[]),
            vec![home(".zshrc.local")],
            "{source}"
        );
    }
    // A default the analyzer does not seed cannot be modeled as the variable.
    for source in [
        "source \"${ZDOTDIR:-$HOME/.config/zsh}/.zshrc.local\"\n",
        "source \"${ZDOTDIR:+$HOME}/.zshrc.local\"\n",
        "source \"${ZDOTDIR#/}/.zshrc.local\"\n",
    ] {
        assert_eq!(zsh_candidates(source, &[]), vec![None], "{source}");
    }
}

#[test]
fn startup_files_edited_in_place_locate_zdotdir_beside_themselves() {
    // The first target exists so that following it keeps the environment
    // known for the second operand.
    let source = "source \"${ZDOTDIR:-$HOME}/aliases.zsh\"\nsource $ZDOTDIR/functions.zsh\n";
    assert_eq!(
        dynamic_candidates(
            ShellDialect::Zsh,
            "/dotfiles/zsh/.zshrc",
            source,
            &[("/dotfiles/zsh/aliases.zsh", "alias ll='ls -l'\n")]
        ),
        vec![
            Some(PathBuf::from("/dotfiles/zsh/aliases.zsh")),
            Some(PathBuf::from("/dotfiles/zsh/functions.zsh")),
        ]
    );
    // The sibling `.zshenv` still wins, as for any startup file.
    assert_eq!(
        dynamic_candidates(
            ShellDialect::Zsh,
            "/dotfiles/zsh/.zshrc",
            source,
            &[
                ("/dotfiles/zsh/.zshenv", "export ZDOTDIR=/elsewhere\n"),
                ("/elsewhere/aliases.zsh", "alias ll='ls -l'\n"),
            ]
        ),
        vec![
            Some(PathBuf::from("/elsewhere/aliases.zsh")),
            Some(PathBuf::from("/elsewhere/functions.zsh")),
        ]
    );
}

#[test]
fn installed_zshenv_supplies_only_zdotdir_to_other_zsh_files() {
    let zshenv = "export ZDOTDIR=\"$HOME/.config/zsh\"\nexport PLUGINS=\"$HOME/.plugins\"\n";
    assert_eq!(
        dynamic_candidates(
            ShellDialect::Zsh,
            "/home/me/.config/zsh/aliases.zsh",
            "source $ZDOTDIR/functions.zsh\nsource \"$PLUGINS/fzf.zsh\"\n",
            &[("/home/me/.zshenv", zshenv)]
        ),
        vec![home(".config/zsh/functions.zsh"), None]
    );
    // The installed `.zshenv` is not consulted for a `.zshrc` elsewhere; that
    // file's own sibling `.zshenv` is.
    assert_eq!(
        dynamic_candidates(
            ShellDialect::Zsh,
            "/dotfiles/zsh/.zshrc",
            "source $ZDOTDIR/functions.zsh\n",
            &[("/home/me/.zshenv", zshenv)]
        ),
        vec![Some(PathBuf::from("/dotfiles/zsh/functions.zsh"))]
    );
}

#[test]
fn xdg_base_directories_follow_the_specification_defaults() {
    for source in [
        "source \"${XDG_CONFIG_HOME:-$HOME/.config}/zsh/aliases.zsh\"\n",
        ". ${XDG_CONFIG_HOME:-~/.config}/zsh/aliases.zsh\n",
        "source \"$XDG_CONFIG_HOME/zsh/aliases.zsh\"\n",
        "XDG_CONFIG_HOME=\"$HOME/.config\"\nsource \"$XDG_CONFIG_HOME/zsh/aliases.zsh\"\n",
    ] {
        assert_eq!(
            zsh_candidates(source, &[]),
            vec![home(".config/zsh/aliases.zsh")],
            "{source}"
        );
    }
    assert_eq!(
        zsh_candidates(
            "source \"${XDG_DATA_HOME:-$HOME/.local/share}/zsh/plugins.zsh\"\nsource \"${XDG_CACHE_HOME:-$HOME/.cache}/zsh/compinit.zsh\"\n",
            &[("/home/me/.local/share/zsh/plugins.zsh", "# plugins\n")]
        ),
        vec![
            home(".local/share/zsh/plugins.zsh"),
            home(".cache/zsh/compinit.zsh"),
        ]
    );
    // A different default is not the seeded one.
    assert_eq!(
        zsh_candidates(
            "source \"${XDG_CONFIG_HOME:-$HOME/.cfg}/zsh/aliases.zsh\"\n",
            &[]
        ),
        vec![None]
    );
}

#[test]
fn bash_keeps_unquoted_expansions_unresolved_but_seeds_home_for_quoted_ones() {
    let path = "/home/me/scripts/init.bash";
    for (source, expected) in [
        // Field splitting and globbing apply to an unquoted expansion.
        ("source $HOME/.aliases\n", None),
        ("source ${HOME}/.aliases\n", None),
        ("source \"$HOME/.aliases\"\n", home(".aliases")),
        (
            "source \"${XDG_CONFIG_HOME:-$HOME/.config}/bash/aliases.bash\"\n",
            home(".config/bash/aliases.bash"),
        ),
        // Assignments never split, so a tilde or bare variable there is fine.
        (
            "BASH_LIB=~/.bash\nsource \"$BASH_LIB/prompt.bash\"\n",
            home(".bash/prompt.bash"),
        ),
        // `ZDOTDIR` is a zsh notion; bash gets no seed for it.
        ("source \"${ZDOTDIR:-$HOME}/.bashrc.local\"\n", None),
    ] {
        assert_eq!(
            dynamic_candidates(ShellDialect::Bash, path, source, &[]),
            vec![expected],
            "{source}"
        );
    }
}

#[test]
fn literal_word_lists_load_each_named_file_in_order() {
    let files = Files::new(&[
        ("/home/me/.config/zsh/aliases.zsh", "alias ll='ls -l'\n"),
        ("/home/me/.config/zsh/functions.zsh", "f() { :; }\n"),
    ]);
    for source in [
        "for f in aliases.zsh missing.zsh functions.zsh; do source \"$HOME/.config/zsh/$f\"; done\n",
        "for f in aliases.zsh functions.zsh; do source ~/.config/zsh/$f; done\n",
        "for f in aliases functions; do source \"${XDG_CONFIG_HOME:-$HOME/.config}/zsh/${f}.zsh\"; done\n",
        "for f in ~/.config/zsh/aliases.zsh ~/.config/zsh/functions.zsh; do source $f; done\n",
        "for f in ~/.config/zsh/{aliases,functions}.zsh; do source \"$f\"; done\n",
    ] {
        let (model, resolved) = analyze(
            ShellDialect::Zsh,
            Path::new("/home/me/.zshrc"),
            source,
            &files,
        );
        assert_eq!(
            resolved.sequence(&model.source_refs()[0]),
            Some(
                [
                    PathBuf::from("/home/me/.config/zsh/aliases.zsh"),
                    PathBuf::from("/home/me/.config/zsh/functions.zsh"),
                ]
                .as_slice()
            ),
            "{source}"
        );
        assert!(resolved.is_complete(), "{source}");
    }
    // Words with an unknown value or a relative pattern stay unresolved.
    for source in [
        "for f in $LIST; do source \"$HOME/.config/zsh/$f\"; done\n",
        "for f in *.zsh; do source \"$HOME/.config/zsh/$f\"; done\n",
        "for f in aliases.zsh; do source \"$f\"; done\n",
    ] {
        let (model, resolved) = analyze(
            ShellDialect::Zsh,
            Path::new("/home/me/.zshrc"),
            source,
            &files,
        );
        assert_eq!(resolved.sequence(&model.source_refs()[0]), None, "{source}");
    }
}
