//! Renders the exact party and ledger master source as one `.xlsx` workbook.

use bridge_tally_protocol::PartyLedgerMasterFieldObservation;
use rust_xlsxwriter::{Format, Workbook, XlsxError};

use super::party_ledger_master::PartyLedgerMasterWorkbook;
use super::party_statement_xlsx::amount_to_f64;
use super::schedule_iii::{build_schedule_iii_view, ScheduleIIIError};
use crate::tally::OutstandingsCurrencyAssertion;

const AMOUNT_NUM_FORMAT: &str = "##,##,##0.00";
const EXCEL_MAX_ROWS: usize = 1_048_576;

fn currency_label(assertion: OutstandingsCurrencyAssertion) -> &'static str {
    match assertion {
        OutstandingsCurrencyAssertion::Inr => "INR",
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PartyLedgerMasterXlsxError {
    #[error("Bridge could not build the party/ledger master workbook: {0}")]
    Workbook(#[from] XlsxError),
    #[error("Bridge could not represent a party/ledger master amount in Excel ({0})")]
    InvalidAmount(String),
    #[error("the party/ledger master exceeds Excel's row limit")]
    RowLimit,
    #[error("Bridge could not derive the traceable Schedule III view: {0}")]
    ScheduleIII(#[from] ScheduleIIIError),
}

pub(crate) fn render_party_ledger_master_xlsx(
    workbook_source: &PartyLedgerMasterWorkbook,
) -> Result<Vec<u8>, PartyLedgerMasterXlsxError> {
    let source = workbook_source.source();
    if source.rows.len().saturating_add(15) > EXCEL_MAX_ROWS {
        return Err(PartyLedgerMasterXlsxError::RowLimit);
    }

    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Ledger master")?;
    let bold = Format::new().set_bold();
    let amount = Format::new().set_num_format(AMOUNT_NUM_FORMAT);

    for (row, label, value) in [
        (0, "Company", source.company.as_str()),
        (1, "Company GUID", source.company_guid.as_str()),
        (2, "Balances as of", source.to.as_str()),
        (3, "Opening period starts", source.from.as_str()),
        (
            4,
            "Master response SHA-256",
            source.master_response_sha256.as_str(),
        ),
        (
            5,
            "Balance response SHA-256",
            source.balance_response_sha256.as_str(),
        ),
        (
            6,
            "Group response SHA-256",
            source.group_response_sha256.as_str(),
        ),
    ] {
        worksheet.write_string_with_format(row, 0, label, &bold)?;
        worksheet.write_string(row, 1, value)?;
    }
    worksheet.write_string_with_format(7, 0, "Repeated read agreement", &bold)?;
    worksheet.write_string(
        7,
        1,
        "Each source response was read twice and the paired wire bytes agreed before this workbook was enabled.",
    )?;
    worksheet.write_string_with_format(8, 0, "Covered", &bold)?;
    worksheet.write_string(
        8,
        1,
        "Ledger identity, parent, Party GSTIN and requested party/master fields when returned, plus opening/closing balances over the named period.",
    )?;
    worksheet.write_string_with_format(9, 0, "Not observed fields", &bold)?;
    worksheet.write_string(
        9,
        1,
        PartyLedgerMasterFieldObservation::NOT_OBSERVED_WORKBOOK_DISCLOSURE,
    )?;
    worksheet.write_string_with_format(10, 0, "Currency", &bold)?;
    worksheet.write_string(10, 1, currency_label(source.currency_assertion))?;
    worksheet.write_string_with_format(11, 0, "Source bytes", &bold)?;
    worksheet.write_string(
        11,
        1,
        format!(
            "master={} balance={} groups={}",
            source.master_response_bytes,
            source.balance_response_bytes,
            source.group_response_bytes
        ),
    )?;

    let header_row = 13u32;
    for (column, label) in [
        "Ledger / party",
        "Parent",
        "Party GSTIN (as returned)",
        "Income Tax number (as returned)",
        "Name on PAN (as returned)",
        "PIN code (as returned)",
        "GST PIN code (as returned)",
        "MSME registration (as returned)",
        "Udyam registration (as returned)",
        "Bank account holder (as returned)",
        "Bank details (as returned)",
        "IFSC (as returned)",
        "Email (as returned)",
        "Phone (as returned)",
        "State (as returned)",
        "Address (as returned)",
        "GUID",
        "Master ID",
        "Alter ID",
        "Opening balance",
        "Closing balance",
        "Opening balance (exact text)",
        "Closing balance (exact text)",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(header_row, column as u16, label, &bold)?;
    }

    for (offset, row) in source.rows.iter().enumerate() {
        let sheet_row = header_row + 1 + offset as u32;
        worksheet.write_string(sheet_row, 0, &row.name)?;
        worksheet.write_string(sheet_row, 1, row.parent.as_deref().unwrap_or(""))?;
        worksheet.write_string(sheet_row, 2, row.party_gstin.workbook_text())?;
        for (column, value) in [
            row.fields.income_tax_number.workbook_text(),
            row.fields.name_on_pan.workbook_text(),
            row.fields.pin_code.workbook_text(),
            row.fields.gst_pin_code.workbook_text(),
            row.fields.msme_registration_number.workbook_text(),
            row.fields.udyam_registration_number.workbook_text(),
            row.fields.bank_account_holder_name.workbook_text(),
            row.fields.bank_details.workbook_text(),
            row.fields.ifsc_code.workbook_text(),
            row.fields.email.workbook_text(),
            row.fields.phone.workbook_text(),
            row.fields.state.workbook_text(),
            row.fields.address.workbook_text(),
        ]
        .into_iter()
        .enumerate()
        {
            worksheet.write_string(sheet_row, 3 + column as u16, value)?;
        }
        worksheet.write_string(sheet_row, 16, &row.guid)?;
        worksheet.write_string(sheet_row, 17, &row.master_id)?;
        worksheet.write_string(sheet_row, 18, &row.alter_id)?;
        worksheet.write_number_with_format(
            sheet_row,
            19,
            amount_to_f64(row.opening_balance.as_str()).map_err(|_| {
                PartyLedgerMasterXlsxError::InvalidAmount(row.opening_balance.as_str().to_string())
            })?,
            &amount,
        )?;
        if let Some(closing_balance) = row.closing_balance.as_ref() {
            worksheet.write_number_with_format(
                sheet_row,
                20,
                amount_to_f64(closing_balance.as_str()).map_err(|_| {
                    PartyLedgerMasterXlsxError::InvalidAmount(closing_balance.as_str().to_string())
                })?,
                &amount,
            )?;
            worksheet.write_string(sheet_row, 22, closing_balance.as_str())?;
        } else {
            worksheet.write_string(sheet_row, 20, "Not established")?;
            worksheet.write_string(sheet_row, 22, "Not established")?;
        }
        worksheet.write_string(sheet_row, 21, row.opening_balance.as_str())?;
    }
    let last_row = header_row + source.rows.len() as u32;
    worksheet.autofilter(header_row, 0, last_row, 22)?;
    worksheet.set_column_width(0, 30)?;
    worksheet.set_column_width(1, 24)?;
    worksheet.set_column_width(2, 24)?;
    for column in 3..=15 {
        worksheet.set_column_width(column, 24)?;
    }
    worksheet.set_column_width(16, 38)?;
    worksheet.set_column_width(17, 14)?;
    worksheet.set_column_width(18, 14)?;
    worksheet.set_column_width(19, 18)?;
    worksheet.set_column_width(20, 18)?;
    worksheet.set_column_width(21, 22)?;
    worksheet.set_column_width(22, 22)?;
    write_schedule_iii(&mut workbook, workbook_source)?;
    workbook
        .save_to_buffer()
        .map_err(PartyLedgerMasterXlsxError::from)
}

fn write_schedule_iii(
    workbook: &mut Workbook,
    workbook_source: &PartyLedgerMasterWorkbook,
) -> Result<(), PartyLedgerMasterXlsxError> {
    let source = workbook_source.source();
    let view = build_schedule_iii_view(source)?;
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Schedule III trace")?;
    let bold = Format::new().set_bold();
    let amount = Format::new().set_num_format(AMOUNT_NUM_FORMAT);
    worksheet.write_string_with_format(0, 0, "Derived Schedule III view", &bold)?;
    worksheet.write_string(
        0,
        1,
        "Traceable group-derived subtotals only; not a replacement for a CA mapping decision.",
    )?;
    worksheet.write_string_with_format(1, 0, "Currency", &bold)?;
    worksheet.write_string(1, 1, currency_label(source.currency_assertion))?;
    worksheet.write_string_with_format(2, 0, "Read period", &bold)?;
    worksheet.write_string(
        2,
        1,
        format!(
            "Read period: {} to {}. No prior-year values were requested or inferred.",
            source.from.as_str(),
            source.to.as_str()
        ),
    )?;
    for (row, label, value) in [
        (3, "Debit total", view.debit_total.as_str()),
        (4, "Credit total", view.credit_total.as_str()),
        (5, "Dr=Cr difference", view.difference.as_str()),
    ] {
        worksheet.write_string_with_format(row, 0, label, &bold)?;
        worksheet.write_string(row, 1, value)?;
    }
    worksheet.write_string_with_format(6, 0, "Check interpretation", &bold)?;
    worksheet.write_string(6, 1, "Difference 0 is the Tally-sign self-check over every captured ledger closing balance; it is evidence, not an assertion of statement completeness.")?;

    let header_row = 8u32;
    for (column, label) in [
        "Section",
        "Group-derived subtotal",
        "Closing balance",
        "Closing balance (exact text)",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(header_row, column as u16, label, &bold)?;
    }
    let mut row = header_row + 1;
    for line in &view.lines {
        worksheet.write_string(row, 0, line.section)?;
        worksheet.write_string(row, 1, line.label)?;
        worksheet.write_number_with_format(
            row,
            2,
            amount_to_f64(line.total.as_str()).map_err(|_| {
                PartyLedgerMasterXlsxError::InvalidAmount(line.total.as_str().to_string())
            })?,
            &amount,
        )?;
        worksheet.write_string(row, 3, line.total.as_str())?;
        row += 1;
    }

    row += 1;
    worksheet.write_string_with_format(row, 0, "TRACE: every included ledger", &bold)?;
    row += 1;
    for (column, label) in [
        "Subtotal",
        "Ledger",
        "Parent",
        "GUID",
        "Closing balance (exact text)",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(row, column as u16, label, &bold)?;
    }
    row += 1;
    for line in &view.lines {
        for index in &line.row_indices {
            let ledger = &source.rows[*index];
            worksheet.write_string(row, 0, line.label)?;
            worksheet.write_string(row, 1, &ledger.name)?;
            worksheet.write_string(row, 2, ledger.parent.as_deref().unwrap_or(""))?;
            worksheet.write_string(row, 3, &ledger.guid)?;
            worksheet.write_string(
                row,
                4,
                ledger
                    .closing_balance
                    .as_ref()
                    .expect("Schedule III includes only established closing balances")
                    .as_str(),
            )?;
            row += 1;
        }
    }

    row += 1;
    worksheet.write_string_with_format(row, 0, "EXCLUSION LIST (loud)", &bold)?;
    row += 1;
    for (column, label) in [
        "Ledger",
        "Parent",
        "GUID",
        "Why no Schedule III head was emitted",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(row, column as u16, label, &bold)?;
    }
    row += 1;
    for exclusion in &view.exclusions {
        let ledger = &source.rows[exclusion.row_index];
        worksheet.write_string(row, 0, &ledger.name)?;
        worksheet.write_string(row, 1, ledger.parent.as_deref().unwrap_or(""))?;
        worksheet.write_string(row, 2, &ledger.guid)?;
        worksheet.write_string(row, 3, &exclusion.reason)?;
        row += 1;
    }
    worksheet.write_string_with_format(row + 1, 0, "Read did not cover", &bold)?;
    worksheet.write_string(row + 1, 1, "Prior-year balances, voucher-level classification, maturity/current-vs-non-current split, note disclosures, share-capital reconciliation, reserves movement, and CA mapping decisions.")?;
    worksheet.set_column_width(0, 34)?;
    worksheet.set_column_width(1, 46)?;
    worksheet.set_column_width(2, 28)?;
    worksheet.set_column_width(3, 62)?;
    worksheet.set_column_width(4, 24)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use bridge_tally_core::{ExactDecimal, TallyDate};
    use zip::ZipArchive;

    use super::*;
    use crate::reports::party_ledger_master::{
        build_party_ledger_master_workbook, PartyLedgerMasterRow, PartyLedgerMasterSource,
    };
    use bridge_tally_protocol::{PartyLedgerMasterFieldObservation, PartyLedgerMasterFields};

    #[test]
    fn renders_evidence_currency_and_returned_fields_in_the_workbook() {
        let workbook = build_party_ledger_master_workbook(PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![PartyLedgerMasterRow {
                name: "Customer".to_string(),
                parent: Some("Sundry Debtors".to_string()),
                party_gstin: PartyLedgerMasterFieldObservation::Returned(
                    "29ABCDE1234F1Z5".to_string(),
                ),
                fields: PartyLedgerMasterFields {
                    email: PartyLedgerMasterFieldObservation::Returned(
                        "synthetic@example.invalid".to_string(),
                    ),
                    ..PartyLedgerMasterFields::default()
                },
                guid: "ledger-guid".to_string(),
                master_id: "7".to_string(),
                alter_id: "9".to_string(),
                opening_balance: ExactDecimal::parse("-100.00".to_string()).unwrap(),
                closing_balance: Some(ExactDecimal::parse("125.00".to_string()).unwrap()),
            }],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 100,
            balance_response_bytes: 200,
            group_response_bytes: 300,
            groups: vec![],
        })
        .unwrap();
        let bytes = render_party_ledger_master_xlsx(&workbook).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        for name in [
            "xl/worksheets/sheet1.xml",
            "xl/worksheets/sheet2.xml",
            "xl/sharedStrings.xml",
        ] {
            std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut text).unwrap();
        }
        assert!(text.contains("Master response SHA-256"));
        assert!(text.contains("Not observed fields"));
        assert!(!text.contains("Unavailable fields"));
        assert!(text.contains(
            "“Not observed” means this Tally response did not return the requested field; it does not establish whether that field is unset in this book or unavailable in this Tally build. Bridge never manufactures master data."
        ));
        assert!(text.contains("Not observed"));
        assert!(!text.contains("was unset in this book"));
        assert!(text.contains("Income Tax number (as returned)"));
        assert!(text.contains("synthetic@example.invalid"));
        assert!(text.contains("Currency"));
        assert!(text.contains("INR"));
        assert!(text.contains(
            "Read period: 20260401 to 20260731. No prior-year values were requested or inferred."
        ));
        assert!(!text.contains("One year read"));
        assert!(text.contains("EXCLUSION LIST (loud)"));
    }

    #[test]
    fn gstin_not_observed_is_labeled_while_an_explicit_empty_gstin_is_not() {
        let rows = vec![
            PartyLedgerMasterRow {
                name: "GSTIN not observed".to_string(),
                parent: Some("Sundry Debtors".to_string()),
                party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
                fields: PartyLedgerMasterFields::default(),
                guid: "ledger-guid-1".to_string(),
                master_id: "7".to_string(),
                alter_id: "9".to_string(),
                opening_balance: ExactDecimal::parse("-100.00".to_string()).unwrap(),
                closing_balance: Some(ExactDecimal::parse("125.00".to_string()).unwrap()),
            },
            PartyLedgerMasterRow {
                name: "GSTIN returned empty".to_string(),
                parent: Some("Sundry Debtors".to_string()),
                party_gstin: PartyLedgerMasterFieldObservation::Returned(String::new()),
                fields: PartyLedgerMasterFields::default(),
                guid: "ledger-guid-2".to_string(),
                master_id: "8".to_string(),
                alter_id: "10".to_string(),
                opening_balance: ExactDecimal::parse("-200.00".to_string()).unwrap(),
                closing_balance: Some(ExactDecimal::parse("250.00".to_string()).unwrap()),
            },
        ];
        let workbook = build_party_ledger_master_workbook(PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows,
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 100,
            balance_response_bytes: 200,
            group_response_bytes: 300,
            groups: vec![],
        })
        .unwrap();
        let bytes = render_party_ledger_master_xlsx(&workbook).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut sheet = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
            &mut sheet,
        )
        .unwrap();
        let mut shared_strings = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
            &mut shared_strings,
        )
        .unwrap();
        let not_observed_index = shared_strings
            .split("<si>")
            .skip(1)
            .position(|entry| entry.contains("<t>Not observed</t>"))
            .expect("workbook contains the disclosure label");
        let not_observed_cell = format!(r#"r="C15" t="s"><v>{not_observed_index}</v>"#);
        let explicitly_empty_cell = format!(r#"r="C16" t="s"><v>{not_observed_index}</v>"#);

        assert!(
            sheet.contains(&not_observed_cell),
            "an omitted GSTIN must render the observation label"
        );
        assert!(
            !sheet.contains(&explicitly_empty_cell),
            "an explicitly returned empty GSTIN must not be mislabeled as not observed"
        );
    }
}
