/// A statement that cannot be rendered exactly must FAIL, never render a
/// substituted figure. This pins the fail-closed behaviour of
/// `amount_to_f64`: an earlier revision returned `unwrap_or(0.0)`, which
/// would have written a real bill as zero into a document sent to a client.
#[test]
fn an_unrepresentable_amount_fails_instead_of_becoming_zero() {
    // Sanity: a well-formed amount converts.
    assert!(amount_to_f64("12").is_ok());

    // Every malformed or non-finite value must be refused outright. The
    // string is this private conversion boundary's direct input; an
    // `ExactDecimal` has already rejected malformed source data earlier.
    for unrepresentable in ["", "not-a-number", "1e999"] {
        assert!(matches!(
            amount_to_f64(unrepresentable),
            Err(PartyStatementXlsxError::InvalidAmount(value)) if value == unrepresentable
        ));
    }
}

#[test]
fn rejects_a_valid_decimal_that_excel_cannot_represent_exactly() {
    assert!(matches!(
        amount_to_f64("9007199254740993"),
        Err(PartyStatementXlsxError::InvalidAmount(value)) if value == "9007199254740993"
    ));

    assert!(matches!(
        amount_to_f64("9007199254740992"),
        Err(PartyStatementXlsxError::InvalidAmount(value)) if value == "9007199254740992"
    ));
    assert_eq!(amount_to_f64("999999999999999").unwrap(), 999999999999999.0);
    assert_eq!(
        amount_to_f64("1000000000000000").unwrap(),
        1_000_000_000_000_000.0
    );
    assert_eq!(
        amount_to_f64("0.000000000000001").unwrap(),
        0.000000000000001
    );
    assert_eq!(
        amount_to_f64("10000000000000.00").unwrap(),
        10_000_000_000_000.0
    );
    assert_eq!(amount_to_f64("42.00").unwrap(), 42.0);
}

#[test]
fn bill_direction_labels_make_mixed_party_amounts_unambiguous() {
    assert_eq!(
        bill_direction_label(ExposureDirection::Receivable),
        "Receivable"
    );
    assert_eq!(bill_direction_label(ExposureDirection::Payable), "Payable");
}
use super::*;
use crate::reports::party_statement::build_party_statement;
use crate::tally::{ExposureDirection, OpenBillRow, UnallocatedParty};
use bridge_tally_core::ExactDecimal;

fn bill(reference: &str, amount: &str, age_days: u32) -> OpenBillRow {
    OpenBillRow {
        party: "Aarav Textiles".to_string(),
        reference: reference.to_string(),
        bill_date: "20260101".to_string(),
        due_date: "20260201".to_string(),
        amount: ExactDecimal::parse(amount).unwrap(),
        age_days: Some(age_days),
        kind: ExposureDirection::Receivable,
    }
}

#[test]
fn renders_a_non_empty_workbook_for_a_billed_and_unallocated_party() {
    let bills = vec![bill("INV-1", "1250.75", 40)];
    let unallocated = vec![UnallocatedParty {
        party: "Aarav Textiles".to_string(),
        amount: ExactDecimal::parse("300.00").unwrap(),
        direction: ExposureDirection::Receivable,
    }];
    let statement =
        build_party_statement("Lab Co", "20260808", "Aarav Textiles", &bills, &unallocated)
            .unwrap();
    let bytes = render_party_statement_xlsx(&statement).unwrap();
    // A well-formed xlsx is a zip archive; the local-file-header
    // signature is the cheapest evidence this is real workbook bytes and
    // not an empty or truncated buffer.
    assert!(bytes.len() > 200);
    assert_eq!(&bytes[0..2], b"PK");
}

#[test]
fn renders_a_workbook_for_a_party_with_no_bills() {
    let unallocated = vec![UnallocatedParty {
        party: "On Account Only".to_string(),
        amount: ExactDecimal::parse("42.00").unwrap(),
        direction: ExposureDirection::Receivable,
    }];
    let statement =
        build_party_statement("Lab Co", "20260808", "On Account Only", &[], &unallocated).unwrap();
    let bytes = render_party_statement_xlsx(&statement).unwrap();
    assert!(bytes.len() > 200);
}

#[test]
fn renders_unallocated_direction_in_the_workbook_text() {
    let unallocated = vec![UnallocatedParty {
        party: "On Account Only".to_string(),
        amount: ExactDecimal::parse("42.00").unwrap(),
        direction: ExposureDirection::Payable,
    }];
    let statement =
        build_party_statement("Lab Co", "20260808", "On Account Only", &[], &unallocated).unwrap();
    let bytes = render_party_statement_xlsx(&statement).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in ["xl/worksheets/sheet1.xml", "xl/sharedStrings.xml"] {
        let mut entry = archive.by_name(name).unwrap();
        std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
    }
    assert!(text.contains("Unallocated Payable magnitude (no bill reference)"));
}

#[test]
fn an_invalid_date_is_rejected_rather_than_written_as_a_string() {
    let statement = build_party_statement(
        "Lab Co",
        "not-a-date",
        "Aarav Textiles",
        &[bill("INV-1", "10.00", 5)],
        &[],
    )
    .unwrap();
    let error = render_party_statement_xlsx(&statement).unwrap_err();
    assert!(matches!(error, PartyStatementXlsxError::InvalidDate(_)));
}
