//! Renders a [`PartyStatement`] as an `.xlsx` workbook.
//!
//! Amounts are written with [`Worksheet::write_number_with_format`], never as
//! text: a rupee figure landing in a cell as a string breaks every downstream
//! `SUM`, which is the single most common way an accounting export turns out
//! to be useless to the person who received it. Bill and due dates are real
//! date cells for the same reason -- a CA sorting or filtering the sheet by
//! date needs Excel's own date type, not a string that merely looks like one.

use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, XlsxError};

use super::party_statement::PartyStatement;

/// Indian-grouping number format (lakh/crore, not thousands) -- the grouping
/// every figure on the Outstandings screen already uses.
const AMOUNT_NUM_FORMAT: &str = "##,##,##0.00";
const DATE_NUM_FORMAT: &str = "dd-mmm-yyyy";

#[derive(Debug, thiserror::Error)]
pub enum PartyStatementXlsxError {
    #[error("Bridge could not build the statement workbook: {0}")]
    Workbook(#[from] XlsxError),
    #[error("Bridge could not read a statement date for the spreadsheet ({0})")]
    InvalidDate(String),
    #[error("Bridge could not represent an amount in the spreadsheet ({0})")]
    InvalidAmount(String),
}

/// Renders `statement` as an in-memory `.xlsx` file.
pub fn render_party_statement_xlsx(
    statement: &PartyStatement,
) -> Result<Vec<u8>, PartyStatementXlsxError> {
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Statement")?;

    let bold = Format::new().set_bold();
    let amount_format = Format::new().set_num_format(AMOUNT_NUM_FORMAT);
    let bold_amount_format = Format::new().set_bold().set_num_format(AMOUNT_NUM_FORMAT);
    let date_format = Format::new().set_num_format(DATE_NUM_FORMAT);

    let mut row = 0u32;
    worksheet.write_string(row, 0, "Company")?;
    worksheet.write_string(row, 1, statement.company.as_str())?;
    row += 1;
    worksheet.write_string(row, 0, "Party")?;
    worksheet.write_string(row, 1, statement.party.as_str())?;
    row += 1;
    worksheet.write_string(row, 0, "As of")?;
    worksheet.write_datetime_with_format(
        row,
        1,
        excel_date(&statement.as_of_yyyymmdd)?,
        &date_format,
    )?;
    row += 1;

    let has_unallocated = !statement.unallocated.is_zero();
    if has_unallocated {
        worksheet.write_string(
            row,
            0,
            "Also carries exposure with no bill reference -- shown separately below, not aged.",
        )?;
        row += 1;
    }

    row += 1; // Blank row before the bill table.
    let header_row = row;
    for (col, label) in [
        "Reference",
        "Bill date",
        "Due date",
        "Amount",
        "Age (days)",
        "Bucket",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(header_row, col as u16, label, &bold)?;
    }
    row += 1;

    for bill in &statement.bills {
        worksheet.write_string(row, 0, bill.reference.as_str())?;
        worksheet.write_datetime_with_format(row, 1, excel_date(&bill.bill_date)?, &date_format)?;
        worksheet.write_datetime_with_format(row, 2, excel_date(&bill.due_date)?, &date_format)?;
        worksheet.write_number_with_format(
            row,
            3,
            amount_to_f64(bill.amount.as_str())?,
            &amount_format,
        )?;
        worksheet.write_number(row, 4, bill.age_days)?;
        worksheet.write_string(row, 5, bill.bucket.label())?;
        row += 1;
    }

    worksheet.write_string_with_format(row, 0, "Total bills", &bold)?;
    worksheet.write_number_with_format(
        row,
        3,
        amount_to_f64(statement.bill_total.as_str())?,
        &bold_amount_format,
    )?;
    row += 1;

    if has_unallocated {
        worksheet.write_string(row, 0, "Unallocated (no bill reference)")?;
        worksheet.write_number_with_format(
            row,
            3,
            amount_to_f64(statement.unallocated.as_str())?,
            &amount_format,
        )?;
        row += 1;

        worksheet.write_string_with_format(row, 0, "Grand total", &bold)?;
        worksheet.write_number_with_format(
            row,
            3,
            amount_to_f64(statement.grand_total.as_str())?,
            &bold_amount_format,
        )?;
    }

    // Freeze the column-header row so it stays visible once the bill table
    // scrolls past the header block above it.
    worksheet.set_freeze_panes(header_row + 1, 0)?;

    worksheet.set_column_width(0, 30)?;
    worksheet.set_column_width(1, 13)?;
    worksheet.set_column_width(2, 13)?;
    worksheet.set_column_width(3, 16)?;
    worksheet.set_column_width(4, 11)?;
    worksheet.set_column_width(5, 13)?;

    workbook
        .save_to_buffer()
        .map_err(PartyStatementXlsxError::from)
}

/// `ExactDecimal` is validated to be plain-decimal ASCII digits with an
/// optional sign and fractional part, so this parse cannot fail on any value
/// that reached this module -- but the fallback keeps the conversion honest
/// rather than assuming it. Display-only: nothing here feeds back into
/// Bridge's own arithmetic, which stays on `ExactDecimal` throughout.
/// Converts an exact amount to the IEEE-754 value Excel's number cell requires.
///
/// **This must fail rather than substitute.** An earlier revision returned
/// `unwrap_or(0.0)`, which would have written a real bill as zero into a
/// statement sent to a client and silently understated the total -- a wrong
/// number presented as a right one, which is the failure mode this codebase
/// exists to prevent. A statement that cannot be rendered exactly must not be
/// rendered at all.
///
/// Bridge's own arithmetic never touches `f64`; this is the last step before
/// the cell, and Excel has no exact-decimal cell type to target instead.
fn amount_to_f64(text: &str) -> Result<f64, PartyStatementXlsxError> {
    let value = text
        .parse::<f64>()
        .map_err(|_| PartyStatementXlsxError::InvalidAmount(text.to_string()))?;
    if !value.is_finite() {
        return Err(PartyStatementXlsxError::InvalidAmount(text.to_string()));
    }
    Ok(value)
}

fn excel_date(yyyymmdd: &str) -> Result<ExcelDateTime, PartyStatementXlsxError> {
    let invalid = || PartyStatementXlsxError::InvalidDate(yyyymmdd.to_string());
    if yyyymmdd.len() != 8 || !yyyymmdd.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let year = yyyymmdd[0..4].parse::<u16>().map_err(|_| invalid())?;
    let month = yyyymmdd[4..6].parse::<u8>().map_err(|_| invalid())?;
    let day = yyyymmdd[6..8].parse::<u8>().map_err(|_| invalid())?;
    ExcelDateTime::from_ymd(year, month, day).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {

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
    use super::*;
    use crate::reports::party_statement::build_party_statement;
    use crate::tally::{OpenBillRow, UnallocatedParty};
    use bridge_tally_core::ExactDecimal;

    fn bill(reference: &str, amount: &str, age_days: u32) -> OpenBillRow {
        OpenBillRow {
            party: "Aarav Textiles".to_string(),
            reference: reference.to_string(),
            bill_date: "20260101".to_string(),
            due_date: "20260201".to_string(),
            amount: ExactDecimal::parse(amount).unwrap(),
            age_days,
            kind: "receivable",
        }
    }

    #[test]
    fn renders_a_non_empty_workbook_for_a_billed_and_unallocated_party() {
        let bills = vec![bill("INV-1", "1250.75", 40)];
        let unallocated = vec![UnallocatedParty {
            party: "Aarav Textiles".to_string(),
            amount: ExactDecimal::parse("300.00").unwrap(),
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
        }];
        let statement =
            build_party_statement("Lab Co", "20260808", "On Account Only", &[], &unallocated)
                .unwrap();
        let bytes = render_party_statement_xlsx(&statement).unwrap();
        assert!(bytes.len() > 200);
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
}
