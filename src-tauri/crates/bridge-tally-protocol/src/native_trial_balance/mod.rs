//! Native Trial Balance source contract and closed request profile.
//! See TALLY_PROTOCOL_REFERENCE section 5.6 for observed field semantics.
use crate::{native_outstandings::NativeLedgerSnapshotPeriod, PartyLedgerMasterFieldObservation};
use bridge_tally_primitives::ExactDecimal;
use serde::Serialize;
use std::fmt;
mod scalar;
mod wire;
pub use wire::parse_native_trial_balance;

/// One amount exactly as the native collection exposed it.
///
/// An empty `TYPE="Amount"` element is source evidence and is never coerced
/// to numeric zero. A missing element is a malformed Trial Balance row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum NativeTrialBalanceAmount {
    Present(ExactDecimal),
    PresentEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeTrialBalanceRow {
    pub name: String,
    pub guid: String,
    pub parent: PartyLedgerMasterFieldObservation,
    pub opening: NativeTrialBalanceAmount,
    pub debit: NativeTrialBalanceAmount,
    pub credit: NativeTrialBalanceAmount,
    pub closing: NativeTrialBalanceAmount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeTrialBalance {
    pub rows: Vec<NativeTrialBalanceRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTrialBalanceError {
    InvalidAmount,
    InvalidResponse(&'static str),
    TallyReportedFailure,
}

impl fmt::Display for NativeTrialBalanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAmount => {
                formatter.write_str("Tally returned an invalid Trial Balance amount")
            }
            Self::InvalidResponse(code) => {
                write!(formatter, "native Trial Balance response invalid ({code})")
            }
            Self::TallyReportedFailure => {
                formatter.write_str("Tally reported failure for the native Trial Balance request")
            }
        }
    }
}

impl std::error::Error for NativeTrialBalanceError {}

/// Renders the only supported native Trial Balance read. Both dates come from
/// the already-admitted snapshot period: `TBALOPENING` and `TBALCLOSING` are
/// meaningful only for the same validated window.
pub fn render_native_trial_balance_request(
    company: &str,
    period: &NativeLedgerSnapshotPeriod,
) -> String {
    render_trial_balance_collection(
        company,
        period,
        "NAME, GUID, PARENT, TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING",
    )
}

/// [`render_native_trial_balance_request`] with `CURRENCYNAME` appended to
/// its `FETCH`, so each row names its ledger's currency and a foreign-currency
/// ledger can be set aside by name (bridge#551). Sent only when the company
/// defines several Currency masters, so a single-currency book's request is
/// byte-for-byte unchanged. UNVERIFIED: that a Trial Balance row carries
/// `CURRENCYNAME` at all waits on a live capture on a several-currency book.
pub fn render_native_trial_balance_request_with_currency(
    company: &str,
    period: &NativeLedgerSnapshotPeriod,
) -> String {
    render_trial_balance_collection(
        company,
        period,
        "NAME, GUID, PARENT, TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING, CURRENCYNAME",
    )
}

fn render_trial_balance_collection(
    company: &str,
    period: &NativeLedgerSnapshotPeriod,
    fetch: &str,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>{fetch}</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = period.from().as_str(),
        to = period.to().as_str(),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests;
