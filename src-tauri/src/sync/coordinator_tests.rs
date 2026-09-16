use super::*;

fn status(run_id: &str) -> SnapshotJobStatus {
    SnapshotJobStatus {
        run_id: run_id.into(),
        mirror_company_id: None,
        pack_id: None,
        requested_from_yyyymmdd: None,
        requested_to_yyyymmdd: None,
        phase: SnapshotPhase::Prepare,
        active_window_id: None,
        completed_windows: 0,
        total_windows: 0,
        verification: None,
        proof_id: None,
        proof_sha256: None,
        gap_codes: Vec::new(),
        warning_codes: Vec::new(),
        failure_code: None,
        requires_resume: false,
        resume_available: false,
    }
}

#[test]
fn recent_keeps_durable_order_and_appends_tracked_before_durable_runs() {
    let mut statuses = vec![status("durable-newest"), status("durable-older")];
    append_missing_tracked_statuses(
        &mut statuses,
        vec![
            ("run-z".into(), status("run-z")),
            ("run-a".into(), status("run-a")),
        ],
    );

    assert_eq!(
        statuses
            .iter()
            .map(|status| status.run_id.as_str())
            .collect::<Vec<_>>(),
        ["durable-newest", "durable-older", "run-a", "run-z"]
    );
}
