//! Short-lived, one-use bindings between a completed native read and its
//! working-paper export. The webview receives only an opaque identifier.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::outstandings_working_paper::OutstandingsWorkingPaperSource;
use crate::tally::OutstandingsLoadResult;

const EXPORT_HANDLE_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_STORED_EXPORTS: usize = 4;
const MAX_BILL_ROWS: usize = 200_000;
const MAX_UNALLOCATED_ROWS: usize = 100_000;
const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CELL_TEXT_BYTES: usize = 4_096;
const MAX_AGGREGATE_TEXT_BYTES: usize = 64 * 1024 * 1024;

struct StoredExport {
    id: String,
    expires_at: Instant,
    revocation_key: String,
    source: OutstandingsWorkingPaperSource,
}

#[derive(Default)]
pub struct WorkingPaperExportStore {
    entries: Mutex<VecDeque<StoredExport>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkingPaperExportStoreError {
    #[error("working-paper source exceeds the bounded export budget")]
    ResourceLimit,
    #[error("working-paper export approval is invalid or expired")]
    InvalidOrExpired,
    #[error("working-paper export store is unavailable")]
    Unavailable,
}

impl WorkingPaperExportStore {
    /// Replaces every older approval for `company_revocation_key` with the source from
    /// the newest completed read. Passing `None` still revokes the older
    /// approval: a partial or otherwise ineligible refresh must not leave a
    /// now-hidden snapshot exportable by a stale webview capability.
    pub fn replace_for_company(
        &self,
        company_revocation_key: &str,
        source: Option<OutstandingsWorkingPaperSource>,
    ) -> Result<Option<String>, WorkingPaperExportStoreError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| WorkingPaperExportStoreError::Unavailable)?;
        let now = Instant::now();
        entries.retain(|entry| {
            entry.expires_at > now && entry.revocation_key != company_revocation_key
        });
        let Some(source) = source else {
            return Ok(None);
        };
        while entries.len() >= MAX_STORED_EXPORTS {
            entries.pop_front();
        }
        let id = Uuid::new_v4().to_string();
        entries.push_back(StoredExport {
            id: id.clone(),
            expires_at: now + EXPORT_HANDLE_TTL,
            revocation_key: company_revocation_key.to_string(),
            source,
        });
        Ok(Some(id))
    }

    pub fn take(
        &self,
        id: &str,
    ) -> Result<OutstandingsWorkingPaperSource, WorkingPaperExportStoreError> {
        if id.len() > 64 || Uuid::parse_str(id).is_err() {
            return Err(WorkingPaperExportStoreError::InvalidOrExpired);
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| WorkingPaperExportStoreError::Unavailable)?;
        let now = Instant::now();
        entries.retain(|entry| entry.expires_at > now);
        let position = entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(WorkingPaperExportStoreError::InvalidOrExpired)?;
        entries
            .remove(position)
            .map(|entry| entry.source)
            .ok_or(WorkingPaperExportStoreError::InvalidOrExpired)
    }
}

/// Copies a working-paper source only after the completed result passes
/// cardinality, source-byte, and text-size budgets. `None` is the legacy path,
/// whose missing unallocated control cannot substantiate an all-party paper.
pub fn source_from_complete_result(
    result: &OutstandingsLoadResult,
    company_guid: &str,
) -> Result<Option<OutstandingsWorkingPaperSource>, WorkingPaperExportStoreError> {
    let OutstandingsLoadResult::Complete {
        report,
        currency_assertion,
        ageing_anchor,
        synced_at_unix_ms,
        unallocated_total,
        statement_unallocated_by_party,
        statement_open_bills,
        ..
    } = result
    else {
        return Ok(None);
    };
    let Some(unallocated_total) = unallocated_total else {
        return Ok(None);
    };
    if statement_open_bills.len() > MAX_BILL_ROWS
        || statement_unallocated_by_party.len() > MAX_UNALLOCATED_ROWS
        || report.source_bytes > MAX_SOURCE_BYTES
        || company_guid.len() > MAX_CELL_TEXT_BYTES
        || report.company_name.len() > MAX_CELL_TEXT_BYTES
    {
        return Err(WorkingPaperExportStoreError::ResourceLimit);
    }
    let mut aggregate_text_bytes = company_guid
        .len()
        .checked_add(report.company_name.len())
        .ok_or(WorkingPaperExportStoreError::ResourceLimit)?;
    for row in statement_open_bills {
        if row.party.len() > MAX_CELL_TEXT_BYTES || row.reference.len() > MAX_CELL_TEXT_BYTES {
            return Err(WorkingPaperExportStoreError::ResourceLimit);
        }
        aggregate_text_bytes = aggregate_text_bytes
            .checked_add(row.party.len())
            .and_then(|value| value.checked_add(row.reference.len()))
            .ok_or(WorkingPaperExportStoreError::ResourceLimit)?;
    }
    for row in statement_unallocated_by_party {
        if row.party.len() > MAX_CELL_TEXT_BYTES {
            return Err(WorkingPaperExportStoreError::ResourceLimit);
        }
        aggregate_text_bytes = aggregate_text_bytes
            .checked_add(row.party.len())
            .ok_or(WorkingPaperExportStoreError::ResourceLimit)?;
    }
    if aggregate_text_bytes > MAX_AGGREGATE_TEXT_BYTES {
        return Err(WorkingPaperExportStoreError::ResourceLimit);
    }

    Ok(Some(OutstandingsWorkingPaperSource {
        company: report.company_name.clone(),
        company_guid: company_guid.to_string(),
        as_of_yyyymmdd: report.as_of_yyyymmdd.clone(),
        currency_assertion: *currency_assertion,
        synced_at_unix_ms: *synced_at_unix_ms,
        source_bytes: report.source_bytes,
        source_ageing_anchor: *ageing_anchor,
        receivable_bill_total: report.receivable_total.clone(),
        payable_bill_total: report.payable_total.clone(),
        unallocated_total: unallocated_total.clone(),
        open_bills: statement_open_bills.clone(),
        unallocated_by_party: statement_unallocated_by_party.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_tally_core::ExactDecimal;
    use bridge_tally_protocol::outstandings_shared::{
        AgeingBillCounts, AgeingBuckets, OutstandingsReport,
    };

    fn zero_complete(source_bytes: usize, unallocated_known: bool) -> OutstandingsLoadResult {
        OutstandingsLoadResult::Complete {
            report: Box::new(OutstandingsReport {
                company_name: "Synthetic Books".to_string(),
                as_of_yyyymmdd: "20260825".to_string(),
                receivable_total: ExactDecimal::zero(),
                payable_total: ExactDecimal::zero(),
                has_unaged_receivable: false,
                ageing: AgeingBuckets {
                    days_0_30: ExactDecimal::zero(),
                    days_31_60: ExactDecimal::zero(),
                    days_61_90: ExactDecimal::zero(),
                    days_90_plus: ExactDecimal::zero(),
                },
                open_receivable_bill_count: 0,
                ageing_bill_counts: AgeingBillCounts {
                    days_0_30: 0,
                    days_31_60: 0,
                    days_61_90: 0,
                    days_90_plus: 0,
                },
                top_parties: Vec::new(),
                source_voucher_count: 0,
                source_bytes,
            }),
            read_strategy: crate::tally::OutstandingsReadStrategy::NativeBills,
            currency_assertion: crate::tally::OutstandingsCurrencyAssertion::Inr,
            ageing_anchor: crate::tally::OutstandingsAgeingAnchor::DueDate,
            synced_at_unix_ms: 1,
            unallocated_total: unallocated_known.then(ExactDecimal::zero),
            statement_unallocated_by_party: Vec::new(),
            statement_open_bills: Vec::new(),
        }
    }

    #[test]
    fn handle_is_one_use_and_rejects_forgery() {
        let store = WorkingPaperExportStore::default();
        let source = OutstandingsWorkingPaperSource {
            company: "Synthetic Books".to_string(),
            company_guid: "synthetic-guid".to_string(),
            as_of_yyyymmdd: "20260825".to_string(),
            currency_assertion: crate::tally::OutstandingsCurrencyAssertion::Inr,
            synced_at_unix_ms: 1,
            source_bytes: 1,
            source_ageing_anchor: crate::tally::OutstandingsAgeingAnchor::DueDate,
            receivable_bill_total: bridge_tally_core::ExactDecimal::zero(),
            payable_bill_total: bridge_tally_core::ExactDecimal::zero(),
            unallocated_total: bridge_tally_core::ExactDecimal::zero(),
            open_bills: Vec::new(),
            unallocated_by_party: Vec::new(),
        };
        let id = store
            .replace_for_company("synthetic-guid", Some(source))
            .expect("handle issued")
            .expect("source produces a handle");
        assert!(store.take(&id).is_ok());
        assert_eq!(
            store.take(&id).unwrap_err(),
            WorkingPaperExportStoreError::InvalidOrExpired
        );
        assert_eq!(
            store.take("not-an-id").unwrap_err(),
            WorkingPaperExportStoreError::InvalidOrExpired
        );
    }

    #[test]
    fn a_new_company_snapshot_revokes_its_superseded_handle() {
        let store = WorkingPaperExportStore::default();
        let first_source = source_from_complete_result(&zero_complete(1, true), "synthetic-guid")
            .unwrap()
            .unwrap();
        let first = store
            .replace_for_company("synthetic-guid", Some(first_source))
            .unwrap()
            .unwrap();

        let replacement_source =
            source_from_complete_result(&zero_complete(2, true), "synthetic-guid")
                .unwrap()
                .unwrap();
        let replacement = store
            .replace_for_company("synthetic-guid", Some(replacement_source))
            .unwrap()
            .unwrap();

        assert_ne!(first, replacement);
        assert_eq!(
            store.take(&first).unwrap_err(),
            WorkingPaperExportStoreError::InvalidOrExpired
        );
        assert!(store.take(&replacement).is_ok());
    }

    #[test]
    fn an_ineligible_refresh_revokes_the_previous_company_snapshot() {
        let store = WorkingPaperExportStore::default();
        let source = source_from_complete_result(&zero_complete(1, true), "synthetic-guid")
            .unwrap()
            .unwrap();
        let id = store
            .replace_for_company("synthetic-guid", Some(source))
            .unwrap()
            .unwrap();

        assert_eq!(
            store.replace_for_company("synthetic-guid", None).unwrap(),
            None
        );
        assert_eq!(
            store.take(&id).unwrap_err(),
            WorkingPaperExportStoreError::InvalidOrExpired
        );
    }

    #[test]
    fn composite_refresh_revokes_a_prior_export_even_when_the_source_keeps_the_raw_guid() {
        let store = WorkingPaperExportStore::default();
        let source = source_from_complete_result(&zero_complete(1, true), "raw-tally-guid")
            .unwrap()
            .unwrap();
        let id = store
            .replace_for_company("opaque-composite-company-key", Some(source))
            .unwrap()
            .unwrap();

        assert_eq!(
            store
                .replace_for_company("opaque-composite-company-key", None)
                .unwrap(),
            None
        );
        assert_eq!(
            store.take(&id).unwrap_err(),
            WorkingPaperExportStoreError::InvalidOrExpired
        );
    }

    #[test]
    fn zero_row_native_result_gets_a_source_but_legacy_result_does_not() {
        assert!(
            source_from_complete_result(&zero_complete(1, true), "synthetic-guid")
                .unwrap()
                .is_some()
        );
        assert!(
            source_from_complete_result(&zero_complete(1, false), "synthetic-guid")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn source_budget_is_enforced_before_rows_are_cloned() {
        assert_eq!(
            source_from_complete_result(
                &zero_complete(MAX_SOURCE_BYTES + 1, true),
                "synthetic-guid"
            )
            .unwrap_err(),
            WorkingPaperExportStoreError::ResourceLimit
        );
    }

    #[test]
    fn command_response_flattens_the_opaque_handle_into_the_existing_shape() {
        let response = crate::commands::FetchOutstandingsResponse {
            result: zero_complete(1, true),
            working_paper_export_id: Some("synthetic-handle".to_string()),
            working_paper_unavailable_reason_code: None,
        };
        let json = serde_json::to_value(response).expect("response serializes");
        assert_eq!(json["state"], "complete");
        assert_eq!(json["working_paper_export_id"], "synthetic-handle");
        assert_eq!(json["report"]["company_name"], "Synthetic Books");
    }
}
