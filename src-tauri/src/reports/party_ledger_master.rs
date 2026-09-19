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

/// The source collection after all read-level guards have passed, but before
/// the exact `(NAME, PARENT)` master/balance join is admitted as a complete
/// workbook source.  `rows` contains only one-to-one joins; every source row
/// outside such a join has a source-scoped unresolved observation instead.
///
/// This is deliberately distinct from [`PartyLedgerMasterSource`].  A
/// diagnostic must not be passed to workbook or Schedule III consumers: only
/// [`Self::into_complete_source`] can construct that complete-only type.
#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterJoinDiagnostic {
    pub(crate) company: String,
    pub(crate) company_guid: String,
    pub(crate) currency_assertion: OutstandingsCurrencyAssertion,
    pub(crate) currency_decimal_places: u8,
    pub(crate) from: TallyDate,
    pub(crate) to: TallyDate,
    pub(crate) rows: Vec<PartyLedgerMasterRow>,
    pub(crate) unresolved: Vec<PartyLedgerMasterJoinUnresolved>,
    pub(crate) master_observation_count: usize,
    pub(crate) balance_observation_count: usize,
    pub(crate) request_sha256: String,
    pub(crate) master_response_sha256: String,
    pub(crate) balance_response_sha256: String,
    pub(crate) group_response_sha256: String,
    pub(crate) master_response_bytes: usize,
    pub(crate) balance_response_bytes: usize,
    pub(crate) group_response_bytes: usize,
    pub(crate) groups: Vec<TallyNamedMaster>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartyLedgerMasterJoinUnresolvedSource {
    Master,
    Balance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartyLedgerMasterJoinUnresolvedReason {
    MasterMissingBalance,
    BalanceWithoutMaster,
    DuplicateMasterDisplayKey,
    DuplicateBalanceDisplayKey,
}

/// One unpaired source observation.  `name` and `parent` are the exact parsed
/// values from that source side; no comparison-time normalization is exposed.
/// No money or compliance fields are retained here, because no exact pair
/// established that they belong to the other source response.
#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterJoinUnresolved {
    pub(crate) source: PartyLedgerMasterJoinUnresolvedSource,
    pub(crate) source_ordinal: usize,
    pub(crate) name: String,
    pub(crate) parent: PartyLedgerMasterFieldObservation,
    pub(crate) reason: PartyLedgerMasterJoinUnresolvedReason,
}

impl PartyLedgerMasterJoinDiagnostic {
    /// The strict source remains constructible only when every observed master
    /// and balance row participated in an exact one-to-one join.
    pub(crate) fn into_complete_source(self) -> Result<PartyLedgerMasterSource, Box<Self>> {
        if !self.unresolved.is_empty() {
            return Err(Box::new(self));
        }
        Ok(PartyLedgerMasterSource {
            company: self.company,
            company_guid: self.company_guid,
            currency_assertion: self.currency_assertion,
            currency_decimal_places: self.currency_decimal_places,
            from: self.from,
            to: self.to,
            rows: self.rows,
            request_sha256: self.request_sha256,
            master_response_sha256: self.master_response_sha256,
            balance_response_sha256: self.balance_response_sha256,
            group_response_sha256: self.group_response_sha256,
            master_response_bytes: self.master_response_bytes,
            balance_response_bytes: self.balance_response_bytes,
            group_response_bytes: self.group_response_bytes,
            groups: self.groups,
        })
    }
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
