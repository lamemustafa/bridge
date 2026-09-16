use std::io::Read;

use bridge_tally_core::ExactDecimal;

use super::*;
use crate::reports::outstandings_working_paper::{
    build_outstandings_working_paper, OutstandingsWorkingPaper, OutstandingsWorkingPaperSource,
};
use crate::tally::{ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, UnallocatedParty};

fn decimal(value: &str) -> ExactDecimal {
    ExactDecimal::parse(value).expect("synthetic exact decimal")
}

fn paper() -> OutstandingsWorkingPaper {
    build_outstandings_working_paper(OutstandingsWorkingPaperSource {
        company: "Synthetic Books".to_string(),
        company_guid: "synthetic-guid".to_string(),
        as_of_yyyymmdd: "20260825".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        synced_at_unix_ms: 1_777_000_000_000,
        source_bytes: 512,
        source_ageing_anchor: OutstandingsAgeingAnchor::DueDate,
        receivable_bill_total: decimal("125.25"),
        payable_bill_total: ExactDecimal::zero(),
        unallocated_total: decimal("10"),
        open_bills: vec![OpenBillRow {
            party: "=FORMULA Party".to_string(),
            reference: "+INV-1 नमस्ते".to_string(),
            bill_date: "20260501".to_string(),
            due_date: "20260601".to_string(),
            amount: decimal("125.25"),
            age_days: Some(85),
            kind: ExposureDirection::Receivable,
        }],
        unallocated_by_party: vec![UnallocatedParty {
            party: "=FORMULA Party".to_string(),
            amount: decimal("10"),
            direction: ExposureDirection::Receivable,
        }],
    })
    .expect("synthetic paper builds")
}

#[test]
fn renders_two_sheet_workbook_with_controls_and_text_cells() {
    let bytes = render_outstandings_working_paper_xlsx(&paper()).expect("workbook renders");
    assert_eq!(&bytes[0..2], b"PK");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in [
        "xl/workbook.xml",
        "xl/worksheets/sheet1.xml",
        "xl/worksheets/sheet2.xml",
        "xl/sharedStrings.xml",
    ] {
        let mut entry = archive.by_name(name).unwrap();
        entry.read_to_string(&mut text).unwrap();
    }
    assert!(text.contains("Summary"));
    assert!(text.contains("Bills"));
    assert!(text.contains("CONTROL TOTALS"));
    assert!(text.contains("=FORMULA Party"));
    assert!(text.contains("+INV-1 नमस्ते"));
    assert!(
        text.contains("<v>125.25</v>"),
        "amount must be a numeric cell"
    );
    assert!(
        !text.contains("<f>"),
        "untrusted labels must not become formulas"
    );
}

#[test]
fn bucket_boundaries_and_future_dates_are_explicit() {
    for (age, label) in [
        (None, "Date not reached"),
        (Some(30), "0-30 days"),
        (Some(31), "31-60 days"),
        (Some(60), "31-60 days"),
        (Some(61), "61-90 days"),
        (Some(90), "61-90 days"),
        (Some(91), "90+ days"),
    ] {
        assert_eq!(ageing_bucket(age), label);
    }
}

#[test]
fn numeric_projection_reuses_the_party_statement_round_trip_policy() {
    assert_eq!(amount_to_f64("0.10").unwrap(), 0.1);
    assert_eq!(amount_to_f64("0.001").unwrap(), 0.001);
    assert!(matches!(
        amount_to_f64("9007199254740993"),
        Err(OutstandingsWorkingPaperXlsxError::InvalidAmount(_))
    ));
}
