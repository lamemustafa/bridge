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

fn bulk_export_request(
    destination: &std::path::Path,
    approval_id: &str,
) -> ExportBulkPartyStatementsRequest {
    ExportBulkPartyStatementsRequest {
        company: "Synthetic Books Pvt Ltd".to_string(),
        as_of_yyyymmdd: "20260808".to_string(),
        format: PartyStatementFormat::Xlsx,
        ageing_anchor: crate::tally::OutstandingsAgeingAnchor::DueDate,
        destination: destination.to_string_lossy().into_owned(),
        approval_id: approval_id.to_string(),
        open_bills: vec![OpenBillRow {
            party: "Synthetic Party".to_string(),
            reference: "SYNTHETIC-1".to_string(),
            bill_date: "20260801".to_string(),
            due_date: "20260831".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse("100.00").expect("synthetic amount"),
            age_days: Some(7),
            kind: crate::tally::ExposureDirection::Receivable,
        }],
        unallocated_by_party: Vec::new(),
    }
}

#[test]
fn statement_export_format_defaults_to_xlsx_and_accepts_pdf() {
    let base = serde_json::json!({
        "company": "Synthetic Books Pvt Ltd",
        "as_of_yyyymmdd": "20260808",
        "party": "Synthetic Party",
        "open_bills": [],
        "unallocated_by_party": [],
    });
    let defaulted: ExportPartyStatementRequest = serde_json::from_value(base.clone()).unwrap();
    assert!(matches!(defaulted.format, PartyStatementFormat::Xlsx));
    assert!(matches!(
        defaulted.ageing_anchor,
        crate::tally::OutstandingsAgeingAnchor::DueDate
    ));

    let mut pdf = base;
    pdf["format"] = serde_json::Value::String("pdf".to_string());
    let pdf: ExportPartyStatementRequest = serde_json::from_value(pdf).unwrap();
    assert!(matches!(pdf.format, PartyStatementFormat::Pdf));
}

#[test]
fn bulk_statement_export_defaults_the_ageing_anchor_for_legacy_callers() {
    let request: ExportBulkPartyStatementsRequest = serde_json::from_value(serde_json::json!({
        "company": "Synthetic Books Pvt Ltd",
        "as_of_yyyymmdd": "20260808",
        "format": "xlsx",
        "destination": "/tmp/statements",
        "approval_id": "synthetic-approval",
        "open_bills": [],
        "unallocated_by_party": [],
    }))
    .unwrap();

    assert!(matches!(
        request.ageing_anchor,
        crate::tally::OutstandingsAgeingAnchor::DueDate
    ));
}

#[test]
fn bulk_statement_export_rejects_an_unselected_destination_without_writing() {
    let destination = tempfile::tempdir().expect("unselected synthetic destination");
    let approvals = PartyStatementDestinationApprovals::default();

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(destination.path(), "not-issued"),
        &approvals,
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
    let approval_id = approvals
        .issue(destination.path().to_path_buf())
        .expect("record picker destination");

    let result = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(destination.path(), &approval_id),
        &approvals,
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
    // Both selections exist before either export starts; neither replaces
    // the other as the previous singleton store did.
    let first_approval = approvals
        .issue(first_destination.path().to_path_buf())
        .expect("approve first destination");
    let second_approval = approvals
        .issue(second_destination.path().to_path_buf())
        .expect("approve second destination");

    let first = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(first_destination.path(), &first_approval),
        &approvals,
    )
    .expect("first approved destination writes");
    let second = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(second_destination.path(), &second_approval),
        &approvals,
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

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(destination.path(), "no-picker-selection"),
        &approvals,
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
    let approval_id = approvals
        .issue(approved_destination.path().to_path_buf())
        .expect("approve first destination");

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(requested_destination.path(), &approval_id),
        &approvals,
    )
    .expect_err("an approval cannot be substituted for another destination");

    assert!(matches!(
        error,
        BulkPartyStatementExportError::DestinationNotAuthorized(error)
            if error.code == "statement_destination_not_authorized"
    ));
    let reuse_error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(approved_destination.path(), &approval_id),
        &approvals,
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
    let approval_id = approvals
        .issue(destination_path.clone())
        .expect("record picker destination");
    destination.close().expect("remove selected destination");

    let error = export_bulk_party_statements_at_selected_destination(
        bulk_export_request(&destination_path, &approval_id),
        &approvals,
    )
    .expect_err("a deleted selected destination must fail the existing directory check");

    assert!(matches!(
        error,
        BulkPartyStatementExportError::Existing(message)
            if message == "Bridge could not use that statement destination folder."
    ));
}

#[test]
fn statement_export_rejects_unknown_bill_direction_at_the_ipc_boundary() {
    let request = serde_json::json!({
        "company": "Synthetic Books Pvt Ltd",
        "as_of_yyyymmdd": "20260808",
        "party": "Synthetic Party",
        "open_bills": [{
            "party": "Synthetic Party",
            "reference": "INV-1",
            "bill_date": "20260801",
            "due_date": "20260831",
            "amount": "100.00",
            "age_days": 7,
            "kind": "unknown"
        }],
        "unallocated_by_party": []
    });

    assert!(serde_json::from_value::<ExportPartyStatementRequest>(request).is_err());
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
