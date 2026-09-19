//! Fish semantic tokens consume the same command identities as diagnostics and hover.
use crate::edit::offset_to_position;
use crate::session::DocumentSnapshot;
use lsp_types::{SemanticToken, SemanticTokens};

pub(crate) fn full(snapshot: &DocumentSnapshot) -> SemanticTokens {
    let document = snapshot.query().document();
    let source = document.contents();
    let fish = shucked_semantic::analyze_fish(source);
    let analysis = snapshot.command_service.analysis(snapshot);
    let mut spans = vec![];
    for function in &fish.functions {
        spans.push((function.name_span, 1, 3));
    }
    for (site, resolution) in &analysis.sites {
        let kind = match resolution {
            shucked_command::CommandResolution::Resolved(resolved)
                if resolved.kind == shucked_command::CommandKind::Function =>
            {
                1
            }
            _ => 9,
        };
        let modifiers = if matches!(resolution, shucked_command::CommandResolution::Missing(_)) {
            1 << 4
        } else {
            0
        };
        spans.push((site.name_span(), kind, modifiers));
        for word in site.words.iter().skip(1) {
            if let Some(raw) = source.get(word.span.start.offset()..word.span.end.offset()) {
                if raw.starts_with(['\'', '"']) {
                    spans.push((word.span, 4, 0));
                } else if raw.starts_with('$') && !raw.contains('(') {
                    spans.push((word.span, 2, 0));
                }
            }
        }
    }
    spans.sort_by_key(|(span, _, _)| (span.start.offset(), span.end.offset()));
    spans.dedup_by_key(|(span, _, _)| (span.start.offset(), span.end.offset()));
    let mut data = vec![];
    let mut previous_line = 0;
    let mut previous_char = 0;
    let mut previous_end = 0;
    for (span, token_type, token_modifiers_bitset) in spans {
        if span.start.offset() < previous_end || span.end.offset() > source.len() {
            continue;
        }
        let start = offset_to_position(
            source,
            document.index(),
            span.start.offset(),
            snapshot.encoding(),
        );
        let end = offset_to_position(
            source,
            document.index(),
            span.end.offset(),
            snapshot.encoding(),
        );
        if start.line != end.line || end.character <= start.character {
            continue;
        }
        let delta_line = start.line.saturating_sub(previous_line);
        data.push(SemanticToken {
            delta_line,
            delta_start: if delta_line == 0 {
                start.character.saturating_sub(previous_char)
            } else {
                start.character
            },
            length: end.character - start.character,
            token_type,
            token_modifiers_bitset,
        });
        previous_line = start.line;
        previous_char = start.character;
        previous_end = span.end.offset();
    }
    SemanticTokens {
        result_id: None,
        data,
    }
}
