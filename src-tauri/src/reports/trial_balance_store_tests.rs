use bridge_tally_core::TallyDate;
use bridge_tally_protocol::{
    native_outstandings::CompanyCurrency, native_trial_balance::parse_native_trial_balance,
};

use super::*;

fn captured_read(name: &str) -> TrialBalanceRead {
    let report = parse_native_trial_balance(
        include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
        ),
        "eebb9a9f-1679-4468-9e8f-814c729674cb",
    )
    .unwrap();
    TrialBalanceRead {
        company_guid: "eebb9a9f-1679-4468-9e8f-814c729674cb".into(),
        company_name: name.into(),
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260902").unwrap(),
        currency: CompanyCurrency {
            symbol: "₹".into(),
            mailing_name: "INR".into(),
            currency_count: 1,
            decimal_places: 2,
            is_inr: true,
        },
        totals: crate::reports::trial_balance::observed_totals(&report).unwrap(),
        report,
        read_at: "2026-09-08T00:00:00Z".into(),
        evidence: crate::tally::runtime::RuntimeReadEvidence {
            request_sha256: "a".repeat(64),
            response_sha256: "b".repeat(64),
            bytes: 42,
        },
    }
}

#[test]
fn replacement_invalidates_old_handle_without_wrong_handle_revoking_new_read() {
    let store = TrialBalanceExportStore::default();
    let old = store.insert(captured_read("old")).unwrap();
    let current = store.insert(captured_read("current")).unwrap();
    assert_eq!(
        store.get(&old).unwrap_err(),
        TrialBalanceExportStoreError::InvalidOrExpired
    );
    assert_eq!(store.get(&current).unwrap().company_name, "current");
}

#[test]
fn current_capture_reports_its_bounded_remaining_lifetime() {
    let store = TrialBalanceExportStore::default();
    let id = store.insert(captured_read("current")).unwrap();
    let capture = store.get_capture(&id).unwrap();
    assert!(!capture.expires_in.is_zero());
    assert!(capture.expires_in <= TTL);
}

#[test]
fn expiry_is_deterministic_and_clears_the_stored_read() {
    let store = TrialBalanceExportStore::default();
    let id = store.insert(captured_read("expired")).unwrap();
    store.entry.lock().unwrap().as_mut().unwrap().expires_at =
        Instant::now() - Duration::from_secs(1);
    assert_eq!(
        store.get(&id).unwrap_err(),
        TrialBalanceExportStoreError::InvalidOrExpired
    );
    assert_eq!(
        store.get(&id).unwrap_err(),
        TrialBalanceExportStoreError::InvalidOrExpired
    );
}

#[test]
fn overbudget_source_is_not_issued_a_handle() {
    let mut read = captured_read("overbudget");
    read.evidence.bytes = MAX_SOURCE_BYTES + 1;
    assert_eq!(
        TrialBalanceExportStore::default().insert(read).unwrap_err(),
        TrialBalanceExportStoreError::ResourceLimit
    );
}
