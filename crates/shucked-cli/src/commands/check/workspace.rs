use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use shucked_indexer::Indexer;
use shucked_linter::{Rule, ShellDialect};
use shucked_parser::parser::Parser;
use shucked_semantic::{
    CallFactSourceEdge, FileCallFacts, SemanticBuildOptions, SemanticModel, WorkspaceVariableIndex,
    WorkspaceVariableUsage,
};

use super::settings::ResolvedCheckSettings;
use super::source_resolver::{NativeSourceResolver, source_ref_candidate_paths};

pub(super) fn variable_usage(
    paths: &[PathBuf],
    settings: &ResolvedCheckSettings,
    resolver: &NativeSourceResolver,
) -> (Arc<WorkspaceVariableUsage>, Vec<PathBuf>) {
    let mut index = WorkspaceVariableIndex::default();
    let mut path_analyzer = shucked_semantic::SourcePathAnalyzer::default();
    let path_provider = resolver.clone();
    let mut visited = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    let mut pending = paths.to_vec();
    if !settings
        .linter_settings
        .rules
        .contains(Rule::UnusedAssignment)
    {
        return (Arc::new(WorkspaceVariableUsage::default()), Vec::new());
    }
    while let Some(path) = pending.pop() {
        let path = path.canonicalize().unwrap_or(path);
        if !visited.insert(path.clone()) {
            continue;
        }
        dependencies.insert(path.clone());
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let shell = settings
            .per_file_shell
            .shell_for_path(&path)
            .unwrap_or_else(|| ShellDialect::infer(&source, Some(&path)));
        let parse = Parser::with_profile(&source, shell.shell_profile()).parse();
        let indexer = Indexer::new(&source, &parse);
        let model = SemanticModel::build_with_options(
            &parse.file,
            &source,
            &indexer,
            SemanticBuildOptions {
                source_path: Some(&path),
                shell_profile: Some(shell.shell_profile()),
                resolve_source_closure: false,
                ..SemanticBuildOptions::default()
            },
        );
        let resolved_paths = path_analyzer.resolve(&model, &path, &path_provider);
        dependencies.extend(resolved_paths.dependency_paths().cloned());
        let edges = model
            .source_refs()
            .iter()
            .filter_map(|source_ref| {
                let mut candidates = if let Some(candidate) = resolved_paths.candidate(source_ref) {
                    candidate.map(PathBuf::from).into_iter().collect()
                } else {
                    source_ref_candidate_paths(&path, source_ref, resolver)
                };
                if resolved_paths.candidate(source_ref).is_none()
                    && candidates.is_empty()
                    && let Some(candidate) = model.current_file_source_candidate(source_ref, &path)
                {
                    candidates.push(candidate);
                }
                let target = candidates
                    .into_iter()
                    .inspect(|candidate| {
                        dependencies.insert(candidate.clone());
                    })
                    .find(|candidate| candidate.is_file())?;
                let target = target.canonicalize().unwrap_or(target);
                pending.push(target.clone());
                Some(CallFactSourceEdge {
                    path: target,
                    span: source_ref.span,
                    conditional: source_ref.conditionally_executed,
                    completion_visible: false,
                })
            })
            .collect();
        let calls = FileCallFacts::project_with_source_edges(&model, edges);
        index.insert(path, &model, &calls.source_effects);
    }
    (
        Arc::new(index.usage(&|| false).unwrap_or_default()),
        dependencies.into_iter().collect(),
    )
}
