pub mod axal;
pub mod commands;
pub mod db;
pub mod documents;
pub mod dsc;
pub mod gst;
pub mod sync;
pub mod tally;

use tauri::Manager;

fn initialize_tally_mirror(app: &tauri::App) -> anyhow::Result<()> {
    let app_data_directory = app.path().app_data_dir()?;
    std::fs::create_dir_all(&app_data_directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&app_data_directory, std::fs::Permissions::from_mode(0o700))?;
    }

    let database_path = app_data_directory.join("tally-mirror-v1.db");
    let _initialization_lock = db::encrypted::lock_mirror_initialization(&database_path)?;
    let key_store = db::OsMirrorKeyStore::for_database(&database_path);
    let resolved_key = db::resolve_mirror_key(&database_path, &key_store)?;
    let pool =
        tauri::async_runtime::block_on(db::connect_encrypted(&database_path, resolved_key.key))?;
    let repository = db::tally_mirror::TallyMirrorRepository::new(pool);
    tauri::async_runtime::block_on(repository.migrate())?;
    app.manage(repository);
    Ok(())
}

pub fn run() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .manage(tally::TallyRuntime::default())
        .manage(sync::coordinator::SnapshotCoordinator::default())
        .setup(|app| {
            initialize_tally_mirror(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_tally_connection,
            commands::probe_tally,
            commands::bootstrap_direct_tally_company,
            commands::qualify_selected_tally_reads,
            commands::save_tally_setup,
            commands::enroll_tally_write_fixture,
            #[cfg(feature = "fixture-canary-runtime-dispatch")]
            commands::dispatch_tally_synthetic_canary,
            commands::tally_write_fixture_enrollment_status,
            commands::revoke_tally_write_fixture_enrollment,
            commands::tally_persisted_company_profiles,
            commands::tally_mirror_explorer_page,
            commands::tally_sync_evidence,
            commands::preview_tally_redacted_proof,
            commands::start_tally_core_snapshot,
            commands::resume_tally_core_snapshot,
            commands::tally_recent_snapshot_runs,
            commands::tally_snapshot_status,
            commands::cancel_tally_snapshot,
            commands::cancel_tally_request,
            commands::tally_runtime_snapshots,
            commands::tally_telemetry_preview,
            commands::fetch_tally_companies,
            commands::fetch_tally_ledgers,
            commands::fetch_standard_tally_ledger_catalog,
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
