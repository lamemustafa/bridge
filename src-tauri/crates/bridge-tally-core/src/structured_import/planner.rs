use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::context::{valid_identifier, valid_row_text};
use super::model::{hash_bytes, hash_serializable};
use super::{
    CompanyLedgerCatalog, DispatchAuthority, DispatchPrecondition, DryRunState,
    ImportLedgerMappings, PlannedLedger, PlannedPosting, PlannedVoucher, PostingSide,
    StructuredImportError, StructuredImportManifest, StructuredImportPlan, VoucherKind,
    MAX_STRUCTURED_IMPORT_JSON_BYTES, MAX_STRUCTURED_IMPORT_ROWS,
    STRUCTURED_IMPORT_CONTRACT_VERSION,
};
use crate::{ExactDecimal, TallyDate};

const MAX_NARRATION_BYTES: usize = 2_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportDocument {
    contract_version: u16,
    rows: Vec<ImportRow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRow {
    source_row_id: String,
    voucher_kind: VoucherKind,
    date: TallyDate,
    amount: ExactDecimal,
    counterparty_ledger_key: String,
    narration: Option<String>,
}

#[derive(Serialize)]
struct RowDigestPreimage<'a> {
    source_row_id: &'a str,
    voucher_kind: VoucherKind,
    date: &'a TallyDate,
    amount: &'a ExactDecimal,
    counterparty_ledger_guid: &'a str,
    settlement_ledger_guid: &'a str,
    narration: Option<&'a str>,
}

pub fn plan_payment_receipt_json(
    json: &[u8],
    catalog: &CompanyLedgerCatalog,
    mappings: &ImportLedgerMappings,
) -> Result<StructuredImportPlan, StructuredImportError> {
    if json.len() > MAX_STRUCTURED_IMPORT_JSON_BYTES {
        return Err(StructuredImportError::InputTooLarge);
    }
    if !mappings.is_bound_to(catalog) {
        return Err(StructuredImportError::StaleLedgerMapping);
    }
    let document: ImportDocument =
        serde_json::from_slice(json).map_err(|_| StructuredImportError::InvalidJson)?;
    if document.contract_version != STRUCTURED_IMPORT_CONTRACT_VERSION {
        return Err(StructuredImportError::UnsupportedContractVersion);
    }
    if document.rows.is_empty() {
        return Err(StructuredImportError::EmptyRows);
    }
    if document.rows.len() > MAX_STRUCTURED_IMPORT_ROWS {
        return Err(StructuredImportError::TooManyRows);
    }

    let input_sha256 = hash_bytes(json);
    let mut row_ids = BTreeSet::new();
    let mut vouchers = Vec::with_capacity(document.rows.len());
    for (ordinal, row) in document.rows.into_iter().enumerate() {
        if !valid_identifier(&row.source_row_id) {
            return Err(StructuredImportError::InvalidRowIdentity { ordinal });
        }
        if !row_ids.insert(row.source_row_id.clone()) {
            return Err(StructuredImportError::DuplicateRowIdentity { ordinal });
        }
        if row.amount.is_zero() || row.amount.is_negative() {
            return Err(StructuredImportError::NonPositiveAmount { ordinal });
        }
        if !catalog.date_is_allowed(&row.date) {
            return Err(StructuredImportError::VoucherDateOutsideAllowedWindow { ordinal });
        }
        if !valid_identifier(&row.counterparty_ledger_key)
            || row
                .narration
                .as_deref()
                .is_some_and(|value| !valid_row_text(value, MAX_NARRATION_BYTES))
        {
            return Err(StructuredImportError::InvalidRowText { ordinal });
        }

        let counterparty = mappings
            .resolve(&row.counterparty_ledger_key)
            .ok_or(StructuredImportError::UnknownLedgerMapping { ordinal })?;
        let (settlement_ledger_guid, settlement_ledger_name) = catalog.settlement_ledger();

        let row_sha256 = hash_serializable(
            b"bridge-structured-import-row-v1\0",
            &RowDigestPreimage {
                source_row_id: &row.source_row_id,
                voucher_kind: row.voucher_kind,
                date: &row.date,
                amount: &row.amount,
                counterparty_ledger_guid: &counterparty.ledger_guid,
                settlement_ledger_guid,
                narration: row.narration.as_deref(),
            },
        )?;
        let (counterparty_side, cash_or_bank_side) = match row.voucher_kind {
            VoucherKind::Payment => (PostingSide::Debit, PostingSide::Credit),
            VoucherKind::Receipt => (PostingSide::Credit, PostingSide::Debit),
        };
        vouchers.push(PlannedVoucher {
            source_row_id: row.source_row_id,
            row_sha256,
            voucher_kind: row.voucher_kind,
            date: row.date,
            narration: row.narration,
            postings: vec![
                PlannedPosting {
                    ledger: PlannedLedger {
                        ledger_guid: counterparty.ledger_guid.clone(),
                        exact_name: counterparty.exact_name.clone(),
                    },
                    side: counterparty_side,
                    amount: row.amount.clone(),
                },
                PlannedPosting {
                    ledger: PlannedLedger {
                        ledger_guid: settlement_ledger_guid.to_string(),
                        exact_name: settlement_ledger_name.to_string(),
                    },
                    side: cash_or_bank_side,
                    amount: row.amount,
                },
            ],
            debits_equal_credits: true,
        });
    }

    let manifest = StructuredImportManifest {
        contract_version: STRUCTURED_IMPORT_CONTRACT_VERSION,
        dry_run_state: DryRunState::NotDispatched,
        dispatch_authority: DispatchAuthority::Absent,
        unresolved_dispatch_preconditions: vec![
            DispatchPrecondition::ExactVoucherTypeIdentityUnverified,
            DispatchPrecondition::ManualNumberingPreflightUnverified,
            DispatchPrecondition::PreventDuplicatesPreflightUnverified,
            DispatchPrecondition::CompanyBookPeriodAcceptanceUnverified,
            DispatchPrecondition::TallyModeDateAcceptanceUnverified,
            DispatchPrecondition::XmlPayloadNotRendered,
            DispatchPrecondition::WriteReadbackNotConfigured,
        ],
        company_guid: catalog.company_guid().to_string(),
        source_run_id: catalog.source_run_id().to_string(),
        source_snapshot_sha256: catalog.source_snapshot_sha256().to_string(),
        input_sha256,
        ledger_catalog_sha256: catalog.snapshot_sha256().to_string(),
        mapping_sha256: mappings.mapping_sha256().to_string(),
        vouchers,
    };
    let manifest_sha256 = hash_serializable(b"bridge-structured-import-manifest-v1\0", &manifest)?;
    Ok(StructuredImportPlan {
        manifest_sha256,
        manifest,
    })
}
