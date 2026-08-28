//! Exact source model for the columnar party and ledger master workbook.
//!
//! This module performs no Tally I/O. Its source is constructed only after
//! paired, company-bracketed reads have established every row and its period.

use std::collections::BTreeSet;

use bridge_tally_core::{ExactDecimal, TallyDate};

#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterSource {
    pub(crate) company: String,
    pub(crate) company_guid: String,
    pub(crate) from: TallyDate,
    pub(crate) to: TallyDate,
    pub(crate) rows: Vec<PartyLedgerMasterRow>,
    pub(crate) master_response_sha256: String,
    pub(crate) balance_response_sha256: String,
    pub(crate) group_response_sha256: String,
    pub(crate) master_response_bytes: usize,
    pub(crate) balance_response_bytes: usize,
    pub(crate) group_response_bytes: usize,
    /// Native group rows captured in the same company-bracketed read as the
    /// ledger rows. Schedule III classification remains a pure derivation of
    /// this source; it never performs an independent reader call.
    pub(crate) groups: Vec<PartyLedgerMasterGroup>,
}

#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterRow {
    pub(crate) name: String,
    pub(crate) parent: Option<String>,
    pub(crate) party_gstin: Option<String>,
    pub(crate) guid: String,
    pub(crate) master_id: String,
    pub(crate) alter_id: String,
    pub(crate) opening_balance: ExactDecimal,
    pub(crate) closing_balance: ExactDecimal,
}

#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterGroup {
    pub(crate) name: String,
    pub(crate) parent: Option<String>,
    /// `Some("")` is Tally's explicit user-created-group signal. It must not
    /// be treated as an alias for a built-in Schedule III category.
    pub(crate) reserved_name: Option<String>,
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
mod tests {
    use super::*;

    fn source() -> PartyLedgerMasterSource {
        PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![PartyLedgerMasterRow {
                name: "Customer".to_string(),
                parent: Some("Sundry Debtors".to_string()),
                party_gstin: None,
                guid: "ledger-guid".to_string(),
                master_id: "7".to_string(),
                alter_id: "9".to_string(),
                opening_balance: ExactDecimal::parse("-100.00".to_string()).unwrap(),
                closing_balance: ExactDecimal::parse("125.00".to_string()).unwrap(),
            }],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 100,
            balance_response_bytes: 200,
            group_response_bytes: 300,
            groups: vec![],
        }
    }

    #[test]
    fn rejects_duplicate_source_identities_before_rendering() {
        let mut duplicate_guid = source();
        duplicate_guid.rows.push(PartyLedgerMasterRow {
            guid: "LEDGER-GUID".to_string(),
            master_id: "8".to_string(),
            ..duplicate_guid.rows[0].clone()
        });
        assert!(matches!(
            build_party_ledger_master_workbook(duplicate_guid),
            Err(PartyLedgerMasterError::DuplicateGuid)
        ));

        let mut duplicate_master_id = source();
        duplicate_master_id.rows.push(PartyLedgerMasterRow {
            guid: "other-guid".to_string(),
            ..duplicate_master_id.rows[0].clone()
        });
        assert!(matches!(
            build_party_ledger_master_workbook(duplicate_master_id),
            Err(PartyLedgerMasterError::DuplicateMasterId)
        ));
    }
}
