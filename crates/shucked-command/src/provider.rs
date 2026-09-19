use serde::{Deserialize, Serialize};

use crate::{ExecutionContext, Provenance, ResolutionSnapshotKey};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWord {
    pub value: String,
    /// None denotes an injected alias word, which is not directly editable.
    pub source_range: Option<SourceRange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRequest {
    pub context: ExecutionContext,
    pub key: ResolutionSnapshotKey,
    pub words: Vec<ProviderWord>,
    pub word_index: usize,
    pub byte_offset_in_word: usize,
    pub pack_id: String,
    pub runtime_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionCandidate {
    pub label: String,
    pub insert_text: String,
    pub replace: Option<SourceRange>,
    pub description: Option<String>,
    pub kind: CandidateKind,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateKind {
    Command,
    Subcommand,
    Flag,
    Value,
    File,
    Directory,
    Variable,
    Function,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderResult {
    pub key: ResolutionSnapshotKey,
    pub candidates: Vec<CompletionCandidate>,
    /// Completion completeness never implies grammar validation authority.
    pub complete: bool,
    pub failures: Vec<String>,
}

impl ProviderRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.key.target_id != self.context.target_id {
            return Err("provider target mismatch");
        }
        if self.words.len() > 4096 {
            return Err("too many completion words");
        }
        let word = self
            .words
            .get(self.word_index)
            .ok_or("completion word index is out of bounds")?;
        if !word.value.is_char_boundary(self.byte_offset_in_word) {
            return Err("completion cursor is not a UTF-8 boundary");
        }
        if self
            .words
            .iter()
            .any(|word| word.value.contains('\0') || word.value.len() > 1024 * 1024)
        {
            return Err("completion word contains unsupported data");
        }
        if self
            .words
            .iter()
            .filter_map(|word| word.source_range.as_ref())
            .any(|range| range.start > range.end)
        {
            return Err("completion source range is reversed");
        }
        Ok(())
    }
}
