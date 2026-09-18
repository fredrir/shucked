use shucked_ast::{BinaryOp, CommandSubstitutionSyntax, ConditionalBinaryOp, Name};
use shucked_indexer::Indexer;
use shucked_parser::parser::{Parser, ShellDialect as ParseShellDialect};
use shucked_semantic::BindingAttributes;
use std::path::Path;

use super::{
    ConditionalNodeFact, ConditionalOperatorFamily, ExpansionContext, ExprStringHelperKind,
    GrepPatternSourceKind, SimpleTestOperatorFamily, SimpleTestShape, SimpleTestSyntax,
    SubstitutionHostKind, SubstitutionOutputIntent, SudoFamilyInvoker, WordFactHostKind,
    build_innermost_command_ids_by_offset, precomputed_command_id_for_offset,
};

use crate::WrapperKind;
use crate::facts::surface::PositionalParameterFragmentKind;
use crate::{
    ArithmeticLiteralBehavior, ArithmeticLiteralKind, LinterFacts, LinterSemanticArtifacts,
    ShellDialect,
};

mod assignments;
mod braces;
mod commands;
mod comments;
mod conditions;
mod flow;
mod functions;
mod support;
mod surface;

use support::{with_facts, with_facts_dialect};
