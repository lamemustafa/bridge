/// `tally_runtime_command_error` classifies by substring-matching the
/// *top-level* message, so any error wrapper added anywhere upstream can
/// silently rewrite how unrelated readers classify. This pins the one
/// property that makes `PairedNativeReportResponseFailure` safe to add: it
/// is transparent, so the code is identical with and without it.
///
/// Without transparency a stage-naming message would contain "report",
/// whose "port" substring routes to `endpoint_configuration_invalid` --
/// blaming the endpoint configuration for a dropped connection.
#[test]
fn a_paired_report_response_marker_does_not_change_the_command_code() {
    use crate::tally::connection::PairedNativeReportResponseFailure;
    use bridge_tally_transport::TallyTransportError;

    for variant in [
        TallyTransportError::ConnectionFailed,
        TallyTransportError::RequestTimedOut,
        TallyTransportError::ResponseTooLarge {
            limit: 1,
            declared_by_peer: true,
        },
    ] {
        let bare = super::tally_runtime_command_error(anyhow::Error::new(variant.clone()));
        let tagged = super::tally_runtime_command_error(anyhow::Error::new(
            PairedNativeReportResponseFailure::new(anyhow::Error::new(variant.clone())),
        ));
        assert_eq!(
            tagged.code, bare.code,
            "the marker must not change how {variant:?} classifies"
        );
        assert_ne!(
            tagged.code, "endpoint_configuration_invalid",
            "a transport fault must not be blamed on the endpoint configuration"
        );
    }
}

use super::all_clients::{
    load_client_group_labels_for_migration, prepare_client_group_label_migration_from_labels,
    ClientGroupLabelMigrationPreparationError,
};
use super::{
    company_sweep_currency_preflight_failure, company_sweep_result, establish_inr_currency,
    first_calendar_day_canary_window, party_ledger_master_currency_admission_error,
    party_ledger_master_runtime_command_error, portable_export_file_name, reconcile_review_cleanup,
    reviewed_probe_commitment_sha256, tally_command_error, tally_runtime_command_error,
    verify_observed_company_tuple_from_companies, write_unique_download, CompanySweepFailure,
    OutstandingsRequest, PersistedTallyCompany, SavedTallySetup, SelectedCompanyIdentity,
    VerifiedCompanyIdentity,
};
// Used only by the `#[cfg(unix)]` non-UTF-8 destination test — an invalid-byte
// path cannot be constructed portably. The import must carry the same gate as
// the test, or Windows fails on an unused import under `-D warnings`.
#[cfg(unix)]
use super::require_utf8_destination;
use crate::tally::{
    ConnectionStatus, OutstandingsCurrencyAssertion, OutstandingsLoadResult, TallyCompany,
    TallyLedger, TallyProbeResult, TallyProduct,
};
use bridge_tally_core::CapabilityProfile;
use bridge_tally_protocol::PartyLedgerMasterFieldObservation;
use std::collections::BTreeMap;

#[test]
fn migration_loader_returns_a_typed_error_for_unreadable_labels() {
    let directory = tempfile::tempdir().expect("temporary config directory");
    std::fs::create_dir(directory.path().join("client-group-labels-v1.json"))
        .expect("unreadable label path");

    assert!(matches!(
        load_client_group_labels_for_migration(directory.path()),
        Err(ClientGroupLabelMigrationPreparationError::LabelsUnavailable)
    ));
}

#[tokio::test]
async fn empty_label_migration_plan_never_opens_the_mirror() {
    let mirror_opened = std::sync::atomic::AtomicBool::new(false);

    let plan = prepare_client_group_label_migration_from_labels(BTreeMap::new(), |_| async {
        mirror_opened.store(true, std::sync::atomic::Ordering::SeqCst);
        Err(ClientGroupLabelMigrationPreparationError::MirrorUnavailable)
    })
    .await
    .expect("empty labels require no mirror");

    assert!(plan.entries.is_empty());
    assert!(!mirror_opened.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn same_guid_case_or_whitespace_sibling_is_not_a_safe_scope() {
    let identity = VerifiedCompanyIdentity::test_fixture("Client Book", "same-guid");
    let sibling = TallyCompany {
        name: " client book ".to_string(),
        guid: Some("SAME-GUID".to_string()),
        company_number: Some("2".to_string()),
        books_from: Some("20270401".to_string()),
    };

    assert!(identity.is_presentation_equivalent_guid_sibling(&sibling));
    assert!(!identity.matches_observed_company(&sibling));
}

#[test]
fn setup_rejects_presentation_equivalent_guid_siblings() {
    let selected = SelectedCompanyIdentity {
        display_name: "Client Book".to_string(),
        company_guid: "same-guid".to_string(),
        company_number: "1".to_string(),
        books_from_yyyymmdd: "20260401".to_string(),
    };
    let error = verify_observed_company_tuple_from_companies(
        &selected,
        vec![
            TallyCompany {
                name: "Client Book".to_string(),
                guid: Some("same-guid".to_string()),
                company_number: Some("1".to_string()),
                books_from: Some("20260401".to_string()),
            },
            TallyCompany {
                name: " client book ".to_string(),
                guid: Some("SAME-GUID".to_string()),
                company_number: Some("2".to_string()),
                books_from: Some("20270401".to_string()),
            },
        ],
    )
    .expect_err("presentation-equivalent company sibling must not be saved");

    assert_eq!(error.code, "company_identity_display_scope_ambiguous");
}

#[test]
fn setup_rejects_identical_name_same_guid_different_book() {
    let selected = SelectedCompanyIdentity {
        display_name: "Client Book".to_string(),
        company_guid: "same-guid".to_string(),
        company_number: "1".to_string(),
        books_from_yyyymmdd: "20260401".to_string(),
    };
    let error = verify_observed_company_tuple_from_companies(
        &selected,
        vec![
            TallyCompany {
                name: "Client Book".to_string(),
                guid: Some("same-guid".to_string()),
                company_number: Some("1".to_string()),
                books_from: Some("20260401".to_string()),
            },
            TallyCompany {
                name: "Client Book".to_string(),
                guid: Some("same-guid".to_string()),
                company_number: Some("2".to_string()),
                books_from: Some("20270401".to_string()),
            },
        ],
    )
    .expect_err("an identically named sibling must not be saved or enrolled");

    assert_eq!(error.code, "company_identity_display_scope_ambiguous");
}

#[test]
fn setup_reports_duplicate_complete_tuple_separately() {
    let selected = SelectedCompanyIdentity {
        display_name: "Client Book".to_string(),
        company_guid: "same-guid".to_string(),
        company_number: "1".to_string(),
        books_from_yyyymmdd: "20260401".to_string(),
    };
    let duplicate = TallyCompany {
        name: "Client Book".to_string(),
        guid: Some("same-guid".to_string()),
        company_number: Some("1".to_string()),
        books_from: Some("20260401".to_string()),
    };
    let error =
        verify_observed_company_tuple_from_companies(&selected, vec![duplicate.clone(), duplicate])
            .expect_err("a duplicate complete tuple must not be saved or enrolled");

    assert_eq!(error.code, "company_identity_ambiguous");
}

/// Regression for the destination-picker leak: `select_party_statement_
/// destination` used to build the folder's IPC string with
/// `to_string_lossy()`, which replaces invalid UTF-8 byte sequences with
/// U+FFFD -- silently turning the folder the operator picked into a
/// *different* path that likely doesn't exist. This failed before the
/// fix because the old conversion never returned an `Err` at all: it
/// always produced a (possibly wrong) string.
#[cfg(unix)]
#[test]
fn require_utf8_destination_rejects_non_utf8_paths_instead_of_rewriting_them() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // 0xFF is never valid UTF-8 on its own, so this cannot be constructed
    // as a Rust string literal -- it has to come in through the raw byte
    // API a real OS path could actually hand back.
    let invalid_bytes = [b'p', b'i', b'c', b'k', 0xFF, b'e', b'd'];
    let path = std::path::PathBuf::from(OsStr::from_bytes(&invalid_bytes));

    let error =
        require_utf8_destination(path).expect_err("non-UTF-8 folder names must be rejected");

    assert!(
        !error.contains('\u{FFFD}'),
        "must not silently substitute a replacement character: {error}"
    );
    assert!(error.to_lowercase().contains("unicode"));
}

#[test]
fn outstandings_accepts_only_an_explicit_inr_currency_assertion() {
    let accepted: OutstandingsRequest = serde_json::from_value(serde_json::json!({
        "config": { "host": "127.0.0.1", "port": 9000 },
        "selected_company": {
            "display_name": "Synthetic Company",
            "company_guid": "synthetic-guid",
            "company_number": "100001",
            "books_from_yyyymmdd": "20260401"
        },
        "currency_assertion": "INR"
    }))
    .expect("INR is the one supported explicit assertion");
    assert_eq!(
        accepted.currency_assertion,
        OutstandingsCurrencyAssertion::Inr
    );

    let rejected = serde_json::from_value::<OutstandingsRequest>(serde_json::json!({
        "config": { "host": "127.0.0.1", "port": 9000 },
        "selected_company": {
            "display_name": "Synthetic Company",
            "company_guid": "synthetic-guid",
            "company_number": "100001",
            "books_from_yyyymmdd": "20260401"
        },
        "currency_assertion": "USD"
    }));
    assert!(
        rejected.is_err(),
        "unsupported currencies must not start a scan"
    );
}

#[test]
fn company_sweep_keeps_probe_and_read_failures_in_band() {
    let outcomes = [
        Ok(OutstandingsLoadResult::Partial {
            reason: crate::tally::OutstandingsPartialReason::code("first_book_partial"),
            synced_at_unix_ms: 1,
        }),
        Err(CompanySweepFailure::ReasonCode(
            "company_currency_probe_failed",
        )),
        Err(CompanySweepFailure::ReasonCode(
            "company_base_currency_undetermined",
        )),
        Err(CompanySweepFailure::ReasonCode(
            "company_outstandings_read_failed",
        )),
        Ok(OutstandingsLoadResult::Partial {
            reason: crate::tally::OutstandingsPartialReason::code("last_book_partial"),
            synced_at_unix_ms: 2,
        }),
    ]
    .into_iter()
    .map(company_sweep_result)
    .collect::<Vec<_>>();

    assert_eq!(
        outcomes.len(),
        5,
        "one bad book must not truncate the sweep"
    );
    assert!(matches!(
        &outcomes[1],
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "company_currency_probe_failed"
    ));
    assert!(matches!(
        &outcomes[2],
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "company_base_currency_undetermined"
    ));
    assert!(matches!(
        &outcomes[3],
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "company_outstandings_read_failed"
    ));
    assert!(matches!(
        &outcomes[4],
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "last_book_partial"
    ));
}

#[test]
fn company_sweep_preserves_a_company_listing_transport_reason() {
    let result = company_sweep_result(Err(CompanySweepFailure::CompanyVerification(
        tally_command_error(
            "endpoint_unreachable",
            "Endpoint configuration",
            "Synthetic transport failure",
            "after_change",
            false,
            "Restore connectivity.",
        ),
    )));

    assert!(matches!(
        result,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "endpoint_unreachable"
    ));
}

#[test]
fn company_sweep_currency_preflight_names_undetermined_base_currency() {
    use bridge_tally_protocol::native_outstandings::{CompanyCurrency, CurrencyMaster};
    let master = |name: &str, original: &str, mailing: &str| CurrencyMaster {
        name: name.to_string(),
        original_name: Some(original.to_string()),
        mailing_name: mailing.to_string(),
        decimal_places: 2,
    };
    let two =
        || CompanyCurrency::from_masters(vec![master("$", "$", "USD"), master("I₹", "₹", "INR")]);
    assert_eq!(
        company_sweep_currency_preflight_failure(&two()),
        Some("company_base_currency_undetermined"),
        "several currency masters alone do not identify the company's base currency"
    );
    let mut flagged = two();
    flagged.is_inr = true;
    assert_eq!(
        company_sweep_currency_preflight_failure(&flagged),
        Some("company_base_currency_undetermined"),
        "an INR flag is not authoritative while the base is undetermined"
    );
    assert_eq!(
        company_sweep_currency_preflight_failure(&two().with_company_currency_name("₹")),
        None,
        "the company's CURRENCYNAME identifies its INR base among several masters"
    );
    assert_eq!(
        company_sweep_currency_preflight_failure(&two().with_company_currency_name("$")),
        Some("company_base_currency_not_inr"),
        "an identified non-INR base is unsupported"
    );
    assert_eq!(
        company_sweep_currency_preflight_failure(&CompanyCurrency::from_masters(vec![master(
            "$",
            "$",
            "US Dollar"
        )])),
        Some("company_base_currency_not_inr"),
        "one non-Indian currency identifies an unsupported base currency"
    );
    let inr = CompanyCurrency::from_masters(vec![master("I₹", "₹", "INR")]);
    assert_eq!(company_sweep_currency_preflight_failure(&inr), None);
    assert_eq!(
        establish_inr_currency(inr.inr_admission()),
        Ok(OutstandingsCurrencyAssertion::Inr),
        "the backend boundary receives a typed INR admission only after the probe established an INR base"
    );
    assert_eq!(
        company_sweep_currency_preflight_failure(&CompanyCurrency::from_masters(Vec::new())),
        Some("company_currency_probe_failed"),
        "an impossible empty collection remains fail-closed"
    );
}

#[test]
fn company_sweep_preserves_runtime_foreign_currency_partial() {
    let outcome = company_sweep_result(Ok(OutstandingsLoadResult::Partial {
        reason: crate::tally::OutstandingsPartialReason::foreign_currency_ledger_balance(
            "Synthetic FX Debtor".to_string(),
        ),
        synced_at_unix_ms: 1,
    }));

    assert!(matches!(
        outcome,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "company_foreign_currency_ledger_balance"
                && reason.foreign_currency_ledger_name.as_deref() == Some("Synthetic FX Debtor")
    ));
}

#[test]
fn export_names_are_portable_and_reserved_devices_are_neutralized() {
    assert_eq!(
        portable_export_file_name("outstandings-A:B?C.csv").unwrap(),
        "outstandings-A-B-C.csv"
    );
    assert_eq!(portable_export_file_name("CON.csv").unwrap(), "_CON.csv");
    assert!(portable_export_file_name("../outside.csv").is_err());
    assert!(portable_export_file_name("nested/report.csv").is_err());
}

#[test]
fn report_downloads_never_overwrite_an_existing_file() {
    let directory = tempfile::tempdir().expect("temporary download directory");
    let first = write_unique_download(directory.path(), "report.csv", b"first").unwrap();
    let second = write_unique_download(directory.path(), "report.csv", b"second").unwrap();

    assert_eq!(first.file_name().unwrap(), "report.csv");
    assert_eq!(second.file_name().unwrap(), "report-2.csv");
    assert_eq!(std::fs::read(first).unwrap(), b"first");
    assert_eq!(std::fs::read(second).unwrap(), b"second");
}

#[test]
fn snapshot_capability_canary_is_exactly_the_requested_first_calendar_day() {
    let canary = first_calendar_day_canary_window("20260228").unwrap();
    assert_eq!(canary.range.from_yyyymmdd, "20260228");
    assert_eq!(canary.range.to_yyyymmdd, "20260228");
    assert_eq!(canary.query_profile.as_str(), "core_accounting_v3");

    let same = first_calendar_day_canary_window("20260228").unwrap();
    assert_eq!(same, canary);
    assert!(first_calendar_day_canary_window("20260229").is_err());
    assert!(first_calendar_day_canary_window("2026-02-28").is_err());
}

#[test]
fn tally_runtime_error_serialization_is_stable_and_redacted() {
    let error = tally_runtime_command_error(anyhow::anyhow!(
        "synthetic reqwest failure at http://127.0.0.1:9000/?token=private"
    ));
    let json = serde_json::to_string(&error).expect("serialize safe Tally command error");
    assert_eq!(error.code, "endpoint_unreachable");
    assert_eq!(error.category, "Endpoint configuration");
    assert!(error.local_state_changed);
    assert!(!error.tally_state_may_have_changed);
    assert!(!json.contains("token=private"));
    assert!(!json.contains("reqwest"));

    let invalid_config =
        tally_runtime_command_error(anyhow::anyhow!("Tally port must be between 1 and 65535"));
    assert_eq!(invalid_config.code, "endpoint_configuration_invalid");
    assert!(!invalid_config.local_state_changed);

    let queue_deadline =
        tally_runtime_command_error(anyhow::anyhow!("endpoint queue deadline exceeded"));
    assert_eq!(queue_deadline.code, "tally_runtime_temporarily_unavailable");
    assert!(queue_deadline.local_state_changed);

    let deadline = tally_runtime_command_error(
        anyhow::Error::new(bridge_tally_transport::TallyTransportError::RequestTimedOut)
            .context("outstandings second segment read failed for 20251002..20251101"),
    );
    assert_eq!(deadline.code, "tally_request_deadline_exceeded");
    assert_eq!(deadline.retry, "after_change");
    assert!(deadline.remediation.contains("Do not retry"));

    let discovery_limit = tally_runtime_command_error(anyhow::anyhow!(
        "interactive discovery listing limit exceeded: synthetic company response"
    ));
    assert_eq!(discovery_limit.code, "untrusted_discovery_limit_exceeded");
    assert_eq!(discovery_limit.category, "Discovery listing");
}

#[test]
fn opening_balance_source_disagreement_is_response_validation_not_endpoint_failure() {
    let error = tally_runtime_command_error(anyhow::Error::new(
        crate::tally::connection::PartyLedgerMasterSourceValidationError::OpeningBalancesDisagreed,
    ));

    assert_eq!(error.code, "response_validation_failed");
    assert_eq!(error.category, "Response validation");
    assert_ne!(error.code, "endpoint_unreachable");
}

#[test]
fn every_party_master_source_validation_is_not_an_endpoint_failure() {
    use crate::tally::connection::PartyLedgerMasterSourceValidationError as Validation;

    for error in [
        Validation::MasterPeriod,
        Validation::BalancePeriod,
        Validation::MasterGuid,
        Validation::MasterId,
        Validation::MasterAlterId,
        Validation::MasterOpeningBalance,
        Validation::DuplicateMasterIdentity,
        Validation::BalanceMissingMasterLedger,
        Validation::OpeningBalancesDisagreed,
        Validation::BalanceLedgerAbsentFromMasterEvidence,
        Validation::DuplicateBalanceDisplayKey,
        Validation::BalanceCompanyIdentityUnverified,
        Validation::GroupCompanyIdentityUnverified,
        Validation::MasterResponseInvalid {
            source: anyhow::anyhow!("synthetic parser failure"),
        },
    ] {
        let mapped = tally_runtime_command_error(anyhow::Error::new(error));
        assert_eq!(mapped.code, "response_validation_failed");
        assert_eq!(mapped.category, "Response validation");
        assert_ne!(mapped.code, "endpoint_unreachable");
    }
}

#[test]
fn retained_wire_evidence_does_not_hide_typed_command_refusals() {
    use crate::tally::runtime::{with_read_evidence, RuntimeReadEvidence};
    for (source, code) in [
        (anyhow::Error::new(crate::tally::runtime::OpeningBoundaryObservationError::Unobserved), "response_validation_failed"),
        (anyhow::Error::new(crate::tally::runtime::OpeningBoundaryObservationError::Changed), "response_validation_failed"),
        (anyhow::Error::new(crate::tally::connection::PartyLedgerMasterSourceValidationError::OpeningBalancesDisagreed), "response_validation_failed"),
        (anyhow::Error::new(crate::tally::connection::PairedReadValidationError::PartyLedgerMaster), "response_validation_failed"),
        (anyhow::Error::new(crate::tally::runtime::TallyRuntimeControlError::QueueDeadline), "tally_runtime_temporarily_unavailable"),
    ] {
        let wrapped = with_read_evidence(source, RuntimeReadEvidence::empty());
        let mapped = tally_runtime_command_error(wrapped);
        assert_eq!(mapped.code, code);
        assert!(!mapped.message.contains("opening balances disagreed"));
    }
}

#[test]
fn drifted_party_master_read_is_response_validation_not_endpoint_failure() {
    let error = tally_runtime_command_error(anyhow::Error::new(
        crate::tally::connection::PairedReadValidationError::PartyLedgerMaster,
    ));

    assert_eq!(error.code, "response_validation_failed");
    assert_eq!(error.category, "Response validation");
    assert_ne!(error.code, "endpoint_unreachable");
}

#[test]
fn party_master_export_keeps_runtime_control_error_code_and_hides_runtime_detail() {
    let error = party_ledger_master_runtime_command_error(anyhow::Error::new(
        crate::tally::runtime::TallyRuntimeControlError::QueueDeadline,
    ));

    assert_eq!(error.code, "tally_runtime_temporarily_unavailable");
    assert_eq!(error.retry, "safe");
    assert!(error.local_state_changed);
    assert!(error
        .message
        .starts_with("Bridge withheld the party/ledger master:"));
    assert!(!error.message.contains("QueueDeadline"));
}

#[test]
fn party_master_currency_admission_does_not_misdiagnose_multiple_masters() {
    let error = party_ledger_master_currency_admission_error("company_base_currency_undetermined");

    assert_eq!(error.code, "company_base_currency_undetermined");
    assert!(error.message.contains("multiple Currency masters"));
    assert!(error
        .message
        .contains("could not match exactly one of them to the selected company's base currency"));
    assert!(!error.message.contains("more than one base currency"));
    assert!(error
        .remediation
        .contains("no operator confirmation can make this read safe"));
    assert!(error
        .remediation
        .contains("Do not retry the unchanged export"));
}

#[test]
fn party_master_currency_admission_does_not_direct_an_inr_retry_for_non_inr_company() {
    let error = party_ledger_master_currency_admission_error("company_base_currency_not_inr");

    assert_eq!(error.code, "company_base_currency_not_inr");
    assert!(error.remediation.contains("select a company"));
    assert!(error
        .remediation
        .contains("A confirmation cannot change an unsupported base currency"));
}

#[test]
fn shared_ledger_command_result_keeps_legacy_nullable_observation_json() {
    let result = vec![
        TallyLedger {
            name: "Returned ledger".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::Returned("29ABCDE1234F1Z5".to_string()),
            opening_balance: Some("0".to_string()),
        },
        TallyLedger {
            name: "Empty ledger".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned(String::new()),
            party_gstin: PartyLedgerMasterFieldObservation::Returned(String::new()),
            opening_balance: None,
        },
        TallyLedger {
            name: "Unobserved ledger".to_string(),
            parent: PartyLedgerMasterFieldObservation::NotObserved,
            party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
            opening_balance: None,
        },
    ];

    assert_eq!(
        serde_json::to_value(result).expect("command result serializes"),
        serde_json::json!([
            {
                "name": "Returned ledger",
                "parent": "Sundry Debtors",
                "party_gstin": "29ABCDE1234F1Z5",
                "opening_balance": "0",
            },
            {
                "name": "Empty ledger",
                "parent": "",
                "party_gstin": "",
                "opening_balance": null,
            },
            {
                "name": "Unobserved ledger",
                "parent": null,
                "party_gstin": null,
                "opening_balance": null,
            },
        ])
    );
}

#[test]
fn explicit_tally_error_preserves_atomic_failure_truth() {
    let error = tally_command_error(
        "reviewed_setup_store_failed",
        "Operation",
        "Synthetic reviewed setup was not stored",
        "after_change",
        false,
        "Inspect local encrypted storage.",
    );
    assert_eq!(error.code, "reviewed_setup_store_failed");
    assert!(!error.local_state_changed);
    assert!(!error.tally_state_may_have_changed);
}

#[test]
fn post_commit_review_cleanup_failure_preserves_durable_success_truth() {
    let saved = SavedTallySetup {
        passport_snapshot_id: "snapshot-1".to_string(),
        canonical_origin: "http://127.0.0.1:9000".to_string(),
        observed_at_unix_ms: 1_000,
        company: PersistedTallyCompany {
            name: "Synthetic Company".to_string(),
            guid: Some("synthetic-guid".to_string()),
            company_number: Some("100001".to_string()),
            books_from_yyyymmdd: Some("20260401".to_string()),
            mirror_company_id: Some("company-1".to_string()),
            correlation_key: Some("c".repeat(64)),
            identity_confidence: "observed",
        },
        review_cleanup_warning: None,
    };
    let result = reconcile_review_cleanup(Ok(saved), false).expect("save stays successful");
    assert_eq!(
        result.review_cleanup_warning,
        Some("review_cache_cleanup_failed_after_save")
    );

    let failed = reconcile_review_cleanup(
        Err(tally_command_error(
            "reviewed_setup_store_failed",
            "Operation",
            "Synthetic failure",
            "after_change",
            false,
            "Retry.",
        )),
        false,
    )
    .expect_err("failed store plus failed cleanup is explicit");
    assert_eq!(failed.code, "reviewed_setup_retry_state_uncertain");
    assert!(failed.local_state_changed);
}

#[test]
fn reviewed_probe_commitment_binds_time_company_name_and_full_company_list() {
    let probe = |names: &[&str]| TallyProbeResult {
        connection: ConnectionStatus {
            reachable: true,
            compatible: false,
            server_text: "Synthetic status".to_string(),
            product: TallyProduct::Unknown,
            error: None,
        },
        companies: names
            .iter()
            .enumerate()
            .map(|(index, name)| TallyCompany {
                name: (*name).to_string(),
                guid: Some(format!("guid-{index}")),
                company_number: Some(format!("1000{index}")),
                books_from: Some("20260401".to_string()),
            })
            .collect(),
        profile: CapabilityProfile {
            profile_version: 2,
            product: "Unknown".to_string(),
            release: None,
            license_tier: None,
            mode: None,
            transports: BTreeMap::new(),
            features: BTreeMap::new(),
            packs: BTreeMap::new(),
        },
        selected_read_scope: None,
        passport_snapshot_id: None,
    };
    let first = reviewed_probe_commitment_sha256(
        "review-a",
        "http://127.0.0.1:9000",
        1_000,
        &probe(&["Synthetic A"]),
    )
    .unwrap();
    let renamed = reviewed_probe_commitment_sha256(
        "review-a",
        "http://127.0.0.1:9000",
        1_000,
        &probe(&["Synthetic Renamed"]),
    )
    .unwrap();
    let expanded = reviewed_probe_commitment_sha256(
        "review-a",
        "http://127.0.0.1:9000",
        1_000,
        &probe(&["Synthetic A", "Synthetic B"]),
    )
    .unwrap();
    let later = reviewed_probe_commitment_sha256(
        "review-a",
        "http://127.0.0.1:9000",
        1_001,
        &probe(&["Synthetic A"]),
    )
    .unwrap();
    assert_ne!(first, renamed);
    assert_ne!(first, expanded);
    assert_ne!(first, later);
    let different_review = reviewed_probe_commitment_sha256(
        "review-b",
        "http://127.0.0.1:9000",
        1_000,
        &probe(&["Synthetic A"]),
    )
    .unwrap();
    assert_ne!(first, different_review);
}
