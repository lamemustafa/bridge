use super::*;
use crate::tally::ExposureDirection;
use bridge_tally_core::ExactDecimal;

fn bill(party: &str, amount: &str) -> OpenBillRow {
    OpenBillRow {
        party: party.to_string(),
        reference: "SYNTHETIC-1".to_string(),
        bill_date: "20260101".to_string(),
        due_date: "20260201".to_string(),
        amount: ExactDecimal::parse(amount).expect("synthetic decimal"),
        age_days: Some(40),
        kind: ExposureDirection::Receivable,
    }
}

fn approved_destination(path: &Path) -> ApprovedPartyStatementDestination {
    let approvals = PartyStatementDestinationApprovals::default();
    let approval_id = approvals
        .issue(path.to_path_buf())
        .expect("approve synthetic destination");
    approvals
        .consume(&approval_id, path)
        .expect("consume synthetic approval")
}

#[test]
fn destination_approval_is_single_use() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let approval_id = approvals
        .issue(destination.path().to_path_buf())
        .expect("approve destination");

    let _approved = approvals
        .consume(&approval_id, destination.path())
        .expect("first export consumes approval");
    assert_eq!(
        approvals
            .consume(&approval_id, destination.path())
            .expect_err("a consumed approval cannot authorize another export"),
        PartyStatementDestinationApprovalError::NotAuthorized
    );
}

#[test]
fn revoked_destination_approval_frees_its_bounded_slot() {
    let approvals = PartyStatementDestinationApprovals::default();
    let approvals_to_revoke = (0..MAX_PENDING_DESTINATION_APPROVALS)
        .map(|index| {
            approvals
                .issue(PathBuf::from(format!("synthetic-destination-{index}")))
                .expect("fill bounded approval store")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        approvals
            .issue(PathBuf::from("synthetic-overflow"))
            .expect_err("a full approval store rejects another selection"),
        PartyStatementDestinationApprovalError::CapacityReached
    );

    approvals
        .revoke(&approvals_to_revoke[0])
        .expect("release abandoned approval");
    assert_eq!(
        approvals
            .consume(
                &approvals_to_revoke[0],
                Path::new("synthetic-destination-0"),
            )
            .expect_err("a revoked approval cannot still authorize an export"),
        PartyStatementDestinationApprovalError::NotAuthorized
    );
    approvals
        .issue(PathBuf::from("synthetic-replacement"))
        .expect("released approval slot accepts another selection");
}

#[test]
fn outstanding_destination_approvals_remain_independent() {
    let first_destination = tempfile::tempdir().expect("first temporary destination");
    let second_destination = tempfile::tempdir().expect("second temporary destination");
    let approvals = PartyStatementDestinationApprovals::default();
    let first_approval = approvals
        .issue(first_destination.path().to_path_buf())
        .expect("approve first destination");
    let second_approval = approvals
        .issue(second_destination.path().to_path_buf())
        .expect("approve second destination");

    let first = approvals
        .consume(&first_approval, first_destination.path())
        .expect("second approval must not invalidate the first");
    assert_eq!(first.path(), first_destination.path());

    let second = approvals
        .consume(&second_approval, second_destination.path())
        .expect("second approval remains available after consuming the first");
    assert_eq!(second.path(), second_destination.path());
}

#[test]
fn traversal_like_party_name_cannot_escape_the_selected_directory() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approved = approved_destination(destination.path());
    let result = write_bulk_party_statements(
        &approved,
        "Synthetic Books Pvt Ltd",
        "20260808",
        "xlsx",
        &[bill("../../etc/passwd", "15.00")],
        &[],
        |_| Ok(b"synthetic workbook".to_vec()),
    )
    .expect("statement batch succeeds");

    assert_eq!(result.written.len(), 1);
    let file = destination.path().join(&result.written[0].file_name);
    assert!(file.starts_with(destination.path()));
    assert!(file.is_file());
    assert_eq!(
        fs::read(&file).expect("statement bytes are readable"),
        b"synthetic workbook"
    );
    assert_eq!(
        result.written[0].file_name,
        "statement-etc-passwd-20260808.xlsx"
    );
}

#[test]
fn renderer_failure_is_recorded_in_result_and_manifest_while_other_parties_write() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approved = approved_destination(destination.path());
    let result = write_bulk_party_statements(
        &approved,
        "Synthetic Books Pvt Ltd",
        "20260808",
        "pdf",
        &[bill("Good Party", "10.00"), bill("Broken Party", "20.00")],
        &[],
        |statement| {
            if statement.party == "Broken Party" {
                Err("synthetic renderer failure".to_string())
            } else {
                Ok(b"synthetic PDF".to_vec())
            }
        },
    )
    .expect("partial batch result is returned");

    assert_eq!(result.written.len(), 1);
    assert_eq!(result.written[0].party, "Good Party");
    assert_eq!(result.written[0].receivable_amount, "10");
    assert_eq!(result.written[0].payable_amount, "0");
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].party, "Broken Party");
    assert_eq!(result.failures[0].code, StatementFailureCode::Rendering);
    let manifest = fs::read_to_string(&result.manifest_path).expect("manifest is written");
    assert!(manifest.contains("20260808"));
    assert!(manifest.contains("Synthetic Books Pvt Ltd"));
    assert!(manifest.contains("\"receivable_amount\": \"10\""));
    assert!(manifest.contains("\"payable_amount\": \"0\""));
    assert!(manifest.contains("\"ageing_anchor\": \"due_date\""));
    assert!(!manifest.contains("\"amount\":"));
    assert!(manifest.contains("Broken Party"));
    assert!(manifest.contains("\"code\": \"rendering\""));
    assert!(manifest.contains("synthetic renderer failure"));
}

#[test]
fn manifest_totals_keep_receivable_and_payable_directions_separate() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approved = approved_destination(destination.path());
    let mut payable_bill = bill("Mixed Party", "4.00");
    payable_bill.kind = ExposureDirection::Payable;
    let unallocated = [UnallocatedParty {
        party: "Mixed Party".to_string(),
        amount: ExactDecimal::parse("3.00").expect("synthetic decimal"),
        direction: ExposureDirection::Payable,
    }];

    let result = write_bulk_party_statements(
        &approved,
        "Synthetic Books Pvt Ltd",
        "20260808",
        "pdf",
        &[bill("Mixed Party", "10.00"), payable_bill],
        &unallocated,
        |_| Ok(b"synthetic PDF".to_vec()),
    )
    .expect("mixed statement writes");

    assert_eq!(result.written[0].receivable_amount, "10");
    assert_eq!(result.written[0].payable_amount, "7");
}

#[test]
fn manifest_discloses_the_selected_ageing_anchor() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approved = approved_destination(destination.path());
    let result = write_bulk_party_statements_with_ageing_anchor(BulkPartyStatementRequest {
        destination: &approved,
        company: "Synthetic Books Pvt Ltd",
        as_of_yyyymmdd: "20260808",
        format: "xlsx",
        open_bills: &[bill("Selected basis", "10.00")],
        unallocated_by_party: &[],
        ageing_anchor: OutstandingsAgeingAnchor::BillDate,
        render: |_: &PartyStatement| Ok(b"synthetic workbook".to_vec()),
    })
    .expect("statement batch succeeds");

    let manifest = fs::read_to_string(&result.manifest_path).expect("manifest is written");
    assert!(manifest.contains("\"ageing_anchor\": \"bill_date\""));
}

#[test]
fn per_party_file_creation_failure_is_retained_in_the_manifest() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approved = approved_destination(destination.path());
    let result = write_bulk_party_statements(
        &approved,
        "Synthetic Books Pvt Ltd",
        "20260808",
        "pdf/invalid",
        &[bill("Write Failure", "10.00")],
        &[],
        |_| Ok(b"synthetic PDF".to_vec()),
    )
    .expect("a partial batch result is returned");

    assert!(result.written.is_empty());
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].party, "Write Failure");
    assert_eq!(result.failures[0].code, StatementFailureCode::Write);
    let manifest = fs::read_to_string(&result.manifest_path).expect("manifest is written");
    assert!(manifest.contains("Write Failure"));
    assert!(manifest.contains("\"code\": \"write\""));
}

/// Regression for the write-failure manifest leak: the underlying `Err`
/// from `write_unique_file` used to embed `path.display()` -- the full
/// destination directory joined with the statement's file name -- so a
/// CA's own directory layout (and, on many machines, their OS username)
/// landed in `StatementManifest.failures`, which is serialized and
/// written out beside the exported statements. A CA could then hand a
/// client an artifact naming their own local folders.
///
/// This failed before the fix: `result.failures[0].error` (now `reason`)
/// was `"Bridge could not create {destination}/statement-...: ..."`, and
/// the manifest file on disk contained the tempdir's absolute path.
#[test]
fn write_failure_reason_never_carries_the_local_destination_path_into_the_manifest() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let destination_text = destination
        .path()
        .to_str()
        .expect("temp destination path is valid UTF-8")
        .to_string();
    let approved = approved_destination(destination.path());

    let result = write_bulk_party_statements(
        &approved,
        "Synthetic Books Pvt Ltd",
        "20260808",
        "pdf/invalid",
        &[bill("Write Failure", "10.00")],
        &[],
        |_| Ok(b"synthetic PDF".to_vec()),
    )
    .expect("a partial batch result is returned");

    assert_eq!(result.failures.len(), 1);
    // The operator still learns which party failed, and in what
    // actionable category, without any raw path making the trip.
    assert_eq!(result.failures[0].party, "Write Failure");
    assert_eq!(result.failures[0].code, StatementFailureCode::Write);
    assert!(!result.failures[0].reason.contains(&destination_text));
    assert!(!result.failures[0].reason.contains('/'));
    assert!(!result.failures[0].reason.contains('\\'));

    let manifest = fs::read_to_string(&result.manifest_path).expect("manifest is written");
    assert!(manifest.contains("Write Failure"));
    assert!(!manifest.contains(&destination_text));
    assert!(!manifest.contains("/Users/"));
    assert!(!manifest.contains("C:\\"));
}

#[test]
fn colliding_safe_names_are_written_to_distinct_files() {
    let destination = tempfile::tempdir().expect("temporary destination");
    let approved = approved_destination(destination.path());
    let result = write_bulk_party_statements(
        &approved,
        "Synthetic Books Pvt Ltd",
        "20260808",
        "xlsx",
        &[bill("A/B", "10.00"), bill("A:B", "20.00")],
        &[],
        |_| Ok(b"synthetic workbook".to_vec()),
    )
    .expect("statements write");

    assert_eq!(result.written.len(), 2);
    assert_ne!(result.written[0].file_name, result.written[1].file_name);
    assert!(result
        .written
        .iter()
        .all(|entry| destination.path().join(&entry.file_name).is_file()));
}

#[test]
fn party_count_deduplicates_nonzero_bill_and_unallocated_parties() {
    let unallocated = vec![
        UnallocatedParty {
            party: "Bill and On Account".to_string(),
            amount: ExactDecimal::parse("25.00").expect("synthetic decimal"),
            direction: ExposureDirection::Receivable,
        },
        UnallocatedParty {
            party: "On Account Only".to_string(),
            amount: ExactDecimal::parse("10.00").expect("synthetic decimal"),
            direction: ExposureDirection::Receivable,
        },
        UnallocatedParty {
            party: "Zero Balance".to_string(),
            amount: ExactDecimal::zero(),
            direction: ExposureDirection::Receivable,
        },
    ];

    assert_eq!(
        bulk_party_statement_party_count(
            &[
                bill("Bill and On Account", "15.00"),
                bill("Bill Only", "5.00"),
                bill("Zero Balance", "0"),
            ],
            &unallocated,
        ),
        3
    );
}
