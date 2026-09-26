//! Unit tests for the delta diff and the range selection over encoded tokens.
use lsp_types::{Position, Range, SemanticToken, SemanticTokensEdit};

use super::{semantic_tokens_edits, tokens_in_range};

fn token(delta_line: u32, delta_start: u32, length: u32, token_type: u32) -> SemanticToken {
    SemanticToken {
        delta_line,
        delta_start,
        length,
        token_type,
        token_modifiers_bitset: 0,
    }
}

fn wire(tokens: &[SemanticToken]) -> Vec<u32> {
    tokens
        .iter()
        .flat_map(|token| {
            [
                token.delta_line,
                token.delta_start,
                token.length,
                token.token_type,
                token.token_modifiers_bitset,
            ]
        })
        .collect()
}

fn apply(old: &[SemanticToken], edits: &[SemanticTokensEdit]) -> Vec<SemanticToken> {
    let mut data = wire(old);
    let mut ordered = edits.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    for edit in ordered {
        let start = edit.start as usize;
        let end = start + edit.delete_count as usize;
        let inserted = edit.data.as_deref().map(wire).unwrap_or_default();
        data.splice(start..end, inserted);
    }
    data.chunks_exact(5)
        .map(|chunk| SemanticToken {
            delta_line: chunk[0],
            delta_start: chunk[1],
            length: chunk[2],
            token_type: chunk[3],
            token_modifiers_bitset: chunk[4],
        })
        .collect()
}

fn assert_single_edit(
    old: &[SemanticToken],
    new: &[SemanticToken],
    start: u32,
    delete_count: u32,
    data: Option<&[SemanticToken]>,
) {
    let edits = semantic_tokens_edits(old, new);
    assert_eq!(
        edits,
        vec![SemanticTokensEdit {
            start,
            delete_count,
            data: data.map(<[SemanticToken]>::to_vec),
        }]
    );
    assert_eq!(apply(old, &edits), new);
}

#[test]
fn identical_data_produces_no_edits() {
    let data = [token(0, 0, 2, 0), token(1, 4, 3, 2), token(0, 5, 1, 4)];
    assert!(semantic_tokens_edits(&data, &data).is_empty());
    assert!(semantic_tokens_edits(&[], &[]).is_empty());
}

#[test]
fn insertion_in_the_middle_is_one_edit_without_deletions() {
    let old = [token(0, 0, 2, 0), token(1, 0, 4, 1), token(2, 0, 2, 0)];
    let inserted = [token(1, 0, 3, 5), token(0, 4, 1, 4)];
    let new = [old[0], inserted[0], inserted[1], old[1], old[2]];
    assert_single_edit(&old, &new, 5, 0, Some(&inserted));
}

#[test]
fn deletion_is_one_edit_without_data() {
    let old = [
        token(0, 0, 2, 0),
        token(1, 0, 4, 1),
        token(0, 5, 1, 4),
        token(2, 0, 2, 0),
    ];
    let new = [old[0], old[3]];
    assert_single_edit(&old, &new, 5, 10, None);
}

#[test]
fn replacement_covers_only_the_changed_middle() {
    let old = [
        token(0, 0, 2, 0),
        token(1, 0, 4, 1),
        token(0, 5, 1, 4),
        token(2, 0, 2, 0),
    ];
    let replacement = [token(1, 0, 9, 9)];
    let new = [old[0], replacement[0], old[3]];
    assert_single_edit(&old, &new, 5, 10, Some(&replacement));
}

#[test]
fn edits_at_the_edges_and_of_empty_data_are_handled() {
    let a = token(0, 0, 2, 0);
    let b = token(1, 0, 4, 1);
    assert_single_edit(&[a, b], &[b], 0, 5, None);
    assert_single_edit(&[a, b], &[a], 5, 5, None);
    assert_single_edit(&[], &[a], 0, 0, Some(&[a]));
    assert_single_edit(&[a], &[], 0, 5, None);
    assert_single_edit(&[a, b], &[b, a], 0, 10, Some(&[b, a]));
}

#[test]
fn repeated_tokens_do_not_let_the_suffix_overlap_the_prefix() {
    let a = token(1, 0, 1, 0);
    assert_single_edit(&[a, a], &[a, a, a], 10, 0, Some(&[a]));
    assert_single_edit(&[a, a, a], &[a, a], 10, 5, None);
}

#[test]
fn range_selection_re_encodes_from_the_document_start() {
    // Absolute positions: (0,0) (0,5) (2,4) (2,8) (5,0).
    let data = [
        token(0, 0, 2, 0),
        token(0, 5, 3, 1),
        token(2, 4, 1, 2),
        token(0, 4, 2, 3),
        token(3, 0, 4, 4),
    ];

    let line_two = tokens_in_range(&data, Range::new(Position::new(2, 0), Position::new(3, 0)));
    assert_eq!(line_two, vec![token(2, 4, 1, 2), token(0, 4, 2, 3)]);

    let tail = tokens_in_range(&data, Range::new(Position::new(2, 9), Position::new(9, 0)));
    assert_eq!(tail, vec![token(2, 8, 2, 3), token(3, 0, 4, 4)]);

    let partial = tokens_in_range(&data, Range::new(Position::new(0, 6), Position::new(0, 7)));
    assert_eq!(partial, vec![token(0, 5, 3, 1)]);

    let boundary = tokens_in_range(&data, Range::new(Position::new(0, 2), Position::new(0, 5)));
    assert!(boundary.is_empty(), "touching ranges do not intersect");

    let empty_line = tokens_in_range(&data, Range::new(Position::new(1, 0), Position::new(2, 0)));
    assert!(empty_line.is_empty());

    let reversed = tokens_in_range(&data, Range::new(Position::new(3, 0), Position::new(2, 0)));
    assert_eq!(reversed, line_two);
}
