//! Source ranges for shared audited command validation evidence.
use crate::session::RequestCancellationToken;
use shucked_ast::Span;
use shucked_command::{
    CommandKind, CommandResolution, EnvironmentSnapshot, ExecutionContext, ValidationIssueKind,
    ValidationPolicy, ValidationResult,
};
use shucked_semantic::CommandSiteFacts;
use std::collections::BTreeMap;

pub(crate) struct ValidationDiagnostic {
    pub span: Span,
    pub code: &'static str,
    pub message: String,
    pub suggestions: Vec<String>,
}

pub(crate) fn validate(
    context: &ExecutionContext,
    environment: &EnvironmentSnapshot,
    sites: &[(CommandSiteFacts, CommandResolution)],
    cancellation: &RequestCancellationToken,
) -> Vec<ValidationDiagnostic> {
    let mut diagnostics = Vec::new();
    if context.policy == ValidationPolicy::Portable || !environment.fresh {
        return diagnostics;
    }
    let mut acquired = BTreeMap::new();
    for (site, resolution) in sites {
        if cancellation.is_cancelled() {
            break;
        }
        let CommandResolution::Resolved(resolved) = resolution else {
            continue;
        };
        if resolved.kind != CommandKind::Executable
            || site.environment_uncertain.is_some()
            || site.effective_words.iter().any(|word| word.text.is_none())
        {
            continue;
        }
        let Some(identity) = &resolved.executable else {
            continue;
        };
        let evidence = if context.policy == ValidationPolicy::Captured {
            environment.validators.get(&resolved.name).cloned()
        } else if context.native_execution_allowed {
            acquired
                .entry(identity.path.clone())
                .or_insert_with(|| {
                    shucked_command::metadata::acquire(context, environment, identity, &|| {
                        cancellation.is_cancelled()
                    })
                })
                .clone()
        } else {
            None
        };
        let Some(evidence) = evidence else {
            continue;
        };
        if !(identity.path == evidence.executable.path
            && identity.size == evidence.executable.size
            && identity.modified_unix_ms == evidence.executable.modified_unix_ms)
        {
            continue;
        }
        let mut invocation = resolved.clone();
        // Acquisition added a tool-reported version, while filesystem lookup
        // intentionally never runs a program just to populate identity fields.
        invocation.executable = Some(evidence.executable.clone());
        let ValidationResult::Invalid(issues) =
            shucked_command::validate_invocation(&invocation, &evidence, &environment.platform)
        else {
            continue;
        };
        for issue in issues {
            let injected = resolved
                .effective_words
                .len()
                .saturating_sub(site.effective_words.len());
            let Some(source_index) = issue.word_index.checked_sub(injected) else {
                continue;
            };
            let Some(word) = site
                .effective_words
                .get(source_index)
                .filter(|word| !word.injected)
            else {
                continue;
            };
            if word.text.as_deref() != Some(issue.value.as_str()) {
                continue;
            }
            let (code, label) = match issue.kind {
                ValidationIssueKind::UnknownSubcommand => ("ENV002", "Unrecognized subcommand"),
                ValidationIssueKind::UnknownFlag => ("ENV003", "Unrecognized flag"),
                ValidationIssueKind::InvalidValue => ("ENV004", "Unrecognized argument value"),
            };
            diagnostics.push(ValidationDiagnostic {
                span: word.span,
                code,
                message: format!(
                    "{label} for {} on {}: {}",
                    resolved.name, context.target_id, issue.value
                ),
                suggestions: issue.suggestions,
            });
        }
    }
    diagnostics
}

#[cfg(all(test, unix))]
#[path = "../../tests/commands/validation.rs"]
mod tests;
