use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

use bridge_tally_core::TallyDate;
use bridge_tally_protocol::{
    native_outstandings::CompanyCurrency, native_trial_balance::parse_native_trial_balance,
};
use quick_xml::{events::Event, Reader};
use zip::ZipArchive;

use super::*;

fn captured_read() -> TrialBalanceRead {
    let report = parse_native_trial_balance(
        include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
        ),
        "eebb9a9f-1679-4468-9e8f-814c729674cb",
    )
    .unwrap();
    TrialBalanceRead {
        company_guid: "eebb9a9f-1679-4468-9e8f-814c729674cb".into(),
        company_name: "Captured Books".into(),
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260902").unwrap(),
        currency: CompanyCurrency {
            symbol: "₹".into(),
            mailing_name: "INR".into(),
            currency_count: 1,
            decimal_places: 3,
            is_inr: true,
            base_determined: true,
            masters: Vec::new(),
        },
        totals: crate::reports::trial_balance::observed_totals(&report).unwrap(),
        report,
        read_at: "2026-09-08T00:00:00Z".into(),
        evidence: crate::tally::runtime::RuntimeReadEvidence {
            request_sha256: "a".repeat(64),
            response_sha256: "b".repeat(64),
            bytes: 42,
        },
    }
}

fn workbook_text(bytes: &[u8]) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in [
        "xl/worksheets/sheet1.xml",
        "xl/sharedStrings.xml",
        "xl/styles.xml",
    ] {
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
    }
    text
}

fn worksheet_xml(bytes: &[u8]) -> String {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    archive
        .by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

fn worksheet_numeric_cells(bytes: &[u8]) -> BTreeMap<String, String> {
    let xml = worksheet_xml(bytes);

    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    let mut cells = BTreeMap::new();
    let mut cell = None;
    let mut in_value = false;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(tag) if tag.name().as_ref() == b"c" => {
                cell = tag.attributes().find_map(|attribute| {
                    let attribute = attribute.ok()?;
                    (attribute.key.as_ref() == b"r")
                        .then(|| String::from_utf8_lossy(attribute.value.as_ref()).into_owned())
                });
            }
            Event::Start(tag) if tag.name().as_ref() == b"v" => in_value = true,
            Event::Text(text) if in_value => {
                if let Some(reference) = cell.as_ref() {
                    cells.insert(reference.clone(), text.decode().unwrap().into_owned());
                }
            }
            Event::End(tag) if tag.name().as_ref() == b"v" => in_value = false,
            Event::End(tag) if tag.name().as_ref() == b"c" => cell = None,
            Event::Eof => break,
            _ => {}
        }
    }
    cells
}

#[test]
fn captured_export_preserves_empty_and_writes_formula_shaped_ledger_as_text() {
    let mut read = captured_read();
    read.report.rows[0].name = "=SUM(A1:A2)".into();
    let bytes = render_trial_balance_xlsx(&read).unwrap();
    let text = workbook_text(&bytes);
    assert!(text.contains("=SUM(A1:A2)"));
    assert!(!text.contains("<f>SUM(A1:A2)</f>"));
    assert!(!text.contains(">-</t>"));
    assert!(text.contains("formatCode=\"##,##,##0.000\""));
}

#[test]
fn captured_export_retains_native_signed_debit_and_credit_cells() {
    let bytes = render_trial_balance_xlsx(&captured_read()).unwrap();
    let cells = worksheet_numeric_cells(&bytes);

    // The second captured ledger has a negative debit and positive credit.
    // These cells must retain the source signs; desktop-only magnitude
    // presentation is not an export transformation.
    assert_eq!(cells.get("E14"), Some(&"-4777".to_string()));
    assert_eq!(cells.get("F14"), Some(&"4500".to_string()));
    assert!(!cells.contains_key("E13"));
    assert!(worksheet_xml(&bytes).contains("<autoFilter ref=\"A12:G18\"/>"));
}

#[test]
fn captured_export_wraps_total_qualification_text() {
    let bytes = render_trial_balance_xlsx(&captured_read()).unwrap();
    let xml = worksheet_xml(&bytes);

    let text = workbook_text(&bytes);
    assert!(text.contains(r#"<alignment wrapText="1"/>"#));
    for cell in ["E19", "F19", "G19"] {
        assert!(
            xml.contains(&format!(r#"<c r="{cell}" s="1" t="s">"#)),
            "expected wrapped cell style for {cell}"
        );
    }
    assert!(text.contains("Observed numeric total:"));
    assert!(text.contains("empty fields:"));
}

#[test]
fn unsafe_excel_precision_withholds_captured_export() {
    let mut read = captured_read();
    read.report.rows[0].opening = NativeTrialBalanceAmount::Present(
        bridge_tally_core::ExactDecimal::parse("9007199254740993").unwrap(),
    );
    assert!(matches!(
        render_trial_balance_xlsx(&read),
        Err(TrialBalanceXlsxError::InvalidAmount(_))
    ));
}
