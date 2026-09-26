//! Renders the exact party and ledger master source as one `.xlsx` workbook.

use std::collections::BTreeMap;

use bridge_tally_protocol::PartyLedgerMasterFieldObservation;
use rust_xlsxwriter::{Format, Workbook, XlsxError};

use super::party_ledger_master::{PartyLedgerMasterSource, PartyLedgerMasterWorkbook};
use super::party_statement_xlsx::amount_to_f64;
use super::schedule_iii::{
    build_schedule_iii_view, DecisionInput, DecisionSource, DecisionStatus, DecisionsUnavailable,
    Finality, LineBasis, NotApplied, ScheduleIIIError, ScheduleIIIView,
};
use crate::tally::OutstandingsCurrencyAssertion;

const EXCEL_MAX_ROWS: usize = 1_048_576;

fn amount_num_format(decimal_places: u8) -> String {
    let mut format = "##,##,##0".to_string();
    if decimal_places > 0 {
        format.push('.');
        format.extend(std::iter::repeat_n('0', decimal_places.into()));
    }
    format
}

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
    decisions: DecisionInput<'_>,
) -> Result<Vec<u8>, PartyLedgerMasterXlsxError> {
    let source = workbook_source.source();
    if source.rows.len().saturating_add(15) > EXCEL_MAX_ROWS {
        return Err(PartyLedgerMasterXlsxError::RowLimit);
    }

    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Ledger master")?;
    let bold = Format::new().set_bold();
    let amount_num_format = amount_num_format(source.currency_decimal_places);
    let amount = Format::new().set_num_format(&amount_num_format);

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
    worksheet.write_string_with_format(11, 0, "Currency decimal places", &bold)?;
    worksheet.write_number(11, 1, f64::from(source.currency_decimal_places))?;
    worksheet.write_string_with_format(12, 0, "Source bytes", &bold)?;
    worksheet.write_string(
        12,
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
        worksheet.write_string(sheet_row, 1, row.parent.workbook_text())?;
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
    write_schedule_iii(&mut workbook, workbook_source, decisions)?;
    workbook
        .save_to_buffer()
        .map_err(PartyLedgerMasterXlsxError::from)
}

fn write_schedule_iii(
    workbook: &mut Workbook,
    workbook_source: &PartyLedgerMasterWorkbook,
    decisions: DecisionInput<'_>,
) -> Result<(), PartyLedgerMasterXlsxError> {
    let source = workbook_source.source();
    let view = build_schedule_iii_view(workbook_source, decisions)?;
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Group subtotal trace")?;
    let bold = Format::new().set_bold();
    let amount_num_format = amount_num_format(source.currency_decimal_places);
    let amount = Format::new().set_num_format(&amount_num_format);
    worksheet.write_string_with_format(0, 0, "Derived group subtotal view", &bold)?;
    worksheet.write_string(
        0,
        1,
        "Tally group identity and balance polarity only. A Schedule III head appears only where a CA grouping decision applies; see CA grouping decisions below.",
    )?;
    worksheet.write_string_with_format(1, 0, "Currency", &bold)?;
    worksheet.write_string(1, 1, currency_label(source.currency_assertion))?;
    worksheet.write_string_with_format(2, 0, "Currency decimal places", &bold)?;
    worksheet.write_number(2, 1, f64::from(source.currency_decimal_places))?;
    worksheet.write_string_with_format(3, 0, "Read period", &bold)?;
    worksheet.write_string(
        3,
        1,
        format!(
            "Read period: {} to {}. No prior-year values were requested or inferred.",
            source.from.as_str(),
            source.to.as_str()
        ),
    )?;
    for (row, label, value) in [
        (4, "Debit total", view.debit_total().as_str()),
        (5, "Credit total", view.credit_total().as_str()),
        (6, "Dr=Cr difference", view.difference().as_str()),
    ] {
        worksheet.write_string_with_format(row, 0, label, &bold)?;
        worksheet.write_string(row, 1, value)?;
    }
    worksheet.write_string_with_format(7, 0, "Check interpretation", &bold)?;
    worksheet.write_string(7, 1, "Difference 0 is the Tally-sign self-check over every captured ledger closing balance; it is evidence, not an assertion of statement completeness.")?;
    worksheet.write_string_with_format(8, 0, "CA grouping decisions", &bold)?;
    worksheet.write_string(8, 1, decisions_summary(&view))?;

    let header_row = 9u32;
    for (column, label) in [
        "Balance side or section",
        "Group subtotal or decided head",
        "Closing balance",
        "Closing balance (exact text)",
        "Basis",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(header_row, column as u16, label, &bold)?;
    }
    let mut row = header_row + 1;
    for line in view.lines() {
        worksheet.write_string(row, 0, line.section())?;
        worksheet.write_string(row, 1, line.label())?;
        worksheet.write_number_with_format(
            row,
            2,
            amount_to_f64(line.total().as_str()).map_err(|_| {
                PartyLedgerMasterXlsxError::InvalidAmount(line.total().as_str().to_string())
            })?,
            &amount,
        )?;
        worksheet.write_string(row, 3, line.total().as_str())?;
        worksheet.write_string(row, 4, basis_text(line.basis()))?;
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
        "Basis",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(row, column as u16, label, &bold)?;
    }
    row += 1;
    let drift_by_row: BTreeMap<usize, String> = view
        .decisions()
        .iter()
        .filter_map(|status| {
            let DecisionStatus::Applied { row_index, .. } = status else {
                return None;
            };
            let notes = drift_notes(source, status);
            (!notes.is_empty()).then(|| (*row_index, notes.join(" ")))
        })
        .collect();
    for line in view.lines() {
        for index in line.row_indices() {
            let ledger = &source.rows[*index];
            worksheet.write_string(row, 0, line.label())?;
            worksheet.write_string(row, 1, &ledger.name)?;
            worksheet.write_string(row, 2, ledger.parent.workbook_text())?;
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
            match drift_by_row.get(index) {
                Some(notes) => worksheet.write_string(
                    row,
                    5,
                    format!("{}. {notes}", basis_text(line.basis())),
                )?,
                None => worksheet.write_string(row, 5, basis_text(line.basis()))?,
            };
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
        "Why no evidence-backed group subtotal was emitted",
    ]
    .into_iter()
    .enumerate()
    {
        worksheet.write_string_with_format(row, column as u16, label, &bold)?;
    }
    row += 1;
    for exclusion in view.exclusions() {
        let ledger = &source.rows[exclusion.row_index()];
        worksheet.write_string(row, 0, &ledger.name)?;
        worksheet.write_string(row, 1, ledger.parent.workbook_text())?;
        worksheet.write_string(row, 2, &ledger.guid)?;
        worksheet.write_string(row, 3, exclusion.reason())?;
        row += 1;
    }
    let given = match decisions {
        DecisionInput::Read(set) => set.decisions(),
        DecisionInput::Unavailable(_) => &[],
    };
    if !given.is_empty() {
        row += 1;
        worksheet.write_string_with_format(row, 0, "CA GROUPING DECISIONS", &bold)?;
        row += 1;
        for (column, label) in [
            "Decision",
            "Ledger when decided",
            "GUID",
            "Decided head",
            "Status",
        ]
        .into_iter()
        .enumerate()
        {
            worksheet.write_string_with_format(row, column as u16, label, &bold)?;
        }
        row += 1;
        for (decision, status) in given.iter().zip(view.decisions()) {
            worksheet.write_string(row, 0, decision.id.0.to_string())?;
            worksheet.write_string(row, 1, &decision.ledger_name_when_made)?;
            worksheet.write_string(row, 2, decision.ledger.as_str())?;
            worksheet.write_string(row, 3, decision.head.caption())?;
            worksheet.write_string(row, 4, status_text(source, status))?;
            row += 1;
        }
    }
    worksheet.write_string_with_format(row + 1, 0, "Read did not cover", &bold)?;
    worksheet.write_string(row + 1, 1, "Prior-year balances, voucher-level classification, maturity/current-vs-non-current split, note disclosures, share-capital reconciliation, and reserves movement.")?;
    worksheet.set_column_width(0, 34)?;
    worksheet.set_column_width(1, 46)?;
    worksheet.set_column_width(2, 28)?;
    worksheet.set_column_width(3, 62)?;
    worksheet.set_column_width(4, 24)?;
    worksheet.set_column_width(5, 22)?;
    Ok(())
}

fn basis_text(basis: LineBasis) -> &'static str {
    match basis {
        LineBasis::GroupSubtotal(_) => "Tally group evidence",
        LineBasis::Decided(_) => "CA grouping decision",
    }
}

fn decisions_summary(view: &ScheduleIIIView) -> String {
    let unavailable = match view.decision_source() {
        DecisionSource::Read => None,
        DecisionSource::Unavailable(DecisionsUnavailable::StoreUnavailable) => {
            Some("the encrypted store could not be opened or read")
        }
        DecisionSource::Unavailable(DecisionsUnavailable::Unreadable) => {
            Some("the stored decisions could not be read by this version of ComplyEaze Bridge")
        }
    };
    if let Some(why) = unavailable {
        return format!(
            "Could not be read: {why}. NOT FINAL: no decision could be applied, and none is assumed absent."
        );
    }
    let (statuses, finality) = (view.decisions(), view.finality());
    if statuses.is_empty() {
        return "None were applied to this export.".to_string();
    }
    let applied = statuses
        .iter()
        .filter(|status| matches!(status, DecisionStatus::Applied { .. }))
        .count();
    let other_year = statuses
        .iter()
        .filter(|status| {
            matches!(
                status,
                DecisionStatus::NotApplied {
                    reason: NotApplied::OtherYear,
                    ..
                }
            )
        })
        .count();
    let attention = statuses.len() - applied - other_year;
    let verdict = match finality {
        Finality::Final => "Nothing needs attention.",
        Finality::NotFinal => "NOT FINAL: resolve each decision that needs attention.",
    };
    format!("{applied} applied; {attention} not applied and need attention; {other_year} for another financial year. {verdict}")
}

/// What changed in Tally since an applied decision was made, without stopping
/// it from applying. Empty for a decision that is not applied.
fn drift_notes(source: &PartyLedgerMasterSource, status: &DecisionStatus) -> Vec<String> {
    let DecisionStatus::Applied {
        row_index,
        renamed_from,
        ancestry_changed,
        ..
    } = status
    else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    if renamed_from.is_some() {
        notes.push(format!(
            "Tally now names this ledger \"{}\"; the GUID is unchanged.",
            source.rows[*row_index].name
        ));
    }
    if let Some(change) = ancestry_changed {
        notes.push(format!(
            "Ancestry changed since the decision: {} → {}.",
            group_path(&change.was),
            group_path(&change.now)
        ));
    }
    notes
}

/// Groups from the top down, as a trial balance reads them.
fn group_path(nearest_first: &[String]) -> String {
    if nearest_first.is_empty() {
        return "(account root)".to_string();
    }
    let mut path: Vec<&str> = nearest_first.iter().map(String::as_str).collect();
    path.reverse();
    path.join(" > ")
}

fn status_text(source: &PartyLedgerMasterSource, status: &DecisionStatus) -> String {
    match status {
        DecisionStatus::Applied { .. } => {
            let mut text = "Applied.".to_string();
            for note in drift_notes(source, status) {
                text.push(' ');
                text.push_str(&note);
            }
            text
        }
        DecisionStatus::NotApplied { reason, .. } => match reason {
            NotApplied::OtherYear => "Not applied: made for another financial year.".to_string(),
            NotApplied::LedgerMissing => "Not applied: the ledger is not in this read.".to_string(),
            NotApplied::BalanceNotEstablished => {
                "Not applied: Tally returned no closing balance for the ledger.".to_string()
            }
            NotApplied::ReadIncomplete => {
                "Not applied: the group read is incomplete for the ledger.".to_string()
            }
            NotApplied::GroupChanged { now } => format!(
                "Not applied: the ledger's group changed after the decision. Now: {}",
                now.description()
            ),
            NotApplied::OppositeSide => {
                "Not applied: the balance is on the other side from the decided head.".to_string()
            }
        },
    }
}

#[cfg(test)]
#[path = "party_ledger_master_xlsx_tests.rs"]
mod tests;
