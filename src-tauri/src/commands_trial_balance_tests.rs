use super::*;
use bridge_tally_protocol::native_trial_balance::NativeTrialBalanceError;

#[test]
fn education_refusal_explains_the_report_qualification_limit() {
    let mapped = read_error(TrialBalanceReadError::EducationUnqualified.into());
    assert_eq!(mapped.code, "trial_balance_education_unqualified");
    assert!(mapped.message.contains("Education mode"));
    assert!(mapped.remediation.contains("Licensed TallyPrime"));
    assert!(!mapped.remediation.contains("day 1"));
}

#[test]
fn ordered_period_keeps_the_actionable_desktop_error() {
    let error = TrialBalancePeriod::new(
        TallyDate::parse("20260902").unwrap(),
        TallyDate::parse("20260401").unwrap(),
    )
    .unwrap_err();
    let mapped = read_error(error.into());
    assert_eq!(mapped.code, "trial_balance_period_invalid");
    assert!(mapped.message.contains("start date"));
    assert!(mapped.remediation.contains("valid date range"));
}

#[test]
fn native_trial_balance_errors_have_distinct_safe_desktop_remediation() {
    for (source, code, message_fragment, remediation_fragment) in [
        (
            NativeTrialBalanceError::TallyReportedFailure,
            "trial_balance_tally_rejected",
            "Tally rejected",
            "selected company and date range in Tally",
        ),
        (
            NativeTrialBalanceError::InvalidAmount,
            "trial_balance_amount_invalid",
            "amount Bridge could not represent safely",
            "affected ledger amount in Tally",
        ),
        (
            NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"),
            "trial_balance_source_invalid",
            "response could not be represented safely",
            "Keep the selected company quiet",
        ),
    ] {
        let mapped = read_error(anyhow::Error::new(source));
        assert_eq!(mapped.code, code);
        assert!(mapped.message.contains(message_fragment));
        assert!(mapped.remediation.contains(remediation_fragment));
        assert!(!mapped.message.contains("trial_balance_xml_malformed"));
        assert!(!mapped.remediation.contains("trial_balance_xml_malformed"));
    }
}

#[test]
fn parent_list_request_refuses_unknown_fields_and_maps_expired_captures() {
    let request: TrialBalanceCaptureParentListRequest =
        serde_json::from_str(r#"{"export_id":"opaque","search":"debtor"}"#).unwrap();
    assert_eq!(request.export_id, "opaque");
    assert_eq!(request.search, "debtor");
    assert!(
        serde_json::from_str::<TrialBalanceCaptureParentListRequest>(
            r#"{"export_id":"opaque","search":"debtor","parent":"unexpected"}"#
        )
        .is_err()
    );

    let expired = parent_list_capture_error(
        crate::reports::trial_balance_store::TrialBalanceExportStoreError::InvalidOrExpired,
    );
    assert_eq!(expired.code, "trial_balance_capture_expired");
    assert!(expired.remediation.contains("Refresh the report"));
}
