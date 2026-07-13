pub mod axal;
pub mod commands;
pub mod db;
pub mod documents;
pub mod dsc;
pub mod gst;
pub mod sync;
pub mod tally;

pub fn run() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::check_tally_connection,
            commands::fetch_tally_companies,
            commands::fetch_tally_ledgers,
            commands::fetch_tally_vouchers,
            commands::prepare_gst_return_draft,
            commands::detect_dsc_token,
            commands::extract_dsc_certificates,
            commands::validate_axal_credentials,
            commands::check_axal_connection_status,
            commands::revoke_axal_credential_session,
            commands::sync_dsc_certificates_to_axal,
            commands::scan_document_paths,
            commands::sync_documents_to_axal,
            commands::revoke_document_authorizations,
            commands::select_document_files,
            commands::select_document_folder
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Bridge");
}

#[cfg(test)]
mod security_config_tests {
    #[test]
    fn renderer_does_not_receive_tauri_core_default_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json"))
                .expect("valid capability JSON");
        assert!(capability["permissions"]
            .as_array()
            .is_some_and(|permissions| permissions.is_empty()));
    }

    #[test]
    fn production_csp_has_no_remote_browser_egress_or_inline_code() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid Tauri config");
        let csp = config["app"]["security"]["csp"]
            .as_str()
            .expect("production CSP string");
        assert!(!csp.contains("unsafe-inline"));
        assert!(!csp.contains("https://"));
        assert!(csp.contains("default-src 'none'"));
    }
}
