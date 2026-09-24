//! Exact source model for the columnar party and ledger master workbook.
//!
//! This module performs no Tally I/O. Its source is constructed only after
//! paired, company-bracketed reads have established every row and its period.

use std::collections::BTreeSet;

use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::{
    PartyLedgerMasterFieldObservation, PartyLedgerMasterFields, TallyNamedMaster,
};

use crate::tally::OutstandingsCurrencyAssertion;

/// Excel preserves at most 15 significant digits. The workbook refuses a
/// currency precision beyond that documented representation boundary rather
/// than quietly selecting a two-decimal format.
pub(crate) const MAX_RENDERABLE_CURRENCY_DECIMAL_PLACES: u8 = 15;

#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterSource {
    pub(crate) company: String,
    pub(crate) company_guid: String,
    /// Money in this workbook may be rendered only after the backend's
    /// existing Tally currency probe established this assertion.
    pub(crate) currency_assertion: OutstandingsCurrencyAssertion,
    /// Tally's observed base-currency display precision. This remains data,
    /// not a renderer default, from the existing currency probe to the XLSX.
    pub(crate) currency_decimal_places: u8,
    pub(crate) from: TallyDate,
    pub(crate) to: TallyDate,
    pub(crate) rows: Vec<PartyLedgerMasterRow>,
    /// Ordered master/balance/group request body commitments (UTF-16LE on wire).
    pub(crate) request_sha256: String,
    pub(crate) master_response_sha256: String,
    pub(crate) balance_response_sha256: String,
    pub(crate) group_response_sha256: String,
    pub(crate) master_response_bytes: usize,
    pub(crate) balance_response_bytes: usize,
    pub(crate) group_response_bytes: usize,
    /// Native group rows captured in the same company-bracketed read as the
    /// ledger rows. Schedule III classification remains a pure derivation of
    /// this source; it never performs an independent reader call.
    pub(crate) groups: Vec<TallyNamedMaster>,
    /// Ledgers kept in a currency other than the base, left out of `rows`
    /// with their master and balance rows (bridge#551). Empty on a book with
    /// one Currency master, whose base refuses such a ledger instead.
    pub(crate) foreign_currency_ledgers_excluded:
        Vec<bridge_tally_protocol::native_outstandings::ForeignCurrencyLedger>,
}

#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterRow {
    pub(crate) name: String,
    pub(crate) parent: PartyLedgerMasterFieldObservation,
    pub(crate) party_gstin: PartyLedgerMasterFieldObservation,
    pub(crate) fields: PartyLedgerMasterFields,
    pub(crate) guid: String,
    pub(crate) master_id: String,
    pub(crate) alter_id: String,
    pub(crate) opening_balance: ExactDecimal,
    /// `None` is an empty Tally `CLOSINGBALANCE`, not a zero balance.
    pub(crate) closing_balance: Option<ExactDecimal>,
}

#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterWorkbook {
    source: PartyLedgerMasterSource,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum PartyLedgerMasterError {
    #[error("party/ledger master source omitted company identity")]
    MissingCompanyIdentity,
    #[error("party/ledger master source omitted a row identity")]
    MissingRowIdentity,
    #[error("party/ledger master source repeated a GUID")]
    DuplicateGuid,
    #[error("party/ledger master source repeated a master ID")]
    DuplicateMasterId,
    #[error("party/ledger master source omitted a source response commitment")]
    MissingResponseCommitment,
    #[error("party/ledger master currency precision cannot be rendered safely ({0})")]
    UnrenderableCurrencyPrecision(u8),
}

pub(crate) fn build_party_ledger_master_workbook(
    source: PartyLedgerMasterSource,
) -> Result<PartyLedgerMasterWorkbook, PartyLedgerMasterError> {
    if source.company.trim().is_empty() || source.company_guid.trim().is_empty() {
        return Err(PartyLedgerMasterError::MissingCompanyIdentity);
    }
    if !sha256(&source.master_response_sha256)
        || !sha256(&source.balance_response_sha256)
        || !sha256(&source.group_response_sha256)
    {
        return Err(PartyLedgerMasterError::MissingResponseCommitment);
    }
    if source.currency_decimal_places > MAX_RENDERABLE_CURRENCY_DECIMAL_PLACES {
        return Err(PartyLedgerMasterError::UnrenderableCurrencyPrecision(
            source.currency_decimal_places,
        ));
    }

    let mut guids = BTreeSet::new();
    let mut master_ids = BTreeSet::new();
    for row in &source.rows {
        if row.name.trim().is_empty()
            || row.guid.trim().is_empty()
            || row.master_id.trim().is_empty()
        {
            return Err(PartyLedgerMasterError::MissingRowIdentity);
        }
        if !guids.insert(row.guid.to_ascii_lowercase()) {
            return Err(PartyLedgerMasterError::DuplicateGuid);
        }
        if !master_ids.insert(row.master_id.clone()) {
            return Err(PartyLedgerMasterError::DuplicateMasterId);
        }
    }

    Ok(PartyLedgerMasterWorkbook { source })
}

impl PartyLedgerMasterWorkbook {
    pub(crate) fn source(&self) -> &PartyLedgerMasterSource {
        &self.source
    }
}

fn sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
#[path = "party_ledger_master_tests.rs"]
mod tests;
