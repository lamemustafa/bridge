use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::evidence::validate_core_catalog_evidence;
use super::model::hash_serializable;
use super::StructuredImportError;
use crate::{
    CanonicalPackWindow, CapabilityPackId, PackBatch, RequestContext, TallyDate,
    CORE_ACCOUNTING_SCHEMA_VERSION,
};

const MAX_IDENTIFIER_BYTES: usize = 200;
const MAX_LEDGER_NAME_BYTES: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementLedgerRole {
    Bank,
    Cash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LedgerCatalogEntry {
    ledger_guid: String,
    exact_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyLedgerCatalog {
    company_guid: String,
    source_run_id: String,
    source_snapshot_sha256: String,
    snapshot_sha256: String,
    allowed_from: TallyDate,
    allowed_to: TallyDate,
    settlement_ledger: LedgerCatalogEntry,
    settlement_role: SettlementLedgerRole,
    settlement_ledger_guids: BTreeSet<String>,
    entries: BTreeMap<String, LedgerCatalogEntry>,
}

impl CompanyLedgerCatalog {
    pub fn from_core_window(
        context: &RequestContext,
        window: &CanonicalPackWindow,
        settlement_ledger_guid: &str,
    ) -> Result<Self, StructuredImportError> {
        if context.pack != CapabilityPackId::CoreAccounting
            || context.schema_version != CORE_ACCOUNTING_SCHEMA_VERSION
            || !valid_identifier(&context.run_id)
            || !valid_identifier(&context.company.identity.company_guid)
            || !valid_identifier(&context.company.identity.observed_fingerprint)
        {
            return Err(StructuredImportError::InvalidCompanyIdentity);
        }
        let PackBatch::CoreAccounting(core) = &window.batch else {
            return Err(StructuredImportError::InvalidSourceEvidence);
        };
        validate_core_catalog_evidence(context, window, core)?;

        let allowed_from = TallyDate::parse(context.window.from_yyyymmdd.clone())
            .map_err(|_| StructuredImportError::InvalidSourceEvidence)?;
        let allowed_to = TallyDate::parse(context.window.to_yyyymmdd.clone())
            .map_err(|_| StructuredImportError::InvalidSourceEvidence)?;
        if allowed_from > allowed_to {
            return Err(StructuredImportError::InvalidSourceEvidence);
        }

        let mut entries = BTreeMap::new();
        let mut names = BTreeSet::new();
        for ledger in &core.ledgers {
            if !valid_identifier(&ledger.source_id)
                || !valid_text(&ledger.name, MAX_LEDGER_NAME_BYTES)
                || !names.insert(ledger.name.clone())
                || entries
                    .insert(
                        ledger.source_id.clone(),
                        LedgerCatalogEntry {
                            ledger_guid: ledger.source_id.clone(),
                            exact_name: ledger.name.clone(),
                        },
                    )
                    .is_some()
            {
                return Err(StructuredImportError::InvalidLedgerCatalog);
            }
        }
        let settlement_ledger = entries
            .get(settlement_ledger_guid)
            .cloned()
            .ok_or(StructuredImportError::InvalidSettlementLedger)?;
        let mut settlement_ledger_guids = BTreeSet::new();
        for ledger in &core.ledgers {
            if ledger_role(core, &ledger.source_id)?.is_some() {
                settlement_ledger_guids.insert(ledger.source_id.clone());
            }
        }
        let settlement_role = ledger_role(core, settlement_ledger_guid)?
            .ok_or(StructuredImportError::InvalidSettlementLedger)?;
        let source_snapshot_sha256 = hash_serializable(
            b"bridge-structured-import-core-window-v1\0",
            &(context, window),
        )?;
        let group_semantics = core
            .groups
            .iter()
            .map(|group| {
                (
                    group.source_id.as_str(),
                    (group.name.as_str(), group.parent_source_id.as_deref()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let ledger_semantics = core
            .ledgers
            .iter()
            .map(|ledger| {
                (
                    ledger.source_id.as_str(),
                    (ledger.name.as_str(), ledger.parent_source_id.as_deref()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let snapshot_sha256 = hash_serializable(
            b"bridge-structured-import-ledger-catalog-v2\0",
            &(
                &context.company.identity,
                &settlement_ledger,
                settlement_role,
                &group_semantics,
                &ledger_semantics,
            ),
        )?;
        Ok(Self {
            company_guid: context.company.identity.company_guid.clone(),
            source_run_id: context.run_id.clone(),
            source_snapshot_sha256,
            snapshot_sha256,
            allowed_from,
            allowed_to,
            settlement_ledger,
            settlement_role,
            settlement_ledger_guids,
            entries,
        })
    }

    pub fn company_guid(&self) -> &str {
        &self.company_guid
    }

    pub fn source_run_id(&self) -> &str {
        &self.source_run_id
    }

    pub fn source_snapshot_sha256(&self) -> &str {
        &self.source_snapshot_sha256
    }

    pub fn snapshot_sha256(&self) -> &str {
        &self.snapshot_sha256
    }

    pub fn settlement_role(&self) -> SettlementLedgerRole {
        self.settlement_role
    }

    pub(super) fn date_is_allowed(&self, date: &TallyDate) -> bool {
        date >= &self.allowed_from && date <= &self.allowed_to
    }

    pub(super) fn settlement_ledger(&self) -> (&str, &str) {
        (
            &self.settlement_ledger.ledger_guid,
            &self.settlement_ledger.exact_name,
        )
    }

    pub(super) fn is_settlement_ledger(&self, ledger_guid: &str) -> bool {
        self.settlement_ledger_guids.contains(ledger_guid)
    }

    pub(super) fn exact_name(&self, ledger_guid: &str) -> Option<&str> {
        self.entries
            .get(ledger_guid)
            .map(|entry| entry.exact_name.as_str())
    }
}

fn ledger_role(
    core: &crate::CoreAccountingBatch,
    ledger_guid: &str,
) -> Result<Option<SettlementLedgerRole>, StructuredImportError> {
    let ledger = core
        .ledgers
        .iter()
        .find(|ledger| ledger.source_id == ledger_guid)
        .ok_or(StructuredImportError::InvalidSettlementLedger)?;
    let groups = core
        .groups
        .iter()
        .map(|group| (group.source_id.as_str(), group))
        .collect::<BTreeMap<_, _>>();
    let mut current = ledger.parent_source_id.as_deref();
    let mut visited = BTreeSet::new();
    while let Some(group_id) = current {
        if !visited.insert(group_id) {
            return Err(StructuredImportError::InvalidSourceEvidence);
        }
        let group = groups
            .get(group_id)
            .ok_or(StructuredImportError::InvalidSourceEvidence)?;
        match group.name.as_str() {
            "Bank Accounts" | "Bank OD A/c" => {
                return Ok(Some(SettlementLedgerRole::Bank));
            }
            "Cash-in-Hand" => return Ok(Some(SettlementLedgerRole::Cash)),
            _ => current = group.parent_source_id.as_deref(),
        }
    }
    Ok(None)
}

pub(super) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

pub(super) fn valid_row_text(value: &str, maximum: usize) -> bool {
    valid_text(value, maximum)
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
