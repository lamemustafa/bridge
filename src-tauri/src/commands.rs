use crate::gst::{GstDraftRequest, GstReturnDraft};
use crate::tally::{
    ConnectionStatus, TallyClient, TallyCompany, TallyConfig, TallyLedger, TallyVoucher,
};
use serde::Deserialize;
use zeroize::Zeroize;

const MAX_DSC_PIN_BYTES: usize = 128;

#[tauri::command]
pub async fn check_tally_connection(config: TallyConfig) -> Result<ConnectionStatus, String> {
    TallyClient::new(config)
        .check_connection()
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn fetch_tally_companies(config: TallyConfig) -> Result<Vec<TallyCompany>, String> {
    TallyClient::new(config)
        .fetch_companies()
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, Deserialize)]
pub struct CompanyRequest {
    pub config: TallyConfig,
    pub company: String,
}

#[derive(Debug, Deserialize)]
pub struct VoucherRequest {
    pub config: TallyConfig,
    pub company: String,
    pub from: String,
    pub to: String,
}

#[tauri::command]
pub async fn fetch_tally_ledgers(request: CompanyRequest) -> Result<Vec<TallyLedger>, String> {
    TallyClient::new(request.config)
        .fetch_ledgers(&request.company)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn fetch_tally_vouchers(request: VoucherRequest) -> Result<Vec<TallyVoucher>, String> {
    TallyClient::new(request.config)
        .fetch_vouchers(&request.company, &request.from, &request.to)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn prepare_gst_return_draft(request: GstDraftRequest) -> Result<GstReturnDraft, String> {
    Ok(GstReturnDraft::empty(request))
}

async fn run_dsc_probe(
    detect_only: bool,
    pins: Option<Vec<String>>,
) -> Result<crate::dsc::ProbeReport, String> {
    tokio::task::spawn_blocking(move || {
        crate::dsc::run_probe_isolated(detect_only, None, pins, true)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("DSC probe task failed: {error}"))?
}

#[tauri::command]
pub async fn detect_dsc_token() -> Result<crate::dsc::ProbeReport, String> {
    run_dsc_probe(true, None).await
}

#[tauri::command]
pub async fn extract_dsc_certificates(
    pins: Option<Vec<String>>,
) -> Result<crate::dsc::ProbeReport, String> {
    let mut pins = pins.ok_or_else(|| "PIN is required to extract DSC certificates".to_string())?;
    if let Err(error) = validate_dsc_pins(&pins) {
        pins.zeroize();
        return Err(error);
    }
    run_dsc_probe(false, Some(pins)).await
}

fn validate_dsc_pins(pins: &[String]) -> Result<(), String> {
    if pins.len() != 1 || pins[0].is_empty() {
        return Err("Provide exactly one non-empty PIN".to_string());
    }
    if pins[0].len() > MAX_DSC_PIN_BYTES || pins[0].chars().any(char::is_control) {
        return Err(
            "DSC PIN must be at most 128 bytes and contain no control characters".to_string(),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn validate_axal_credentials(
    credentials: crate::axal::AxalCredentials,
) -> Result<crate::axal::AxalSessionResponse, String> {
    crate::axal::establish_credential_session(credentials)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn check_axal_connection_status(
    credential_session_id: String,
) -> Result<crate::axal::ConnectionStatusResponse, String> {
    crate::axal::check_connection_status(&credential_session_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn revoke_axal_credential_session(credential_session_id: String) -> Result<(), String> {
    crate::axal::revoke_credential_session(&credential_session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn sync_dsc_certificates_to_axal(
    request: crate::axal::DscSyncRequest,
) -> Result<crate::axal::DscSyncResponse, String> {
    crate::axal::sync_dsc_certificates(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn scan_document_paths(
    request: crate::documents::ScanDocumentsRequest,
) -> Result<crate::documents::ScanDocumentsResponse, String> {
    crate::documents::scan_documents(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn sync_documents_to_axal(
    request: crate::documents::SyncDocumentsRequest,
) -> Result<crate::documents::SyncDocumentsResponse, String> {
    crate::documents::sync_documents(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn revoke_document_authorizations(
    selection_ids: Vec<String>,
    scan_session_id: Option<String>,
) -> Result<(), String> {
    crate::documents::revoke_document_authorizations(&selection_ids, scan_session_id.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn select_document_files() -> Result<Vec<crate::documents::SelectedDocumentPath>, String>
{
    tokio::task::spawn_blocking(|| {
        let paths = rfd::FileDialog::new()
            .set_title("Select documents")
            .pick_files()
            .unwrap_or_default();
        crate::documents::authorize_selected_paths(paths).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("File picker failed: {error}"))?
}

#[tauri::command]
pub async fn select_document_folder() -> Result<Vec<crate::documents::SelectedDocumentPath>, String>
{
    tokio::task::spawn_blocking(|| {
        let paths = rfd::FileDialog::new()
            .set_title("Select document folder")
            .pick_folder()
            .into_iter()
            .collect::<Vec<_>>();
        crate::documents::authorize_selected_paths(paths).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Folder picker failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::validate_dsc_pins;

    #[test]
    fn dsc_pin_input_is_strictly_bounded() {
        assert!(validate_dsc_pins(&["1234".to_string()]).is_ok());
        assert!(validate_dsc_pins(&["".to_string()]).is_err());
        assert!(validate_dsc_pins(&["1\n2".to_string()]).is_err());
        assert!(validate_dsc_pins(&["x".repeat(129)]).is_err());
        assert!(validate_dsc_pins(&["1".to_string(), "2".to_string()]).is_err());
    }
}
