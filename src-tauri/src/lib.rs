pub mod axal;
pub mod client_groups;
pub mod commands;
pub mod db;
pub mod documents;
pub mod dsc;
pub mod gst;
// Crate-internal only: the previously separate `bridge-tally-observability` crate had exactly
// one consumer inside this crate, so it does not need to be reachable from outside `bridge_lib`.
mod observability;
pub mod reports;
pub mod sync;
pub mod tally;
pub mod warning_codes;

use std::path::PathBuf;
use tauri::Manager;
use tokio::sync::OnceCell;

/// Holds everything needed to open the encrypted Tally mirror without doing any of the actual
/// (keychain-touching, disk-touching) work until a caller genuinely needs the mirror.
///
/// The mirror is expensive to open on macOS: resolving its SQLCipher key from the OS keychain
/// triggers an authorisation prompt. Most Bridge sessions (native outstandings reads, company
/// discovery/probe, base-currency detection, CSV export) never touch the mirror at all, so eagerly
/// opening it on every app start means paying that prompt for no reason. Initialising lazily, on
/// first use, means the prompt is shown only to sessions that exercise a feature that truly needs
/// the mirror (snapshot, mirror-explorer, write-fixture, proof export).
pub struct LazyTallyMirror {
    app_data_directory: PathBuf,
    repository: OnceCell<db::tally_mirror::TallyMirrorRepository>,
}

impl LazyTallyMirror {
    pub fn new(app_data_directory: PathBuf) -> Self {
        Self {
            app_data_directory,
            repository: OnceCell::new(),
        }
    }

    /// Returns the initialised mirror repository, performing the (at most once) initialisation
    /// work on first call. `OnceCell::get_or_try_init` guarantees that concurrent callers racing
    /// this accessor observe exactly one initialisation attempt: the first caller runs it while
    /// later callers await the same in-flight attempt rather than starting their own.
    pub async fn get(&self) -> anyhow::Result<&db::tally_mirror::TallyMirrorRepository> {
        self.repository
            .get_or_try_init(|| async {
                std::fs::create_dir_all(&self.app_data_directory)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &self.app_data_directory,
                        std::fs::Permissions::from_mode(0o700),
                    )?;
                }

                let database_path = self.app_data_directory.join("tally-mirror-v1.db");
                let _initialization_lock =
                    db::encrypted::lock_mirror_initialization(&database_path)?;
                let key_store = db::OsMirrorKeyStore::for_database(&database_path);
                let resolved_key = db::resolve_mirror_key(&database_path, &key_store)?;
                let pool = db::connect_encrypted(&database_path, resolved_key.key).await?;
                let repository = db::tally_mirror::TallyMirrorRepository::new(pool);
                repository.migrate().await?;
                Ok(repository)
            })
            .await
    }
}

pub fn run() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .manage(tally::TallyRuntime::default())
        .manage(reports::bulk_party_statement::PartyStatementDestinationApprovals::default())
        .manage(sync::coordinator::SnapshotCoordinator::default())
        .setup(|app| {
            let app_data_directory = app.path().app_data_dir()?;
            app.manage(LazyTallyMirror::new(app_data_directory));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_tally_connection,
            commands::probe_tally,
            commands::bootstrap_direct_tally_company,
            commands::save_tally_setup,
            commands::enroll_tally_write_fixture,
            commands::tally_write_fixture_enrollment_status,
            commands::revoke_tally_write_fixture_enrollment,
            commands::save_report_download,
            commands::export_tally_trial_balance,
            commands::reveal_exported_file,
            commands::export_party_statement,
            commands::select_party_statement_destination,
            commands::revoke_party_statement_destination,
            commands::preview_bulk_party_statements,
            commands::export_bulk_party_statements,
            commands::fetch_tally_outstandings_all_companies,
            commands::load_client_group_labels,
            commands::save_client_group_label,
            commands::load_client_sort_preference,
            commands::save_client_sort_preference,
            commands::detect_tally_base_currency,
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
            commands::fetch_tally_outstandings,
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
mod lazy_tally_mirror_concurrency_tests {
    //! `LazyTallyMirror::get()` is built directly on `tokio::sync::OnceCell::get_or_try_init`,
    //! which is exactly what supplies the "one initialisation, not two" guarantee under
    //! concurrent callers required by this change: the first caller to reach the cell runs the
    //! initialisation future while every other concurrent caller awaits that same in-flight
    //! attempt instead of starting its own.
    //!
    //! This test proves that guarantee by racing many tasks against a shared `OnceCell` using
    //! the identical `get_or_try_init` call `LazyTallyMirror::get()` makes, and asserting the
    //! initialisation closure ran exactly once.
    //!
    //! It deliberately does NOT call `LazyTallyMirror::get()` end-to-end. A real call resolves
    //! the SQLCipher key through the OS keychain (`db::OsMirrorKeyStore` -> `keyring::Entry`),
    //! which would create or query a real macOS keychain item from an automated `cargo test`
    //! process outside any app bundle/code signature — the very kind of environment-dependent,
    //! potentially interactive behaviour this change exists to avoid triggering on every launch.
    //! The `keyring` dependency is built without a mockable backend (feature `v1` only), so there
    //! is no offline/deterministic way to substitute a fake credential store for that path. That
    //! makes the keychain-touching portion of initialisation untestable offline; this test proves
    //! the concurrency mechanism it relies on instead of asserting something it cannot honestly
    //! demonstrate.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{Barrier, OnceCell};

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_callers_observe_exactly_one_initialization() {
        const CONCURRENT_CALLERS: usize = 16;

        let cell: Arc<OnceCell<u32>> = Arc::new(OnceCell::new());
        let init_calls = Arc::new(AtomicUsize::new(0));
        // Every task waits at the barrier so they all call `get_or_try_init` at (as close to)
        // the same instant as possible, maximising the chance of a real race rather than an
        // accidentally-serialised sequence of calls.
        let barrier = Arc::new(Barrier::new(CONCURRENT_CALLERS));

        let tasks: Vec<_> = (0..CONCURRENT_CALLERS)
            .map(|_| {
                let cell = Arc::clone(&cell);
                let init_calls = Arc::clone(&init_calls);
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await;
                    cell.get_or_try_init(|| async {
                        init_calls.fetch_add(1, Ordering::SeqCst);
                        // Hold the in-flight initialisation open for long enough that, absent
                        // `OnceCell`'s mutual exclusion, other callers would very likely start
                        // their own concurrent initialisation attempt.
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        Ok::<u32, anyhow::Error>(42)
                    })
                    .await
                    .copied()
                })
            })
            .collect();

        let mut results = Vec::with_capacity(CONCURRENT_CALLERS);
        for task in tasks {
            results.push(task.await.expect("initialization task must not panic"));
        }

        assert_eq!(
            init_calls.load(Ordering::SeqCst),
            1,
            "initialization must run at most once under concurrent access"
        );
        for result in results {
            assert_eq!(
                result.expect("initialization must succeed"),
                42,
                "every concurrent caller must observe the single initialization's value"
            );
        }
    }
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

#[cfg(test)]
mod client_preference_mount_tests {
    #[test]
    fn client_preference_commands_are_mirror_and_keychain_free() {
        let commands = include_str!("commands.rs");
        let start = commands
            .find("pub fn load_client_group_labels")
            .expect("group-label load command");
        let end = commands[start..]
            .find("pub struct AllCompaniesOutstandingsRequest")
            .map(|offset| start + offset)
            .expect("end of group-label commands");
        let client_preference_commands = &commands[start..end];

        assert!(client_preference_commands.contains("load_client_sort_preference"));
        assert!(client_preference_commands.contains("save_client_sort_preference"));
        assert!(client_preference_commands.contains("app_config_dir"));
        assert!(!client_preference_commands.contains("LazyTallyMirror"));
        assert!(!client_preference_commands.contains("keyring"));
    }
}
