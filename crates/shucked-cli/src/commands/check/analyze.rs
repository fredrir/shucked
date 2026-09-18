use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use shucked_linter::{Applicability, LinterSettings, RuleSet, ShellCheckCodeMap, ShellDialect};
use shucked_parser::{
    Error as ParseError,
    parser::{ParseResult, Parser},
};

use super::cache::CheckCacheData;
use super::display::{display_lint_diagnostics, display_parse_error};
use super::embedded::analyze_embedded_file;
use super::settings::CompiledPerFileShellList;
use super::source_resolver::{
    NativeSourceResolver, resolve_source_ref_paths, source_ref_candidate_paths,
};
use crate::commands::check_output::DisplayedDiagnostic;
use crate::commands::project_runner::PendingProjectFile;
use shucked_discover::{DiscoveredFile, FileKind};

#[derive(Debug, Clone)]
pub(super) struct FileCheckResult {
    pub(super) file: DiscoveredFile,
    pub(super) file_key: shucked_cache::FileCacheKey,
    pub(super) cache_data: CheckCacheData,
    pub(super) diagnostics: Vec<DisplayedDiagnostic>,
    pub(super) dependency_paths: Vec<std::path::PathBuf>,
    /// Resolved on-disk targets of `lint=true` source directives in this
    /// file, to be linted as additional inputs by the runner. Empty when
    /// `lint-sources` is disabled or no directive target resolves.
    pub(super) followed_paths: Vec<std::path::PathBuf>,
    pub(super) fixes_applied: usize,
    pub(super) parse_failed: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn analyze_file(
    pending: PendingProjectFile,
    base_linter_settings: &LinterSettings,
    per_file_shell: &CompiledPerFileShellList,
    plugin_resolver: Option<&(dyn shucked_semantic::PluginResolver + Send + Sync)>,
    source_path_resolver: Option<&(dyn shucked_semantic::SourcePathResolver + Send + Sync)>,
    source_resolver: Option<&NativeSourceResolver>,
    lint_sources: bool,
    shellcheck_map: &ShellCheckCodeMap,
    include_source: bool,
    fix_applicability: Option<Applicability>,
    fixable_rules: &RuleSet,
) -> Result<FileCheckResult> {
    match pending.file.kind {
        FileKind::Shell => analyze_shell_file(
            pending,
            base_linter_settings,
            per_file_shell,
            plugin_resolver,
            source_path_resolver,
            source_resolver,
            lint_sources,
            shellcheck_map,
            include_source,
            fix_applicability,
            fixable_rules,
        ),
        FileKind::Embedded => analyze_embedded_file(
            pending,
            base_linter_settings,
            shellcheck_map,
            include_source,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_shell_file(
    pending: PendingProjectFile,
    base_linter_settings: &LinterSettings,
    per_file_shell: &CompiledPerFileShellList,
    plugin_resolver: Option<&(dyn shucked_semantic::PluginResolver + Send + Sync)>,
    source_path_resolver: Option<&(dyn shucked_semantic::SourcePathResolver + Send + Sync)>,
    source_resolver: Option<&NativeSourceResolver>,
    lint_sources: bool,
    shellcheck_map: &ShellCheckCodeMap,
    include_source: bool,
    fix_applicability: Option<Applicability>,
    fixable_rules: &RuleSet,
) -> Result<FileCheckResult> {
    let mut source = read_shared_source(&pending.file.absolute_path)?;
    let inferred_shell = per_file_shell
        .shell_for_path(&pending.file.absolute_path)
        .unwrap_or_else(|| ShellDialect::infer(&source, Some(&pending.file.absolute_path)));
    let parse_dialect = inferred_shell.parser_dialect();

    let linter_settings = base_linter_settings.clone().with_shell(inferred_shell);
    let mut parse_result = Parser::with_dialect(&source, parse_dialect).parse();
    let mut analysis = collect_lint_diagnostics(
        &source,
        &parse_result,
        &linter_settings,
        plugin_resolver,
        source_path_resolver,
        shellcheck_map,
        &pending.file.absolute_path,
    );
    let mut diagnostics = analysis.diagnostics;
    let mut fixes_applied = 0;

    if let Some(applicability) = fix_applicability {
        let fixable_diagnostics = diagnostics
            .iter()
            .filter(|diagnostic| fixable_rules.contains(diagnostic.rule))
            .cloned()
            .collect::<Vec<_>>();
        let applied = shucked_linter::apply_fixes(&source, &fixable_diagnostics, applicability);
        if applied.fixes_applied > 0 {
            source = Arc::<str>::from(applied.code);
            fs::write(&pending.file.absolute_path, &*source)?;
            parse_result = Parser::with_dialect(&source, parse_dialect).parse();
            analysis = collect_lint_diagnostics(
                &source,
                &parse_result,
                &linter_settings,
                plugin_resolver,
                source_path_resolver,
                shellcheck_map,
                &pending.file.absolute_path,
            );
            diagnostics = analysis.diagnostics;
            fixes_applied = applied.fixes_applied;
        }
    }

    let parse_failed = parse_result.is_err();
    let diagnostics = if parse_failed && diagnostics.is_empty() {
        let ParseError::Parse {
            message,
            line,
            column,
        } = parse_result.strict_error();
        vec![display_parse_error(
            &pending.file.display_path,
            &pending.file.relative_path,
            &pending.file.absolute_path,
            line,
            column,
            message,
            include_source.then_some(source.clone()),
        )]
    } else {
        display_lint_diagnostics(&pending, &source, &diagnostics, include_source)
    };
    let mut dependency_paths = analysis.semantic.imported_dependency_paths().to_vec();
    let collected_source_paths = source_resolver
        .map(|resolver| {
            collect_source_paths(
                &analysis.semantic,
                &pending.file.absolute_path,
                resolver,
                lint_sources,
            )
        })
        .unwrap_or_default();
    dependency_paths.extend(collected_source_paths.dependencies);
    dependency_paths.sort();
    dependency_paths.dedup();
    let followed_paths = collected_source_paths.followed;
    let cache_data = CheckCacheData::from_displayed(
        &diagnostics,
        parse_failed,
        &dependency_paths,
        &followed_paths,
    );

    Ok(FileCheckResult {
        file: pending.file,
        file_key: pending.file_key,
        cache_data,
        diagnostics,
        dependency_paths,
        followed_paths,
        fixes_applied,
        parse_failed,
    })
}

#[derive(Debug, Default)]
struct CollectedSourcePaths {
    followed: Vec<std::path::PathBuf>,
    dependencies: Vec<std::path::PathBuf>,
}

/// Resolves the targets to lint and records every path that can affect source
/// resolution up to the current winner. Missing higher-precedence candidates
/// are dependencies too: creating one must invalidate the cached resolution
/// and wake watch mode.
fn collect_source_paths(
    semantic: &shucked_semantic::SemanticModel,
    source_path: &Path,
    resolver: &NativeSourceResolver,
    lint_sources: bool,
) -> CollectedSourcePaths {
    let mut collected = CollectedSourcePaths::default();
    for source_ref in semantic.source_refs() {
        for candidate in source_ref_candidate_paths(source_path, source_ref, resolver) {
            let resolved = candidate.is_file();
            collected.dependencies.push(candidate);
            if resolved {
                break;
            }
        }
        if lint_sources
            && source_ref.lints_target()
            && let Some(target) = resolve_source_ref_paths(source_path, source_ref, resolver)
        {
            collected.followed.push(target);
        }
    }
    collected.followed.sort();
    collected.followed.dedup();
    collected.dependencies.sort();
    collected.dependencies.dedup();
    collected
}
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_lint_diagnostics(
    source: &Arc<str>,
    parse_result: &ParseResult,
    linter_settings: &LinterSettings,
    plugin_resolver: Option<&(dyn shucked_semantic::PluginResolver + Send + Sync)>,
    source_path_resolver: Option<&(dyn shucked_semantic::SourcePathResolver + Send + Sync)>,
    shellcheck_map: &ShellCheckCodeMap,
    source_path: &Path,
) -> shucked_linter::AnalysisResult {
    let mut request =
        shucked_linter::AnalysisRequest::from_parse_result(parse_result, source, linter_settings)
            .with_source_path(source_path)
            .with_shellcheck_map(shellcheck_map)
            .with_optional_plugin_resolver(plugin_resolver);
    if let Some(resolver) = source_path_resolver {
        request = request.with_source_path_resolver(resolver);
    }
    request.analyze()
}
pub(super) fn read_shared_source(path: &Path) -> Result<Arc<str>> {
    Ok(Arc::<str>::from(fs::read_to_string(path)?))
}

/// Performs a lightweight parse of a followed file to discover its transitive
/// `lint=true` targets before any followed file is linted.
pub(super) fn discover_followed_paths(
    path: &Path,
    per_file_shell: &CompiledPerFileShellList,
    resolver: &NativeSourceResolver,
) -> Result<Vec<std::path::PathBuf>> {
    let source = read_shared_source(path)?;
    let shell = per_file_shell
        .shell_for_path(path)
        .unwrap_or_else(|| ShellDialect::infer(&source, Some(path)));
    let parse_result = Parser::with_dialect(&source, shell.parser_dialect()).parse();
    let indexer = shucked_indexer::Indexer::new(&source, &parse_result);
    let semantic = shucked_semantic::SemanticModel::build_with_options(
        &parse_result.file,
        &source,
        &indexer,
        shucked_semantic::SemanticBuildOptions {
            source_path: Some(path),
            shell_profile: Some(shell.shell_profile()),
            resolve_source_closure: false,
            ..shucked_semantic::SemanticBuildOptions::default()
        },
    );
    Ok(collect_source_paths(&semantic, path, resolver, true).followed)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use shucked_config::ConfigArguments;
    use shucked_linter::{LinterSettings, Rule, RuleSelector, RuleSet, ShellCheckCodeMap};
    use shucked_parser::parser::Parser;
    use tempfile::tempdir;

    use super::{analyze_file, collect_lint_diagnostics, read_shared_source};
    use crate::ExitStatus;
    use crate::args::RuleSelectionArgs;
    use crate::commands::check::cache::CachedDisplayedDiagnosticKind;
    use crate::commands::check::display::display_lint_diagnostics;
    use crate::commands::check::run::run_check_with_cwd;
    use crate::commands::check::test_support::{
        cache_root, check_args, empty_per_file_shell, pending_project_file,
    };
    use crate::commands::check_output::DisplayedDiagnosticKind;

    #[test]
    fn reports_parse_errors() {
        let tempdir = tempdir().unwrap();
        fs::write(tempdir.path().join("broken.sh"), "#!/bin/bash\nif true\n").unwrap();

        let report = run_check_with_cwd(
            &check_args(false),
            &ConfigArguments::default(),
            tempdir.path(),
            &cache_root(tempdir.path()),
        )
        .unwrap();

        assert_eq!(report.exit_status(false, false), ExitStatus::Failure);
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.cache_hits, 0);
        assert_eq!(report.cache_misses, 1);
    }

    #[test]
    fn reports_missing_fi_as_c035_lint() {
        let tempdir = tempdir().unwrap();
        fs::write(
            tempdir.path().join("broken.sh"),
            "#!/bin/sh\nif true; then\n  :\n",
        )
        .unwrap();

        let report = run_check_with_cwd(
            &check_args(false),
            &ConfigArguments::default(),
            tempdir.path(),
            &cache_root(tempdir.path()),
        )
        .unwrap();

        let codes = report
            .diagnostics
            .iter()
            .map(|diagnostic| match &diagnostic.kind {
                DisplayedDiagnosticKind::Lint { code, .. } => code.as_str(),
                other => panic!("expected lint diagnostic, got {other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(codes, vec!["C034", "C035"]);
    }

    #[test]
    fn ignore_can_trigger_parse_error_fallback() {
        let tempdir = tempdir().unwrap();
        fs::write(
            tempdir.path().join("broken.sh"),
            "#!/bin/sh\nif true; then\n  :\n",
        )
        .unwrap();

        let mut args = check_args(true);
        args.rule_selection = RuleSelectionArgs {
            ignore: vec![
                RuleSelector::Rule(Rule::UnterminatedIf),
                RuleSelector::Rule(Rule::MissingFi),
            ],
            ..RuleSelectionArgs::default()
        };

        let report = run_check_with_cwd(
            &args,
            &ConfigArguments::default(),
            tempdir.path(),
            &cache_root(tempdir.path()),
        )
        .unwrap();

        assert_eq!(report.diagnostics.len(), 1);
        assert!(matches!(
            report.diagnostics[0].kind,
            DisplayedDiagnosticKind::ParseError
        ));
    }

    #[test]
    fn reports_missing_fi_as_parse_error_when_parse_rule_is_disabled() {
        let tempdir = tempdir().unwrap();
        let broken_path = tempdir.path().join("broken.sh");
        fs::write(&broken_path, "#!/bin/sh\nif true; then\n  :\n").unwrap();

        let result = analyze_file(
            pending_project_file(&broken_path, tempdir.path()),
            &LinterSettings::for_rule(shucked_linter::Rule::UnusedAssignment)
                .with_analyzed_paths([broken_path.clone()]),
            &empty_per_file_shell(tempdir.path()),
            None,
            None,
            None,
            false,
            &ShellCheckCodeMap::default(),
            false,
            None,
            &RuleSet::all(),
        )
        .unwrap();

        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.cache_data.diagnostics.len(), 1);
        assert!(matches!(
            result.cache_data.diagnostics[0].kind,
            CachedDisplayedDiagnosticKind::ParseError
        ));
        match &result.diagnostics[0].kind {
            DisplayedDiagnosticKind::ParseError => {}
            other => panic!("expected parse error, got {other:?}"),
        }
        assert!(result.diagnostics[0].message.contains("expected 'fi'"));
    }

    #[test]
    fn infers_shell_from_extension_for_local_rule() {
        let tempdir = tempdir().unwrap();
        fs::write(tempdir.path().join("posix.sh"), "local foo=bar\n").unwrap();
        fs::write(tempdir.path().join("bashy.bash"), "local foo=bar\n").unwrap();

        let report = run_check_with_cwd(
            &check_args(true),
            &ConfigArguments::default(),
            tempdir.path(),
            &cache_root(tempdir.path()),
        )
        .unwrap();
        let c014 = report
            .diagnostics
            .iter()
            .filter(|diagnostic| matches!(&diagnostic.kind, DisplayedDiagnosticKind::Lint { code, .. } if code == "C014"))
            .collect::<Vec<_>>();

        assert_eq!(c014.len(), 1);
        assert_eq!(c014[0].path, PathBuf::from("bashy.bash"));
    }

    #[test]
    fn lint_diagnostics_share_the_original_source_arc_for_full_output() {
        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("warn.sh");
        fs::write(&path, "#!/bin/bash\nunused=1\necho ok\n").unwrap();

        let pending = pending_project_file(&path, tempdir.path());
        let source = read_shared_source(&path).unwrap();
        let parse_result =
            Parser::with_dialect(&source, shucked_parser::ShellDialect::Bash).parse();

        let diagnostics = collect_lint_diagnostics(
            &source,
            &parse_result,
            &LinterSettings::default(),
            None,
            None,
            &ShellCheckCodeMap::default(),
            &path,
        );
        let diagnostics =
            display_lint_diagnostics(&pending, &source, &diagnostics.diagnostics, true);

        let diagnostic_source = diagnostics[0]
            .source
            .as_ref()
            .expect("full output should retain source");
        assert!(Arc::ptr_eq(diagnostic_source, &source));
    }
}
