//! Renders a completed native Trial Balance observation; it never contacts Tally.

use bridge_tally_protocol::native_trial_balance::NativeTrialBalanceAmount;
use rust_xlsxwriter::{Format, Workbook, XlsxError};

use super::party_statement_xlsx::amount_to_f64;
use crate::tally::runtime::TrialBalanceRead;

const EXCEL_MAX_ROWS: usize = 1_048_576;

#[derive(Debug, thiserror::Error)]
pub enum TrialBalanceXlsxError {
    #[error("Bridge could not build the Trial Balance workbook: {0}")]
    Workbook(#[from] XlsxError),
    #[error("Trial Balance exceeds Excel's row limit")]
    RowLimit,
    #[error("Bridge could not represent a Trial Balance amount in Excel ({0})")]
    InvalidAmount(String),
}

/// Renders the already-completed observation, including its freshness and
/// source commitment. All untrusted names are explicit string cells.
pub fn render_trial_balance_xlsx(
    read: &TrialBalanceRead,
) -> Result<Vec<u8>, TrialBalanceXlsxError> {
    if read.report.rows.len().saturating_add(18) > EXCEL_MAX_ROWS {
        return Err(TrialBalanceXlsxError::RowLimit);
    }
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.set_name("Trial Balance")?;
    let bold = Format::new().set_bold();
    let number_format = amount_num_format(read)?;
    let amount = Format::new().set_num_format(&number_format);
    let bold_amount = Format::new().set_bold().set_num_format(&number_format);

    let mut row = 0;
    for (label, value) in [
        ("Company", read.company_name.as_str()),
        ("Company GUID", read.company_guid.as_str()),
        (
            "Period",
            &format!("{} to {}", read.from.as_str(), read.to.as_str()),
        ),
        ("Currency", read.currency.mailing_name.as_str()),
        ("Read completed (UTC)", read.read_at.as_str()),
        (
            "Native Trial Balance rows",
            &read.report.rows.len().to_string(),
        ),
        (
            "Source request SHA-256",
            read.evidence.request_sha256.as_str(),
        ),
        (
            "Source response SHA-256",
            read.evidence.response_sha256.as_str(),
        ),
        ("Source bytes", &read.evidence.bytes.to_string()),
    ] {
        sheet.write_string(row, 0, label)?;
        sheet.write_string(row, 1, value)?;
        row += 1;
    }
    sheet.write_string(row, 0, "Limitation")?;
    sheet.write_string(row, 1, "Observed native Trial Balance fields only. Opening, debit, credit and closing retain Tally's signed values. Negative opening/closing is Dr, positive is Cr; the desktop displays debit/credit magnitudes. Paired source stability does not establish an atomic Tally snapshot.")?;
    row += 2;

    for (column, label) in [
        "Ledger",
        "GUID",
        "Parent",
        "Opening (source signed)",
        "Debit (source signed)",
        "Credit (source signed)",
        "Closing (source signed)",
    ]
    .into_iter()
    .enumerate()
    {
        sheet.write_string_with_format(row, column as u16, label, &bold)?;
    }
    let header = row;
    row += 1;
    for item in &read.report.rows {
        sheet.write_string(row, 0, &item.name)?;
        sheet.write_string(row, 1, &item.guid)?;
        sheet.write_string(row, 2, item.parent.workbook_text())?;
        for (column, value) in [
            (3, &item.opening),
            (4, &item.debit),
            (5, &item.credit),
            (6, &item.closing),
        ] {
            write_amount(sheet, row, column, value, &amount)?;
        }
        row += 1;
    }
    let last_ledger_row = row.saturating_sub(1);

    sheet.write_string_with_format(row, 0, "OBSERVED TOTALS", &bold)?;
    for (column, total) in [
        (3, &read.totals.opening),
        (4, &read.totals.debit),
        (5, &read.totals.credit),
        (6, &read.totals.closing),
    ] {
        if total.empty_count == 0 {
            sheet.write_number_with_format(
                row,
                column,
                decimal(total.sum.as_str())?,
                &bold_amount,
            )?;
        } else {
            sheet.write_string(
                row,
                column,
                format!(
                    "Observed numeric total: {}; empty fields: {}",
                    total.sum.as_str(),
                    total.empty_count
                ),
            )?;
        }
    }
    row += 1;
    if read.totals.opening.empty_count == 0 {
        sheet.write_string_with_format(row, 0, "Opening difference (observed)", &bold)?;
        sheet.write_number_with_format(
            row,
            3,
            decimal(read.totals.opening.sum.as_str())?,
            &bold_amount,
        )?;
    } else {
        sheet.write_string_with_format(row, 0, "Opening values", &bold)?;
        sheet.write_string(
            row,
            3,
            format!(
                "Observed only; {} empty fields",
                read.totals.opening.empty_count
            ),
        )?;
    }

    sheet.set_freeze_panes(header + 1, 1)?;
    sheet.autofilter(header, 0, last_ledger_row, 6)?;
    sheet.set_column_width(0, 34)?;
    sheet.set_column_width(1, 38)?;
    sheet.set_column_width(2, 28)?;
    for column in 3..=6 {
        sheet.set_column_width(column, 22)?;
    }
    workbook
        .save_to_buffer()
        .map_err(TrialBalanceXlsxError::from)
}

fn write_amount(
    sheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    column: u16,
    value: &NativeTrialBalanceAmount,
    format: &Format,
) -> Result<(), TrialBalanceXlsxError> {
    match value {
        NativeTrialBalanceAmount::Present(value) => sheet
            .write_number_with_format(row, column, decimal(value.as_str())?, format)
            .map(|_| ())
            .map_err(TrialBalanceXlsxError::from),
        NativeTrialBalanceAmount::PresentEmpty => sheet
            .write_blank(row, column, format)
            .map(|_| ())
            .map_err(TrialBalanceXlsxError::from),
    }
}

fn decimal(value: &str) -> Result<f64, TrialBalanceXlsxError> {
    amount_to_f64(value).map_err(|_| TrialBalanceXlsxError::InvalidAmount(value.to_string()))
}

fn amount_num_format(read: &TrialBalanceRead) -> Result<String, TrialBalanceXlsxError> {
    let mut places = usize::from(read.currency.decimal_places);
    for row in &read.report.rows {
        for value in [&row.opening, &row.debit, &row.credit, &row.closing] {
            if let NativeTrialBalanceAmount::Present(value) = value {
                decimal(value.as_str())?;
                places = places.max(
                    value
                        .as_str()
                        .split_once('.')
                        .map_or(0, |(_, fraction)| fraction.len()),
                );
            }
        }
    }
    if places > 15 {
        return Err(TrialBalanceXlsxError::InvalidAmount(
            "currency precision exceeds Excel-safe exact projection".to_string(),
        ));
    }
    Ok(if places == 0 {
        "##,##,##0".to_string()
    } else {
        format!("##,##,##0.{}", "0".repeat(places))
    })
}

#[cfg(test)]
mod tests {
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
            include_str!("../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"),
            "eebb9a9f-1679-4468-9e8f-814c729674cb",
        ).unwrap();
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
}
