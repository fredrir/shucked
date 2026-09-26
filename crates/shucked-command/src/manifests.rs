//! Finite option-name coverage tied to audited upstream versions. These records
//! contain CLI interface facts only, without upstream implementation or prose.
use std::collections::BTreeSet;

use serde::Deserialize;

use crate::{CommandGrammar, FlagSpec, FlagValue};

#[derive(Clone, Debug)]
pub struct VersionedGrammar {
    pub grammar: CommandGrammar,
    pub source: String,
    pub unsupported_flags: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    tool: String,
    version: String,
    source: String,
    flags: std::collections::BTreeMap<String, FlagEntry>,
    #[serde(default = "yes")]
    flags_complete: bool,
    #[serde(default)]
    subcommands: BTreeSet<String>,
    #[serde(default)]
    unsupported_flags: BTreeSet<String>,
    #[serde(default)]
    long_abbreviations: bool,
    #[serde(default = "yes")]
    positional_arguments: bool,
}

/// A flag records its value arity either as the bare arity name or as an
/// object that also carries a completion description.
#[derive(Deserialize)]
#[serde(untagged)]
enum FlagEntry {
    Arity(FlagValue),
    Described {
        value: FlagValue,
        #[serde(default)]
        description: Option<String>,
    },
}

impl FlagEntry {
    fn into_spec(self) -> FlagSpec {
        match self {
            Self::Arity(value) => FlagSpec {
                value,
                ..FlagSpec::default()
            },
            Self::Described { value, description } => FlagSpec {
                value,
                description: description
                    .map(|text| text.trim().to_owned())
                    .filter(|text| !text.is_empty()),
                ..FlagSpec::default()
            },
        }
    }
}

fn yes() -> bool {
    true
}

/// Every bundled grammar as `(tool, version, json)`.
const GRAMMARS: &[(&str, &str, &str)] = &[
    (
        "docker",
        "27.5.1",
        include_str!("../data/validators/docker-27.5.1.json"),
    ),
    (
        "docker",
        "28.0.0",
        include_str!("../data/validators/docker-28.0.0.json"),
    ),
    (
        "kubectl",
        "1.32.0",
        include_str!("../data/validators/kubectl-1.32.0.json"),
    ),
    (
        "kubectl",
        "1.33.0",
        include_str!("../data/validators/kubectl-1.33.0.json"),
    ),
    (
        "kubectl",
        "1.34.0",
        include_str!("../data/validators/kubectl-1.34.0.json"),
    ),
    (
        "openssh",
        "9.8p1",
        include_str!("../data/validators/openssh-9.8p1.json"),
    ),
    (
        "openssh",
        "9.9p2",
        include_str!("../data/validators/openssh-9.9p2.json"),
    ),
    (
        "openssh",
        "10.0p1",
        include_str!("../data/validators/openssh-10.0p1.json"),
    ),
    (
        "openssh",
        "10.1p1",
        include_str!("../data/validators/openssh-10.1p1.json"),
    ),
    (
        "openssh",
        "10.2p1",
        include_str!("../data/validators/openssh-10.2p1.json"),
    ),
    (
        "openssh",
        "10.3p1",
        include_str!("../data/validators/openssh-10.3p1.json"),
    ),
    (
        "openssh",
        "10.4p1",
        include_str!("../data/validators/openssh-10.4p1.json"),
    ),
    (
        "openssh",
        "10.5p1",
        include_str!("../data/validators/openssh-10.5p1.json"),
    ),
    (
        "apple-ls",
        "457.140.3",
        include_str!("../data/validators/apple-ls-457.140.3.json"),
    ),
    (
        "apple-ls",
        "475",
        include_str!("../data/validators/apple-ls-475.json"),
    ),
    (
        "apple-ls",
        "479",
        include_str!("../data/validators/apple-ls-479.json"),
    ),
    (
        "curl",
        "8.7.1",
        include_str!("../data/validators/curl-8.7.1.json"),
    ),
    (
        "curl",
        "8.12.1",
        include_str!("../data/validators/curl-8.12.1.json"),
    ),
    (
        "curl",
        "8.14.1",
        include_str!("../data/validators/curl-8.14.1.json"),
    ),
    (
        "curl",
        "8.15.0",
        include_str!("../data/validators/curl-8.15.0.json"),
    ),
    (
        "eza",
        "0.23.0",
        include_str!("../data/validators/eza-0.23.0.json"),
    ),
    (
        "eza",
        "0.23.1",
        include_str!("../data/validators/eza-0.23.1.json"),
    ),
    (
        "eza",
        "0.23.2",
        include_str!("../data/validators/eza-0.23.2.json"),
    ),
    (
        "eza",
        "0.23.3",
        include_str!("../data/validators/eza-0.23.3.json"),
    ),
    (
        "eza",
        "0.23.4",
        include_str!("../data/validators/eza-0.23.4.json"),
    ),
    (
        "eza",
        "0.23.5",
        include_str!("../data/validators/eza-0.23.5.json"),
    ),
    (
        "ripgrep",
        "14.1.1",
        include_str!("../data/validators/ripgrep-14.1.1.json"),
    ),
    (
        "ripgrep",
        "15.1.0",
        include_str!("../data/validators/ripgrep-15.1.0.json"),
    ),
    (
        "ripgrep",
        "15.2.0",
        include_str!("../data/validators/ripgrep-15.2.0.json"),
    ),
    (
        "pacman",
        "7.0.0",
        include_str!("../data/validators/pacman-7.0.0.json"),
    ),
    (
        "pacman",
        "7.1.0",
        include_str!("../data/validators/pacman-7.1.0.json"),
    ),
    (
        "fd",
        "10.3.0",
        include_str!("../data/validators/fd-10.3.0.json"),
    ),
    (
        "bat",
        "0.25.0",
        include_str!("../data/validators/bat-0.25.0.json"),
    ),
    (
        "gnu-ls",
        "9.7",
        include_str!("../data/validators/gnu-ls-9.7.json"),
    ),
];

/// The `(tool, version)` pairs with a bundled grammar, in bundle order.
pub fn known_tool_versions() -> impl Iterator<Item = (&'static str, &'static str)> {
    GRAMMARS.iter().map(|(tool, version, _)| (*tool, *version))
}

/// Unknown releases receive no negative flag validation. Matching a major
/// version or parsing a completion/help list is deliberately insufficient.
pub fn known_tool_grammar(tool: &str, version: &str) -> Option<VersionedGrammar> {
    let (_, _, data) = GRAMMARS.iter().find(|(candidate, candidate_version, _)| {
        *candidate == tool && *candidate_version == version
    })?;
    let manifest: Manifest = serde_json::from_str(data).ok()?;
    if manifest.tool != tool || manifest.version != version {
        return None;
    }
    Some(VersionedGrammar {
        grammar: CommandGrammar {
            flags: manifest
                .flags
                .into_iter()
                .map(|(name, entry)| (name, entry.into_spec()))
                .collect(),
            flags_complete: manifest.flags_complete,
            subcommands_complete: !manifest.subcommands.is_empty(),
            requires_subcommand: !manifest.subcommands.is_empty(),
            subcommands: manifest
                .subcommands
                .into_iter()
                .map(|name| (name, CommandGrammar::default()))
                .collect(),
            short_flag_clusters: true,
            positional_arguments: manifest.positional_arguments,
            long_abbreviations: manifest.long_abbreviations,
            opaque_flags: manifest.unsupported_flags.clone(),
        },
        source: manifest.source,
        unsupported_flags: manifest.unsupported_flags,
    })
}

/// The most recent bundled grammar for a tool, for suggestions when the
/// installed version cannot be identified. It never justifies a rejection.
pub fn newest_tool_grammar(tool: &str) -> Option<(&'static str, VersionedGrammar)> {
    let version = known_tool_versions()
        .filter(|(candidate, _)| *candidate == tool)
        .map(|(_, version)| version)
        .max_by_key(|version| version_key(version))?;
    known_tool_grammar(tool, version).map(|grammar| (version, grammar))
}

/// Numeric runs of a version string, so `10.0p1` sorts after `9.9p2`.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .map(|run| run.parse().unwrap_or(u64::MAX))
        .collect()
}

impl VersionedGrammar {
    pub fn covers_words(&self, words: &[String]) -> bool {
        !words.iter().skip(1).any(|word| {
            let name = word.split('=').next().unwrap_or(word);
            self.unsupported_flags.contains(name)
                || (name.starts_with('-')
                    && !name.starts_with("--")
                    && name
                        .chars()
                        .skip(1)
                        .any(|character| self.unsupported_flags.contains(&format!("-{character}"))))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_and_described_flag_entries_mix_within_one_record() {
        let manifest: Manifest = serde_json::from_str(
            r#"{
                "tool": "fixture", "version": "1", "source": "test",
                "flags": {
                    "--bare": "required",
                    "--described": {"value": "optionalAttached", "description": " Do the thing "},
                    "--blank": {"value": "none", "description": ""},
                    "--untagged": {"value": "none"}
                }
            }"#,
        )
        .unwrap();
        let flags: std::collections::BTreeMap<_, _> = manifest
            .flags
            .into_iter()
            .map(|(name, entry)| (name, entry.into_spec()))
            .collect();
        assert_eq!(flags["--bare"].value, FlagValue::Required);
        assert_eq!(flags["--bare"].description, None);
        assert_eq!(flags["--described"].value, FlagValue::OptionalAttached);
        assert_eq!(
            flags["--described"].description.as_deref(),
            Some("Do the thing")
        );
        assert_eq!(flags["--blank"].description, None);
        assert_eq!(flags["--untagged"].value, FlagValue::None);
        assert!(
            serde_json::from_str::<Manifest>(
                r#"{"tool": "x", "version": "1", "source": "s", "flags": {"--bad": "sometimes"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn version_keys_compare_numeric_runs() {
        assert!(version_key("10.0p1") > version_key("9.9p2"));
        assert!(version_key("479") > version_key("457.140.3"));
        assert!(version_key("0.23.5") > version_key("0.23.4"));
    }
}
