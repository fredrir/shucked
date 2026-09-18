#![warn(missing_docs)]

//! Shell lexer and parser APIs for the Shucked workspace.
//!
//! `shucked-parser` turns shell source text into `shucked-ast` syntax trees and also exposes a
//! source-backed lexer for lower-level tooling.
//!
//! [`parser::ParseResult`] is produced by [`parser::Parser::parse`] and keeps its fields public for
//! inspection. It is non-exhaustive so parser-owned output can gain fields without invalidating
//! downstream code. [`parser::ParseStatus`] and shell dialects remain exhaustive semantic sets.
//! The returned syntax tree is a [`shucked_ast::File`]; consumers that traverse AST nodes should add
//! `shucked-ast` as a direct dependency.
//!
//! ```
//! use shucked_parser::parser::{ParseStatus, Parser};
//!
//! let result = Parser::new("echo hello\n").parse();
//! match result.status {
//!     ParseStatus::Clean => assert!(result.diagnostics.is_empty()),
//!     ParseStatus::Recovered | ParseStatus::Fatal => {}
//! }
//! let _file = &result.file;
//! ```

mod error;
/// Parsing entrypoints, lexer types, and shell-profile configuration.
pub mod parser;
/// Shebang parsing helpers shared by Shucked crates.
pub mod shebang;

/// Error types returned by parser operations.
pub use error::{Error, Result};
/// Shell dialect, profile, and option types exposed by the parser.
pub use parser::{
    OptionValue, ShellDialect, ShellProfile, ZshEmulationMode, ZshOptionState,
    text_is_self_contained_arithmetic_expression, text_looks_like_nontrivial_arithmetic_expression,
};
