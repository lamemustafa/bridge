//! Fixed-cardinality, local-only Tally transport observations.
//!
//! This crate deliberately has no runtime, HTTP, database, logging, tracing,
//! persistence, system-metrics, or exporter dependency. Its preview is a
//! privacy-reduced operational aid, not Proof of Sync or performance support.

use std::{fmt, sync::Mutex, time::Duration};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const PREVIEW_SCHEMA: &str = "bridge.tally.telemetry-preview/2";
pub const MAX_SERIALIZED_PREVIEW_BYTES: usize = 64 * 1024;
pub const LATENCY_UPPER_BOUNDS_MICROS: [u64; 8] = [
    1_000, 5_000, 25_000, 100_000, 500_000, 2_000_000, 10_000_000, 30_000_000,
];
pub const RESPONSE_BYTE_UPPER_BOUNDS: [u64; 6] = [
    0,
    1_024,
    64 * 1_024,
    1_024 * 1_024,
    8 * 1_024 * 1_024,
    32 * 1_024 * 1_024,
];

const LATENCY_BUCKETS: usize = LATENCY_UPPER_BOUNDS_MICROS.len() + 1;
const BYTE_BUCKETS: usize = RESPONSE_BYTE_UPPER_BOUNDS.len() + 1;
const QUEUE_OUTCOMES: usize = QueueOutcome::ALL.len();
const RESPONSE_OUTCOMES: usize = ResponseOutcome::ALL.len();
const CIRCUIT_REJECT_REASONS: usize = CircuitRejectReason::ALL.len();
const REQUEST_CLASSES: usize = RequestClass::ALL.len();
const QUEUE_CELLS: usize = REQUEST_CLASSES * QUEUE_OUTCOMES * LATENCY_BUCKETS;
const RESPONSE_LATENCY_CELLS: usize = REQUEST_CLASSES * RESPONSE_OUTCOMES * LATENCY_BUCKETS;
const RESPONSE_BYTE_CELLS: usize = REQUEST_CLASSES * RESPONSE_OUTCOMES * BYTE_BUCKETS;
const RESPONSE_BYTE_UNAVAILABLE_CELLS: usize = REQUEST_CLASSES * RESPONSE_OUTCOMES;
const CIRCUIT_REJECTION_CELLS: usize = REQUEST_CLASSES * CIRCUIT_REJECT_REASONS;
pub const FIXED_HISTOGRAM_CELL_COUNT: usize = QUEUE_CELLS
    + RESPONSE_LATENCY_CELLS
    + RESPONSE_BYTE_CELLS
    + RESPONSE_BYTE_UNAVAILABLE_CELLS
    + CIRCUIT_REJECTION_CELLS;
const EXPORT_HASH_DOMAIN: &[u8] = b"bridge.tally.telemetry-preview-payload/2\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestClass {
    Status,
    Capability,
    CompanyList,
    MasterExport,
    VoucherExport,
    ReportExport,
    Import,
    OtherRead,
}

impl RequestClass {
    pub const ALL: [Self; 8] = [
        Self::Status,
        Self::Capability,
        Self::CompanyList,
        Self::MasterExport,
        Self::VoucherExport,
        Self::ReportExport,
        Self::Import,
        Self::OtherRead,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueOutcome {
    Acquired,
    Deadline,
    Cancelled,
}

impl QueueOutcome {
    pub const ALL: [Self; 3] = [Self::Acquired, Self::Deadline, Self::Cancelled];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseOutcome {
    Success,
    Cancelled,
    Timeout,
    Transport,
    HttpStatus,
    SizeLimit,
    Decode,
    Application,
    Parse,
    Validation,
}

impl ResponseOutcome {
    pub const ALL: [Self; 10] = [
        Self::Success,
        Self::Cancelled,
        Self::Timeout,
        Self::Transport,
        Self::HttpStatus,
        Self::SizeLimit,
        Self::Decode,
        Self::Application,
        Self::Parse,
        Self::Validation,
    ];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitRejectReason {
    Cooldown,
    HalfOpenProbeInFlight,
}

impl CircuitRejectReason {
    pub const ALL: [Self; 2] = [Self::Cooldown, Self::HalfOpenProbeInFlight];

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyBytesObservation {
    Observed(u64),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptObservation {
    CircuitRejected {
        class: RequestClass,
        reason: CircuitRejectReason,
    },
    QueueDeadline {
        class: RequestClass,
        queue_wait: Duration,
    },
    QueueCancelled {
        class: RequestClass,
        queue_wait: Duration,
    },
    Response {
        class: RequestClass,
        queue_wait: Duration,
        outcome: ResponseOutcome,
        /// Custom Bridge pipeline duration from send start through bounded
        /// body read and decode completion. This is not an OTel HTTP duration.
        response_pipeline_elapsed: Duration,
        /// Bytes consumed before the terminal outcome, including partial
        /// failed bodies. This is not an OTel HTTP response-body-size metric.
        observed_body_bytes: BodyBytesObservation,
    },
}

/// A sink accepts one caller-supplied terminal attempt and no dynamic labels
/// or text. It aggregates observations; it does not authenticate provenance or
/// detect duplicate calls.
pub trait ObservationSink {
    fn record_attempt(&self, observation: AttemptObservation);
}

#[derive(Clone)]
struct AggregateState {
    queue_latency: [u64; QUEUE_CELLS],
    response_latency: [u64; RESPONSE_LATENCY_CELLS],
    response_bytes: [u64; RESPONSE_BYTE_CELLS],
    response_bytes_unavailable: [u64; RESPONSE_BYTE_UNAVAILABLE_CELLS],
    circuit_rejections: [u64; CIRCUIT_REJECTION_CELLS],
    saturated_cell_increments: u64,
}

impl Default for AggregateState {
    fn default() -> Self {
        Self {
            queue_latency: [0; QUEUE_CELLS],
            response_latency: [0; RESPONSE_LATENCY_CELLS],
            response_bytes: [0; RESPONSE_BYTE_CELLS],
            response_bytes_unavailable: [0; RESPONSE_BYTE_UNAVAILABLE_CELLS],
            circuit_rejections: [0; CIRCUIT_REJECTION_CELLS],
            saturated_cell_increments: 0,
        }
    }
}

/// Fixed-memory coherent aggregation. Poison recovery retains observations;
/// measurement failure never changes a Tally operation result.
#[derive(Default)]
pub struct TelemetryCollector {
    state: Mutex<AggregateState>,
}

impl fmt::Debug for TelemetryCollector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryCollector")
            .field("fixed_histogram_cell_count", &FIXED_HISTOGRAM_CELL_COUNT)
            .finish_non_exhaustive()
    }
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preview_v2(&self) -> TallyTelemetryPreviewV2 {
        let state = self.snapshot();
        let rows = RequestClass::ALL.map(|class| build_row(&state, class));
        TallyTelemetryPreviewV2 {
            schema: PREVIEW_SCHEMA,
            schema_version: 2,
            privacy_profile: "fixed_dimensions_bucketed_values_v1",
            collection_scope: "unstamped_collector_instance_lifetime",
            snapshot_consistency: "coherent_mutex_snapshot",
            observation_provenance: "caller_supplied_not_authenticated",
            collection_completeness: "not_established",
            lifecycle_consistency: "one_terminal_observation_per_attempt_duplicates_not_detected",
            standards_mapping: "custom_lossy_summary_not_an_opentelemetry_histogram",
            integrity_claim: "checksum_only",
            authenticity_claim: "none",
            collector_has_network_exporter: false,
            establishes_performance_support: false,
            rows_are_taxonomy_not_capability: true,
            fixed_histogram_cell_count: FIXED_HISTOGRAM_CELL_COUNT as u16,
            latency_upper_bounds_micros: LATENCY_UPPER_BOUNDS_MICROS,
            response_byte_upper_bounds: RESPONSE_BYTE_UPPER_BOUNDS,
            saturated_cell_increments: count_bucket(state.saturated_cell_increments),
            rows,
        }
    }

    pub fn privacy_reduced_export_v2(
        &self,
    ) -> Result<PrivacyReducedTelemetryExport, TelemetryExportError> {
        let preview = self.preview_v2();
        let json =
            serde_json::to_string(&preview).map_err(|_| TelemetryExportError::Serialization)?;
        if json.len() > MAX_SERIALIZED_PREVIEW_BYTES {
            return Err(TelemetryExportError::PreviewTooLarge);
        }
        let payload_sha256 = hash_payload(json.as_bytes());
        Ok(PrivacyReducedTelemetryExport {
            json,
            payload_sha256,
        })
    }

    fn snapshot(&self) -> AggregateState {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn increment(state: &mut AggregateState, cell: &mut u64) {
        if *cell == u64::MAX {
            state.saturated_cell_increments = state.saturated_cell_increments.saturating_add(1);
        } else {
            *cell += 1;
        }
    }
}

impl ObservationSink for TelemetryCollector {
    fn record_attempt(&self, observation: AttemptObservation) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match observation {
            AttemptObservation::CircuitRejected { class, reason } => {
                let index = class.index() * CIRCUIT_REJECT_REASONS + reason.index();
                let mut cell = state.circuit_rejections[index];
                Self::increment(&mut state, &mut cell);
                state.circuit_rejections[index] = cell;
            }
            AttemptObservation::QueueDeadline { class, queue_wait } => {
                increment_queue(&mut state, class, QueueOutcome::Deadline, queue_wait);
            }
            AttemptObservation::QueueCancelled { class, queue_wait } => {
                increment_queue(&mut state, class, QueueOutcome::Cancelled, queue_wait);
            }
            AttemptObservation::Response {
                class,
                queue_wait,
                outcome,
                response_pipeline_elapsed,
                observed_body_bytes,
            } => {
                increment_queue(&mut state, class, QueueOutcome::Acquired, queue_wait);
                let latency = latency_bucket(response_pipeline_elapsed);
                let series = class.index() * RESPONSE_OUTCOMES + outcome.index();
                let latency_index = series * LATENCY_BUCKETS + latency;
                let mut latency_cell = state.response_latency[latency_index];
                Self::increment(&mut state, &mut latency_cell);
                state.response_latency[latency_index] = latency_cell;
                match observed_body_bytes {
                    BodyBytesObservation::Observed(bytes) => {
                        let byte_index = series * BYTE_BUCKETS + byte_bucket(bytes);
                        let mut byte_cell = state.response_bytes[byte_index];
                        Self::increment(&mut state, &mut byte_cell);
                        state.response_bytes[byte_index] = byte_cell;
                    }
                    BodyBytesObservation::Unavailable => {
                        let mut cell = state.response_bytes_unavailable[series];
                        Self::increment(&mut state, &mut cell);
                        state.response_bytes_unavailable[series] = cell;
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CountBucket {
    Zero,
    One,
    TwoToFive,
    SixToTwenty,
    TwentyOneToHundred,
    HundredOneToThousand,
    OverThousand,
}

#[derive(Debug, Clone, Serialize)]
pub struct TallyTelemetryPreviewV2 {
    schema: &'static str,
    schema_version: u16,
    privacy_profile: &'static str,
    collection_scope: &'static str,
    snapshot_consistency: &'static str,
    observation_provenance: &'static str,
    collection_completeness: &'static str,
    lifecycle_consistency: &'static str,
    standards_mapping: &'static str,
    integrity_claim: &'static str,
    authenticity_claim: &'static str,
    collector_has_network_exporter: bool,
    establishes_performance_support: bool,
    rows_are_taxonomy_not_capability: bool,
    fixed_histogram_cell_count: u16,
    latency_upper_bounds_micros: [u64; 8],
    response_byte_upper_bounds: [u64; 6],
    saturated_cell_increments: CountBucket,
    rows: [OperationTelemetryRow; 8],
}

impl TallyTelemetryPreviewV2 {
    pub fn rows(&self) -> &[OperationTelemetryRow; 8] {
        &self.rows
    }

    pub const fn establishes_performance_support(&self) -> bool {
        self.establishes_performance_support
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationTelemetryRow {
    request_class: RequestClass,
    circuit_rejections: [CircuitTelemetryRow; 2],
    queue: [QueueTelemetryRow; 3],
    response: [ResponseTelemetryRow; 10],
}

impl OperationTelemetryRow {
    pub const fn request_class(&self) -> RequestClass {
        self.request_class
    }
}

#[derive(Debug, Clone, Serialize)]
struct QueueTelemetryRow {
    outcome: QueueOutcome,
    latency_buckets: [CountBucket; LATENCY_BUCKETS],
}

#[derive(Debug, Clone, Serialize)]
struct ResponseTelemetryRow {
    outcome: ResponseOutcome,
    latency_buckets: [CountBucket; LATENCY_BUCKETS],
    bytes_received_buckets: [CountBucket; BYTE_BUCKETS],
    bytes_measurement_unavailable: CountBucket,
}

#[derive(Debug, Clone, Serialize)]
struct CircuitTelemetryRow {
    reason: CircuitRejectReason,
    count: CountBucket,
}

#[derive(Clone)]
pub struct PrivacyReducedTelemetryExport {
    json: String,
    payload_sha256: String,
}

impl fmt::Debug for PrivacyReducedTelemetryExport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivacyReducedTelemetryExport")
            .field("json_bytes", &self.json.len())
            .field("payload_sha256", &self.payload_sha256)
            .finish()
    }
}

impl PrivacyReducedTelemetryExport {
    pub fn json(&self) -> &str {
        &self.json
    }

    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryExportError {
    Serialization,
    PreviewTooLarge,
}

impl TelemetryExportError {
    pub const fn safe_code(self) -> &'static str {
        match self {
            Self::Serialization => "tally_telemetry_serialization_failed",
            Self::PreviewTooLarge => "tally_telemetry_preview_too_large",
        }
    }
}

impl fmt::Display for TelemetryExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.safe_code())
    }
}

impl std::error::Error for TelemetryExportError {}

fn build_row(state: &AggregateState, class: RequestClass) -> OperationTelemetryRow {
    let circuit_rejections = CircuitRejectReason::ALL.map(|reason| {
        let index = class.index() * CIRCUIT_REJECT_REASONS + reason.index();
        CircuitTelemetryRow {
            reason,
            count: count_bucket(state.circuit_rejections[index]),
        }
    });
    let queue = QueueOutcome::ALL.map(|outcome| {
        let base = (class.index() * QUEUE_OUTCOMES + outcome.index()) * LATENCY_BUCKETS;
        QueueTelemetryRow {
            outcome,
            latency_buckets: std::array::from_fn(|offset| {
                count_bucket(state.queue_latency[base + offset])
            }),
        }
    });
    let response = ResponseOutcome::ALL.map(|outcome| {
        let series = class.index() * RESPONSE_OUTCOMES + outcome.index();
        let latency_base = series * LATENCY_BUCKETS;
        let byte_base = series * BYTE_BUCKETS;
        ResponseTelemetryRow {
            outcome,
            latency_buckets: std::array::from_fn(|offset| {
                count_bucket(state.response_latency[latency_base + offset])
            }),
            bytes_received_buckets: std::array::from_fn(|offset| {
                count_bucket(state.response_bytes[byte_base + offset])
            }),
            bytes_measurement_unavailable: count_bucket(state.response_bytes_unavailable[series]),
        }
    });
    OperationTelemetryRow {
        request_class: class,
        circuit_rejections,
        queue,
        response,
    }
}

fn increment_queue(
    state: &mut AggregateState,
    class: RequestClass,
    outcome: QueueOutcome,
    elapsed: Duration,
) {
    let bucket = latency_bucket(elapsed);
    let index = (class.index() * QUEUE_OUTCOMES + outcome.index()) * LATENCY_BUCKETS + bucket;
    let mut cell = state.queue_latency[index];
    TelemetryCollector::increment(state, &mut cell);
    state.queue_latency[index] = cell;
}

fn latency_bucket(duration: Duration) -> usize {
    let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
    LATENCY_UPPER_BOUNDS_MICROS
        .iter()
        .position(|upper| micros <= *upper)
        .unwrap_or(LATENCY_BUCKETS - 1)
}

fn byte_bucket(bytes: u64) -> usize {
    RESPONSE_BYTE_UPPER_BOUNDS
        .iter()
        .position(|upper| bytes <= *upper)
        .unwrap_or(BYTE_BUCKETS - 1)
}

fn count_bucket(value: u64) -> CountBucket {
    match value {
        0 => CountBucket::Zero,
        1 => CountBucket::One,
        2..=5 => CountBucket::TwoToFive,
        6..=20 => CountBucket::SixToTwenty,
        21..=100 => CountBucket::TwentyOneToHundred,
        101..=1_000 => CountBucket::HundredOneToThousand,
        _ => CountBucket::OverThousand,
    }
}

fn hash_payload(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(EXPORT_HASH_DOMAIN);
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, thread};

    #[test]
    fn histogram_boundaries_are_inclusive_and_queue_semantics_are_explicit() {
        let collector = TelemetryCollector::new();
        for micros in [1_000, 1_001, 30_000_000, 30_000_001] {
            collector.record_attempt(AttemptObservation::Response {
                class: RequestClass::Status,
                queue_wait: Duration::from_micros(micros),
                outcome: ResponseOutcome::Success,
                response_pipeline_elapsed: Duration::ZERO,
                observed_body_bytes: BodyBytesObservation::Observed(0),
            });
        }
        for bytes in [
            0,
            1,
            1_024,
            1_025,
            32 * 1_024 * 1_024,
            32 * 1_024 * 1_024 + 1,
        ] {
            collector.record_attempt(AttemptObservation::Response {
                class: RequestClass::VoucherExport,
                queue_wait: Duration::ZERO,
                outcome: ResponseOutcome::SizeLimit,
                response_pipeline_elapsed: Duration::from_millis(10),
                observed_body_bytes: BodyBytesObservation::Observed(bytes),
            });
        }
        let preview = collector.preview_v2();
        assert_eq!(preview.rows().len(), RequestClass::ALL.len());
        assert!(!preview.establishes_performance_support());
        let state = collector.snapshot();
        let queue_base = (RequestClass::Status.index() * QUEUE_OUTCOMES
            + QueueOutcome::Acquired.index())
            * LATENCY_BUCKETS;
        assert_eq!(
            &state.queue_latency[queue_base..queue_base + LATENCY_BUCKETS],
            &[1, 1, 0, 0, 0, 0, 0, 1, 1]
        );
        let response_series = RequestClass::VoucherExport.index() * RESPONSE_OUTCOMES
            + ResponseOutcome::SizeLimit.index();
        let latency_base = response_series * LATENCY_BUCKETS;
        assert_eq!(state.response_latency[latency_base + 2], 6);
        assert_eq!(
            &state.response_bytes
                [response_series * BYTE_BUCKETS..response_series * BYTE_BUCKETS + BYTE_BUCKETS],
            &[1, 2, 1, 0, 0, 1, 1]
        );
        let json = collector.privacy_reduced_export_v2().unwrap().json;
        assert!(json.contains("\"unstamped_collector_instance_lifetime\""));
        assert!(json.contains("\"coherent_mutex_snapshot\""));
        assert!(!json.contains("post_request_spacing"));
    }

    #[test]
    fn cardinality_and_export_size_remain_fixed_after_many_observations() {
        let collector = TelemetryCollector::new();
        for index in 0..100_000_u64 {
            let class = RequestClass::ALL[index as usize % RequestClass::ALL.len()];
            collector.record_attempt(AttemptObservation::Response {
                class,
                queue_wait: Duration::from_micros(index),
                outcome: ResponseOutcome::Success,
                response_pipeline_elapsed: Duration::from_micros(index),
                observed_body_bytes: BodyBytesObservation::Observed(index),
            });
        }
        let export = collector
            .privacy_reduced_export_v2()
            .expect("bounded export");
        assert_eq!(collector.preview_v2().rows().len(), RequestClass::ALL.len());
        assert!(export.json().len() <= MAX_SERIALIZED_PREVIEW_BYTES);
        assert_eq!(FIXED_HISTOGRAM_CELL_COUNT, 1_592);
    }

    #[test]
    fn preview_has_no_input_surface_for_sensitive_or_high_cardinality_values() {
        let collector = TelemetryCollector::new();
        collector.record_attempt(AttemptObservation::Response {
            class: RequestClass::CompanyList,
            queue_wait: Duration::from_millis(2),
            outcome: ResponseOutcome::Decode,
            response_pipeline_elapsed: Duration::from_millis(7),
            observed_body_bytes: BodyBytesObservation::Observed(777),
        });
        let export = collector.privacy_reduced_export_v2().unwrap();
        let debug = format!("{collector:?} {export:?}");
        for forbidden in [
            "BRIDGE SECRET COMPANY",
            "27ABCDE1234F1Z5",
            "ABCDE1234F",
            "<ENVELOPE>",
            "127.0.0.1:9000",
            "developer-home-path-sentinel",
            "request-secret-id",
        ] {
            assert!(!export.json().contains(forbidden));
            assert!(!debug.contains(forbidden));
        }
        assert!(!export.json().contains("company_guid"));
        assert!(!export.json().contains("endpoint"));
        assert!(!export.json().contains("timestamp"));
        assert!(!export.json().contains("payload"));
    }

    #[test]
    fn preview_is_coherent_under_concurrent_recording_and_repeatable_when_idle() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Barrier,
        };

        let collector = Arc::new(TelemetryCollector::new());
        let start = Arc::new(Barrier::new(9));
        let active = Arc::new(AtomicUsize::new(8));
        let threads = (0..8)
            .map(|_| {
                let collector = Arc::clone(&collector);
                let start = Arc::clone(&start);
                let active = Arc::clone(&active);
                thread::spawn(move || {
                    start.wait();
                    for index in 0..20_000 {
                        collector.record_attempt(AttemptObservation::Response {
                            class: RequestClass::MasterExport,
                            queue_wait: Duration::from_millis(1),
                            outcome: ResponseOutcome::Success,
                            response_pipeline_elapsed: Duration::from_millis(5),
                            observed_body_bytes: BodyBytesObservation::Observed(1_024),
                        });
                        if index % 100 == 0 {
                            thread::yield_now();
                        }
                    }
                    active.fetch_sub(1, Ordering::Release);
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        let response_series = RequestClass::MasterExport.index() * RESPONSE_OUTCOMES
            + ResponseOutcome::Success.index();
        let mut concurrent_snapshots = 0_u64;
        while active.load(Ordering::Acquire) > 0 {
            let state = collector.snapshot();
            let latency_total = state.response_latency
                [response_series * LATENCY_BUCKETS..(response_series + 1) * LATENCY_BUCKETS]
                .iter()
                .sum::<u64>();
            let byte_total = state.response_bytes
                [response_series * BYTE_BUCKETS..(response_series + 1) * BYTE_BUCKETS]
                .iter()
                .sum::<u64>();
            assert_eq!(latency_total, byte_total);
            assert_eq!(collector.preview_v2().rows().len(), RequestClass::ALL.len());
            concurrent_snapshots += 1;
            thread::yield_now();
        }
        for thread in threads {
            thread.join().expect("observation thread");
        }
        assert!(concurrent_snapshots > 0);
        let first = collector.privacy_reduced_export_v2().unwrap();
        let second = collector.privacy_reduced_export_v2().unwrap();
        assert_eq!(first.json(), second.json());
        assert_eq!(first.payload_sha256(), second.payload_sha256());
    }

    #[test]
    fn saturation_never_wraps_and_is_disclosed() {
        let collector = TelemetryCollector::new();
        {
            let mut state = collector.state.lock().unwrap();
            state.queue_latency[0] = u64::MAX;
        }
        collector.record_attempt(AttemptObservation::Response {
            class: RequestClass::Status,
            queue_wait: Duration::ZERO,
            outcome: ResponseOutcome::Success,
            response_pipeline_elapsed: Duration::ZERO,
            observed_body_bytes: BodyBytesObservation::Observed(0),
        });
        let state = collector.snapshot();
        assert_eq!(state.queue_latency[0], u64::MAX);
        assert_eq!(state.saturated_cell_increments, 1);
        assert!(collector
            .privacy_reduced_export_v2()
            .unwrap()
            .json()
            .contains("\"saturated_cell_increments\":\"one\""));
    }

    #[test]
    fn longest_serialized_count_bucket_still_fits_the_reviewed_preview_ceiling() {
        let collector = TelemetryCollector::new();
        {
            let mut state = collector.state.lock().unwrap();
            state.queue_latency.fill(101);
            state.response_latency.fill(101);
            state.response_bytes.fill(101);
            state.response_bytes_unavailable.fill(101);
            state.circuit_rejections.fill(101);
            state.saturated_cell_increments = 101;
        }
        let export = collector
            .privacy_reduced_export_v2()
            .expect("worst textual bucket export");
        assert!(export.json().len() <= MAX_SERIALIZED_PREVIEW_BYTES);
        assert_eq!(collector.preview_v2().rows().len(), RequestClass::ALL.len());
    }

    #[test]
    fn schema_v2_taxonomy_bounds_and_zero_preview_bytes_are_golden() {
        assert_eq!(PREVIEW_SCHEMA, "bridge.tally.telemetry-preview/2");
        assert_eq!(
            LATENCY_UPPER_BOUNDS_MICROS,
            [1_000, 5_000, 25_000, 100_000, 500_000, 2_000_000, 10_000_000, 30_000_000,]
        );
        assert_eq!(
            RESPONSE_BYTE_UPPER_BOUNDS,
            [0, 1_024, 65_536, 1_048_576, 8_388_608, 33_554_432]
        );
        assert_eq!(QueueOutcome::ALL.len(), 3);
        assert_eq!(ResponseOutcome::ALL.len(), 10);
        assert_eq!(CircuitRejectReason::ALL.len(), 2);
        assert_eq!(FIXED_HISTOGRAM_CELL_COUNT, 1_592);
        let export = TelemetryCollector::new()
            .privacy_reduced_export_v2()
            .expect("golden zero preview");
        assert_eq!(
            export.payload_sha256(),
            "013e4b52577f9b89c22a31c203ec8940a783b3ada13e3f9d5e508b79a92ed37a"
        );
    }

    #[test]
    fn terminal_attempt_shape_keeps_queue_failures_out_of_response_histograms() {
        let collector = TelemetryCollector::new();
        collector.record_attempt(AttemptObservation::QueueDeadline {
            class: RequestClass::Capability,
            queue_wait: Duration::from_secs(30),
        });
        collector.record_attempt(AttemptObservation::QueueCancelled {
            class: RequestClass::Capability,
            queue_wait: Duration::from_millis(5),
        });
        collector.record_attempt(AttemptObservation::Response {
            class: RequestClass::Capability,
            queue_wait: Duration::from_millis(1),
            outcome: ResponseOutcome::Timeout,
            response_pipeline_elapsed: Duration::from_secs(10),
            observed_body_bytes: BodyBytesObservation::Unavailable,
        });

        let state = collector.snapshot();
        let queue_base = RequestClass::Capability.index() * QUEUE_OUTCOMES * LATENCY_BUCKETS;
        let queue_total = state.queue_latency
            [queue_base..queue_base + QUEUE_OUTCOMES * LATENCY_BUCKETS]
            .iter()
            .sum::<u64>();
        let response_base = RequestClass::Capability.index() * RESPONSE_OUTCOMES * LATENCY_BUCKETS;
        let response_total = state.response_latency
            [response_base..response_base + RESPONSE_OUTCOMES * LATENCY_BUCKETS]
            .iter()
            .sum::<u64>();
        assert_eq!(queue_total, 3);
        assert_eq!(response_total, 1);
    }

    #[test]
    fn circuit_rejections_and_unavailable_byte_measurement_are_explicit() {
        let collector = TelemetryCollector::new();
        collector.record_attempt(AttemptObservation::CircuitRejected {
            class: RequestClass::CompanyList,
            reason: CircuitRejectReason::Cooldown,
        });
        collector.record_attempt(AttemptObservation::Response {
            class: RequestClass::CompanyList,
            queue_wait: Duration::ZERO,
            outcome: ResponseOutcome::Application,
            response_pipeline_elapsed: Duration::from_millis(2),
            observed_body_bytes: BodyBytesObservation::Unavailable,
        });
        let state = collector.snapshot();
        let circuit = RequestClass::CompanyList.index() * CIRCUIT_REJECT_REASONS
            + CircuitRejectReason::Cooldown.index();
        let response = RequestClass::CompanyList.index() * RESPONSE_OUTCOMES
            + ResponseOutcome::Application.index();
        assert_eq!(state.circuit_rejections[circuit], 1);
        assert_eq!(state.response_bytes_unavailable[response], 1);
        let json = collector.privacy_reduced_export_v2().unwrap().json;
        assert!(json.contains("\"cooldown\""));
        assert!(json.contains("\"bytes_measurement_unavailable\":\"one\""));
    }
}
