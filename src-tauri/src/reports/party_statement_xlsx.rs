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
use crate::tally::ExposureDirection;

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
    #[error("Bridge could not classify a statement bill direction ({0})")]
    InvalidDirection(String),
    #[error("Bridge found an inconsistent statement age state")]
    InvalidAgeState,
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
        "Direction",
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
        worksheet.write_string(row, 3, bill_direction_label(bill.kind)?)?;
        worksheet.write_number_with_format(
            row,
            4,
            amount_to_f64(bill.amount.as_str())?,
            &amount_format,
        )?;
        match (bill.age_days, bill.bucket) {
            (Some(age_days), Some(bucket)) => {
                worksheet.write_number(row, 5, age_days)?;
                worksheet.write_string(row, 6, bucket.label())?;
            }
            (None, None) => {
                worksheet.write_string(row, 5, "Not due")?;
                worksheet.write_string(row, 6, "Unaged")?;
            }
            _ => return Err(PartyStatementXlsxError::InvalidAgeState),
        }
        row += 1;
    }

    worksheet.write_string_with_format(row, 0, "Total bill magnitudes (not net)", &bold)?;
    worksheet.write_number_with_format(
        row,
        4,
        amount_to_f64(statement.bill_total.as_str())?,
        &bold_amount_format,
    )?;
    row += 1;

    if has_unallocated {
        let direction =
            statement
                .unallocated_direction
                .ok_or(PartyStatementXlsxError::InvalidDirection(
                    "unallocated direction missing".to_string(),
                ))?;
        worksheet.write_string(
            row,
            0,
            format!(
                "Unallocated {} (no bill reference)",
                exposure_direction_label(direction)
            ),
        )?;
        worksheet.write_number_with_format(
            row,
            4,
            amount_to_f64(statement.unallocated.as_str())?,
            &amount_format,
        )?;
        row += 1;

        worksheet.write_string_with_format(row, 0, "Grand total", &bold)?;
        worksheet.write_number_with_format(
            row,
            4,
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
    worksheet.set_column_width(3, 27)?;
    worksheet.set_column_width(4, 16)?;
    worksheet.set_column_width(5, 11)?;
    worksheet.set_column_width(6, 13)?;

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
    if !value.is_finite() || !same_decimal_value(text, &value.to_string()) {
        return Err(PartyStatementXlsxError::InvalidAmount(text.to_string()));
    }
    Ok(value)
}

fn bill_direction_label(kind: &str) -> Result<&'static str, PartyStatementXlsxError> {
    match kind {
        "receivable" => Ok("Receivable"),
        "payable" => Ok("Payable"),
        _ => Err(PartyStatementXlsxError::InvalidDirection(kind.to_string())),
    }
}

fn exposure_direction_label(direction: ExposureDirection) -> &'static str {
    match direction {
        ExposureDirection::Receivable => "Receivable",
        ExposureDirection::Payable => "Payable",
    }
}

/// `f64::to_string` emits the shortest decimal that round-trips to the binary
/// value. Comparing numeric decimal forms (rather than their spellings) keeps
/// harmless source scale such as `42.00`, while rejecting a value whose Excel
/// number cell would change the amount.
fn same_decimal_value(left: &str, right: &str) -> bool {
    canonical_decimal_value(left) == canonical_decimal_value(right)
}

fn canonical_decimal_value(value: &str) -> Option<String> {
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |unsigned| (true, unsigned));
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole = whole.trim_start_matches('0');
    let fraction = fraction.trim_end_matches('0');
    if whole.is_empty() && fraction.is_empty() {
        return Some("0".to_string());
    }
    let mut canonical = String::with_capacity(value.len());
    if negative {
        canonical.push('-');
    }
    canonical.push_str(if whole.is_empty() { "0" } else { whole });
    if !fraction.is_empty() {
        canonical.push('.');
        canonical.push_str(fraction);
    }
    Some(canonical)
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

    #[test]
    fn rejects_a_valid_decimal_that_excel_cannot_represent_exactly() {
        assert!(matches!(
            amount_to_f64("9007199254740993"),
            Err(PartyStatementXlsxError::InvalidAmount(value)) if value == "9007199254740993"
        ));

        assert_eq!(
            amount_to_f64("9007199254740992").unwrap(),
            9007199254740992.0
        );
        assert_eq!(amount_to_f64("42.00").unwrap(), 42.0);
    }

    #[test]
    fn bill_direction_labels_make_mixed_party_amounts_unambiguous() {
        assert_eq!(bill_direction_label("receivable").unwrap(), "Receivable");
        assert_eq!(bill_direction_label("payable").unwrap(), "Payable");
        assert!(matches!(
            bill_direction_label("unknown"),
            Err(PartyStatementXlsxError::InvalidDirection(_))
        ));
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
            kind: "receivable",
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
            build_party_statement("Lab Co", "20260808", "On Account Only", &[], &unallocated)
                .unwrap();
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
            build_party_statement("Lab Co", "20260808", "On Account Only", &[], &unallocated)
                .unwrap();
        let bytes = render_party_statement_xlsx(&statement).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        for name in ["xl/worksheets/sheet1.xml", "xl/sharedStrings.xml"] {
            let mut entry = archive.by_name(name).unwrap();
            std::io::Read::read_to_string(&mut entry, &mut text).unwrap();
        }
        assert!(text.contains("Unallocated Payable (no bill reference)"));
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
