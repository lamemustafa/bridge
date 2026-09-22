//! bridge#612: a Bills report carrying an opening bill dated before the
//! company's `BOOKSFROM`, captured (`bills_receivable_forex_live`,
//! `LEDGER_CURRENCY_CAPTURE_PROVENANCE.md`).

use bridge_tally_primitives::TallyDate;
use bridge_tally_protocol::native_outstandings::parse_native_bill_rows;

fn decode(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

/// With the book's own `BOOKSFROM` (20250401), every bill parses, the two
/// opening bills dated `31-Mar-25` included. Before bridge#612 this refused
/// with `native_date_year_outside_book_window`.
#[test]
fn a_captured_bills_report_with_opening_bills_before_books_from_parses() {
    let rows = parse_native_bill_rows(
        &decode(include_bytes!(
            "fixtures/bills_receivable_forex_live.utf16le.xml"
        )),
        &TallyDate::parse("20250401").unwrap(),
        &TallyDate::parse("20250930").unwrap(),
    )
    .unwrap();
    assert_eq!(rows.len(), 18);
    let opening = rows
        .iter()
        .filter(|row| row.bill_date.as_str() == "20250331")
        .map(|row| (row.reference.as_str(), row.due_date.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        opening,
        [("FX-OPEN-1", "20250331"), ("INR-OPEN-1", "20250331")]
    );
}

/// Each due date is read against its own bill date, never the as-of date: read
/// ten years later, the same captured rows keep every due date in 2025 rather
/// than a century past the as-of date.
#[test]
fn a_due_date_is_read_against_its_bill_date_not_the_as_of_date() {
    let rows = parse_native_bill_rows(
        &decode(include_bytes!(
            "fixtures/bills_receivable_forex_live.utf16le.xml"
        )),
        &TallyDate::parse("20250401").unwrap(),
        &TallyDate::parse("20350930").unwrap(),
    )
    .unwrap();
    assert_eq!(rows.len(), 18);
    for row in &rows {
        assert!(
            row.due_date.as_str().starts_with("2025"),
            "{} due {}",
            row.reference,
            row.due_date.as_str()
        );
    }
}
