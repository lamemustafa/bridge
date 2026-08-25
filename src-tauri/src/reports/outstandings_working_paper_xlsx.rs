//! Renders the exact, all-party outstandings working paper as an `.xlsx`.

use rust_xlsxwriter::{ExcelDateTime, Format, Workbook, XlsxError};

use super::outstandings_working_paper::{
    AgeingBucketControls, DualAgeBillRow, OutstandingsWorkingPaper, PartyWorkingPaperRow,
};
use super::party_statement_xlsx::amount_to_f64 as statement_amount_to_f64;
use crate::tally::OutstandingsCurrencyAssertion;

const AMOUNT_NUM_FORMAT: &str = "##,##,##0.00";
const DATE_NUM_FORMAT: &str = "dd-mmm-yyyy";
const EXCEL_MAX_ROWS: usize = 1_048_576;
const SUMMARY_FIXED_ROWS: usize = 24;
const BILLS_FIXED_ROWS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum OutstandingsWorkingPaperXlsxError {
    #[error("Bridge could not build the working-paper workbook: {0}")]
    Workbook(#[from] XlsxError),
    #[error("Bridge could not represent a working-paper amount in Excel ({0})")]
    InvalidAmount(String),
    #[error("Bridge could not represent a working-paper date in Excel ({0})")]
    InvalidDate(String),
    #[error("Bridge could not represent the working-paper source timestamp")]
    InvalidTimestamp,
    #[error("the working paper exceeds Excel's row limit")]
    RowLimit,
}

pub fn render_outstandings_working_paper_xlsx(
    paper: &OutstandingsWorkingPaper,
) -> Result<Vec<u8>, OutstandingsWorkingPaperXlsxError> {
    if paper.parties().len().saturating_add(SUMMARY_FIXED_ROWS) > EXCEL_MAX_ROWS
        || paper.bills().len().saturating_add(BILLS_FIXED_ROWS) > EXCEL_MAX_ROWS
    {
        return Err(OutstandingsWorkingPaperXlsxError::RowLimit);
    }

    let mut workbook = Workbook::new();
    render_summary(&mut workbook, paper)?;
    render_bills(&mut workbook, paper)?;
    workbook
        .save_to_buffer()
        .map_err(OutstandingsWorkingPaperXlsxError::from)
}

fn render_summary(
    workbook: &mut Workbook,
    paper: &OutstandingsWorkingPaper,
) -> Result<(), OutstandingsWorkingPaperXlsxError> {
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Summary")?;
    let bold = Format::new().set_bold();
    let amount = Format::new().set_num_format(AMOUNT_NUM_FORMAT);
    let bold_amount = Format::new().set_bold().set_num_format(AMOUNT_NUM_FORMAT);
    let date = Format::new().set_num_format(DATE_NUM_FORMAT);

    worksheet.write_string(0, 0, "Company")?;
    worksheet.write_string(0, 1, paper.company())?;
    worksheet.write_string(1, 0, "Company GUID")?;
    worksheet.write_string(1, 1, paper.company_guid())?;
    worksheet.write_string(2, 0, "As of")?;
    worksheet.write_datetime_with_format(2, 1, excel_date(paper.as_of().as_str())?, &date)?;
    worksheet.write_string(3, 0, "Currency assertion")?;
    worksheet.write_string(3, 1, currency_label(paper.currency_assertion()))?;
    worksheet.write_string(4, 0, "Source read completed (UTC)")?;
    let synced_at = chrono::DateTime::from_timestamp_millis(paper.synced_at_unix_ms())
        .ok_or(OutstandingsWorkingPaperXlsxError::InvalidTimestamp)?;
    worksheet.write_string(4, 1, synced_at.to_rfc3339())?;
    worksheet.write_string(5, 0, "Source bytes")?;
    worksheet.write_string(5, 1, paper.source_bytes().to_string())?;
    worksheet.write_string(6, 0, "Native bill rows")?;
    worksheet.write_string(6, 1, paper.bills().len().to_string())?;
    worksheet.write_string(7, 0, "Source dashboard ageing basis")?;
    worksheet.write_string(7, 1, paper.source_ageing_anchor().label())?;
    worksheet.write_string(8, 0, "Ageing scope")?;
    worksheet.write_string(
        8,
        1,
        "Bill and due ages are both derived from the same completed read. Unallocated exposure is not ageable.",
    )?;

    worksheet.write_string(9, 0, "Dashboard tie-out")?;
    worksheet.write_string(
        9,
        1,
        "The source dashboard includes date-not-reached bills in 0-30. For a dashboard-equivalent 0-30 figure, add the two separate working-paper columns.",
    )?;
    worksheet.write_string(10, 0, "Amount representation")?;
    worksheet.write_string(
        10,
        1,
        "Numeric cells are bounded Excel projections for calculation. Exact source decimals are preserved in the Exact amounts (text) columns.",
    )?;

    let header_row = 12u32;
    for (column, label) in [
        "Party",
        "Receivable bills",
        "Receivable unallocated",
        "Receivable total",
        "Payable bills",
        "Payable unallocated",
        "Payable total",
        "Outstanding magnitude",
        "Oldest bill-date age",
        "Oldest due-date age",
        "Exact amounts (text)",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(header_row, column as u16, label, &bold)?;
    }

    let mut row = header_row + 1;
    for party in paper.parties() {
        write_party_row(worksheet, row, party, &amount)?;
        row += 1;
    }
    let filter_end = row.saturating_sub(1).max(header_row);
    worksheet.write_string_with_format(row, 0, "CONTROL TOTALS", &bold)?;
    let controls = paper.controls();
    for (column, value) in [
        (1, &controls.receivable_bills),
        (2, &controls.receivable_unallocated),
        (3, &controls.receivable_total),
        (4, &controls.payable_bills),
        (5, &controls.payable_unallocated),
        (6, &controls.payable_total),
        (7, &controls.outstanding_total),
    ] {
        worksheet.write_number_with_format(
            row,
            column,
            amount_to_f64(value.as_str())?,
            &bold_amount,
        )?;
    }
    worksheet.write_string(row, 10, exact_control_text(controls))?;

    row += 2;
    row = write_ageing_controls(
        worksheet,
        row,
        "Bill-date ageing controls",
        &controls.bill_date_ageing.receivable,
        &controls.bill_date_ageing.payable,
        &controls.receivable_bills,
        &controls.payable_bills,
        &bold,
        &amount,
        &bold_amount,
    )?;
    row += 1;
    let _ = write_ageing_controls(
        worksheet,
        row,
        "Due-date ageing controls",
        &controls.due_date_ageing.receivable,
        &controls.due_date_ageing.payable,
        &controls.receivable_bills,
        &controls.payable_bills,
        &bold,
        &amount,
        &bold_amount,
    )?;

    worksheet.set_freeze_panes(header_row + 1, 1)?;
    worksheet.autofilter(header_row, 0, filter_end, 10)?;
    worksheet.set_column_width(0, 32)?;
    for column in 1..=7 {
        worksheet.set_column_width(column, 18)?;
    }
    worksheet.set_column_width(8, 20)?;
    worksheet.set_column_width(9, 20)?;
    worksheet.set_column_width(10, 78)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_ageing_controls(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    mut row: u32,
    title: &str,
    receivable: &AgeingBucketControls,
    payable: &AgeingBucketControls,
    receivable_total: &bridge_tally_core::ExactDecimal,
    payable_total: &bridge_tally_core::ExactDecimal,
    bold: &Format,
    amount: &Format,
    bold_amount: &Format,
) -> Result<u32, OutstandingsWorkingPaperXlsxError> {
    worksheet.write_string_with_format(row, 0, title, bold)?;
    row += 1;
    for (column, label) in [
        "Direction",
        "Date not reached",
        "0-30 days",
        "31-60 days",
        "61-90 days",
        "90+ days",
        "Total",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(row, column as u16, label, bold)?;
    }
    worksheet.write_string_with_format(row, 10, "Exact buckets (text)", bold)?;
    for (direction, controls, total) in [
        ("Receivable", receivable, receivable_total),
        ("Payable", payable, payable_total),
    ] {
        row += 1;
        worksheet.write_string(row, 0, direction)?;
        for (column, value) in [
            (1, &controls.date_not_reached),
            (2, &controls.days_0_30),
            (3, &controls.days_31_60),
            (4, &controls.days_61_90),
            (5, &controls.days_90_plus),
        ] {
            worksheet.write_number_with_format(
                row,
                column,
                amount_to_f64(value.as_str())?,
                amount,
            )?;
        }
        worksheet.write_number_with_format(row, 6, amount_to_f64(total.as_str())?, bold_amount)?;
        worksheet.write_string(
            row,
            10,
            format!(
                "not reached={}; 0-30={}; 31-60={}; 61-90={}; 90+={}; total={}",
                controls.date_not_reached.as_str(),
                controls.days_0_30.as_str(),
                controls.days_31_60.as_str(),
                controls.days_61_90.as_str(),
                controls.days_90_plus.as_str(),
                total.as_str(),
            ),
        )?;
    }
    Ok(row)
}

fn write_party_row(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    party: &PartyWorkingPaperRow,
    amount_format: &Format,
) -> Result<(), OutstandingsWorkingPaperXlsxError> {
    worksheet.write_string(row, 0, &party.party)?;
    for (column, value) in [
        (1, &party.receivable_bills),
        (2, &party.receivable_unallocated),
        (3, &party.receivable_total),
        (4, &party.payable_bills),
        (5, &party.payable_unallocated),
        (6, &party.payable_total),
        (7, &party.outstanding_total),
    ] {
        worksheet.write_number_with_format(
            row,
            column,
            amount_to_f64(value.as_str())?,
            amount_format,
        )?;
    }
    write_age(worksheet, row, 8, party.oldest_bill_age_days)?;
    write_age(worksheet, row, 9, party.oldest_due_age_days)?;
    worksheet.write_string(
        row,
        10,
        format!(
            "R bills={}; R unallocated={}; R total={}; P bills={}; P unallocated={}; P total={}; magnitude={}",
            party.receivable_bills.as_str(),
            party.receivable_unallocated.as_str(),
            party.receivable_total.as_str(),
            party.payable_bills.as_str(),
            party.payable_unallocated.as_str(),
            party.payable_total.as_str(),
            party.outstanding_total.as_str(),
        ),
    )?;
    Ok(())
}

fn render_bills(
    workbook: &mut Workbook,
    paper: &OutstandingsWorkingPaper,
) -> Result<(), OutstandingsWorkingPaperXlsxError> {
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Bills")?;
    let bold = Format::new().set_bold();
    let amount = Format::new().set_num_format(AMOUNT_NUM_FORMAT);
    let date = Format::new().set_num_format(DATE_NUM_FORMAT);
    let header_row = 0u32;
    for (column, label) in [
        "Party",
        "Reference",
        "Direction",
        "Amount",
        "Bill date",
        "Due date",
        "Bill-date age",
        "Bill-date bucket",
        "Due-date age",
        "Due-date bucket",
        "Exact amount (text)",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(header_row, column as u16, label, &bold)?;
    }

    let mut row = 1u32;
    for bill in paper.bills() {
        write_bill_row(worksheet, row, bill, &amount, &date)?;
        row += 1;
    }
    let filter_end = row.saturating_sub(1).max(header_row);
    worksheet.autofilter(header_row, 0, filter_end, 10)?;
    worksheet.set_freeze_panes(1, 2)?;
    worksheet.set_column_width(0, 30)?;
    worksheet.set_column_width(1, 24)?;
    worksheet.set_column_width(2, 13)?;
    worksheet.set_column_width(3, 16)?;
    worksheet.set_column_width(4, 13)?;
    worksheet.set_column_width(5, 13)?;
    worksheet.set_column_width(6, 15)?;
    worksheet.set_column_width(7, 20)?;
    worksheet.set_column_width(8, 15)?;
    worksheet.set_column_width(9, 20)?;
    worksheet.set_column_width(10, 24)?;
    Ok(())
}

fn write_bill_row(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    bill: &DualAgeBillRow,
    amount_format: &Format,
    date_format: &Format,
) -> Result<(), OutstandingsWorkingPaperXlsxError> {
    worksheet.write_string(row, 0, &bill.party)?;
    worksheet.write_string(row, 1, &bill.reference)?;
    worksheet.write_string(row, 2, bill.direction.label())?;
    worksheet.write_number_with_format(
        row,
        3,
        amount_to_f64(bill.amount.as_str())?,
        amount_format,
    )?;
    worksheet.write_datetime_with_format(
        row,
        4,
        excel_date(bill.bill_date.as_str())?,
        date_format,
    )?;
    worksheet.write_datetime_with_format(
        row,
        5,
        excel_date(bill.due_date.as_str())?,
        date_format,
    )?;
    write_age(worksheet, row, 6, bill.bill_age_days)?;
    worksheet.write_string(row, 7, ageing_bucket(bill.bill_age_days))?;
    write_age(worksheet, row, 8, bill.due_age_days)?;
    worksheet.write_string(row, 9, ageing_bucket(bill.due_age_days))?;
    worksheet.write_string(row, 10, bill.amount.as_str())?;
    Ok(())
}

fn exact_control_text(
    controls: &super::outstandings_working_paper::WorkingPaperControls,
) -> String {
    format!(
        "R bills={}; R unallocated={}; R total={}; P bills={}; P unallocated={}; P total={}; magnitude={}",
        controls.receivable_bills.as_str(),
        controls.receivable_unallocated.as_str(),
        controls.receivable_total.as_str(),
        controls.payable_bills.as_str(),
        controls.payable_unallocated.as_str(),
        controls.payable_total.as_str(),
        controls.outstanding_total.as_str(),
    )
}

fn write_age(
    worksheet: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    column: u16,
    age_days: Option<u32>,
) -> Result<(), XlsxError> {
    match age_days {
        Some(age_days) => worksheet.write_number(row, column, age_days).map(|_| ()),
        None => worksheet
            .write_string(row, column, "Date not reached")
            .map(|_| ()),
    }
}

fn ageing_bucket(age_days: Option<u32>) -> &'static str {
    match age_days {
        None => "Date not reached",
        Some(0..=30) => "0-30 days",
        Some(31..=60) => "31-60 days",
        Some(61..=90) => "61-90 days",
        Some(_) => "90+ days",
    }
}

fn currency_label(assertion: OutstandingsCurrencyAssertion) -> &'static str {
    match assertion {
        OutstandingsCurrencyAssertion::Inr => "INR",
    }
}

fn amount_to_f64(text: &str) -> Result<f64, OutstandingsWorkingPaperXlsxError> {
    // Reuse the party-statement projection policy. Both workbooks are fed by
    // the same ExactDecimal source and must accept or reject the same Excel
    // numeric values; source scale alone is not a financial boundary.
    statement_amount_to_f64(text)
        .map_err(|_| OutstandingsWorkingPaperXlsxError::InvalidAmount(text.to_string()))
}

fn excel_date(yyyymmdd: &str) -> Result<ExcelDateTime, OutstandingsWorkingPaperXlsxError> {
    let invalid = || OutstandingsWorkingPaperXlsxError::InvalidDate(yyyymmdd.to_string());
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
    use std::io::Read;

    use bridge_tally_core::ExactDecimal;

    use super::*;
    use crate::reports::outstandings_working_paper::{
        build_outstandings_working_paper, OutstandingsWorkingPaper, OutstandingsWorkingPaperSource,
    };
    use crate::tally::{
        ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, UnallocatedParty,
    };

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
}
