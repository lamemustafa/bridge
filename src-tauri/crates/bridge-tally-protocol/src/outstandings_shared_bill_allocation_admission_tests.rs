use super::bill_allocation_without_type_is_placeholder;

#[test]
fn an_untyped_row_with_no_name_is_a_placeholder() {
    for name in [None, Some(""), Some("   ")] {
        assert!(
            bill_allocation_without_type_is_placeholder(name),
            "{name:?} carries no bill identity, so the row is a placeholder"
        );
    }
}

#[test]
fn an_untyped_row_that_names_a_bill_is_not_a_placeholder() {
    for name in [Some("SET-INV-001"), Some("  SET-INV-001  ")] {
        assert!(
            !bill_allocation_without_type_is_placeholder(name),
            "{name:?} names a bill, so the missing type is malformed input"
        );
    }
}
