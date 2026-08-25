use std::collections::BTreeMap;

use serde::Serialize;

use super::context::{valid_identifier, CompanyLedgerCatalog};
use super::model::hash_serializable;
use super::StructuredImportError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportLedgerMappingInput {
    pub source_ledger_key: String,
    pub ledger_guid: String,
    pub expected_exact_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct BoundLedgerMapping {
    pub(super) ledger_guid: String,
    pub(super) exact_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportLedgerMappings {
    company_guid: String,
    ledger_catalog_sha256: String,
    mapping_sha256: String,
    entries: BTreeMap<String, BoundLedgerMapping>,
}

impl ImportLedgerMappings {
    pub fn bind(
        catalog: &CompanyLedgerCatalog,
        entries: Vec<ImportLedgerMappingInput>,
    ) -> Result<Self, StructuredImportError> {
        if entries.is_empty() {
            return Err(StructuredImportError::InvalidLedgerMapping);
        }
        let mut bound = BTreeMap::new();
        for entry in entries {
            let Some(observed_name) = catalog.exact_name(&entry.ledger_guid) else {
                return Err(StructuredImportError::InvalidLedgerMapping);
            };
            if !valid_identifier(&entry.source_ledger_key)
                || catalog.is_settlement_ledger(&entry.ledger_guid)
                || observed_name != entry.expected_exact_name
                || bound
                    .insert(
                        entry.source_ledger_key,
                        BoundLedgerMapping {
                            ledger_guid: entry.ledger_guid,
                            exact_name: entry.expected_exact_name,
                        },
                    )
                    .is_some()
            {
                return Err(StructuredImportError::InvalidLedgerMapping);
            }
        }
        let mapping_sha256 = hash_serializable(
            b"bridge-structured-import-ledger-mapping-v2\0",
            &(catalog.company_guid(), catalog.snapshot_sha256(), &bound),
        )?;
        Ok(Self {
            company_guid: catalog.company_guid().to_string(),
            ledger_catalog_sha256: catalog.snapshot_sha256().to_string(),
            mapping_sha256,
            entries: bound,
        })
    }

    pub fn mapping_sha256(&self) -> &str {
        &self.mapping_sha256
    }

    pub(super) fn is_bound_to(&self, catalog: &CompanyLedgerCatalog) -> bool {
        self.company_guid == catalog.company_guid()
            && self.ledger_catalog_sha256 == catalog.snapshot_sha256()
    }

    pub(super) fn resolve(&self, key: &str) -> Option<&BoundLedgerMapping> {
        self.entries.get(key)
    }
}
