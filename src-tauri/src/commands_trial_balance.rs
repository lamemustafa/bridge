//! Desktop adapters for one completed native Trial Balance observation.
use super::*;
use crate::reports::trial_balance_store::TrialBalanceExportStore;
use crate::reports::trial_balance_xlsx::render_trial_balance_xlsx;
use crate::tally::runtime::{TrialBalanceRead, TrialBalanceReadError};

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

fn read_error(error: anyhow::Error) -> TallyCommandError {
    if let Some(reason) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TrialBalanceReadError>())
    {
        return local_error(reason.safe_code(), "Bridge could not admit this Trial Balance period or currency.",
            "Choose dates on or after book start. Education mode requires day 1, 2 or 31 at both ends. This report currently requires one observed INR currency master.");
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
}

#[tauri::command]
pub async fn fetch_tally_trial_balance(
    request: TrialBalanceRequest,
    runtime: State<'_, TallyRuntime>,
    exports: State<'_, TrialBalanceExportStore>,
) -> Result<TrialBalanceResponse, TallyCommandError> {
    if request.from > request.to {
        return Err(local_error(
            "trial_balance_period_invalid",
            "The start date must be on or before the end date.",
            "Choose a valid date range and refresh the report.",
        ));
    }
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
        .fetch_trial_balance(request.config, &identity, request.from, request.to)
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
