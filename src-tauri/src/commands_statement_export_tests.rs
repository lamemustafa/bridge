use super::*;

#[test]
fn repeated_statement_exports_get_distinct_new_files() {
    let destination = tempfile::tempdir().expect("synthetic destination");
    let first = write_unique_statement_file(
        destination.path(),
        "statement-party-20260809",
        "xlsx",
        b"one",
    )
    .expect("first statement");
    let second = write_unique_statement_file(
        destination.path(),
        "statement-party-20260809",
        "xlsx",
        b"two",
    )
    .expect("second statement");

    assert_eq!(
        first.file_name().and_then(|name| name.to_str()),
        Some("statement-party-20260809.xlsx")
    );
    assert_eq!(
        second.file_name().and_then(|name| name.to_str()),
        Some("statement-party-20260809-2.xlsx")
    );
    assert_eq!(std::fs::read(first).expect("first bytes"), b"one");
    assert_eq!(std::fs::read(second).expect("second bytes"), b"two");
}
