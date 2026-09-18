use shucked_ast::TextRange;

use crate::PositionEncoding;

pub(crate) trait RangeExt {
    fn to_text_range(
        &self,
        text: &str,
        index: &shucked_indexer::LineIndex,
        encoding: PositionEncoding,
    ) -> TextRange;
}

pub(crate) trait PositionExt {
    fn to_offset(
        &self,
        text: &str,
        index: &shucked_indexer::LineIndex,
        encoding: PositionEncoding,
    ) -> usize;
}

impl RangeExt for lsp_types::Range {
    fn to_text_range(
        &self,
        text: &str,
        index: &shucked_indexer::LineIndex,
        encoding: PositionEncoding,
    ) -> TextRange {
        crate::edit::to_text_range(self, text, index, encoding)
    }
}

impl PositionExt for lsp_types::Position {
    fn to_offset(
        &self,
        text: &str,
        index: &shucked_indexer::LineIndex,
        encoding: PositionEncoding,
    ) -> usize {
        crate::edit::position_to_offset(text, index, *self, encoding)
    }
}
