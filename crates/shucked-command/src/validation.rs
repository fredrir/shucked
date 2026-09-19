use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CommandKind, ExecutableIdentity, Provenance, ResolvedCommand, typo_candidates};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceKind {
    /// A tool's audited metadata endpoint explicitly describes the full scope.
    StructuredInterface,
    /// A separately reviewed grammar bound to an exact executable version.
    VersionedManifest,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlagValue {
    #[default]
    None,
    Required,
    /// Optional values are accepted only when attached (`--flag=value`).
    OptionalAttached,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagSpec {
    pub value: FlagValue,
    /// Empty means value validation is not covered by this evidence.
    pub values: BTreeSet<String>,
    pub values_complete: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandGrammar {
    pub flags: BTreeMap<String, FlagSpec>,
    pub flags_complete: bool,
    pub subcommands: BTreeMap<String, CommandGrammar>,
    pub subcommands_complete: bool,
    /// At this node the first positional word selects a child command.
    pub requires_subcommand: bool,
    /// Whether combined short options such as `-al` are covered.
    pub short_flag_clusters: bool,
    /// False means later positional syntax is outside this grammar's scope.
    pub positional_arguments: bool,
    /// GNU-style unique long-option prefixes are accepted by some parsers.
    /// Prefix invocations stay Unknown until their value semantics are modeled.
    #[serde(default)]
    pub long_abbreviations: bool,
    /// These options transfer parsing to another command or an uncovered mode.
    #[serde(default)]
    pub opaque_flags: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationEvidence {
    pub executable: ExecutableIdentity,
    pub platform: String,
    pub kind: EvidenceKind,
    pub grammar: CommandGrammar,
    pub extensions: BTreeSet<String>,
    /// Must be true for a plugin-extensible subcommand grammar.
    pub extensions_complete: bool,
    pub plugin_extensible: bool,
    pub fresh: bool,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationIssueKind {
    UnknownFlag,
    UnknownSubcommand,
    InvalidValue,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationIssue {
    /// Index in `ResolvedCommand::effective_words`, including the command.
    /// The consumer maps it to the original source and excludes injected words.
    pub word_index: usize,
    pub value: String,
    pub kind: ValidationIssueKind,
    pub suggestions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "camelCase")]
pub enum ValidationResult {
    Valid,
    Invalid(Vec<ValidationIssue>),
    Unknown(String),
}

/// Validate only the exact identity and grammar scope supplied by independent
/// authority. A completion list is deliberately not accepted by this API.
pub fn validate_invocation(
    command: &ResolvedCommand,
    evidence: &ValidationEvidence,
    platform: &str,
) -> ValidationResult {
    if command.kind != CommandKind::Executable {
        return ValidationResult::Unknown(
            "Functions and builtins do not inherit an external tool's grammar".into(),
        );
    }
    if command.executable.as_ref() != Some(&evidence.executable) || platform != evidence.platform {
        return ValidationResult::Unknown(
            "Validation evidence does not match the executable and platform".into(),
        );
    }
    if !evidence.fresh {
        return ValidationResult::Unknown("Validation evidence is stale".into());
    }
    if evidence.kind == EvidenceKind::VersionedManifest && evidence.executable.version.is_none() {
        return ValidationResult::Unknown(
            "A versioned grammar requires an identified tool version".into(),
        );
    }
    if evidence.plugin_extensible && !evidence.extensions_complete {
        return ValidationResult::Unknown(
            "Installed extension commands have not been completely discovered".into(),
        );
    }
    let mut grammar = &evidence.grammar;
    let mut index = 1;
    let mut flags_enabled = true;
    while let Some(word) = command.effective_words.get(index) {
        if word == "--" && flags_enabled {
            flags_enabled = false;
            index += 1;
            continue;
        }
        if flags_enabled && word.starts_with('-') && word != "-" {
            let (name, attached) = word
                .split_once('=')
                .map_or((word.as_str(), None), |(name, value)| (name, Some(value)));
            if grammar.opaque_flags.contains(name) {
                return ValidationResult::Unknown(
                    "This option transfers parsing to an uncovered command context".into(),
                );
            }
            if let Some(flag) = grammar.flags.get(name) {
                match flag_value(command, index, flag, attached) {
                    Ok(consumed) => {
                        index += consumed + 1;
                        continue;
                    }
                    Err(result) => return result,
                }
            }
            if grammar.long_abbreviations
                && name.starts_with("--")
                && grammar
                    .flags
                    .keys()
                    .any(|candidate| candidate.starts_with(name))
            {
                return ValidationResult::Unknown(
                    "Abbreviated long-option semantics are outside this validation scope".into(),
                );
            }
            if grammar.short_flag_clusters
                && word.starts_with('-')
                && !word.starts_with("--")
                && word.len() > 2
            {
                match short_cluster(command, index, grammar) {
                    Ok(consumed) => {
                        index += consumed + 1;
                        continue;
                    }
                    Err(result) => return result,
                }
            }
            if !grammar.flags_complete {
                return ValidationResult::Unknown(
                    "Flag coverage is incomplete for this command context".into(),
                );
            }
            return invalid(
                index,
                word,
                ValidationIssueKind::UnknownFlag,
                grammar.flags.keys().map(String::as_str),
            );
        }
        if grammar.requires_subcommand {
            if let Some(child) = grammar.subcommands.get(word) {
                grammar = child;
                flags_enabled = true;
                index += 1;
                continue;
            }
            if evidence.extensions.contains(word) {
                return ValidationResult::Unknown(
                    "The extension exists, but its arguments are not covered".into(),
                );
            }
            if !grammar.subcommands_complete {
                return ValidationResult::Unknown(
                    "Subcommand coverage is incomplete for this command context".into(),
                );
            }
            return invalid(
                index,
                word,
                ValidationIssueKind::UnknownSubcommand,
                grammar.subcommands.keys().map(String::as_str),
            );
        }
        if !grammar.positional_arguments {
            return ValidationResult::Unknown(
                "Positional syntax is not covered by this grammar".into(),
            );
        }
        index += 1;
    }
    // Missing words are expected while typing; validation only rejects a
    // concrete supplied token, never a temporarily unfinished invocation.
    ValidationResult::Valid
}

fn flag_value(
    command: &ResolvedCommand,
    index: usize,
    spec: &FlagSpec,
    attached: Option<&str>,
) -> Result<usize, ValidationResult> {
    let (value, consumed) = match (spec.value, attached) {
        (FlagValue::None, None) => return Ok(0),
        (FlagValue::None, Some(_)) => {
            return Err(ValidationResult::Unknown(
                "Unexpected attached flag values are outside this validation scope".into(),
            ));
        }
        (_, Some(value)) => (value, 0),
        (FlagValue::OptionalAttached, None) => return Ok(0),
        (FlagValue::Required, None) => {
            let Some(value) = command.effective_words.get(index + 1) else {
                return Ok(0);
            };
            (value.as_str(), 1)
        }
    };
    if spec.values_complete && !spec.values.contains(value) {
        return Err(invalid(
            index + consumed,
            value,
            ValidationIssueKind::InvalidValue,
            spec.values.iter().map(String::as_str),
        ));
    }
    Ok(consumed)
}

fn short_cluster(
    command: &ResolvedCommand,
    index: usize,
    grammar: &CommandGrammar,
) -> Result<usize, ValidationResult> {
    let word = &command.effective_words[index];
    for (offset, character) in word.char_indices().skip(1) {
        let name = format!("-{character}");
        if grammar.opaque_flags.contains(&name) {
            return Err(ValidationResult::Unknown(
                "This option transfers parsing to an uncovered command context".into(),
            ));
        }
        let Some(spec) = grammar.flags.get(&name) else {
            return Err(if grammar.flags_complete {
                // Replace the original cluster only through an explicitly
                // mapped edit; suggesting a lone letter could drop good flags.
                ValidationResult::Invalid(vec![ValidationIssue {
                    word_index: index,
                    value: word.clone(),
                    kind: ValidationIssueKind::UnknownFlag,
                    suggestions: Vec::new(),
                }])
            } else {
                ValidationResult::Unknown("Short-flag coverage is incomplete".into())
            });
        };
        if spec.value != FlagValue::None {
            let rest = &word[offset + character.len_utf8()..];
            return flag_value(command, index, spec, (!rest.is_empty()).then_some(rest));
        }
    }
    Ok(0)
}

fn invalid<'a>(
    index: usize,
    value: &str,
    kind: ValidationIssueKind,
    candidates: impl IntoIterator<Item = &'a str>,
) -> ValidationResult {
    ValidationResult::Invalid(vec![ValidationIssue {
        word_index: index,
        value: value.into(),
        kind,
        suggestions: typo_candidates(value, candidates, 3),
    }])
}
