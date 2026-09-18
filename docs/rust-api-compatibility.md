# Rust API compatibility

shucked publishes its analysis crates so tools can embed the parser and linter. Those crates are
still pre-1.0, and the entire public type graph is not yet a stable compatibility contract. The
supported integration path is intentionally narrower than everything that Rust visibility makes
reachable today.

## Supported integration path

For lint integrations, prefer these root-level APIs from `shucked-linter`:

- construct `LinterSettings` with `Default`, `for_rule`, `for_rules`, or `from_selectors`;
- create an `AnalysisRequest` from a parser result or AST;
- call `analyze` or `lint`;
- inspect `AnalysisResult` and `Diagnostic` fields;
- identify rules with `Rule::code`, `code_to_rule`, and `Rule::iter`.

For parser integrations, construct a `shucked_parser::parser::Parser`, call `parse`, and inspect the
public fields on `ParseResult`.

## Returned type and dependency graph

Supported entrypoints intentionally cross crate boundaries. Add a direct dependency on the crate
that owns a returned type when traversing it or naming it in your own public API.

| Supported API                         | Exposed type                                                                                                         | Consumer dependency                                                                                                                                                                         |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `shucked_parser::parser::ParseResult` | `shucked_ast::File`, plus `Span` values in parser diagnostics and syntax facts                                       | Depend on `shucked-ast` to traverse the syntax tree or work with source ranges.                                                                                                             |
| `shucked_linter::AnalysisRequest`     | `shucked_parser::parser::ParseResult`, `shucked_ast::File`, `shucked_indexer::Indexer`, and semantic resolver traits | Depend directly on the parser, AST, or indexer crate when using those types. `SourcePathResolver`, `PluginResolver`, and plugin request/response types are re-exported by `shucked-linter`. |
| `shucked_linter::AnalysisResult`      | `shucked_semantic::SemanticModel`                                                                                    | Depend on `shucked-semantic` when naming the model or traversing its semantic query results.                                                                                                |
| `shucked_linter::Diagnostic`          | `shucked_ast::Span`                                                                                                  | Depend on `shucked-ast` for position, span, and byte-range operations.                                                                                                                      |

The `shucked-ast` crate documents syntax nodes at its root, beginning with `File`, `Stmt`, and
`Command`. Internal traversal and semantic-build helpers remain hidden.

`LinterSettings`, `Diagnostic`, `AnalysisResult`, and `ParseResult` are non-exhaustive structs.
Their fields remain public for ergonomic inspection and, for settings, mutation. Downstream code
should not construct producer-owned result types with struct literals.

```rust
use shucked_linter::{LinterSettings, Rule, Severity};

let mut settings = LinterSettings::for_rules([
    Rule::UnusedAssignment,
    Rule::UndefinedVariable,
]);
settings
    .severity_overrides
    .insert(Rule::UnusedAssignment, Severity::Hint);
settings.report_environment_style_names = true;
```

`Rule` is a non-exhaustive enum because adding rules is routine. Match selected rules with a
fallback, or use rule codes when persisting an identity outside the process. Numeric enum
discriminants are an implementation detail.

```rust
use shucked_linter::Rule;

fn is_assignment_rule(rule: Rule) -> bool {
    match rule {
        Rule::UnusedAssignment | Rule::AssignmentSpacing => true,
        _ => false,
    }
}
```

## Intentional exhaustive sets

`Severity` and `ParseStatus` remain exhaustive. Their variants are deliberately closed semantic
sets, and downstream exhaustive matches are useful compiler checks. The parser and linter shell
dialect enums also remain exhaustive; adding a dialect is expected to require deliberate updates
throughout consumers rather than silently falling into a catch-all branch.

## Current inventory and boundary

| Crate              | Downstream-facing role                                                                | Compatibility decision                                                                                |
| ------------------ | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `shucked-linter`   | Settings, analysis requests/results, diagnostics, rules, metadata, and selected facts | Protect routine rule growth and top-level configuration/output structs. Keep closed enums exhaustive. |
| `shucked-parser`   | Parser entrypoints, profiles, parse output, lexer, and diagnostics                    | Protect `ParseResult` field growth. Keep `ParseStatus` and dialects exhaustive.                       |
| `shucked-ast`      | Shared syntax tree, spans, tokens, and operators                                      | Public and inspectable, but still structurally unstable; no blanket non-exhaustive annotations.       |
| `shucked-indexer`  | Positional and structural indexes                                                     | Public query APIs remain pre-1.0; no blanket annotations on index facts.                              |
| `shucked-semantic` | Semantic model, source closure, call graph, CFG, dataflow, and analysis facts         | Public query APIs remain pre-1.0; no blanket annotations on semantic facts.                           |

Opaque types that already use private fields and accessors do not need `#[non_exhaustive]` to
prevent external construction. Public AST and fact records remain directly constructible where
they are currently part of cross-crate assembly inside the workspace.

## Compatibility intent

Within this supported path, shucked can add a `Rule` variant or a field to one of the protected
configuration/output structs without forcing downstream exhaustive matches or struct literals to
change. Existing public fields remain readable, and `LinterSettings` remains customizable after
supported construction.

This is not a promise that every public AST, index, semantic fact, or rule implementation module
is stable. Those surfaces may change during the pre-1.0 period. Changes should be called out in
release notes, and consumers should prefer the root-level parser and linter entrypoints above over
depending on implementation-shaped modules.
