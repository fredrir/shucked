use shucked_ast::Span;
use shucked_linter::{Applicability, Diagnostic, Edit, Fix, Rule, Violation, apply_fixes};

struct TestEdit;
impl Violation for TestEdit {
    fn rule() -> Rule {
        Rule::UnusedAssignment
    }
    fn message(&self) -> String {
        "test edit".into()
    }
}

fn diagnostic(edit: Edit) -> Diagnostic {
    Diagnostic::new(TestEdit, Span::default()).with_fix(Fix::unsafe_edit(edit))
}

#[test]
fn applied_fixes_map_unchanged_ranges_and_reject_replaced_ranges() {
    let applied = apply_fixes(
        "FIRST=old\nVALUE=current\n",
        &[diagnostic(Edit::replacement_at(0, 5, "_"))],
        Applicability::Unsafe,
    );
    assert_eq!(applied.map_range(10..15), Some(6..11));
    assert_eq!(applied.map_range(0..5), None);
    assert_eq!(applied.map_range(3..7), None);
}

#[test]
fn range_mapping_uses_only_applied_nonconflicting_edits() {
    let applied = apply_fixes(
        "FIRST=old\nVALUE=current\n",
        &[
            diagnostic(Edit::replacement_at(0, 5, "_")),
            diagnostic(Edit::deletion_at(0, 9)),
            diagnostic(Edit::insertion(10, "# comment\n")),
            diagnostic(Edit::insertion(23, "# end\n")),
        ],
        Applicability::Unsafe,
    );
    assert_eq!(applied.fixes_applied, 3);
    assert_eq!(applied.map_range(10..15), Some(16..21));
    assert_eq!(&applied.code[16..21], "VALUE");
    assert_eq!(applied.map_range(5..9), Some(1..5));
}

#[test]
fn insertions_inside_a_binding_invalidate_its_range() {
    let applied = apply_fixes(
        "VALUE=1",
        &[diagnostic(Edit::insertion(2, "OTHER"))],
        Applicability::Unsafe,
    );
    assert_eq!(applied.map_range(0..5), None);
    let unchanged = apply_fixes("VALUE=1", &[], Applicability::Safe);
    assert_eq!(unchanged.map_range(0..5), Some(0..5));
}
