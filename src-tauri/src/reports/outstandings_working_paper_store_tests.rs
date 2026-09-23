use super::*;
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::outstandings_shared::{
    AgeingBillCounts, AgeingBuckets, OutstandingsReport,
};
use std::sync::Arc;

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

    let replacement_source = source_from_complete_result(&zero_complete(2, true), "synthetic-guid")
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
        source_from_complete_result(&zero_complete(MAX_SOURCE_BYTES + 1, true), "synthetic-guid")
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
        party_statement_source_id: Some("synthetic-statements".to_string()),
    };
    let json = serde_json::to_value(response).expect("response serializes");
    assert_eq!(json["state"], "complete");
    assert_eq!(json["working_paper_export_id"], "synthetic-handle");
    assert_eq!(json["party_statement_source_id"], "synthetic-statements");
    assert_eq!(json["report"]["company_name"], "Synthetic Books");
}

fn synthetic_source() -> OutstandingsWorkingPaperSource {
    OutstandingsWorkingPaperSource {
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
    }
}

/// bridge#551: a statement source serves every party of one read, so reading
/// it leaves it in place; a refresh of the same company revokes it, and a
/// forged handle reads nothing.
#[test]
fn a_statement_source_serves_many_reads_until_a_refresh_revokes_it() {
    let store = PartyStatementSourceStore::default();
    let id = store
        .replace_for_company("synthetic-guid", Some(Arc::new(synthetic_source())))
        .expect("handle issued")
        .expect("source produces a handle");
    for _ in 0..2 {
        assert_eq!(
            store.get(&id).expect("still held").company,
            "Synthetic Books"
        );
    }
    for forged in ["not-an-id", "00000000-0000-4000-8000-000000000009"] {
        assert_eq!(
            store.get(forged).unwrap_err(),
            WorkingPaperExportStoreError::InvalidOrExpired,
            "{forged}"
        );
    }
    store
        .replace_for_company("synthetic-guid", None)
        .expect("refresh without a source");
    assert_eq!(
        store.get(&id).unwrap_err(),
        WorkingPaperExportStoreError::InvalidOrExpired
    );
}

/// A statement handle outlives the working paper's fifteen minutes, but not
/// its own lifetime: an expired handle reads nothing.
#[test]
fn a_statement_source_expires_with_its_own_lifetime() {
    assert!(STATEMENT_SOURCE_TTL > EXPORT_HANDLE_TTL);
    let expired = PartyStatementSourceStore::with_ttl(std::time::Duration::ZERO);
    let id = expired
        .replace_for_company("synthetic-guid", Some(Arc::new(synthetic_source())))
        .expect("handle issued")
        .expect("source produces a handle");
    assert_eq!(
        expired.get(&id).unwrap_err(),
        WorkingPaperExportStoreError::InvalidOrExpired
    );
}
