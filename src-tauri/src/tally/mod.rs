#[allow(
    dead_code,
    reason = "the sealed coordinator is intentionally staged before its command layer"
)]
pub(crate) mod canary_preflight;
#[allow(
    dead_code,
    reason = "the private preflight preparation seam is staged before its command layer"
)]
pub(crate) mod canary_preflight_preparation;
// This is intentionally feature-gated and has no Tauri command. It performs
// only local, read-only admission checks before a future separately reviewed
// runtime-dispatch boundary can be considered.
#[cfg(feature = "fixture-canary-dispatch-seam")]
#[allow(
    dead_code,
    reason = "the application admission seam is intentionally staged before its command layer"
)]
pub(crate) mod canary_dispatch_admission;
// The dispatch coordinator is compiled only with the explicit non-default
// runtime feature. A future command boundary still needs separate review.
#[cfg(feature = "fixture-canary-runtime-dispatch")]
#[allow(
    dead_code,
    reason = "the runtime coordinator is intentionally staged before its command layer"
)]
pub(crate) mod canary_runtime_dispatch_coordinator;
pub mod capability_packs;
pub mod connection;
pub mod connector;
pub mod incremental;
pub mod runtime;
pub mod serial_queue;
pub mod tdl_engine;
pub mod validators;
pub(crate) mod write_sandbox;
pub mod xml_builder;
pub mod xml_parser;

pub use bridge_tally_core as core;
pub use connection::{
    ConnectionStatus, SelectedReadObservation, SelectedReadScopeEvidence, TallyClient, TallyConfig,
    TallyProbeResult, TallyProduct, SELECTED_LEDGER_QUERY_PROFILE_ID,
    SELECTED_VOUCHER_QUERY_PROFILE_ID,
};
pub(crate) use connector::core_snapshot_start_authorized_codes;
pub use connector::{
    company_source_identity, core_snapshot_start_authorized, source_lineage, RuntimeTallyConnector,
};
pub use runtime::{
    CachedProbeReservation, EndpointKey, TallyRuntime, TallySessionSnapshot,
    TallyTelemetryPreviewExport,
};
pub use xml_parser::{TallyCompany, TallyImportResult, TallyLedger, TallyVoucher};
