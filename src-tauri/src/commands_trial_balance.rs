//! Desktop adapters for one completed native Trial Balance observation.
use super::*;
use crate::reports::trial_balance_store::TrialBalanceExportStore;
use crate::reports::trial_balance_xlsx::render_trial_balance_xlsx;
use crate::tally::runtime::{TrialBalancePeriod, TrialBalanceRead, TrialBalanceReadError};
use bridge_tally_protocol::PartyLedgerMasterFieldObservation;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialBalanceRequest {
    config: TallyConfig,
    selected_company: SelectedCompanyIdentity,
    from: TallyDate,
    to: TallyDate,
}

#[derive(Serialize)]
pub struct TrialBalanceResponse {
    read: TrialBalanceRead,
    export_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialBalanceCaptureParentQueryRequest {
    export_id: String,
    parent: PartyLedgerMasterFieldObservation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialBalanceCaptureParentListRequest {
    export_id: String,
    search: String,
}

#[derive(Serialize)]
pub struct TrialBalanceCaptureProvenance {
    company_guid: String,
    company_name: String,
    from: TallyDate,
    to: TallyDate,
    read_at: String,
    request_sha256: String,
    response_sha256: String,
    source_bytes: usize,
    expires_in_seconds: u64,
}

#[derive(Serialize)]
pub struct TrialBalanceCaptureParentQueryResponse {
    query: crate::reports::trial_balance::TrialBalanceParentQuery,
    capture: TrialBalanceCaptureProvenance,
}

fn local_error(code: &'static str, message: &str, remediation: &'static str) -> TallyCommandError {
    tally_command_error(
        code,
        "Trial Balance",
        message,
        "after_change",
        false,
        remediation,
    )
}

fn parent_list_capture_error(
    error: crate::reports::trial_balance_store::TrialBalanceExportStoreError,
) -> TallyCommandError {
    match error {
        crate::reports::trial_balance_store::TrialBalanceExportStoreError::InvalidOrExpired => {
            local_error(
                "trial_balance_capture_expired",
                "This captured Trial Balance is no longer available for a follow-up query.",
                "Refresh the report before selecting a parent again.",
            )
        }
        crate::reports::trial_balance_store::TrialBalanceExportStoreError::ResourceLimit
        | crate::reports::trial_balance_store::TrialBalanceExportStoreError::Unavailable => {
            local_error(
                "trial_balance_capture_unavailable",
                "Bridge could not access the retained Trial Balance safely.",
                "Refresh the report before selecting a parent again.",
            )
        }
    }
}

fn read_error(error: anyhow::Error) -> TallyCommandError {
    if let Some(reason) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TrialBalanceReadError>())
    {
        if matches!(
            reason,
            TrialBalanceReadError::Period(
                bridge_tally_protocol::native_outstandings::NativeLedgerSnapshotPeriodError::InvalidRange
            )
        ) {
            return local_error(
                "trial_balance_period_invalid",
                "The start date must be on or before the end date.",
                "Choose a valid date range and refresh the report.",
            );
        }
        if matches!(reason, TrialBalanceReadError::EducationUnqualified) {
            return local_error(reason.safe_code(), "Native Trial Balance is not yet qualified for Education mode.",
                "This report currently requires observed Licensed TallyPrime. Education support needs further qualification.");
        }
        return local_error(reason.safe_code(), "Bridge could not admit this Trial Balance period or currency.",
            "Choose dates on or after book start. This report currently requires one observed INR currency master.");
    }
    if let Some(reason) = error.chain().find_map(|cause| {
        cause.downcast_ref::<bridge_tally_protocol::native_trial_balance::NativeTrialBalanceError>()
    }) {
        return match reason {
            bridge_tally_protocol::native_trial_balance::NativeTrialBalanceError::TallyReportedFailure => local_error(
                "trial_balance_tally_rejected",
                "Tally rejected the Trial Balance request.",
                "Confirm the selected company and date range in Tally, then retry the report.",
            ),
            bridge_tally_protocol::native_trial_balance::NativeTrialBalanceError::InvalidAmount => local_error(
                "trial_balance_amount_invalid",
                "Tally returned a Trial Balance amount Bridge could not represent safely.",
                "Keep the selected company quiet and retry the report. If it persists, review the affected ledger amount in Tally.",
            ),
            bridge_tally_protocol::native_trial_balance::NativeTrialBalanceError::InvalidResponse(_) => local_error(
                "trial_balance_source_invalid",
                "Tally's Trial Balance response could not be represented safely.",
                "Keep the selected company quiet, confirm the date range and retry the read.",
            ),
        };
    }
    tally_runtime_command_error(error)
}

#[cfg(test)]
mod tests {
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
}

#[tauri::command]
pub async fn fetch_tally_trial_balance(
    request: TrialBalanceRequest,
    runtime: State<'_, TallyRuntime>,
    exports: State<'_, TrialBalanceExportStore>,
) -> Result<TrialBalanceResponse, TallyCommandError> {
    let period = TrialBalancePeriod::new(request.from, request.to)
        .map_err(|error| read_error(error.into()))?;
    exports.clear().map_err(|_| {
        local_error(
            "trial_balance_export_unavailable",
            "Bridge could not reset the previous export.",
            "Restart Bridge, then refresh the report.",
        )
    })?;
    let identity =
        verify_observed_company_tuple(&runtime, &request.config, &request.selected_company).await?;
    let read = runtime
        .fetch_trial_balance(request.config, &identity, period)
        .await
        .map_err(read_error)?;
    let export_id = exports.insert(read.clone()).map_err(|_| {
        local_error(
            "trial_balance_export_budget",
            "The captured Trial Balance exceeds the local export budget.",
            "Review the selected company and retry with a smaller supported source.",
        )
    })?;
    Ok(TrialBalanceResponse { read, export_id })
}

/// The webview sends only an opaque handle; no amounts, rows or Tally request
/// can enter this formatting-only operation.
#[tauri::command]
pub async fn export_tally_trial_balance(
    app: tauri::AppHandle,
    export_id: String,
    exports: State<'_, TrialBalanceExportStore>,
) -> Result<String, TallyCommandError> {
    let read = exports.get(&export_id).map_err(|_| {
        local_error(
            "trial_balance_export_expired",
            "This captured Trial Balance is no longer available for export.",
            "Refresh the report, then export the newly captured result.",
        )
    })?;
    let mut slug = statement_filename_slug(&read.company_name);
    slug.truncate(150);
    let filename = format!("trial-balance-{slug}-{}.xlsx", read.to.as_str());
    let bytes = tauri::async_runtime::spawn_blocking(move || render_trial_balance_xlsx(&read)).await
        .map_err(|_| local_error("trial_balance_export_failed", "Bridge could not finish the workbook.", "Retry the export."))?
        .map_err(|_| local_error("trial_balance_export_failed", "Bridge could not represent this workbook safely.", "Review the captured report; amounts that exceed Excel precision cannot be exported as numbers."))?;
    save_report_download_bytes(&app, &filename, &bytes).map_err(|_| {
        local_error(
            "trial_balance_save_failed",
            "Bridge could not save the workbook to Downloads.",
            "Check Downloads-folder access and retry the export.",
        )
    })
}

/// Derives a bounded parent subset from one already captured report. This
/// command cannot acquire Tally data, accept a company identity, or write.
#[tauri::command]
pub async fn query_tally_trial_balance_capture_parent(
    request: TrialBalanceCaptureParentQueryRequest,
    exports: State<'_, TrialBalanceExportStore>,
) -> Result<TrialBalanceCaptureParentQueryResponse, TallyCommandError> {
    let capture = exports.get_capture(&request.export_id).map_err(|_| {
        local_error(
            "trial_balance_capture_expired",
            "This captured Trial Balance is no longer available for a follow-up query.",
            "Refresh the report before selecting a parent again.",
        )
    })?;
    let query =
        crate::reports::trial_balance::query_observed_parent(&capture.read.report, &request.parent)
            .map_err(|error| match error {
                crate::reports::trial_balance::TrialBalanceParentQueryError::ParentNotInCapture => {
                    local_error(
                        "trial_balance_capture_parent_unavailable",
                        "That exact parent was not returned by the retained capture.",
                        "Select a parent returned by this capture or refresh the report.",
                    )
                }
                crate::reports::trial_balance::TrialBalanceParentQueryError::TotalsUnavailable => {
                    local_error(
                        "trial_balance_capture_invalid",
                        "The retained Trial Balance cannot be summarized safely.",
                        "Refresh the report before selecting a parent again.",
                    )
                }
            })?;
    Ok(TrialBalanceCaptureParentQueryResponse {
        query,
        capture: TrialBalanceCaptureProvenance {
            company_guid: capture.read.company_guid.clone(),
            company_name: capture.read.company_name.clone(),
            from: capture.read.from.clone(),
            to: capture.read.to.clone(),
            read_at: capture.read.read_at.clone(),
            request_sha256: capture.read.evidence.request_sha256.clone(),
            response_sha256: capture.read.evidence.response_sha256.clone(),
            source_bytes: capture.read.evidence.bytes,
            expires_in_seconds: capture.expires_in.as_secs(),
        },
    })
}

/// Lists a bounded set of parent observations from one already retained Trial
/// Balance. The opaque handle is resolved locally before an off-thread scan;
/// this command cannot acquire Tally data, accept an identity, or write.
#[tauri::command]
pub async fn list_tally_trial_balance_capture_parents(
    request: TrialBalanceCaptureParentListRequest,
    exports: State<'_, TrialBalanceExportStore>,
) -> Result<crate::reports::trial_balance::TrialBalanceCaptureParentOptions, TallyCommandError> {
    let capture = exports
        .get_capture(&request.export_id)
        .map_err(parent_list_capture_error)?;
    let read = capture.read;
    tauri::async_runtime::spawn_blocking(move || {
        crate::reports::trial_balance::list_observed_capture_parents(&read.report, request.search)
    })
    .await
    .map_err(|_| {
        local_error(
            "trial_balance_capture_parent_list_failed",
            "Bridge could not scan the retained Trial Balance.",
            "Refresh the report before selecting a parent again.",
        )
    })?
    .map_err(|error| match error {
        crate::reports::trial_balance::TrialBalanceCaptureParentOptionsError::SearchInvalid => {
            local_error(
                "trial_balance_capture_parent_search_invalid",
                "That parent search is not valid for this retained capture.",
                "Use a shorter parent name or clear the search.",
            )
        }
    })
}
