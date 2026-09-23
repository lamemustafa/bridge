use super::*;

#[test]
fn utf8_picker_destination_round_trips_to_the_authorized_path() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let selected_path = destination.path().to_path_buf();
    let approvals = PartyStatementDestinationApprovals::default();
    let approval_id = approvals
        .issue(selected_path.clone())
        .expect("approve picker destination");
    let ipc_destination =
        require_utf8_destination(selected_path).expect("temporary destination is valid UTF-8");

    let approved = approvals
        .consume(&approval_id, std::path::Path::new(&ipc_destination))
        .expect("IPC path reconstruction retains the selected destination");

    assert_eq!(approved.path(), std::path::Path::new(&ipc_destination));
}

/// A statement source held as `fetch_tally_outstandings` holds one, with a
/// single synthetic bill.
fn statement_sources() -> (
    crate::reports::outstandings_working_paper_store::PartyStatementSourceStore,
    String,
) {
    let sources =
        crate::reports::outstandings_working_paper_store::PartyStatementSourceStore::default();
    let id = sources
        .replace_for_company(
            "synthetic-company",
            Some(std::sync::Arc::new(
                crate::reports::outstandings_working_paper::OutstandingsWorkingPaperSource {
                    company: "Synthetic Books Pvt Ltd".to_string(),
                    company_guid: "synthetic-guid".to_string(),
                    as_of_yyyymmdd: "20260808".to_string(),
                    currency_assertion: crate::tally::OutstandingsCurrencyAssertion::Inr,
                    synced_at_unix_ms: 1,
                    source_bytes: 1,
                    source_ageing_anchor: crate::tally::OutstandingsAgeingAnchor::DueDate,
                    receivable_bill_total: bridge_tally_core::ExactDecimal::parse("100.00")
                        .unwrap(),
                    payable_bill_total: bridge_tally_core::ExactDecimal::zero(),
                    unallocated_total: bridge_tally_core::ExactDecimal::zero(),
                    open_bills: vec![crate::tally::OpenBillRow {
                        party: "Synthetic Party".to_string(),
                        reference: "SYNTHETIC-1".to_string(),
                        bill_date: "20260801".to_string(),
                        due_date: "20260831".to_string(),
                        amount: bridge_tally_core::ExactDecimal::parse("100.00")
                            .expect("synthetic amount"),
                        age_days: Some(7),
                        kind: crate::tally::ExposureDirection::Receivable,
                    }],
                    unallocated_by_party: Vec::new(),
                },
            )),
        )
        .expect("source held")
        .expect("source handle");
    (sources, id)
}

fn bulk_export_request(
    destination: &std::path::Path,
    approval_id: &str,
    source_id: &str,
) -> ExportBulkPartyStatementsRequest {
    ExportBulkPartyStatementsRequest {
        source_id: source_id.to_string(),
        format: PartyStatementFormat::Xlsx,
        destination: destination.to_string_lossy().into_owned(),
        approval_id: approval_id.to_string(),
    }
}

#[test]
fn statement_export_format_defaults_to_xlsx_and_accepts_pdf() {
    let base = serde_json::json!({
        "source_id": "00000000-0000-4000-8000-000000000001",
        "party": "Synthetic Party",
    });
    let defaulted: ExportPartyStatementRequest = serde_json::from_value(base.clone()).unwrap();
    assert!(matches!(defaulted.format, PartyStatementFormat::Xlsx));

    let mut pdf = base;
    pdf["format"] = serde_json::Value::String("pdf".to_string());
    let pdf: ExportPartyStatementRequest = serde_json::from_value(pdf).unwrap();
    assert!(matches!(pdf.format, PartyStatementFormat::Pdf));
}

/// bridge#551: the webview names a server-held source and can no longer
/// supply statement rows, a company, an as-of date or an ageing anchor. A
/// request that still carries any of them is refused at the IPC boundary
/// rather than silently ignored. Replaces two tests of the row-carrying
/// requests: the ageing anchor's default and an unknown bill direction.
#[test]
fn statement_requests_refuse_renderer_supplied_rows() {
    let source_id = "00000000-0000-4000-8000-000000000001";
    // Each request parses without the extra field, so every refusal below is
    // the extra field's.
    serde_json::from_value::<ExportPartyStatementRequest>(
        serde_json::json!({"source_id": source_id, "party": "Synthetic Party"}),
    )
    .unwrap();
    serde_json::from_value::<ExportBulkPartyStatementsRequest>(serde_json::json!({
        "source_id": source_id,
        "format": "xlsx",
        "destination": "/tmp/statements",
        "approval_id": "synthetic-approval",
    }))
    .unwrap();
    serde_json::from_value::<PreviewBulkPartyStatementsRequest>(
        serde_json::json!({"source_id": source_id}),
    )
    .unwrap();
    for extra in [
        ("open_bills", serde_json::json!([])),
        ("unallocated_by_party", serde_json::json!([])),
        ("company", serde_json::json!("Synthetic Books Pvt Ltd")),
        ("as_of_yyyymmdd", serde_json::json!("20260808")),
        ("ageing_anchor", serde_json::json!("due_date")),
    ] {
        let mut single = serde_json::json!({"source_id": source_id, "party": "Synthetic Party"});
        single[extra.0] = extra.1.clone();
        assert!(
            serde_json::from_value::<ExportPartyStatementRequest>(single).is_err(),
            "single {}",
            extra.0
        );
        let mut bulk = serde_json::json!({
            "source_id": source_id,
            "format": "xlsx",
            "destination": "/tmp/statements",
            "approval_id": "synthetic-approval",
        });
        bulk[extra.0] = extra.1.clone();
        assert!(
            serde_json::from_value::<ExportBulkPartyStatementsRequest>(bulk).is_err(),
            "bulk {}",
            extra.0
        );
        let mut preview = serde_json::json!({"source_id": source_id});
        preview[extra.0] = extra.1;
        assert!(
            serde_json::from_value::<PreviewBulkPartyStatementsRequest>(preview).is_err(),
            "preview {}",
            extra.0
        );
    }
}

/// bridge#551: a handle that names no held source writes nothing, and the
/// operator is told to refresh.
#[test]
fn bulk_statement_export_refuses_an_unknown_source_without_writing() {
    let destination = tempfile::tempdir().expect("selected synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let approval_id = approvals
        .issue(destination.path().to_path_buf())
        .expect("record picker destination");
    let (sources, _) = statement_sources();

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(
            destination.path(),
            &approval_id,
            "00000000-0000-4000-8000-000000000009",
        ),
        &approvals,
        &sources,
    )
    .expect_err("an unknown source must be refused");
    assert!(matches!(
        error,
        BulkPartyStatementExportError::Existing(message) if message.contains("Refresh outstandings")
    ));
    assert!(
        std::fs::read_dir(destination.path())
            .expect("destination remains readable")
            .next()
            .is_none(),
        "the refused batch must not write a statement or manifest"
    );
}

#[test]
fn bulk_statement_export_rejects_an_unselected_destination_without_writing() {
    let destination = tempfile::tempdir().expect("unselected synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let (sources, source_id) = statement_sources();

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(destination.path(), "not-issued", &source_id),
        &approvals,
        &sources,
    )
    .expect_err("an unselected destination must be rejected");

    match error {
        BulkPartyStatementExportError::DestinationNotAuthorized(error) => {
            assert_eq!(error.code, "statement_destination_not_authorized");
            assert_eq!(error.retry, "after_change");
            assert!(!error.local_state_changed);
        }
        BulkPartyStatementExportError::Existing(error) => {
            panic!("expected typed destination error, got {error}");
        }
    }
    assert!(
        std::fs::read_dir(destination.path())
            .expect("destination remains readable")
            .next()
            .is_none(),
        "the rejected batch must not write a statement or manifest"
    );
}

#[test]
fn bulk_statement_export_writes_to_the_destination_approved_by_the_picker() {
    let destination = tempfile::tempdir().expect("selected synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let (sources, source_id) = statement_sources();
    let approval_id = approvals
        .issue(destination.path().to_path_buf())
        .expect("record picker destination");

    let result = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(destination.path(), &approval_id, &source_id),
        &approvals,
        &sources,
    )
    .expect("recorded destination is accepted");

    assert_eq!(result.written.len(), 1);
    assert!(std::path::Path::new(&result.manifest_path).is_file());
    assert!(destination
        .path()
        .join(&result.written[0].file_name)
        .is_file());
}

#[test]
fn independent_picker_approvals_export_to_their_own_destinations() {
    let first_destination = tempfile::tempdir().expect("first synthetic destination");
    let second_destination = tempfile::tempdir().expect("second synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let (sources, source_id) = statement_sources();
    // Both selections exist before either export starts; neither replaces
    // the other as the previous singleton store did.
    let first_approval = approvals
        .issue(first_destination.path().to_path_buf())
        .expect("approve first destination");
    let second_approval = approvals
        .issue(second_destination.path().to_path_buf())
        .expect("approve second destination");

    let first = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(first_destination.path(), &first_approval, &source_id),
        &approvals,
        &sources,
    )
    .expect("first approved destination writes");
    let second = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(second_destination.path(), &second_approval, &source_id),
        &approvals,
        &sources,
    )
    .expect("second approved destination writes");

    assert!(std::path::Path::new(&first.manifest_path).starts_with(first_destination.path()));
    assert!(std::path::Path::new(&second.manifest_path).starts_with(second_destination.path()));
    assert!(first_destination
        .path()
        .join(&first.written[0].file_name)
        .is_file());
    assert!(second_destination
        .path()
        .join(&second.written[0].file_name)
        .is_file());
}

#[test]
fn cancelled_picker_leaves_no_approval_that_can_export() {
    let destination = tempfile::tempdir().expect("cancelled synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let (sources, source_id) = statement_sources();

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(destination.path(), "no-picker-selection", &source_id),
        &approvals,
        &sources,
    )
    .expect_err("a cancelled picker has no approval to consume");

    assert!(matches!(
        error,
        BulkPartyStatementExportError::DestinationNotAuthorized(error)
            if error.code == "statement_destination_not_authorized"
    ));
    assert!(
        std::fs::read_dir(destination.path())
            .expect("destination remains readable")
            .next()
            .is_none(),
        "the cancelled selection must not write a statement or manifest"
    );
}

#[test]
fn approval_destination_mismatch_is_rejected_without_writing() {
    let approved_destination = tempfile::tempdir().expect("approved synthetic destination");
    let requested_destination = tempfile::tempdir().expect("requested synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let (sources, source_id) = statement_sources();
    let approval_id = approvals
        .issue(approved_destination.path().to_path_buf())
        .expect("approve first destination");

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(requested_destination.path(), &approval_id, &source_id),
        &approvals,
        &sources,
    )
    .expect_err("an approval cannot be substituted for another destination");

    assert!(matches!(
        error,
        BulkPartyStatementExportError::DestinationNotAuthorized(error)
            if error.code == "statement_destination_not_authorized"
    ));
    let reuse_error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(approved_destination.path(), &approval_id, &source_id),
        &approvals,
        &sources,
    )
    .expect_err("a mismatched approval must be consumed");
    assert!(matches!(
        reuse_error,
        BulkPartyStatementExportError::DestinationNotAuthorized(error)
            if error.code == "statement_destination_not_authorized"
    ));
    assert!(
        std::fs::read_dir(requested_destination.path())
            .expect("requested destination remains readable")
            .next()
            .is_none(),
        "the rejected mismatch must not write a statement or manifest"
    );
    assert!(
        std::fs::read_dir(approved_destination.path())
            .expect("approved destination remains readable")
            .next()
            .is_none(),
        "a consumed mismatch approval must not write after a retry"
    );
}

#[test]
fn bulk_statement_export_keeps_the_existing_deleted_destination_failure() {
    let destination = tempfile::tempdir().expect("selected synthetic destination");
    let destination_path = destination.path().to_path_buf();
    let approvals = PartyStatementDestinationApprovals::default();
    let (sources, source_id) = statement_sources();
    let approval_id = approvals
        .issue(destination_path.clone())
        .expect("record picker destination");
    destination.close().expect("remove selected destination");

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(&destination_path, &approval_id, &source_id),
        &approvals,
        &sources,
    )
    .expect_err("a deleted selected destination must fail the existing directory check");

    assert!(matches!(
        error,
        BulkPartyStatementExportError::Existing(message)
            if message == "Bridge could not use that statement destination folder."
    ));
}

#[test]
fn local_export_file_names_reject_path_like_and_hidden_values() {
    assert_eq!(
        checked_export_file_name(" statement.csv ").unwrap(),
        "statement.csv"
    );
    for name in [
        "",
        ".hidden.csv",
        "../statement.csv",
        "nested/report.csv",
        "nested\\report.csv",
    ] {
        assert!(
            checked_export_file_name(name).is_err(),
            "{name:?} must be rejected"
        );
    }
}

#[test]
fn statement_party_slug_is_portable_and_nonempty() {
    assert_eq!(statement_filename_slug("  ../Aarav & Sons  "), "aarav-sons");
    assert_eq!(statement_filename_slug("///"), "party");
}
