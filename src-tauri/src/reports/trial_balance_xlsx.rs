//! Renders a native, ledger-wise book-to-date Trial Balance as `.xlsx`.

use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::trial_balance::TrialBalanceLedger;
use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, Worksheet};

use super::party_statement_xlsx::amount_to_f64 as statement_amount_to_f64;
use super::trial_balance::{TrialBalanceWorkbookSource, TrialBalanceXlsxError};

const AMOUNT_NUM_FORMAT: &str = "##,##,##0.00";

#[derive(Debug, Default)]
struct DirectionalTotals {
    debit: ExactTotal,
    credit: ExactTotal,
}

#[derive(Debug)]
struct ExactTotal(ExactDecimal);

impl Default for ExactTotal {
    fn default() -> Self {
        Self(ExactDecimal::zero())
    }
}

pub fn render_trial_balance_xlsx(
    source: &TrialBalanceWorkbookSource,
) -> Result<Vec<u8>, TrialBalanceXlsxError> {
    let controls = controls(source)?;
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Trial Balance")?;

    let bold = Format::new().set_bold();
    let amount = Format::new().set_num_format(AMOUNT_NUM_FORMAT);
    let bold_amount = Format::new().set_bold().set_num_format(AMOUNT_NUM_FORMAT);
    let date = Format::new().set_num_format("dd-mmm-yyyy");

    let mut row = 0_u32;
    worksheet.write_string(row, 0, "Company")?;
    worksheet.write_string(row, 1, &source.company)?;
    row += 1;
    worksheet.write_string(row, 0, "Period")?;
    worksheet.write_datetime_with_format(row, 1, excel_date(&source.from_yyyymmdd)?, &date)?;
    worksheet.write_string(row, 2, "to")?;
    worksheet.write_datetime_with_format(row, 3, excel_date(&source.to_yyyymmdd)?, &date)?;
    row += 1;
    worksheet.write_string(row, 0, "Source")?;
    worksheet.write_string(
        row,
        1,
        format!(
            "{} native ledger rows · {} response bytes",
            source.trial_balance.rows.len(),
            source.source_bytes
        ),
    )?;
    row += 1;
    worksheet.write_string(row, 0, "Currency")?;
    worksheet.write_string(
        row,
        1,
        "As reported by Tally; Bridge does not assert a currency for this export.",
    )?;
    row += 1;
    worksheet.write_string(row, 0, "Control")?;
    worksheet.write_string(
        row,
        1,
        "Opening, net change, and closing balances each reconcile exactly.",
    )?;
    row += 1;
    worksheet.write_string(
        row,
        0,
        "Net change is closing minus opening; it is not gross debit/credit voucher turnover.",
    )?;
    row += 2;

    let header_row = row;
    for (column, label) in [
        "Ledger",
        "Group",
        "Opening Dr",
        "Opening Cr",
        "Net change Dr",
        "Net change Cr",
        "Closing Dr",
        "Closing Cr",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(row, column as u16, label, &bold)?;
    }
    row += 1;

    for ledger in &source.trial_balance.rows {
        write_ledger_row(worksheet, row, ledger, &amount)?;
        row += 1;
    }

    worksheet.write_string_with_format(row, 0, "Grand Total", &bold)?;
    write_total_pair(worksheet, row, 2, &controls.opening, &bold_amount)?;
    write_total_pair(worksheet, row, 4, &controls.movement, &bold_amount)?;
    write_total_pair(worksheet, row, 6, &controls.closing, &bold_amount)?;

    worksheet.set_freeze_panes(header_row + 1, 0)?;
    worksheet.set_column_width(0, 34)?;
    worksheet.set_column_width(1, 28)?;
    for column in 2..=7 {
        worksheet.set_column_width(column, 16)?;
    }
    workbook
        .save_to_buffer()
        .map_err(TrialBalanceXlsxError::from)
}

struct TrialBalanceControls {
    opening: DirectionalTotals,
    movement: DirectionalTotals,
    closing: DirectionalTotals,
}

fn controls(
    source: &TrialBalanceWorkbookSource,
) -> Result<TrialBalanceControls, TrialBalanceXlsxError> {
    let mut opening = DirectionalTotals::default();
    let mut movement = DirectionalTotals::default();
    let mut closing = DirectionalTotals::default();
    for ledger in &source.trial_balance.rows {
        opening.add(&ledger.opening)?;
        closing.add(&ledger.closing)?;
        movement.add(&subtract(&ledger.closing, &ledger.opening)?)?;
    }
    if !opening.balances()? || !movement.balances()? || !closing.balances()? {
        return Err(TrialBalanceXlsxError::ControlMismatch);
    }
    Ok(TrialBalanceControls {
        opening,
        movement,
        closing,
    })
}

impl DirectionalTotals {
    fn add(&mut self, value: &ExactDecimal) -> Result<(), TrialBalanceXlsxError> {
        if value.is_negative() {
            self.debit.add(
                &value
                    .abs()
                    .map_err(|_| TrialBalanceXlsxError::ControlMismatch)?,
            )
        } else {
            self.credit.add(value)
        }
    }

    fn balances(&self) -> Result<bool, TrialBalanceXlsxError> {
        Ok(subtract(&self.debit.0, &self.credit.0)?.is_zero())
    }
}

impl ExactTotal {
    fn add(&mut self, value: &ExactDecimal) -> Result<(), TrialBalanceXlsxError> {
        self.0 = self
            .0
            .checked_add(value)
            .map_err(|_| TrialBalanceXlsxError::ControlMismatch)?;
        Ok(())
    }
}

fn write_ledger_row(
    worksheet: &mut Worksheet,
    row: u32,
    ledger: &TrialBalanceLedger,
    format: &Format,
) -> Result<(), TrialBalanceXlsxError> {
    worksheet.write_string(row, 0, &ledger.name)?;
    worksheet.write_string(row, 1, ledger.parent.as_deref().unwrap_or(""))?;
    write_directional(worksheet, row, 2, &ledger.opening, format)?;
    write_directional(
        worksheet,
        row,
        4,
        &subtract(&ledger.closing, &ledger.opening)?,
        format,
    )?;
    write_directional(worksheet, row, 6, &ledger.closing, format)?;
    Ok(())
}

fn write_directional(
    worksheet: &mut Worksheet,
    row: u32,
    debit_column: u16,
    value: &ExactDecimal,
    format: &Format,
) -> Result<(), TrialBalanceXlsxError> {
    if value.is_zero() {
        return Ok(());
    }
    let column = if value.is_negative() {
        debit_column
    } else {
        debit_column + 1
    };
    let magnitude = value
        .abs()
        .map_err(|_| TrialBalanceXlsxError::ControlMismatch)?;
    worksheet.write_number_with_format(row, column, amount_to_f64(&magnitude)?, format)?;
    Ok(())
}

fn write_total_pair(
    worksheet: &mut Worksheet,
    row: u32,
    debit_column: u16,
    totals: &DirectionalTotals,
    format: &Format,
) -> Result<(), TrialBalanceXlsxError> {
    worksheet.write_number_with_format(
        row,
        debit_column,
        amount_to_f64(&totals.debit.0)?,
        format,
    )?;
    worksheet.write_number_with_format(
        row,
        debit_column + 1,
        amount_to_f64(&totals.credit.0)?,
        format,
    )?;
    Ok(())
}

fn subtract(
    left: &ExactDecimal,
    right: &ExactDecimal,
) -> Result<ExactDecimal, TrialBalanceXlsxError> {
    left.checked_subtract(right)
        .map_err(|_| TrialBalanceXlsxError::ControlMismatch)
}

fn amount_to_f64(value: &ExactDecimal) -> Result<f64, TrialBalanceXlsxError> {
    statement_amount_to_f64(value.as_str())
        .map_err(|_| TrialBalanceXlsxError::InvalidAmount(value.as_str().to_string()))
}

fn excel_date(value: &str) -> Result<ExcelDateTime, TrialBalanceXlsxError> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TrialBalanceXlsxError::InvalidDate(value.to_string()));
    }
    let year = value[0..4]
        .parse::<u16>()
        .map_err(|_| TrialBalanceXlsxError::InvalidDate(value.to_string()))?;
    let month = value[4..6]
        .parse::<u8>()
        .map_err(|_| TrialBalanceXlsxError::InvalidDate(value.to_string()))?;
    let day = value[6..8]
        .parse::<u8>()
        .map_err(|_| TrialBalanceXlsxError::InvalidDate(value.to_string()))?;
    ExcelDateTime::from_ymd(year, month, day)
        .map_err(|_| TrialBalanceXlsxError::InvalidDate(value.to_string()))
}
