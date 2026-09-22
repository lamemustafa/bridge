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
        .map(|row| row.reference.as_str())
        .collect::<Vec<_>>();
    assert_eq!(opening, ["FX-OPEN-1", "INR-OPEN-1"]);
}
