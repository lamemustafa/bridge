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
    let wrapped = Format::new().set_text_wrap();

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
        sheet.write_string_with_format(row, 1, value, &wrapped)?;
        // Long source commitments must remain readable without bleeding into
        // adjacent cells in the exported workbook.
        if matches!(label, "Source request SHA-256" | "Source response SHA-256") {
            sheet.set_row_height(row, 30)?;
        }
        row += 1;
    }
    sheet.write_string(row, 0, "Limitation")?;
    sheet.write_string_with_format(row, 1, "Observed native Trial Balance fields only. Opening, debit, credit and closing retain Tally's signed values. Negative opening/closing is Dr, positive is Cr; the desktop displays debit/credit magnitudes. Paired source stability does not establish an atomic Tally snapshot.", &wrapped)?;
    // This 271-character qualification requires six lines at the committed
    // column width, so preserve a full six-line row rather than Excel's
    // default clipped height.
    sheet.set_row_height(row, 90)?;
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
            sheet.write_string_with_format(
                row,
                column,
                format!(
                    "Observed numeric total: {}; empty fields: {}",
                    total.sum.as_str(),
                    total.empty_count
                ),
                &wrapped,
            )?;
        }
    }
    // Qualification text in the totals row is meaningful evidence, not a
    // decorative footer; give wrapped cells enough vertical space to show it.
    if [
        read.totals.opening.empty_count,
        read.totals.debit.empty_count,
        read.totals.credit.empty_count,
        read.totals.closing.empty_count,
    ]
    .into_iter()
    .any(|count| count > 0)
    {
        sheet.set_row_height(row, 45)?;
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
    sheet.set_column_width(1, 48)?;
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
#[path = "trial_balance_xlsx_tests.rs"]
mod tests;
