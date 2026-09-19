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
    flags: std::collections::BTreeMap<String, FlagValue>,
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
fn yes() -> bool {
    true
}

/// Unknown releases receive no negative flag validation. Matching a major
/// version or parsing a completion/help list is deliberately insufficient.
pub fn known_tool_grammar(tool: &str, version: &str) -> Option<VersionedGrammar> {
    let data = match (tool, version) {
        ("docker", "27.5.1") => include_str!("../data/validators/docker-27.5.1.json"),
        ("docker", "28.0.0") => include_str!("../data/validators/docker-28.0.0.json"),
        ("kubectl", "1.32.0") => include_str!("../data/validators/kubectl-1.32.0.json"),
        ("kubectl", "1.33.0") => include_str!("../data/validators/kubectl-1.33.0.json"),
        ("kubectl", "1.34.0") => include_str!("../data/validators/kubectl-1.34.0.json"),
        ("openssh", "9.8p1") => include_str!("../data/validators/openssh-9.8p1.json"),
        ("openssh", "9.9p2") => include_str!("../data/validators/openssh-9.9p2.json"),
        ("openssh", "10.0p1") => include_str!("../data/validators/openssh-10.0p1.json"),
        ("openssh", "10.1p1") => include_str!("../data/validators/openssh-10.1p1.json"),
        ("openssh", "10.2p1") => include_str!("../data/validators/openssh-10.2p1.json"),
        ("openssh", "10.3p1") => include_str!("../data/validators/openssh-10.3p1.json"),
        ("openssh", "10.4p1") => include_str!("../data/validators/openssh-10.4p1.json"),
        ("openssh", "10.5p1") => include_str!("../data/validators/openssh-10.5p1.json"),
        ("apple-ls", "479") => include_str!("../data/validators/apple-ls-479.json"),
        ("apple-ls", "475") => include_str!("../data/validators/apple-ls-475.json"),
        ("apple-ls", "457.140.3") => include_str!("../data/validators/apple-ls-457.140.3.json"),
        ("curl", "8.7.1") => include_str!("../data/validators/curl-8.7.1.json"),
        ("curl", "8.12.1") => include_str!("../data/validators/curl-8.12.1.json"),
        ("curl", "8.14.1") => include_str!("../data/validators/curl-8.14.1.json"),
        ("curl", "8.15.0") => include_str!("../data/validators/curl-8.15.0.json"),
        ("eza", "0.23.1") => include_str!("../data/validators/eza-0.23.1.json"),
        ("eza", "0.23.2") => include_str!("../data/validators/eza-0.23.2.json"),
        ("eza", "0.23.3") => include_str!("../data/validators/eza-0.23.3.json"),
        ("eza", "0.23.4") => include_str!("../data/validators/eza-0.23.4.json"),
        ("eza", "0.23.5") => include_str!("../data/validators/eza-0.23.5.json"),
        ("ripgrep", "15.1.0") => include_str!("../data/validators/ripgrep-15.1.0.json"),
        ("ripgrep", "15.2.0") => include_str!("../data/validators/ripgrep-15.2.0.json"),
        ("pacman", "7.0.0") => include_str!("../data/validators/pacman-7.0.0.json"),
        ("pacman", "7.1.0") => include_str!("../data/validators/pacman-7.1.0.json"),
        ("eza", "0.23.0") => include_str!("../data/validators/eza-0.23.0.json"),
        ("ripgrep", "14.1.1") => include_str!("../data/validators/ripgrep-14.1.1.json"),
        ("fd", "10.3.0") => include_str!("../data/validators/fd-10.3.0.json"),
        ("bat", "0.25.0") => include_str!("../data/validators/bat-0.25.0.json"),
        ("gnu-ls", "9.7") => include_str!("../data/validators/gnu-ls-9.7.json"),
        _ => return None,
    };
    let manifest: Manifest = serde_json::from_str(data).ok()?;
    if manifest.tool != tool || manifest.version != version {
        return None;
    }
    Some(VersionedGrammar {
        grammar: CommandGrammar {
            flags: manifest
                .flags
                .into_iter()
                .map(|(name, value)| {
                    (
                        name,
                        FlagSpec {
                            value,
                            ..FlagSpec::default()
                        },
                    )
                })
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
