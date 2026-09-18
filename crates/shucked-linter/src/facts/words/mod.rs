use super::*;
use crate::Locator;

mod traversal;

pub(in crate::facts) use traversal::{
    WordSubtreeVisitor, WordTraversalContext, WordTraversalOrigin, WordTraversalPatternContext,
    WordTraversalState, walk_word_subtree,
};

include!("expansion.rs");
include!("occurrence.rs");
include!("arithmetic.rs");
include!("command_facts.rs");
include!("quote_spans.rs");
include!("tests.rs");
