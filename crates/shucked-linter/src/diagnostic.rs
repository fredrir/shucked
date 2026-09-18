use shucked_ast::Span;

use crate::{Fix, Rule, Violation};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Advisory diagnostic that does not indicate likely incorrect behavior.
    Hint,
    /// Potential problem that warrants review.
    Warning,
    /// Definite error or invalid shell construct.
    Error,
}

impl Severity {
    /// Returns the lowercase name used by Shucked report formats.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// A diagnostic produced by Shucked analysis.
///
/// Fields remain public for downstream inspection, but consumers should not construct
/// diagnostics with struct literals.
///
/// ```compile_fail
/// use shucked_linter::{Diagnostic, Rule, Severity};
///
/// let _ = Diagnostic {
///     rule: Rule::UnusedAssignment,
///     message: String::new(),
///     severity: Severity::Warning,
///     span: todo!(),
///     fix: None,
///     fix_title: None,
///     alternative_fixes: Vec::new(),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    /// Rule that emitted this diagnostic.
    pub rule: Rule,
    /// Human-readable explanation of the violation.
    pub message: String,
    /// Effective severity after applying configuration overrides.
    pub severity: Severity,
    /// Source span attributed to the diagnostic.
    ///
    /// Spans are defined by the `shucked-ast` crate and use byte offsets into the analyzed source.
    pub span: Span,
    /// Optional edit set that can correct the violation.
    pub fix: Option<Fix>,
    /// Optional human-readable label for the fix.
    pub fix_title: Option<String>,
    /// Optional alternative fixes that can correct the violation.
    pub alternative_fixes: Vec<AlternativeFix>,
}

/// An alternative autofix that can resolve a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlternativeFix {
    /// Human-readable label for the alternative fix.
    pub title: String,
    /// The edit set to apply.
    pub fix: Fix,
}

impl AlternativeFix {
    pub fn new(title: impl Into<String>, fix: Fix) -> Self {
        Self {
            title: title.into(),
            fix,
        }
    }
}

impl Diagnostic {
    /// Creates a diagnostic from a rule-specific violation and source span.
    pub fn new<V: Violation>(violation: V, span: Span) -> Self {
        Self {
            rule: V::rule(),
            message: violation.message(),
            severity: V::rule().default_severity(),
            span,
            fix: None,
            fix_title: violation.fix_title(),
            alternative_fixes: Vec::new(),
        }
    }

    /// Returns the stable rule code for this diagnostic.
    pub const fn code(&self) -> &'static str {
        self.rule.code()
    }

    /// Attaches an autofix to this diagnostic.
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    /// Attaches an alternative autofix to this diagnostic.
    pub fn with_alternative_fix(mut self, title: impl Into<String>, fix: Fix) -> Self {
        self.alternative_fixes.push(AlternativeFix::new(title, fix));
        self
    }

    /// Sets or overrides the fix title.
    pub fn with_fix_title(mut self, title: impl Into<String>) -> Self {
        self.fix_title = Some(title.into());
        self
    }
}
