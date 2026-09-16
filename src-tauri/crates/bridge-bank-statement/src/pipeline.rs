//! The fail-closed order of operations, in one place.
//!
//! Every stage refuses the whole run; nothing downstream sees a statement an
//! earlier stage could not prove. The order is the reference's: bind the
//! account before trusting the arithmetic, prove the arithmetic before mapping,
//! and re-read what was built.

use crate::bank::Bank;
use crate::date::Date;
use crate::geometry::Page;
use crate::mapping::Mapping;
use crate::money::{reconcile, statement_totals, verify_against_statement, Controls, Totals};
use crate::parse::{parse_statement, require_account_match};
use crate::proposals::{
    build, group_counterparties, selfcheck, Build, BuildOptions, CounterpartyGroup, Selfcheck,
};
use crate::refusal::Refusal;
use bridge_tally_primitives::ExactDecimal;

pub struct StatementRequest<'a> {
    pub bank: Bank,
    /// The operator's account label; its digits bind the statement.
    pub account_label: &'a str,
    pub controls: &'a Controls,
    pub bank_ledger: &'a str,
    pub suspense_ledger: &'a str,
    pub mapping: &'a Mapping,
    pub date_from: Option<Date>,
    pub date_to: Option<Date>,
}

#[derive(Debug)]
pub struct ParsedStatement {
    /// The account number printed on the statement's account-number line.
    pub account_number: String,
    /// Every row the statement prints, before the date window.
    pub statement_rows: usize,
    pub closing: ExactDecimal,
    pub totals: Totals,
    pub build: Build,
    pub check: Selfcheck,
    pub counterparties: Vec<CounterpartyGroup>,
}

/// Pages → proven proposals, or the first refusal.
///
/// The balance and totals are proven over **every** row the statement prints;
/// the date window only narrows which rows become proposals afterwards.
pub fn prepare(pages: &[Page], request: &StatementRequest<'_>) -> Result<ParsedStatement, Refusal> {
    if request.bank.prints_totals() && request.controls.debits.is_none() {
        return Err(Refusal::new(
            "control_totals_required",
            format!(
                "a {} statement prints its debit and credit totals; supply both",
                request.bank.name().to_uppercase()
            ),
        ));
    }
    let rows = parse_statement(pages, request.bank)?;
    let account_number = require_account_match(pages, request.bank, request.account_label)?;
    let closing = reconcile(&rows, &request.controls.opening, &request.controls.closing)?;
    // `Controls` holds both totals or neither
    let totals = match (&request.controls.debits, &request.controls.credits) {
        (Some(debits), Some(credits)) => verify_against_statement(&rows, debits, credits)?,
        _ => statement_totals(&rows)?,
    };
    let build = build(
        &rows,
        request.bank,
        request.mapping,
        &BuildOptions {
            bank_ledger: request.bank_ledger,
            suspense_ledger: request.suspense_ledger,
            account_label: request.account_label,
            account_number: &account_number,
            date_from: request.date_from,
            date_to: request.date_to,
        },
    )?;
    let check = selfcheck(&build, request.bank_ledger)?;
    let counterparties = group_counterparties(&build.records)?;
    Ok(ParsedStatement {
        account_number,
        statement_rows: rows.len(),
        closing,
        totals,
        build,
        check,
        counterparties,
    })
}
