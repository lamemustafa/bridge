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
    worksheet.write_string(row, 0, "Ageing basis")?;
    worksheet.write_string(row, 1, statement.ageing_anchor.label())?;
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
        worksheet.write_string(row, 3, bill_direction_label(bill.kind))?;
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

    worksheet.write_string_with_format(
        row,
        0,
        "Ageing subtotals by direction (magnitudes; not net)",
        &bold,
    )?;
    row += 1;
    for (direction, subtotals) in statement.subtotals.by_direction() {
        for (bucket, subtotal) in [
            ("Not yet due", &subtotals.not_yet_due),
            ("0-30 days", &subtotals.days_0_30),
            ("31-60 days", &subtotals.days_31_60),
            ("61-90 days", &subtotals.days_61_90),
            ("90+ days", &subtotals.days_90_plus),
        ] {
            worksheet.write_string(row, 0, format!("{direction} | {bucket}"))?;
            worksheet.write_number_with_format(
                row,
                4,
                amount_to_f64(subtotal.as_str())?,
                &amount_format,
            )?;
            row += 1;
        }
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
                "Unallocated {} magnitude (no bill reference)",
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

        worksheet.write_string_with_format(row, 0, "Grand total magnitudes (not net)", &bold)?;
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
pub(super) fn amount_to_f64(text: &str) -> Result<f64, PartyStatementXlsxError> {
    let canonical = canonical_decimal_value(text)
        .ok_or_else(|| PartyStatementXlsxError::InvalidAmount(text.to_string()))?;
    let value = text
        .parse::<f64>()
        .map_err(|_| PartyStatementXlsxError::InvalidAmount(text.to_string()))?;
    if canonical.significand.len() > 15
        || !value.is_finite()
        || canonical_f64_decimal_value(&value.to_string()).as_ref() != Some(&canonical)
    {
        return Err(PartyStatementXlsxError::InvalidAmount(text.to_string()));
    }
    Ok(value)
}

fn bill_direction_label(direction: ExposureDirection) -> &'static str {
    exposure_direction_label(direction)
}

fn exposure_direction_label(direction: ExposureDirection) -> &'static str {
    match direction {
        ExposureDirection::Receivable => "Receivable",
        ExposureDirection::Payable => "Payable",
    }
}

/// A decimal normalized as a nonzero significand and base-10 exponent. This
/// removes insignificant zeroes on both sides of the decimal point: trailing
/// integer zeroes shift the exponent, so `1000000000000000` has one meaningful
/// digit, while `9007199254740992` has sixteen.
#[derive(Debug, PartialEq, Eq)]
struct CanonicalDecimalValue {
    negative: bool,
    significand: String,
    exponent: i32,
}

/// `f64::to_string` emits the shortest decimal that round-trips to the binary
/// value, sometimes using scientific notation. Canonical decimal form compares
/// its mathematical value rather than its display spelling, so harmless source
/// scale and powers of ten stay renderable while changed amounts fail closed.
fn canonical_decimal_value(value: &str) -> Option<CanonicalDecimalValue> {
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
    let digits = format!("{whole}{fraction}");
    let first_nonzero = digits.bytes().position(|byte| byte != b'0');
    let Some(first_nonzero) = first_nonzero else {
        return Some(CanonicalDecimalValue {
            negative: false,
            significand: "0".to_string(),
            exponent: 0,
        });
    };
    let last_nonzero = digits.bytes().rposition(|byte| byte != b'0')?;
    let significand = digits[first_nonzero..=last_nonzero].to_string();
    let exponent = i32::try_from(whole.len())
        .ok()?
        .checked_sub(i32::try_from(first_nonzero).ok()?)?
        .checked_sub(i32::try_from(significand.len()).ok()?)?;
    Some(CanonicalDecimalValue {
        negative,
        significand,
        exponent,
    })
}

fn canonical_f64_decimal_value(value: &str) -> Option<CanonicalDecimalValue> {
    let (significand, scientific_exponent) = match value.split_once(['e', 'E']) {
        Some((significand, exponent)) if !exponent.is_empty() => {
            (significand, exponent.parse::<i32>().ok()?)
        }
        Some(_) => return None,
        None => (value, 0),
    };
    let mut canonical = canonical_decimal_value(significand)?;
    canonical.exponent = canonical.exponent.checked_add(scientific_exponent)?;
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
#[path = "party_statement_xlsx_tests.rs"]
mod tests;
