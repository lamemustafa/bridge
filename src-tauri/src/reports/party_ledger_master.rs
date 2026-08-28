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
    /// `None` is an empty Tally `CLOSINGBALANCE`, not a zero balance.
    pub(crate) closing_balance: Option<ExactDecimal>,
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
    use std::collections::BTreeMap;

    use bridge_tally_protocol::{
        native_outstandings::{
            parse_native_group_snapshot_with_evidence, parse_native_ledger_snapshot,
        },
        parse_native_ledger_source_records_with_evidence,
    };

    use super::*;

    const MASTER_FIELDS_LAB_COMPANY_GUID: &str = "56359347-3976-4d01-b44e-56fa0f6a422c";
    const MASTER_FIELDS_LAB_LEDGERS: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/ledgers_native_master_fields_lab.utf8.xml"
    );
    const MASTER_FIELDS_LAB_BALANCES: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/ledger_snapshot_master_fields_lab.utf8.xml"
    );
    const MASTER_FIELDS_LAB_GROUPS: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_master_fields_lab.utf8.xml"
    );

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
                closing_balance: Some(ExactDecimal::parse("125.00".to_string()).unwrap()),
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

    #[test]
    fn captured_master_fields_lab_drives_the_party_export_and_schedule_iii_view() {
        let master = parse_native_ledger_source_records_with_evidence(
            MASTER_FIELDS_LAB_LEDGERS,
            MASTER_FIELDS_LAB_COMPANY_GUID,
        )
        .expect("captured ledger-master response parses");
        let mut balances = BTreeMap::new();
        for balance in parse_native_ledger_snapshot(MASTER_FIELDS_LAB_BALANCES)
            .expect("captured balance response parses")
        {
            let key = (balance.name.clone(), balance.parent.clone());
            assert!(
                balances.insert(key, balance).is_none(),
                "captured balance response has one row per display key"
            );
        }

        let mut rows = Vec::new();
        for source in master.records {
            let key = (source.record.name.clone(), source.record.parent.clone());
            let balance = balances
                .remove(&key)
                .expect("every captured master has a corresponding balance");
            assert_eq!(
                source.record.opening_balance.as_deref(),
                Some(balance.opening_balance.as_str()),
                "captured master and balance openings agree"
            );
            rows.push(PartyLedgerMasterRow {
                name: source.record.name,
                parent: source.record.parent,
                party_gstin: source.record.party_gstin,
                guid: source.identities.guid.expect("captured GUID"),
                master_id: source.identities.master_id.expect("captured MASTERID"),
                alter_id: source.alter_id.expect("captured ALTERID"),
                opening_balance: balance.opening_balance,
                closing_balance: balance.closing_balance,
            });
        }
        assert!(
            balances.is_empty(),
            "the captured balance response has no unmatched ledger"
        );

        let groups = parse_native_group_snapshot_with_evidence(
            MASTER_FIELDS_LAB_GROUPS,
            MASTER_FIELDS_LAB_COMPANY_GUID,
        )
        .expect("captured group response parses")
        .into_iter()
        .map(|entry| PartyLedgerMasterGroup {
            name: entry.record.name,
            parent: entry.record.parent,
            reserved_name: entry.record.reserved_name,
        })
        .collect();
        let source = PartyLedgerMasterSource {
            company: "BRIDGE MASTER FIELDS LAB".to_string(),
            company_guid: MASTER_FIELDS_LAB_COMPANY_GUID.to_string(),
            from: TallyDate::parse("20250401").unwrap(),
            to: TallyDate::parse("20260331").unwrap(),
            rows,
            master_response_sha256:
                "859475b66770917dc87a10d798d9c5ce4c356974ae29cb308623d7819209a79c".to_string(),
            balance_response_sha256:
                "9ac5c7f2e61fec8864a8601ac146c54ba9a3ffcad22057070c5971acb985b964".to_string(),
            group_response_sha256:
                "89b2051c37251dbb028de698d2220296dabe8847cc778a0a0d4ab530b38780c9".to_string(),
            master_response_bytes: MASTER_FIELDS_LAB_LEDGERS.len(),
            balance_response_bytes: MASTER_FIELDS_LAB_BALANCES.len(),
            group_response_bytes: MASTER_FIELDS_LAB_GROUPS.len(),
            groups,
        };
        let workbook = build_party_ledger_master_workbook(source).expect("captured source admits");
        let source = workbook.source();
        assert_eq!(source.rows.len(), 17);
        assert!(source.rows.iter().any(|row| {
            row.name == "BRIDGE MFLAB DEBTOR CREDIT BALANCE"
                && row.opening_balance.as_str() == "-1250.00"
                && row.closing_balance.as_str() == "-1250.00"
        }));
        assert!(source.rows.iter().any(|row| {
            row.name == "BRIDGE MFLAB CREDITOR DEBIT BALANCE"
                && row.opening_balance.as_str() == "1250.00"
                && row.closing_balance.as_str() == "1250.00"
        }));

        let schedule = super::super::schedule_iii::build_schedule_iii_view(source)
            .expect("captured Schedule III derivation succeeds");
        assert!(schedule.difference.is_zero());
        let receivables = schedule
            .lines
            .iter()
            .find(|line| line.label == "Trade receivables (maturity split not determined)")
            .expect("captured debtors classify");
        assert_eq!(receivables.total.as_str(), "-1250");
        let payables = schedule
            .lines
            .iter()
            .find(|line| line.label == "Trade payables (maturity split not determined)")
            .expect("captured creditors classify");
        assert_eq!(payables.total.as_str(), "1250");
    }
}
