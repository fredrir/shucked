use std::path::{Path, PathBuf};

use shucked_indexer::Indexer;
use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};
use shucked_semantic::{
    ResolvedSourcePaths, SemanticBuildOptions, SemanticModel, SourcePathAnalyzer,
    SourcePathFileProvider,
};

struct Files {
    home: PathBuf,
}
impl SourcePathFileProvider for Files {
    fn candidates(&self, from: &Path, candidate: &str) -> Vec<PathBuf> {
        vec![from.parent().unwrap().join(candidate)]
    }
    fn home_dir(&self) -> Option<PathBuf> {
        Some(self.home.clone())
    }
}

fn resolve(path: &Path, source: &str, home: &Path) -> (SemanticModel, ResolvedSourcePaths) {
    let profile = ShellProfile::native(ShellDialect::Zsh);
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
    let result = SourcePathAnalyzer::default().resolve(
        &model,
        path,
        &Files {
            home: home.to_path_buf(),
        },
    );
    (model, result)
}

#[test]
fn brace_groups_preserve_load_order_and_track_directory_membership() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "02-utils.zsh",
        "30-aliases.zsh",
        "31-extra.zsh",
        "unrelated.zsh",
        ".30-hidden.zsh",
    ] {
        std::fs::write(root.path().join(name), "# module\n").unwrap();
    }
    let source = format!(
        "ROOT=\"{}\"\nfor file in \"$ROOT\"/{{3[0-9],0[2-9]}}-*.zsh(N); do source \"$file\"; done\n",
        root.path().display()
    );
    let (model, resolved) = resolve(&root.path().join("main.zsh"), &source, root.path());
    let reference = &model.source_refs()[0];
    let sequence = resolved
        .sequence(reference)
        .expect("bounded loop should resolve");
    assert_eq!(
        sequence
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap())
            .collect::<Vec<_>>(),
        ["30-aliases.zsh", "31-extra.zsh", "02-utils.zsh"]
    );
    assert!(resolved.dependency_paths().any(|path| path == root.path()));
    assert!(
        resolved
            .dependency_paths()
            .any(|path| path == &root.path().join("02-utils.zsh"))
    );
    assert!(resolved.is_complete());
}

#[test]
fn unknown_roots_modified_loop_bodies_and_unsupported_qualifiers_stay_unresolved() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("02-utils.zsh"), "LINUX=1\n").unwrap();
    for source in [
        "for file in \"$UNKNOWN\"/*.zsh(N); do source \"$file\"; done\n".to_owned(),
        format!(
            "for file in {}/*.zsh(N); do file=$OTHER; source \"$file\"; done\n",
            root.path().display()
        ),
        format!(
            "for file in {}/*.zsh(om); do source \"$file\"; done\n",
            root.path().display()
        ),
        format!(
            "(for file in {}/*.zsh(N); do source \"$file\"; done)\n",
            root.path().display()
        ),
        format!(
            "PATTERN='{}/*.zsh'\nfor file in \"$PATTERN\"; do source \"$file\"; done\n",
            root.path().display()
        ),
    ] {
        let (model, resolved) = resolve(&root.path().join("main.zsh"), &source, root.path());
        assert!(
            resolved.sequence(&model.source_refs()[0]).is_none(),
            "{source}"
        );
    }
}

#[test]
fn null_glob_can_resolve_to_no_sources_and_limits_remain_visible() {
    let root = tempfile::tempdir().unwrap();
    let source = format!(
        "for file in {}/*.zsh(N); do source \"$file\"; done\n",
        root.path().display()
    );
    let (model, resolved) = resolve(&root.path().join("main.zsh"), &source, root.path());
    assert_eq!(
        resolved.sequence(&model.source_refs()[0]),
        Some([].as_slice())
    );
    assert!(resolved.is_complete());
    for n in 0..129 {
        std::fs::write(root.path().join(format!("{n:03}.zsh")), "# module\n").unwrap();
    }
    let (model, resolved) = resolve(&root.path().join("main.zsh"), &source, root.path());
    assert!(resolved.sequence(&model.source_refs()[0]).is_none());
    assert!(!resolved.is_complete());
}

#[cfg(unix)]
#[test]
fn installed_startup_symlinks_supply_path_values_without_executing_files() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let modules = home.join("dotfiles/zsh");
    std::fs::create_dir_all(&modules).unwrap();
    let globals = modules.join("00-global.zsh");
    std::fs::write(
        &globals,
        "export DOTFILES=\"$HOME/dotfiles\"\nexport ZCONF=\"$DOTFILES/zsh\"\n",
    )
    .unwrap();
    let loader = modules.join("01-init.zsh");
    let source = "for file in \"$ZCONF\"/0[2-9]-*.zsh(N); do source \"$file\"; done\n";
    std::fs::write(&loader, source).unwrap();
    let helper = modules.join("02-utils.zsh");
    std::fs::write(&helper, "LINUX=1\n").unwrap();
    std::os::unix::fs::symlink(&globals, home.join(".zshenv")).unwrap();
    std::os::unix::fs::symlink(&loader, home.join(".zshrc")).unwrap();
    let (model, resolved) = resolve(&loader, source, &home);
    assert_eq!(
        resolved.sequence(&model.source_refs()[0]),
        Some([helper].as_slice())
    );
    assert!(
        resolved
            .dependency_paths()
            .any(|path| path == &home.join(".zshenv"))
    );
    assert!(
        resolved
            .dependency_paths()
            .any(|path| path == &home.join(".zshrc"))
    );
    let (model, resolved) = resolve(&modules.join("unrelated.zsh"), source, &home);
    assert!(resolved.sequence(&model.source_refs()[0]).is_none());
}
