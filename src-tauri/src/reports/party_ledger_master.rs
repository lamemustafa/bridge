//! Exact source model for the columnar party and ledger master workbook.
//!
//! This module performs no Tally I/O. Its source is constructed only after
//! paired, company-bracketed reads have established every row and its period.

use std::collections::BTreeSet;

use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::{PartyLedgerMasterFieldObservation, PartyLedgerMasterFields};

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
pub(crate) struct PartyLedgerMasterGroup {
    pub(crate) name: String,
    pub(crate) parent: PartyLedgerMasterFieldObservation,
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
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;

    use bridge_tally_protocol::{
        native_outstandings::{
            parse_native_group_snapshot_with_evidence, parse_native_ledger_snapshot,
        },
        parse_native_party_ledger_master_records_with_evidence, PartyLedgerMasterFieldObservation,
        PartyLedgerMasterFields,
    };

    use super::*;
    use crate::tally::OutstandingsCurrencyAssertion;
    use zip::ZipArchive;

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
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            currency_decimal_places: 2,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![PartyLedgerMasterRow {
                name: "Customer".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
                party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
                fields: PartyLedgerMasterFields::default(),
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
        let master = parse_native_party_ledger_master_records_with_evidence(
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
            let key = (
                source.record.ledger.name.clone(),
                source
                    .record
                    .ledger
                    .parent
                    .nonempty_returned_text()
                    .map(str::to_owned),
            );
            let balance = balances
                .remove(&key)
                .expect("every captured master has a corresponding balance");
            assert_eq!(
                source.record.ledger.opening_balance.as_deref(),
                Some(balance.opening_balance.as_str()),
                "captured master and balance openings agree"
            );
            rows.push(PartyLedgerMasterRow {
                name: source.record.ledger.name,
                parent: source.record.ledger.parent,
                party_gstin: source.record.ledger.party_gstin,
                fields: source.record.fields,
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
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            currency_decimal_places: 2,
            from: TallyDate::parse("20250401").unwrap(),
            to: TallyDate::parse("20260331").unwrap(),
            rows,
            master_response_sha256:
                "def766e42d0e36b4b73d7a176fa0ad08d1a7467e301000650fb5ba9a2ae06f29".to_string(),
            balance_response_sha256:
                "7e7af264d7251713d5179c6b6614f47329d860e941587ec9a5245597bf059f77".to_string(),
            group_response_sha256:
                "a9839b578ed776b707597e5dad93d155ab443b93c03d5ecb0b70bfd7ac207b1b".to_string(),
            master_response_bytes: MASTER_FIELDS_LAB_LEDGERS.len(),
            balance_response_bytes: MASTER_FIELDS_LAB_BALANCES.len(),
            group_response_bytes: MASTER_FIELDS_LAB_GROUPS.len(),
            groups,
        };
        let workbook = build_party_ledger_master_workbook(source).expect("captured source admits");
        let xlsx =
            super::super::party_ledger_master_xlsx::render_party_ledger_master_xlsx(&workbook)
                .expect("captured source renders a workbook");
        assert!(
            xlsx.starts_with(b"PK"),
            "captured source rendered an XLSX archive"
        );
        let mut archive = ZipArchive::new(Cursor::new(xlsx)).expect("valid XLSX archive");
        let mut xlsx_text = String::new();
        for name in [
            "xl/worksheets/sheet1.xml",
            "xl/worksheets/sheet2.xml",
            "xl/sharedStrings.xml",
        ] {
            std::io::Read::read_to_string(
                &mut archive.by_name(name).expect("XLSX part"),
                &mut xlsx_text,
            )
            .expect("read XLSX part");
        }
        assert!(xlsx_text.contains("ZZZZZ0002Z"));
        assert!(xlsx_text.contains("INR"));
        let source = workbook.source();
        assert_eq!(source.rows.len(), 17);
        assert!(source.rows.iter().any(|row| {
            row.name == "BRIDGE MFLAB DEBTOR CREDIT BALANCE"
                && row.opening_balance.as_str() == "1250.00"
                && row
                    .closing_balance
                    .as_ref()
                    .is_some_and(|balance| balance.as_str() == "1250.00")
        }));
        assert!(source.rows.iter().any(|row| {
            row.name == "BRIDGE MFLAB CREDITOR DEBIT BALANCE"
                && row.opening_balance.as_str() == "-1250.00"
                && row
                    .closing_balance
                    .as_ref()
                    .is_some_and(|balance| balance.as_str() == "-1250.00")
        }));
        assert!(source.rows.iter().any(|row| {
            row.name == "BRIDGE MFLAB DEBTOR BETA"
                && row.fields.income_tax_number
                    == PartyLedgerMasterFieldObservation::Returned("ZZZZZ0002Z".to_string())
                && row.fields.name_on_pan
                    == PartyLedgerMasterFieldObservation::Returned(
                        "BRIDGE MFLAB DEBTOR BETA".to_string(),
                    )
                && row.fields.pin_code == PartyLedgerMasterFieldObservation::Returned(String::new())
                && row.fields.state == PartyLedgerMasterFieldObservation::NotObserved
        }));

        let schedule = super::super::schedule_iii::build_schedule_iii_view(source)
            .expect("captured Schedule III derivation succeeds");
        assert!(schedule.difference.is_zero());
        assert!(schedule
            .lines
            .iter()
            .all(|line| line.label.ends_with("group subtotal")));
        assert!(schedule.exclusions.iter().any(|entry| {
            source.rows[entry.row_index].name == "BRIDGE MFLAB DEBTOR CREDIT BALANCE"
                && entry.reason.contains("opposite polarity")
        }));
        assert!(schedule.exclusions.iter().any(|entry| {
            source.rows[entry.row_index].name == "BRIDGE MFLAB CREDITOR DEBIT BALANCE"
                && entry.reason.contains("opposite polarity")
        }));
    }
}
