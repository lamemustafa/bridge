use super::{ConnectionStatus, TallyClient, TallyCompany, TallyConfig, TallyLedger};
use super::{TallyProbeResult, TallyVoucher};
use crate::commands::VerifiedCompanyIdentity;
use crate::observability::BodyBytesObservation;
use crate::tally::connection::NativePairedRead;
use crate::tally::connection::{canonical_loopback_origin, SelectedReadObservation};
#[cfg(feature = "voucher-scan")]
use crate::tally::connection::{LedgerOpeningCoverageRead, OutstandingsSegmentObservation};
use crate::tally::connector::SealedReadRequest;
#[cfg(feature = "voucher-scan")]
use crate::tally::outstandings_runtime::{
    CalibratedSegmentPolicy, SegmentPlan, SegmentTrendGuard, MAX_SEGMENT_PAIRS_PER_SCAN,
};
use crate::tally::runtime_control::{
    EndpointCircuitState, EndpointIdentity, EndpointRuntimeSnapshot, PortableReadRuntime,
    ReadAttempt, ReadExecutionError, ReadFailureClass, ReadOperation, ReadRetryPolicy,
    TELEMETRY_PREVIEW_SCHEMA,
};
use crate::warning_codes::WarningCode;
use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::native_outstandings::{
    compute_native_outstandings, parse_company_currency, parse_native_bill_rows,
    parse_native_group_snapshot, parse_native_ledger_snapshot, render_company_currency_request,
    render_native_bills_request, render_native_group_snapshot_request,
    render_native_ledger_snapshot_request, AgeingAnchor as NativeAgeingAnchor, CompanyCurrency,
    LedgerSnapshotEntry, NativeBillsReportKind, NativeGroupSnapshot, NativeLedgerSnapshotPeriod,
    NativeMasterSnapshot, NativeOutstandingsError, NativeOverdueCrosscheck,
};
#[cfg(feature = "voucher-scan")]
use bridge_tally_protocol::outstandings::{
    assemble_partitioned_scan, assemble_scan, compute_outstandings_with_ageing_anchor,
    corroborate_empty_date_partition, nearest_non_empty_primary_partition,
    AgeingAnchor as LegacyAgeingAnchor, CompleteWitnessPair, CorroboratedDatePartition, DateWindow,
    NarrowDateWindow, PartialScan, ScanResult, SegmentVerification, StrictlyWiderDateCover,
    VoucherAlterIdHighWater, WitnessPairVerification,
};
use bridge_tally_protocol::outstandings_shared::{DateBoundaryProfile, OutstandingsReport};
use bridge_tally_transport::TallyTransportError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

const MAX_ENDPOINT_SESSIONS: usize = 32;

/// Re-enumerate the complete identity immediately before or after a scoped
/// read. Tally accepts a company name as the scope selector, so the GUID alone
/// is not a sufficient witness when company names differ only by presentation.
#[cfg(not(test))]
async fn bracket_verified_company_identity(
    client: &TallyClient,
    identity: &VerifiedCompanyIdentity,
) -> anyhow::Result<()> {
    let companies = client.fetch_companies().await?;
    if companies
        .iter()
        .any(|company| identity.is_case_or_whitespace_guid_sibling(company))
    {
        anyhow::bail!(
            "Tally returned a same-GUID company name differing only by case or whitespace"
        );
    }
    let matches = companies
        .iter()
        .filter(|company| identity.matches_observed_company(company))
        .count();
    if matches != 1 {
        anyhow::bail!("Tally complete company identity was absent or ambiguous");
    }
    Ok(())
}

#[cfg(test)]
async fn bracket_verified_company_identity(
    _client: &TallyClient,
    _identity: &VerifiedCompanyIdentity,
) -> anyhow::Result<()> {
    // Existing transport fixtures predate CompanyListV2. Targeted identity
    // tests exercise the matcher directly; production reads always bracket.
    Ok(())
}

/// Private capability witness for the future native-report + ledger-residual
/// implementation. There is intentionally no constructor: a future promotion
/// must add the release-qualified evidence and the reconciliation itself,
/// rather than merely opt the voucher scan back in.
#[cfg(feature = "voucher-scan")]
#[derive(Clone)]
struct QualifiedUnallocatedBalanceCoverage;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointKey(String);

impl EndpointKey {
    pub fn from_config(config: &TallyConfig) -> anyhow::Result<Self> {
        Ok(Self(canonical_loopback_origin(config)?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TallySessionSnapshot {
    pub session_id: String,
    pub canonical_endpoint: String,
    pub issued_requests: u64,
    pub active_requests: usize,
    pub active_request_ids: Vec<String>,
    pub consecutive_failures: u32,
    pub circuit_state: CircuitState,
    pub circuit_retry_after_unix_ms: Option<i64>,
    pub last_success_unix_ms: Option<i64>,
    pub last_failure_unix_ms: Option<i64>,
    pub cached_capability_observed_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TallyTelemetryPreviewExport {
    pub schema: &'static str,
    pub payload_sha256: String,
    pub preview_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// An operator assertion, not currency evidence exported from Tally. Unit A
/// supports only an explicit assertion that the selected company's base
/// currency is INR; no other currency can reach the INR formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum OutstandingsCurrencyAssertion {
    #[serde(rename = "INR")]
    Inr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OutstandingsLoadResult {
    Complete {
        report: Box<OutstandingsReport>,
        currency_assertion: OutstandingsCurrencyAssertion,
        /// The date whose distance from `report.as_of_yyyymmdd` determines
        /// the serialized ageing buckets. Consumers must disclose this rather
        /// than inferring it from the read path.
        ageing_anchor: OutstandingsAgeingAnchor,
        synced_at_unix_ms: i64,
        /// Total exposure carrying no bill reference, when the read path can
        /// establish it. `None` means "not computed", which is not the same as
        /// zero and must never be rendered as zero: the voucher scan cannot
        /// establish this figure, while the native path recovers it exactly
        /// from the ledger closing balances.
        ///
        /// It matters more than its size suggests. On a bulk book measured
        /// 2026-08-07 the named bills totalled Rs 10.36 lakh while the
        /// unallocated remainder was Rs 2.79 crore -- so a screen showing only
        /// the bills would be short by 96% with nothing to indicate it.
        #[serde(skip_serializing_if = "Option::is_none")]
        unallocated_total: Option<ExactDecimal>,
        /// Complete per-party unallocated exposure, largest first. The
        /// frontend applies its display limit locally so the same data can
        /// also power complete statement exports without a duplicate payload.
        ///
        /// On a book where most balances carry no bill reference, the ageing
        /// buckets describe a rounding error and this list is the actual
        /// answer -- so it is surfaced rather than collapsed into the single
        /// total above. Empty when the path cannot establish it.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        statement_unallocated_by_party: Vec<UnallocatedParty>,
        /// Every open bill the native reports returned. The frontend applies
        /// its display limit locally; this uncapped source is also what the
        /// complete statement export consumes.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        statement_open_bills: Vec<OpenBillRow>,
    },
    Partial {
        #[serde(flatten)]
        reason: OutstandingsPartialReason,
        synced_at_unix_ms: i64,
    },
}

/// A machine-readable reason for withholding outstandings totals. The stable
/// `reason_code` serialization stays compatible with the frontend while the
/// exceptional variants carry their diagnostic values as separate, typed
/// fields rather than asking presentation code to parse a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutstandingsPartialReason {
    pub reason_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_as_of_yyyymmdd: Option<TallyDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tally_as_of_yyyymmdd: Option<TallyDate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_currency_ledger_name: Option<String>,
}

impl OutstandingsPartialReason {
    pub fn code(reason_code: impl Into<String>) -> Self {
        Self {
            reason_code: reason_code.into(),
            requested_as_of_yyyymmdd: None,
            tally_as_of_yyyymmdd: None,
            foreign_currency_ledger_name: None,
        }
    }

    fn refused_as_of(requested_as_of: &TallyDate, tally_as_of: &TallyDate) -> Self {
        Self {
            reason_code: "native_outstandings_as_of_refused".to_string(),
            requested_as_of_yyyymmdd: Some(requested_as_of.clone()),
            tally_as_of_yyyymmdd: Some(tally_as_of.clone()),
            foreign_currency_ledger_name: None,
        }
    }

    pub fn foreign_currency_ledger_balance(ledger_name: String) -> Self {
        Self {
            reason_code: "company_foreign_currency_ledger_balance".to_string(),
            requested_as_of_yyyymmdd: None,
            tally_as_of_yyyymmdd: None,
            foreign_currency_ledger_name: Some(ledger_name),
        }
    }
}

/// Flattens both native reports into displayable bill rows, oldest first.
///
const MISSING_BILL_REFERENCE_LABEL: &str = "No reference reported";

fn all_open_bill_rows(
    receivable: &[bridge_tally_protocol::native_outstandings::NativeBillRow],
    payable: &[bridge_tally_protocol::native_outstandings::NativeBillRow],
    ageing_anchor: OutstandingsAgeingAnchor,
    as_of: &TallyDate,
) -> Vec<OpenBillRow> {
    let mut rows = receivable
        .iter()
        .map(|row| (row, ExposureDirection::Receivable))
        .chain(payable.iter().map(|row| (row, ExposureDirection::Payable)))
        .filter_map(|(row, kind)| {
            let amount = row.closing_balance.abs().ok()?;
            // Tally can retain a fully settled native bill row with BILLCL=0.
            // It is not an open exposure and must be removed at this native
            // boundary before any statement or working-paper consumer sees it.
            if amount.is_zero() {
                return None;
            }
            let anchor_date = match ageing_anchor {
                OutstandingsAgeingAnchor::DueDate => &row.due_date,
                OutstandingsAgeingAnchor::BillDate => &row.bill_date,
            };
            let age_days = if anchor_date > as_of {
                None
            } else {
                Some(
                    bridge_tally_protocol::native_outstandings::age_in_days(anchor_date, as_of)
                        .ok()?,
                )
            };
            Some(OpenBillRow {
                party: row.party.clone(),
                reference: if row.reference.trim().is_empty() {
                    MISSING_BILL_REFERENCE_LABEL.to_string()
                } else {
                    row.reference.clone()
                },
                bill_date: row.bill_date.as_str().to_string(),
                due_date: row.due_date.as_str().to_string(),
                amount,
                age_days,
                kind,
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .age_days
            .cmp(&left.age_days)
            .then_with(|| left.party.cmp(&right.party))
            .then_with(|| left.reference.cmp(&right.reference))
    });
    rows
}

/// Returns every unallocated party ranked by exposure, largest first.
///
/// Zero residuals are dropped rather than listed: a party whose ledger agrees
/// exactly with its bills has nothing unallocated, and showing it as a zero row
/// buries the parties that do.
fn all_unallocated_parties(
    residuals: &[bridge_tally_protocol::native_outstandings::PartyResidual],
) -> Vec<UnallocatedParty> {
    let mut ranked = residuals
        .iter()
        .filter(|residual| !residual.amount.is_zero())
        .filter_map(|residual| {
            residual.amount.abs().ok().map(|amount| UnallocatedParty {
                party: residual.party.clone(),
                amount,
                direction: if residual.amount.is_negative() {
                    ExposureDirection::Receivable
                } else {
                    ExposureDirection::Payable
                },
            })
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .amount
            .cmp_magnitude(&left.amount)
            .then_with(|| left.party.cmp(&right.party))
    });
    ranked
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenBillRow {
    pub party: String,
    pub reference: String,
    pub bill_date: String,
    pub due_date: String,
    pub amount: ExactDecimal,
    pub age_days: Option<u32>,
    /// Direction of the native report that returned this bill. A supplier
    /// advance can still be receivable, so this is balance direction rather
    /// than party role.
    pub kind: ExposureDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureDirection {
    Receivable,
    Payable,
}

impl ExposureDirection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Receivable => "Receivable",
            Self::Payable => "Payable",
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutstandingsAgeingAnchor {
    #[default]
    DueDate,
    BillDate,
}

impl OutstandingsAgeingAnchor {
    pub const fn label(self) -> &'static str {
        match self {
            Self::DueDate => "Due date",
            Self::BillDate => "Bill date",
        }
    }

    const fn native_anchor(self) -> NativeAgeingAnchor {
        match self {
            Self::DueDate => NativeAgeingAnchor::DueDate,
            Self::BillDate => NativeAgeingAnchor::BillDate,
        }
    }

    #[cfg(feature = "voucher-scan")]
    const fn legacy_anchor(self) -> LegacyAgeingAnchor {
        match self {
            Self::DueDate => LegacyAgeingAnchor::DueDate,
            Self::BillDate => LegacyAgeingAnchor::BillDate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnallocatedParty {
    pub party: String,
    pub amount: ExactDecimal,
    pub direction: ExposureDirection,
}

fn partial_result(reason: impl Into<OutstandingsPartialReason>) -> OutstandingsLoadResult {
    OutstandingsLoadResult::Partial {
        reason: reason.into(),
        synced_at_unix_ms: chrono::Utc::now().timestamp_millis(),
    }
}

enum NativeLedgerSnapshotPeriodAdmission {
    Period(NativeLedgerSnapshotPeriod),
    Partial(OutstandingsLoadResult),
}

enum NativeLedgerSnapshotAdmission {
    Snapshot(Vec<LedgerSnapshotEntry>),
    Partial(OutstandingsLoadResult),
}

fn admit_native_ledger_snapshot_period(
    boundary_profile: DateBoundaryProfile,
    books_from: TallyDate,
    as_of: TallyDate,
) -> NativeLedgerSnapshotPeriodAdmission {
    match NativeLedgerSnapshotPeriod::new(boundary_profile, books_from, as_of) {
        Ok(period) => NativeLedgerSnapshotPeriodAdmission::Period(period),
        Err(_) => NativeLedgerSnapshotPeriodAdmission::Partial(partial_result(
            "as_of_has_no_valid_window_boundary",
        )),
    }
}

fn admit_native_ledger_snapshot(
    snapshot: anyhow::Result<Vec<LedgerSnapshotEntry>>,
) -> anyhow::Result<NativeLedgerSnapshotAdmission> {
    match snapshot {
        Ok(snapshot) => Ok(NativeLedgerSnapshotAdmission::Snapshot(snapshot)),
        Err(error) => {
            let Some(NativeOutstandingsError::ForeignCurrencyLedgerBalance { ledger_name }) = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<NativeOutstandingsError>())
            else {
                return Err(error);
            };
            Ok(NativeLedgerSnapshotAdmission::Partial(partial_result(
                OutstandingsPartialReason::foreign_currency_ledger_balance(ledger_name.clone()),
            )))
        }
    }
}

impl From<&str> for OutstandingsPartialReason {
    fn from(value: &str) -> Self {
        Self::code(value)
    }
}

fn native_crosscheck_partial_reason(
    result: &bridge_tally_protocol::native_outstandings::NativeOutstandingsResult,
    requested_as_of: &TallyDate,
) -> Option<OutstandingsPartialReason> {
    match &result.overdue_crosscheck {
        NativeOverdueCrosscheck::Honored => None,
        NativeOverdueCrosscheck::Inconsistent => Some(OutstandingsPartialReason::code(
            "native_overdue_crosscheck_mismatch",
        )),
        NativeOverdueCrosscheck::RefusedAsOf { tally_as_of } => Some(
            OutstandingsPartialReason::refused_as_of(requested_as_of, tally_as_of),
        ),
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutBillReferences => {
            Some(OutstandingsPartialReason::code(
                WarningCode::NativeOutstandingsAsOfUnconfirmedWithoutBillReferences.as_str(),
            ))
        }
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence => {
            Some(OutstandingsPartialReason::code(
                WarningCode::NativeOutstandingsAsOfUnconfirmedWithoutEffectiveDateEvidence.as_str(),
            ))
        }
    }
}

#[cfg(feature = "voucher-scan")]
fn closing_coverage_partial_reason(
    closing_coverage_matches_opening: bool,
    closing_coverage_is_fully_covered_by_vouchers: bool,
) -> Option<&'static str> {
    if !closing_coverage_is_fully_covered_by_vouchers {
        Some("ledger_opening_bills_not_covered")
    } else if !closing_coverage_matches_opening {
        Some("ledger_master_identity_changed_during_scan")
    } else {
        None
    }
}

#[cfg(feature = "voucher-scan")]
fn paired_coverage_partial_reason(coverage: &LedgerOpeningCoverageRead) -> Option<&'static str> {
    match coverage {
        LedgerOpeningCoverageRead::Stable(_) => None,
        LedgerOpeningCoverageRead::Drifted => Some("ledger_master_identity_changed_during_scan"),
    }
}

fn select_date_boundary_profile(
    profile: Option<&bridge_tally_core::CapabilityProfile>,
) -> DateBoundaryProfile {
    let Some(profile) = profile else {
        return DateBoundaryProfile::ModeAgnostic;
    };
    let product = profile
        .product
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let supported_product = matches!(
        product.as_str(),
        "tallyprime" | "tallyprimeeditlog" | "tallyerp9"
    );
    let education_mode = profile.mode.as_deref().is_some_and(|mode| {
        mode.eq_ignore_ascii_case("education") || mode.eq_ignore_ascii_case("educational")
    });
    if supported_product && education_mode {
        DateBoundaryProfile::EducationRestricted
    } else {
        DateBoundaryProfile::ModeAgnostic
    }
}

#[cfg(feature = "voucher-scan")]
fn outstandings_read_failure_reason(error: &anyhow::Error) -> &'static str {
    if let Some(transport) = error.downcast_ref::<TallyTransportError>() {
        return match transport {
            TallyTransportError::EndpointInvalid { .. } => "segment_endpoint_invalid",
            TallyTransportError::PolicyInvalid { .. } => "segment_transport_policy_invalid",
            TallyTransportError::ClientInitializationFailed => {
                "segment_http_client_initialization_failed"
            }
            TallyTransportError::RequestTooLarge { .. } => "segment_request_size_limit_exceeded",
            TallyTransportError::ConnectionFailed => "segment_endpoint_unreachable",
            TallyTransportError::RequestTimedOut => "tally_segment_deadline_restart_recommended",
            TallyTransportError::RequestFailed => "segment_request_failed",
            TallyTransportError::HttpStatus { .. } => "segment_http_status_failure",
            TallyTransportError::ResponseTooLarge { .. } => "segment_response_size_limit_exceeded",
            TallyTransportError::ResponseTruncated => "segment_response_truncated",
            TallyTransportError::ResponseReadFailed => "segment_response_read_failed",
            TallyTransportError::InvalidEncoding { .. }
            | TallyTransportError::UnsupportedContentEncoding => {
                "segment_response_encoding_invalid"
            }
        };
    }
    let deadline_exceeded = error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains("request exceeded its deadline")
            || message.contains("request deadline exceeded")
    });
    if deadline_exceeded {
        "tally_segment_deadline_restart_recommended"
    } else {
        "segment_read_failed"
    }
}

/// An outstandings read transport failure must cross `execute_cancellable` as an error so
/// the endpoint circuit sees it. `fetch_outstandings` converts this back to
/// the established typed Partial only after that health boundary.
#[cfg(feature = "voucher-scan")]
#[derive(Debug, thiserror::Error)]
#[error("outstandings read transport failure: {source}")]
struct OutstandingsReadTransportFailure {
    reason_code: &'static str,
    #[source]
    source: anyhow::Error,
}

#[cfg(feature = "voucher-scan")]
fn partial_after_outstandings_read_transport_failure(
    result: anyhow::Result<OutstandingsLoadResult>,
) -> anyhow::Result<OutstandingsLoadResult> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => match error.downcast_ref::<OutstandingsReadTransportFailure>() {
            Some(failure) => Ok(partial_result(failure.reason_code)),
            None => Err(error),
        },
    }
}

#[cfg(feature = "voucher-scan")]
fn outstandings_read_transport_failure(error: anyhow::Error) -> anyhow::Error {
    let reason_code = outstandings_read_failure_reason(&error);
    anyhow::Error::new(OutstandingsReadTransportFailure {
        reason_code,
        source: error,
    })
}

#[cfg(feature = "voucher-scan")]
fn is_outstandings_transport_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<TallyTransportError>().is_some())
}

#[cfg(feature = "voucher-scan")]
async fn fetch_empty_partition_witness<F, Fut>(
    high_water: VoucherAlterIdHighWater,
    fetch: F,
) -> anyhow::Result<Result<CompleteWitnessPair, PartialScan>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<WitnessPairVerification>>,
{
    debug_assert!(high_water.get() > 0);
    match fetch().await {
        Ok(WitnessPairVerification::Complete(pair)) => Ok(Ok(pair)),
        Ok(WitnessPairVerification::Partial(partial)) => Ok(Err(partial)),
        Err(error) if is_outstandings_transport_error(&error) => Err(error),
        Err(_) => Ok(Err(PartialScan::new(
            "empty_date_witness_profile_unavailable",
        ))),
    }
}

#[derive(Debug, Default)]
struct SessionHealth {
    last_success_unix_ms: Option<i64>,
    last_failure_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthOutcome {
    TransportSuccess,
    TransportFailure,
    ApplicationRejected,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TallyRuntimeControlError {
    #[error("read_request_cancelled")]
    Cancelled,
    #[error("endpoint_queue_deadline_exceeded")]
    QueueDeadline,
    #[error("endpoint_circuit_cooldown")]
    CircuitCooldown,
    #[error("endpoint_half_open_probe_in_flight")]
    HalfOpenProbeInFlight,
    #[error("endpoint_session_capacity_reached")]
    EndpointSessionCapacity,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TallyRuntimeReadError {
    #[error("application_response_rejected")]
    ApplicationResponseRejected,
}

#[derive(Clone)]
struct CachedProbe {
    review_id: String,
    observed_at_unix_ms: i64,
    freshness_origin_unix_ms: i64,
    result: TallyProbeResult,
    reserved: bool,
}

struct TallySession {
    session_id: String,
    endpoint: EndpointKey,
    client: TallyClient,
    sequence: AtomicU64,
    active_requests: Mutex<HashMap<String, CancellationToken>>,
    health: Mutex<SessionHealth>,
    cached_probe: RwLock<Option<CachedProbe>>,
    active_ordinary_reads: AtomicU64,
}

impl TallySession {
    fn new(endpoint: EndpointKey, config: TallyConfig) -> anyhow::Result<Self> {
        Ok(Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            endpoint,
            client: TallyClient::new(config)?,
            sequence: AtomicU64::new(0),
            active_requests: Mutex::new(HashMap::new()),
            health: Mutex::new(SessionHealth::default()),
            cached_probe: RwLock::new(None),
            active_ordinary_reads: AtomicU64::new(0),
        })
    }

    #[cfg(test)]
    fn with_transport_policy(
        endpoint: EndpointKey,
        config: TallyConfig,
        policy: bridge_tally_transport::TransportPolicy,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            endpoint,
            client: TallyClient::with_transport_policy(config, policy)?,
            sequence: AtomicU64::new(0),
            active_requests: Mutex::new(HashMap::new()),
            health: Mutex::new(SessionHealth::default()),
            cached_probe: RwLock::new(None),
            active_ordinary_reads: AtomicU64::new(0),
        })
    }

    fn begin_request(self: &Arc<Self>) -> anyhow::Result<RuntimeRequest> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let request_id = format!("{}:{sequence}", self.session_id);
        let cancellation = CancellationToken::new();
        self.active_requests
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally cancellation registry is unavailable"))?
            .insert(request_id.clone(), cancellation.clone());
        Ok(RuntimeRequest {
            session: Arc::clone(self),
            request_id,
            cancellation,
        })
    }

    fn record_result(&self, outcome: HealthOutcome) {
        let Ok(mut health) = self.health.lock() else {
            return;
        };
        let now = chrono::Utc::now().timestamp_millis();
        match outcome {
            HealthOutcome::TransportSuccess => {
                health.last_success_unix_ms = Some(now);
            }
            HealthOutcome::TransportFailure => {
                health.last_failure_unix_ms = Some(now);
            }
            // A rejected/malformed application response proves a responder was
            // reached but must not erase earlier transport failures. Operator
            // cancellation says nothing about endpoint health.
            HealthOutcome::ApplicationRejected | HealthOutcome::Cancelled => {}
        }
    }

    fn cancel(&self, request_id: &str) -> anyhow::Result<bool> {
        let requests = self
            .active_requests
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally cancellation registry is unavailable"))?;
        let Some(token) = requests.get(request_id) else {
            return Ok(false);
        };
        token.cancel();
        Ok(true)
    }

    fn snapshot(
        &self,
        control: Option<EndpointRuntimeSnapshot>,
    ) -> anyhow::Result<TallySessionSnapshot> {
        let (last_success_unix_ms, fallback_last_failure_unix_ms) = {
            let health = self
                .health
                .lock()
                .map_err(|_| anyhow::anyhow!("Tally session health is unavailable"))?;
            (health.last_success_unix_ms, health.last_failure_unix_ms)
        };
        let mut active_request_ids = self
            .active_requests
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally cancellation registry is unavailable"))?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        active_request_ids.sort();
        let cached_capability_observed_at_unix_ms = self
            .cached_probe
            .read()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?
            .as_ref()
            .map(|probe| probe.observed_at_unix_ms);
        let (
            consecutive_failures,
            circuit_state,
            circuit_retry_after_unix_ms,
            last_failure_unix_ms,
        ) = match control {
            Some(control) => (
                control.consecutive_failures,
                match control.circuit_state {
                    EndpointCircuitState::Closed => CircuitState::Closed,
                    EndpointCircuitState::Open => CircuitState::Open,
                    EndpointCircuitState::HalfOpen => CircuitState::HalfOpen,
                },
                control.circuit_retry_after_unix_ms,
                control.last_failure_unix_ms,
            ),
            None => (0, CircuitState::Closed, None, fallback_last_failure_unix_ms),
        };
        Ok(TallySessionSnapshot {
            session_id: self.session_id.clone(),
            canonical_endpoint: self.endpoint.as_str().to_string(),
            issued_requests: self.sequence.load(Ordering::Relaxed),
            active_requests: active_request_ids.len(),
            active_request_ids,
            consecutive_failures,
            circuit_state,
            circuit_retry_after_unix_ms,
            last_success_unix_ms,
            last_failure_unix_ms,
            cached_capability_observed_at_unix_ms,
        })
    }
}

struct RuntimeRequest {
    session: Arc<TallySession>,
    request_id: String,
    cancellation: CancellationToken,
}

struct OrdinaryReadLease {
    session: Arc<TallySession>,
}

impl Drop for OrdinaryReadLease {
    fn drop(&mut self) {
        self.session
            .active_ordinary_reads
            .fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for RuntimeRequest {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.session.active_requests.lock() {
            requests.remove(&self.request_id);
        }
    }
}

struct SessionSlot {
    session: Arc<TallySession>,
    last_used: Instant,
}

#[cfg_attr(
    not(feature = "live-calibration-harness"),
    doc = "```compile_fail\nuse bridge_lib::tally::TallyRuntime;\nlet _ = TallyRuntime::for_billwise_lab_reconciliation_exit_check();\n```"
)]
#[derive(Clone)]
pub struct TallyRuntime {
    sessions: Arc<Mutex<HashMap<EndpointKey, SessionSlot>>>,
    runtime_identity: Arc<()>,
    control: PortableReadRuntime,
    #[cfg(feature = "voucher-scan")]
    outstandings_segment_policy: Option<CalibratedSegmentPolicy>,
    // A complete voucher scan proves only the bill allocations it can read.
    // This witness may be constructed only by a qualified residual path that
    // independently reconciles direct postings without BILLALLOCATIONS.LIST.
    // Until then, returning a Complete result would turn an unknown balance
    // into a plausible total (TALLY_PROTOCOL_REFERENCE.md §12a.6).
    #[cfg(feature = "voucher-scan")]
    unallocated_balance_coverage: Option<QualifiedUnallocatedBalanceCoverage>,
    #[cfg(feature = "voucher-scan")]
    outstandings_boundary_profile_override: Option<DateBoundaryProfile>,
    #[cfg(test)]
    transport_policy: Option<bridge_tally_transport::TransportPolicy>,
}

/// Opaque, owner-bound authority over one fresh reviewed probe.
///
/// The lease keeps the endpoint session alive and releases the reservation on
/// every unwind/abort/early-return path unless it was explicitly consumed or
/// atomically replaced. Drop never touches a different or newer review.
pub struct CachedProbeReservation {
    session: Arc<TallySession>,
    runtime_identity: Arc<()>,
    review_id: String,
    observed_at_unix_ms: i64,
    result: TallyProbeResult,
    armed: bool,
}

impl CachedProbeReservation {
    pub fn observed_at_unix_ms(&self) -> i64 {
        self.observed_at_unix_ms
    }

    pub fn result(&self) -> &TallyProbeResult {
        &self.result
    }

    pub fn review_id(&self) -> &str {
        &self.review_id
    }

    fn authorize(&self, runtime: &TallyRuntime, config: &TallyConfig) -> anyhow::Result<()> {
        if !self.armed
            || !Arc::ptr_eq(&self.runtime_identity, &runtime.runtime_identity)
            || self.session.endpoint != EndpointKey::from_config(config)?
        {
            anyhow::bail!("Tally reviewed setup operation ownership changed");
        }
        if self
            .session
            .cached_probe
            .read()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?
            .as_ref()
            .is_some_and(|probe| probe.reserved && probe.review_id == self.review_id)
        {
            Ok(())
        } else {
            anyhow::bail!("Tally reviewed setup operation ownership changed")
        }
    }

    pub fn release(&mut self) -> anyhow::Result<bool> {
        self.finish(false)
    }

    pub fn consume(&mut self) -> anyhow::Result<bool> {
        self.finish(true)
    }

    pub fn replace(
        &mut self,
        replacement_review_id: String,
        observed_at_unix_ms: i64,
        result: TallyProbeResult,
    ) -> anyhow::Result<bool> {
        if replacement_review_id.is_empty()
            || replacement_review_id.len() > 128
            || replacement_review_id.chars().any(char::is_control)
        {
            anyhow::bail!("Tally replacement review ID is invalid");
        }
        let mut cache = self
            .session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        let Some(current) = cache.as_ref() else {
            self.armed = false;
            return Ok(false);
        };
        if !self.armed || current.review_id != self.review_id || !current.reserved {
            self.armed = false;
            return Ok(false);
        }
        let freshness_origin_unix_ms = current.freshness_origin_unix_ms;
        *cache = Some(CachedProbe {
            review_id: replacement_review_id,
            observed_at_unix_ms,
            freshness_origin_unix_ms,
            result,
            reserved: false,
        });
        self.armed = false;
        Ok(true)
    }

    fn finish(&mut self, consume: bool) -> anyhow::Result<bool> {
        if !self.armed {
            return Ok(false);
        }
        let mut cache = self
            .session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        let is_reserved_match = cache
            .as_ref()
            .is_some_and(|probe| probe.review_id == self.review_id && probe.reserved);
        if !is_reserved_match {
            self.armed = false;
            return Ok(false);
        }
        if consume {
            cache.take();
        } else if let Some(probe) = cache.as_mut() {
            probe.reserved = false;
        }
        self.armed = false;
        Ok(true)
    }
}

impl Drop for CachedProbeReservation {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl Default for TallyRuntime {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            runtime_identity: Arc::new(()),
            control: PortableReadRuntime::default(),
            #[cfg(feature = "voucher-scan")]
            outstandings_segment_policy: None,
            #[cfg(feature = "voucher-scan")]
            unallocated_balance_coverage: None,
            #[cfg(feature = "voucher-scan")]
            outstandings_boundary_profile_override: None,
            #[cfg(test)]
            transport_policy: None,
        }
    }
}

fn apply_scoped_standard_identity(result: &mut TallyProbeResult, company: TallyCompany) {
    result.companies = vec![company];
    for feature in [
        bridge_tally_core::CapabilityFeatureId::LoadedCompanies,
        bridge_tally_core::CapabilityFeatureId::StableCompanyIdentity,
    ] {
        result.profile.features.insert(
            feature,
            bridge_tally_core::CapabilityEvidence {
                state: bridge_tally_core::CapabilityState::Supported,
                confidence: bridge_tally_core::EvidenceConfidence::Observed,
                safe_reason_code: Some("scoped_standard_identity_observed".to_string()),
            },
        );
    }
    result.profile.transports.insert(
        bridge_tally_core::TransportId::XmlHttp,
        bridge_tally_core::CapabilityEvidence {
            state: bridge_tally_core::CapabilityState::Supported,
            confidence: bridge_tally_core::EvidenceConfidence::Observed,
            safe_reason_code: Some("standard_ledger_identity_profile_observed".to_string()),
        },
    );
}

impl TallyRuntime {
    #[cfg(test)]
    pub(crate) fn with_transport_policy(policy: bridge_tally_transport::TransportPolicy) -> Self {
        Self {
            transport_policy: Some(policy),
            ..Self::default()
        }
    }

    /// Manual-only admission for the ignored Billwise Lab reconciliation
    /// check. Both owner-bound target ports are Educational instances, so the
    /// harness also carries that attested compatibility profile. No generic or
    /// default-build width/profile constructor exists.
    #[cfg(feature = "live-calibration-harness")]
    pub fn for_billwise_lab_reconciliation_exit_check() -> Self {
        Self {
            outstandings_segment_policy: Some(
                CalibratedSegmentPolicy::for_billwise_lab_exit_check(),
            ),
            outstandings_boundary_profile_override: Some(DateBoundaryProfile::EducationRestricted),
            ..Self::default()
        }
    }

    fn session(&self, config: TallyConfig) -> anyhow::Result<Arc<TallySession>> {
        let endpoint = EndpointKey::from_config(&config)?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        if let Some(slot) = sessions.get_mut(&endpoint) {
            slot.last_used = Instant::now();
            return Ok(Arc::clone(&slot.session));
        }
        if sessions.len() >= MAX_ENDPOINT_SESSIONS {
            let inactive_oldest =
                sessions
                    .iter()
                    .filter(|(_, slot)| {
                        Arc::strong_count(&slot.session) == 1
                            && slot.session.cached_probe.read().is_ok_and(|cache| {
                                !cache.as_ref().is_some_and(|probe| probe.reserved)
                            })
                    })
                    .min_by_key(|(_, slot)| slot.last_used)
                    .map(|(key, _)| key.clone());
            if let Some(key) = inactive_oldest {
                sessions.remove(&key);
            } else {
                anyhow::bail!("Tally runtime endpoint-session limit is in use");
            }
        }
        #[cfg(test)]
        let session = Arc::new(match self.transport_policy {
            Some(policy) => TallySession::with_transport_policy(endpoint.clone(), config, policy)?,
            None => TallySession::new(endpoint.clone(), config)?,
        });
        #[cfg(not(test))]
        let session = Arc::new(TallySession::new(endpoint.clone(), config)?);
        sessions.insert(
            endpoint,
            SessionSlot {
                session: Arc::clone(&session),
                last_used: Instant::now(),
            },
        );
        Ok(session)
    }

    async fn execute<T, F, Fut>(
        &self,
        config: TallyConfig,
        operation_class: ReadOperation,
        retry: ReadRetryPolicy,
        operation: F,
    ) -> anyhow::Result<T>
    where
        F: FnMut(TallyClient) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.execute_cancellable(config, None, operation_class, retry, operation)
            .await
    }

    async fn execute_cancellable<T, F, Fut>(
        &self,
        config: TallyConfig,
        external_cancellation: Option<CancellationToken>,
        operation_class: ReadOperation,
        retry: ReadRetryPolicy,
        mut operation: F,
    ) -> anyhow::Result<T>
    where
        F: FnMut(TallyClient) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let session = self.session(config)?;
        let request = session.begin_request()?;
        let client = session.client.clone();
        let endpoint = EndpointIdentity::new(session.endpoint.as_str().to_string())
            .map_err(anyhow::Error::new)?;
        let effective_cancellation = request.cancellation.child_token();
        let external_watcher = external_cancellation.map(|external| {
            let effective_cancellation = effective_cancellation.clone();
            tokio::spawn(async move {
                external.cancelled().await;
                effective_cancellation.cancel();
            })
        });
        let result = self
            .control
            .execute_read(
                endpoint,
                operation_class,
                retry,
                effective_cancellation,
                move |_| {
                    let attempt_client = client.clone();
                    attempt_client.reset_observed_body_bytes();
                    let future = operation(attempt_client.clone());
                    async move {
                        let observed_body_bytes = || {
                            if operation_class == ReadOperation::Capability {
                                BodyBytesObservation::Unavailable
                            } else {
                                attempt_client
                                    .observed_body_bytes()
                                    .map(BodyBytesObservation::Observed)
                                    .unwrap_or(BodyBytesObservation::Unavailable)
                            }
                        };
                        match future.await {
                            Ok(value) => ReadAttempt::Success {
                                value,
                                observed_body_bytes: observed_body_bytes(),
                            },
                            Err(error) => ReadAttempt::Failure {
                                class: classify_failure(&error),
                                error,
                                observed_body_bytes: observed_body_bytes(),
                            },
                        }
                    }
                },
            )
            .await;
        if let Some(watcher) = external_watcher {
            watcher.abort();
        }
        let health_outcome = match &result {
            Ok(_) => HealthOutcome::TransportSuccess,
            Err(ReadExecutionError::Attempt(error)) => classify_error(error),
            Err(ReadExecutionError::Cancelled) => HealthOutcome::Cancelled,
            Err(
                ReadExecutionError::QueueDeadline
                | ReadExecutionError::CircuitRejected { .. }
                | ReadExecutionError::EndpointSessionLimit,
            ) => HealthOutcome::ApplicationRejected,
        };
        session.record_result(health_outcome);
        result.map_err(map_execution_error)
    }

    pub async fn check_connection(&self, config: TallyConfig) -> anyhow::Result<ConnectionStatus> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::Status,
            ReadRetryPolicy::transient_default(),
            |client| async move { client.check_connection_strict().await },
        )
        .await
    }

    pub async fn probe_with_observation(
        &self,
        config: TallyConfig,
    ) -> anyhow::Result<(String, i64, TallyProbeResult)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let session = self.session(config.clone())?;
        let result = self
            .execute(
                config,
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe().await },
            )
            .await?;
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let review_id = uuid::Uuid::new_v4().to_string();
        let mut cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            anyhow::bail!("Tally reviewed setup save is in progress");
        }
        *cache = Some(CachedProbe {
            review_id: review_id.clone(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: result.clone(),
            reserved: false,
        });
        Ok((review_id, observed_at_unix_ms, result))
    }

    /// Establishes one setup-review candidate only after the direct listing is
    /// re-read and a separate shaped standard ledger collection confirms its
    /// computed name/GUID context. This never upgrades the direct listing
    /// itself into evidence.
    pub async fn bootstrap_direct_company_with_observation(
        &self,
        config: TallyConfig,
        candidate_name: String,
    ) -> anyhow::Result<(String, i64, TallyProbeResult)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let session = self.session(config.clone())?;
        let mut result = self
            .execute(
                config.clone(),
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe().await },
            )
            .await?;
        if !result.companies.is_empty() {
            anyhow::bail!("Tally direct company bootstrap was not required");
        }
        let company = self
            .execute(
                config,
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                move |client| {
                    let candidate_name = candidate_name.clone();
                    async move { client.bootstrap_direct_company(&candidate_name).await }
                },
            )
            .await?;
        apply_scoped_standard_identity(&mut result, company);
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let review_id = uuid::Uuid::new_v4().to_string();
        let mut cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            anyhow::bail!("Tally reviewed setup save is in progress");
        }
        *cache = Some(CachedProbe {
            review_id: review_id.clone(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: result.clone(),
            reserved: false,
        });
        Ok((review_id, observed_at_unix_ms, result))
    }

    /// Observe the endpoint for snapshot admission without creating or replacing
    /// an interactive setup review. Snapshot start and end probes are lifecycle
    /// evidence, not user-reviewed setup state, so they must remain uncached.
    pub(crate) async fn snapshot_probe_with_observation(
        &self,
        config: TallyConfig,
        expected_company_name: &str,
    ) -> anyhow::Result<(i64, TallyProbeResult)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let mut result = self
            .execute(
                config.clone(),
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe().await },
            )
            .await?;
        if result.companies.is_empty() {
            let expected_company_name = expected_company_name.to_string();
            let company = self
                .execute(
                    config,
                    ReadOperation::Capability,
                    ReadRetryPolicy::SINGLE_ATTEMPT,
                    move |client| {
                        let expected_company_name = expected_company_name.clone();
                        async move {
                            client
                                .bootstrap_direct_company(&expected_company_name)
                                .await
                        }
                    },
                )
                .await?;
            apply_scoped_standard_identity(&mut result, company);
        }
        Ok((chrono::Utc::now().timestamp_millis(), result))
    }

    pub async fn probe(&self, config: TallyConfig) -> anyhow::Result<TallyProbeResult> {
        self.probe_with_observation(config)
            .await
            .map(|(_, _, result)| result)
    }

    pub async fn fetch_companies(&self, config: TallyConfig) -> anyhow::Result<Vec<TallyCompany>> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::CompanyList,
            ReadRetryPolicy::transient_default(),
            |client| async move { client.fetch_companies().await },
        )
        .await
    }

    /// Re-enumerates companies while one reviewed setup operation holds the
    /// exclusive cached-probe reservation. This is deliberately separate from
    /// `fetch_companies`: an ordinary read must remain forbidden while setup
    /// authority is reserved, but the reservation owner needs a fresh tuple
    /// check before it can qualify its selected reads.
    pub async fn fetch_companies_for_reservation(
        &self,
        config: TallyConfig,
        reservation: &CachedProbeReservation,
    ) -> anyhow::Result<Vec<TallyCompany>> {
        reservation.authorize(self, &config)?;
        self.execute(
            config,
            ReadOperation::CompanyList,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            |client| async move { client.fetch_companies().await },
        )
        .await
    }

    pub async fn fetch_ledgers(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let boundary_profile = self.master_ledger_export_boundary_profile(&config)?;
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let identity = identity.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    let ledgers = client
                        .fetch_ledgers(
                            identity.display_name(),
                            identity.company_guid(),
                            boundary_profile,
                        )
                        .await?;
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(ledgers)
                }
            },
        )
        .await
    }

    /// Fetches the limited, documented standard collection response used for
    /// compatibility diagnostics. This is intentionally separate from the
    /// Bridge ledger export: it returns only ledger names and parents and is
    /// never eligible for qualification or synchronization.
    pub async fn fetch_standard_ledger_catalog(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    let ledgers = client
                        .fetch_standard_ledger_catalog(
                            identity.display_name(),
                            identity.company_guid(),
                        )
                        .await?;
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(ledgers)
                }
            },
        )
        .await
    }

    pub async fn qualify_selected_ledgers(
        &self,
        config: TallyConfig,
        reservation: &CachedProbeReservation,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<SelectedReadObservation> {
        reservation.authorize(self, &config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    let observation = client
                        .qualify_selected_ledgers(identity.display_name(), identity.company_guid())
                        .await?;
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(observation)
                }
            },
        )
        .await
    }

    pub async fn fetch_vouchers(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        from: String,
        to: String,
    ) -> anyhow::Result<Vec<TallyVoucher>> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let identity = identity.clone();
                let from = from.clone();
                let to = to.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    let vouchers = client
                        .fetch_vouchers(
                            identity.display_name(),
                            identity.company_guid(),
                            &from,
                            &to,
                        )
                        .await?;
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(vouchers)
                }
            },
        )
        .await
    }

    /// Outstandings via Tally's own `TYPE=Data` bills reports plus one ledger
    /// snapshot.
    ///
    /// Four paired reads, bracketed by a GUID-pinned company extent probe
    /// before and after. The extent probe is what binds identity: the native
    /// report carries **no GUID anywhere**, so it cannot be identity-checked
    /// from its own bytes. It does fail closed on an unloaded company
    /// (`STATUS=0`, `LINEERROR: Could not set 'SVCurrentCompany'`, verified
    /// live 2026-08-07), which the Collection path does not -- that path
    /// silently substitutes whichever company is loaded.
    ///
    /// The bills reports alone are **not** complete: unallocated "on account"
    /// balances carry no bill reference and appear in neither report. The
    /// ledger snapshot recovers them exactly, as
    /// `CLOSINGBALANCE - sum(BILLCL)` per party -- measured to 0.00 to the
    /// paisa on every bill-carrying party of both a bill-dominated book (6 of
    /// 10 parties exact, residual Rs 1,05,000) and an on-account-dominated one
    /// (7 of 7 exact, residual Rs 2.79 crore against Rs 10.36 lakh of named
    /// bills). Reporting the bills reports without that residual would show
    /// 3.7% of exposure on the second book, with no error.
    async fn fetch_outstandings_native(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: OutstandingsCurrencyAssertion,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<OutstandingsLoadResult> {
        let boundary_profile = self.master_ledger_export_boundary_profile(&config)?;
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                let as_of = as_of.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    let company = identity.display_name();
                    let expected_company_guid = identity.company_guid();
                    let extent = client
                        .fetch_company_book_extent(company, expected_company_guid)
                        .await?;
                    if &as_of < extent.books_from() {
                        return Ok(partial_result("as_of_precedes_books_from"));
                    }
                    let books_from = extent.books_from().clone();
                    let snapshot_period = match admit_native_ledger_snapshot_period(
                        boundary_profile,
                        books_from.clone(),
                        as_of.clone(),
                    ) {
                        NativeLedgerSnapshotPeriodAdmission::Period(period) => period,
                        NativeLedgerSnapshotPeriodAdmission::Partial(partial) => {
                            return Ok(partial);
                        }
                    };
                    let mut total_bytes = 0usize;
                    let read =
                        |kind| render_native_bills_request(kind, company, &books_from, &as_of);
                    let receivable = client
                        .fetch_native_report_paired(read(NativeBillsReportKind::Receivable))
                        .await?;
                    let NativePairedRead::Stable {
                        body: receivable_body,
                        encoded_bytes,
                    } = receivable
                    else {
                        return Ok(partial_result("native_bills_report_drifted"));
                    };
                    total_bytes += encoded_bytes;

                    // A ledger's immediate parent can be an arbitrary custom
                    // subgroup. The native group snapshot resolves it all the
                    // way to Sundry Debtors/Creditors without importing the
                    // legacy custom-TDL profile.
                    let groups = client
                        .fetch_native_report_paired(render_native_group_snapshot_request(company))
                        .await?;
                    let NativePairedRead::Stable {
                        body: group_body,
                        encoded_bytes,
                    } = groups
                    else {
                        return Ok(partial_result("native_group_snapshot_drifted"));
                    };
                    total_bytes += encoded_bytes;

                    let payable = client
                        .fetch_native_report_paired(read(NativeBillsReportKind::Payable))
                        .await?;
                    let NativePairedRead::Stable {
                        body: payable_body,
                        encoded_bytes,
                    } = payable
                    else {
                        return Ok(partial_result("native_bills_report_drifted"));
                    };
                    total_bytes += encoded_bytes;

                    let ledgers = client
                        .fetch_native_report_paired(render_native_ledger_snapshot_request(
                            company,
                            &snapshot_period,
                        ))
                        .await?;
                    let NativePairedRead::Stable {
                        body: ledger_body,
                        encoded_bytes,
                    } = ledgers
                    else {
                        return Ok(partial_result("native_ledger_snapshot_drifted"));
                    };
                    total_bytes += encoded_bytes;

                    // Re-pin after every read. The native rows cannot be
                    // GUID-checked individually, so an unchanged extent across
                    // the whole sequence is the only identity evidence
                    // available.
                    let closing_extent = client
                        .fetch_company_book_extent(company, expected_company_guid)
                        .await?;
                    if closing_extent != extent {
                        return Ok(partial_result("book_changed_during_read"));
                    }
                    bracket_verified_company_identity(&client, &identity).await?;

                    let receivable_rows =
                        parse_native_bill_rows(&receivable_body, &books_from, &as_of)?;
                    let payable_rows = parse_native_bill_rows(&payable_body, &books_from, &as_of)?;
                    let ledger_rows = match admit_native_ledger_snapshot(
                        parse_native_ledger_snapshot(&ledger_body).map_err(anyhow::Error::from),
                    )? {
                        NativeLedgerSnapshotAdmission::Snapshot(snapshot) => snapshot,
                        NativeLedgerSnapshotAdmission::Partial(partial) => return Ok(partial),
                    };
                    let group_rows =
                        parse_native_group_snapshot(&group_body, expected_company_guid)?;

                    // Ageing anchors on the DUE date. Measured 2026-08-07: on a
                    // bill carrying a 30-day credit period Tally's own
                    // BILLOVERDUE counted 61 days, which is the age from
                    // BILLDUE, not the 91 days from BILLDATE. Where no credit
                    // period exists the two dates coincide, so this is correct
                    // on both books.
                    let result = compute_native_outstandings(
                        company,
                        &receivable_rows,
                        &payable_rows,
                        NativeMasterSnapshot {
                            ledgers: &ledger_rows,
                            groups: NativeGroupSnapshot::Complete(&group_rows),
                        },
                        ageing_anchor.native_anchor(),
                        &as_of,
                        total_bytes,
                    )?;

                    if let Some(reason) = native_crosscheck_partial_reason(&result, &as_of) {
                        return Ok(partial_result(reason));
                    }

                    let statement_open_bills =
                        all_open_bill_rows(&receivable_rows, &payable_rows, ageing_anchor, &as_of);
                    let statement_unallocated_by_party = all_unallocated_parties(&result.residuals);
                    Ok(OutstandingsLoadResult::Complete {
                        report: Box::new(result.report),
                        currency_assertion,
                        ageing_anchor,
                        synced_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                        unallocated_total: Some(result.residual_total),
                        statement_unallocated_by_party,
                        statement_open_bills,
                    })
                }
            },
        )
        .await
    }

    /// Reads the company's currency masters so Bridge can establish the base
    /// currency itself.
    ///
    /// The INR assertion is a real safety property -- formatting a foreign
    /// balance with a rupee symbol misstates money -- but it is a fact Tally
    /// holds, and making the operator click it on every company was a step the
    /// product could answer for itself. This satisfies the assertion rather
    /// than removing it: where the answer is not determinable (several
    /// currencies defined, or a non-Indian one), the caller still has to ask.
    pub async fn detect_base_currency(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<CompanyCurrency> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    // Pin identity first: a currency read against the wrong
                    // company is worse than none.
                    let extent = client
                        .fetch_company_book_extent(identity.display_name(), identity.company_guid())
                        .await?;
                    let body = client
                        .fetch_native_report_paired(render_company_currency_request(
                            identity.display_name(),
                        ))
                        .await?;
                    let NativePairedRead::Stable { body, .. } = body else {
                        anyhow::bail!("Tally currency masters changed between paired reads");
                    };
                    let closing_extent = client
                        .fetch_company_book_extent(identity.display_name(), identity.company_guid())
                        .await?;
                    if closing_extent != extent {
                        anyhow::bail!("Tally company book changed during currency detection");
                    }
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(parse_company_currency(&body)?)
                }
            },
        )
        .await
    }

    /// With `voucher-scan` off, the legacy scan cannot execute in any shipped
    /// build (its only width-calibration constructors are `#[cfg(test)]` and
    /// `#[cfg(feature = "live-calibration-harness")]`, and this crate's
    /// default build has neither), so this simply *is* the native path: no
    /// `Option`, no branch, no dead arm to compile in and never take.
    #[cfg(not(feature = "voucher-scan"))]
    pub async fn fetch_outstandings(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: OutstandingsCurrencyAssertion,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<OutstandingsLoadResult> {
        self.fetch_outstandings_native(config, identity, as_of, currency_assertion, ageing_anchor)
            .await
    }

    #[cfg(feature = "voucher-scan")]
    pub async fn fetch_outstandings(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: OutstandingsCurrencyAssertion,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<OutstandingsLoadResult> {
        // Tally's own Bills Receivable/Payable reports answer this question in
        // O(open bills) instead of O(vouchers), so they need no segment
        // calibration at all. Measured 2026-08-07 against the same book the
        // voucher scan reconciles against: identical 48 open bills,
        // Rs 45,14,597 receivable and 4/4/4/36 ageing, in 0.21 s and 11 KB
        // against the scan's 8.30 s, 54 requests and 3.44 MB.
        //
        // This ordering matters for a reason that is not a preference: a
        // production build has no calibrated width by construction --
        // `CalibratedSegmentPolicy`'s only constructors are `#[cfg(test)]` and
        // `#[cfg(feature = "live-calibration-harness")]`, and `Self::default`
        // sets the field to `None`. Before this, the command returned
        // `outstandings_segment_sizing_uncalibrated` for every company on every
        // book, before any request, and the screen could only ever say "No
        // Tally data was read".
        let Some(segment_policy) = self.outstandings_segment_policy else {
            return self
                .fetch_outstandings_native(
                    config,
                    identity,
                    as_of,
                    currency_assertion,
                    ageing_anchor,
                )
                .await;
        };
        let Some(_coverage) = self.unallocated_balance_coverage.as_ref() else {
            return Ok(partial_result("unallocated_direct_postings_not_covered"));
        };
        let cached_probe = self.cached_probe(&config)?;
        let boundary_profile = self
            .outstandings_boundary_profile_override
            .unwrap_or_else(|| {
                select_date_boundary_profile(cached_probe.as_ref().map(|probe| &probe.profile))
            });
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        let result = self
            .execute(
                config,
                ReadOperation::VoucherExport,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                move |client| {
                    let identity = identity.clone();
                    let as_of = as_of.clone();
                    async move {
                        bracket_verified_company_identity(&client, &identity).await?;
                        let company = identity.display_name();
                        let expected_company_guid = identity.company_guid();
                        let extent = client
                            .fetch_company_book_extent(company, expected_company_guid)
                            .await?;
                        // The reporting window must never run past the as-of date.
                        // A future-dated voucher pushes LastVoucherDate beyond
                        // today, and a window ending there makes the computation
                        // reject the entire read rather than simply excluding
                        // future activity. Clamp through the compatibility profile,
                        // because an Education boundary is legal only on day
                        // 01/02/31.
                        let cutoff = if extent.last_voucher_date() <= &as_of {
                            extent.last_voucher_date().clone()
                        } else {
                            let Some(clamped) =
                                boundary_profile.latest_boundary_at_or_before(&as_of)
                            else {
                                return Ok(partial_result("as_of_has_no_valid_window_boundary"));
                            };
                            clamped
                        };
                        if &cutoff < extent.books_from() {
                            return Ok(partial_result("as_of_precedes_books_from"));
                        }
                        // One extra paired read, before any voucher segment. Bills
                        // opened by a ledger's bill-wise OPENING balance exist with
                        // no voucher at all, so this scan cannot observe them.
                        // Detect them and refuse to claim Complete rather than
                        // silently under-report a client's outstandings.
                        let opening_coverage = client
                            .fetch_ledger_opening_coverage(extent.company())
                            .await?;
                        if let Some(reason_code) = paired_coverage_partial_reason(&opening_coverage) {
                            return Ok(partial_result(reason_code));
                        }
                        let LedgerOpeningCoverageRead::Stable(opening_coverage) = opening_coverage
                        else {
                            unreachable!(
                                "paired coverage drift returns before a stable value is required"
                            )
                        };
                        if !opening_coverage.is_fully_covered_by_vouchers() {
                            return Ok(partial_result("ledger_opening_bills_not_covered"));
                        }
                        let requested = DateWindow::parse(
                            boundary_profile,
                            extent.books_from().as_str(),
                            cutoff.as_str(),
                        )?;
                        let Some(high_water) = extent.voucher_alter_id_high_water() else {
                            return Ok(partial_result(
                                "company_voucher_alter_id_high_water_missing",
                            ));
                        };
                        let date_partitions = requested.narrow_partitions()?;
                        let plan = SegmentPlan::new(
                            date_partitions.len(),
                            high_water.get(),
                            segment_policy,
                        )?;
                        tracing::info!(
                            target: "bridge::tally::outstandings",
                            date_partitions = plan.date_partitions,
                            alter_id_high_water = plan.alter_id_high_water,
                            initial_width = plan.initial_width,
                            planned_primary_segment_pairs = plan.planned_primary_segment_pairs,
                            reserved_empty_partition_witness_pairs = plan.reserved_empty_partition_witness_pairs,
                            planned_segment_pairs = plan.planned_segment_pairs,
                            maximum_segment_pairs = MAX_SEGMENT_PAIRS_PER_SCAN,
                            admitted = plan.is_admitted(),
                            "planned outstandings segment scan"
                        );
                        let Some(mut pair_budget) = plan.admitted_budget() else {
                            return Ok(partial_result("outstandings_segment_plan_exceeds_budget"));
                        };
                        let mut trend_guard = SegmentTrendGuard::new(segment_policy);
                        let mut completed_date_partitions = Vec::new();
                        for segment_window in date_partitions {
                            let verification_window = segment_window.as_date_window().clone();
                            let mut segments = Vec::new();
                            let mut cursor = 0_u64;
                            while let Some(alter_id_range) =
                                trend_guard.next_range(cursor, high_water.get())?
                            {
                                if !pair_budget.admit_next() {
                                    // NOT the preflight refusal: reaching here means
                                    // the trend guard shrank the width after the plan
                                    // was computed, so live requests have already been
                                    // spent. The UI renders the preflight code as "no
                                    // voucher scan started", which would be false.
                                    return Ok(partial_result(
                                        "outstandings_segment_budget_exhausted_mid_scan",
                                    ));
                                }
                                let observation = match client
                                    .fetch_outstandings_segment_pair(
                                        extent.company(),
                                        segment_window.clone(),
                                        alter_id_range,
                                    )
                                    .await
                                {
                                    Ok(observation) => observation,
                                    Err(error) => {
                                        return Err(outstandings_read_transport_failure(error));
                                    }
                                };
                                let OutstandingsSegmentObservation {
                                    verification,
                                    first_read_elapsed,
                                    second_read_elapsed,
                                } = observation;
                                let max_read_elapsed = first_read_elapsed.max(second_read_elapsed);
                                match verification {
                                    SegmentVerification::Complete(segment) => {
                                        let end = segment.alter_id_range().inclusive_end();
                                        let should_stop = trend_guard
                                            .observe_complete_segment(&segment, max_read_elapsed);
                                        segments.push(SegmentVerification::Complete(segment));
                                        if should_stop {
                                            return Ok(partial_result(
                                            "tally_segment_latency_trending_restart_recommended",
                                        ));
                                        }
                                        cursor = end;
                                    }
                                    SegmentVerification::Partial(partial) => {
                                        return Ok(partial_result(partial.reason_code.as_str()))
                                    }
                                }
                            }
                            match assemble_scan(
                                extent.company().clone(),
                                verification_window,
                                high_water,
                                segments,
                            ) {
                                ScanResult::Complete(scan) => completed_date_partitions.push(scan),
                                ScanResult::Partial(partial) => {
                                    return Ok(partial_result(partial.reason_code.as_str()))
                                }
                            }
                        }
                        // `VoucherEmptyPartitionWitnessV1` was qualified under
                        // supervised dispatch. A positive-high-water empty primary
                        // partition now needs its nearest non-empty control plus
                        // every date-shifted cover slice before it can enter totals.
                        let mut corroborated_date_partitions =
                            Vec::with_capacity(completed_date_partitions.len());
                        for partition in &completed_date_partitions {
                            let corroborated = if partition.vouchers().is_empty() {
                                if high_water.get() == 0 {
                                    CorroboratedDatePartition::empty_book(partition.clone())
                                } else {
                                    let primary = match NarrowDateWindow::try_from(
                                        partition.window().clone(),
                                    ) {
                                        Ok(window) => window,
                                        Err(_) => {
                                            return Ok(partial_result(
                                                "empty_date_witness_scope_mismatch",
                                            ))
                                        }
                                    };
                                    let Some(cover) = StrictlyWiderDateCover::for_primary(&primary)
                                    else {
                                        return Ok(partial_result(
                                            "empty_date_witness_cover_unavailable",
                                        ));
                                    };
                                    let Some(control) = nearest_non_empty_primary_partition(
                                        &completed_date_partitions,
                                        partition.window(),
                                    ) else {
                                        return Ok(partial_result("empty_date_partition_no_control"));
                                    };
                                    let control_window = match NarrowDateWindow::try_from(
                                        control.window().clone(),
                                    ) {
                                        Ok(window) => window,
                                        Err(_) => {
                                            return Ok(partial_result(
                                                "empty_date_witness_control_scope_mismatch",
                                            ))
                                        }
                                    };
                                    if !pair_budget.admit_next() {
                                        return Ok(partial_result(
                                            "outstandings_segment_budget_exhausted_mid_scan",
                                        ));
                                    }
                                    let control_pair = match fetch_empty_partition_witness(
                                        high_water,
                                        || {
                                            client.fetch_empty_partition_witness_pair(
                                                extent.company(),
                                                control_window,
                                            )
                                        },
                                    )
                                    .await
                                    {
                                        Ok(Ok(pair)) => pair,
                                        Ok(Err(partial)) => {
                                            return Ok(partial_result(partial.reason_code.as_str()))
                                        }
                                        Err(error) => {
                                            return Err(outstandings_read_transport_failure(error))
                                        }
                                    };
                                    let mut cover_pairs = Vec::with_capacity(cover.slices().len());
                                    for slice in cover.slices() {
                                        if !pair_budget.admit_next() {
                                            return Ok(partial_result(
                                                "outstandings_segment_budget_exhausted_mid_scan",
                                            ));
                                        }
                                        let pair = match fetch_empty_partition_witness(
                                            high_water,
                                            || {
                                                client.fetch_empty_partition_witness_pair(
                                                    extent.company(),
                                                    slice.clone(),
                                                )
                                            },
                                        )
                                        .await
                                        {
                                            Ok(Ok(pair)) => pair,
                                            Ok(Err(partial)) => {
                                                return Ok(partial_result(partial.reason_code.as_str()))
                                            }
                                            Err(error) => {
                                                return Err(outstandings_read_transport_failure(error))
                                            }
                                        };
                                        cover_pairs.push(pair);
                                    }
                                    corroborate_empty_date_partition(
                                        partition.clone(),
                                        &completed_date_partitions,
                                        cover,
                                        control_pair,
                                        cover_pairs,
                                    )
                                }
                            } else {
                                CorroboratedDatePartition::non_empty(partition.clone())
                            };
                            match corroborated {
                                Ok(partition) => corroborated_date_partitions.push(partition),
                                Err(partial) => {
                                    return Ok(partial_result(partial.reason_code.as_str()))
                                }
                            }
                        }
                        // A scan is not instantaneous: primary segments and empty
                        // partition witnesses must describe one book state before
                        // a Complete result can be assembled.
                        let closing_extent = client
                            .fetch_company_book_extent(
                                extent.company().name(),
                                extent.company().guid(),
                            )
                            .await?;
                        if closing_extent != extent {
                            return Ok(partial_result("book_changed_during_scan"));
                        }
                        // The extent does not cover bill-wise ledger openings, so
                        // revalidate their GUID-to-name coverage after the witness
                        // loop as well.
                        let closing_coverage = client
                            .fetch_ledger_opening_coverage(extent.company())
                            .await?;
                        if let Some(reason_code) = paired_coverage_partial_reason(&closing_coverage) {
                            return Ok(partial_result(reason_code));
                        }
                        let LedgerOpeningCoverageRead::Stable(closing_coverage) = closing_coverage
                        else {
                            unreachable!(
                                "paired coverage drift returns before a stable value is required"
                            )
                        };
                        if let Some(reason_code) = closing_coverage_partial_reason(
                            closing_coverage == opening_coverage,
                            closing_coverage.is_fully_covered_by_vouchers(),
                        ) {
                            return Ok(partial_result(reason_code));
                        }
                        bracket_verified_company_identity(&client, &identity).await?;
                        match assemble_partitioned_scan(
                            &extent,
                            requested,
                            corroborated_date_partitions,
                        ) {
                            ScanResult::Complete(scan) => Ok(OutstandingsLoadResult::Complete {
                                report: Box::new(compute_outstandings_with_ageing_anchor(
                                    &scan,
                                    as_of,
                                    ageing_anchor.legacy_anchor(),
                                )?),
                                currency_assertion,
                                ageing_anchor,
                                synced_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                                // The voucher scan derives bills from vouchers
                                // and cannot establish the unallocated
                                // remainder, so it must stay absent rather
                                // than be reported as zero.
                                unallocated_total: None,
                                statement_unallocated_by_party: Vec::new(),
                                statement_open_bills: Vec::new(),
                            }),
                            ScanResult::Partial(partial) => {
                                Ok(partial_result(partial.reason_code.as_str()))
                            }
                        }
                    }
                },
            )
            .await;
        partial_after_outstandings_read_transport_failure(result)
    }

    pub async fn qualify_selected_vouchers(
        &self,
        config: TallyConfig,
        reservation: &CachedProbeReservation,
        identity: &VerifiedCompanyIdentity,
        from: String,
        to: String,
    ) -> anyhow::Result<SelectedReadObservation> {
        reservation.authorize(self, &config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                let from = from.clone();
                let to = to.clone();
                async move {
                    bracket_verified_company_identity(&client, &identity).await?;
                    let observation = client
                        .qualify_selected_vouchers(
                            identity.display_name(),
                            identity.company_guid(),
                            &from,
                            &to,
                        )
                        .await?;
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(observation)
                }
            },
        )
        .await
    }

    pub(super) async fn post_xml_cancellable_validated<P>(
        &self,
        config: TallyConfig,
        request: SealedReadRequest,
        cancellation: CancellationToken,
        validate_application_response: P,
    ) -> anyhow::Result<String>
    where
        P: Fn(&str) -> bool + Send + Sync,
    {
        let _lease = self.begin_ordinary_read(&config)?;
        let request_xml = request.into_xml();
        let validate_application_response = Arc::new(validate_application_response);
        self.execute_cancellable(
            config,
            Some(cancellation),
            ReadOperation::ReportExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let request_xml = request_xml.clone();
                let validate_application_response = Arc::clone(&validate_application_response);
                async move {
                    let xml = client.post_xml(request_xml).await?;
                    if validate_application_response(&xml) {
                        Ok(xml)
                    } else {
                        Err(anyhow::Error::new(
                            TallyRuntimeReadError::ApplicationResponseRejected,
                        ))
                    }
                }
            },
        )
        .await
    }

    pub fn cancel_request(&self, request_id: &str) -> anyhow::Result<bool> {
        if request_id.is_empty()
            || request_id.len() > 256
            || request_id.chars().any(char::is_control)
        {
            anyhow::bail!("Tally request ID is invalid");
        }
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        for slot in sessions.values() {
            if slot.session.cancel(request_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn snapshots(&self) -> anyhow::Result<Vec<TallySessionSnapshot>> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let mut snapshots = sessions
            .values()
            .map(|slot| {
                let endpoint = EndpointIdentity::new(slot.session.endpoint.as_str().to_string())
                    .map_err(anyhow::Error::new)?;
                slot.session
                    .snapshot(self.control.endpoint_snapshot(&endpoint))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        snapshots.sort_by(|left, right| left.canonical_endpoint.cmp(&right.canonical_endpoint));
        Ok(snapshots)
    }

    pub fn cached_probe(&self, config: &TallyConfig) -> anyhow::Result<Option<TallyProbeResult>> {
        let endpoint = EndpointKey::from_config(config)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let Some(session) = sessions
            .get(&endpoint)
            .map(|slot| Arc::clone(&slot.session))
        else {
            return Ok(None);
        };
        let cached = session
            .cached_probe
            .read()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?
            .as_ref()
            .map(|probe| probe.result.clone());
        Ok(cached)
    }

    pub(crate) fn master_ledger_export_boundary_profile(
        &self,
        config: &TallyConfig,
    ) -> anyhow::Result<DateBoundaryProfile> {
        Ok(self.master_ledger_export_boundary_profile_from_profile(
            self.cached_probe(config)?
                .as_ref()
                .map(|probe| &probe.profile),
        ))
    }

    pub(crate) fn master_ledger_export_boundary_profile_from_profile(
        &self,
        profile: Option<&bridge_tally_core::CapabilityProfile>,
    ) -> DateBoundaryProfile {
        select_date_boundary_profile(profile)
    }

    pub fn reserve_cached_probe_fresh(
        &self,
        config: &TallyConfig,
        expected_review_id: &str,
        max_age_ms: i64,
    ) -> anyhow::Result<Option<CachedProbeReservation>> {
        if !(1..=600_000).contains(&max_age_ms) {
            anyhow::bail!("Tally capability cache freshness bound is invalid");
        }
        let endpoint = EndpointKey::from_config(config)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let Some(session) = sessions
            .get(&endpoint)
            .map(|slot| Arc::clone(&slot.session))
        else {
            return Ok(None);
        };
        let now = chrono::Utc::now().timestamp_millis();
        let mut cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if session.active_ordinary_reads.load(Ordering::Acquire) != 0 {
            anyhow::bail!("Tally read operation is already in progress");
        }
        let Some(probe) = cache.as_mut() else {
            return Ok(None);
        };
        if probe.review_id != expected_review_id
            || probe.reserved
            || probe.freshness_origin_unix_ms > now
            || now.saturating_sub(probe.freshness_origin_unix_ms) > max_age_ms
        {
            return Ok(None);
        }
        probe.reserved = true;
        let reservation = CachedProbeReservation {
            session: Arc::clone(&session),
            runtime_identity: Arc::clone(&self.runtime_identity),
            review_id: probe.review_id.clone(),
            observed_at_unix_ms: probe.observed_at_unix_ms,
            result: probe.result.clone(),
            armed: true,
        };
        drop(cache);
        Ok(Some(reservation))
    }

    pub fn telemetry_preview(&self) -> anyhow::Result<TallyTelemetryPreviewExport> {
        let export = self.control.collector().privacy_reduced_export_v2()?;
        Ok(TallyTelemetryPreviewExport {
            schema: TELEMETRY_PREVIEW_SCHEMA,
            payload_sha256: export.payload_sha256().to_string(),
            preview_json: export.json().to_string(),
        })
    }

    fn begin_ordinary_read(&self, config: &TallyConfig) -> anyhow::Result<OrdinaryReadLease> {
        let endpoint = EndpointKey::from_config(config)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let Some(session) = sessions
            .get(&endpoint)
            .map(|slot| Arc::clone(&slot.session))
        else {
            drop(sessions);
            let session = self.session(config.clone())?;
            return self.begin_ordinary_read_for_session(session);
        };
        drop(sessions);
        self.begin_ordinary_read_for_session(session)
    }

    fn begin_ordinary_read_for_session(
        &self,
        session: Arc<TallySession>,
    ) -> anyhow::Result<OrdinaryReadLease> {
        let cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            anyhow::bail!("Tally reviewed setup operation is in progress");
        }
        session
            .active_ordinary_reads
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("Tally read admission capacity is unavailable"))?;
        drop(cache);
        Ok(OrdinaryReadLease { session })
    }
}

fn classify_error(error: &anyhow::Error) -> HealthOutcome {
    match classify_failure(error) {
        ReadFailureClass::Connection
        | ReadFailureClass::RequestTimeout
        | ReadFailureClass::RequestFailed
        | ReadFailureClass::HttpServer
        | ReadFailureClass::RateLimited => HealthOutcome::TransportFailure,
        ReadFailureClass::HttpClient
        | ReadFailureClass::SizeLimit
        | ReadFailureClass::Decode
        | ReadFailureClass::Application
        | ReadFailureClass::Validation => HealthOutcome::ApplicationRejected,
    }
}

fn classify_failure(error: &anyhow::Error) -> ReadFailureClass {
    if matches!(
        error.downcast_ref::<TallyRuntimeReadError>(),
        Some(TallyRuntimeReadError::ApplicationResponseRejected)
    ) {
        return ReadFailureClass::Application;
    }
    let transport_error = error.downcast_ref::<TallyTransportError>().or_else(|| {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<TallyTransportError>())
    });
    match transport_error {
        Some(TallyTransportError::ConnectionFailed) => ReadFailureClass::Connection,
        Some(TallyTransportError::RequestTimedOut) => ReadFailureClass::RequestTimeout,
        Some(
            TallyTransportError::RequestFailed
            | TallyTransportError::ResponseTruncated
            | TallyTransportError::ResponseReadFailed,
        ) => ReadFailureClass::RequestFailed,
        Some(TallyTransportError::HttpStatus { status: 429 }) => ReadFailureClass::RateLimited,
        Some(TallyTransportError::HttpStatus { status }) if *status >= 500 => {
            ReadFailureClass::HttpServer
        }
        Some(TallyTransportError::HttpStatus { .. }) => ReadFailureClass::HttpClient,
        Some(
            TallyTransportError::RequestTooLarge { .. }
            | TallyTransportError::ResponseTooLarge { .. },
        ) => ReadFailureClass::SizeLimit,
        Some(
            TallyTransportError::UnsupportedContentEncoding
            | TallyTransportError::InvalidEncoding { .. },
        ) => ReadFailureClass::Decode,
        Some(
            TallyTransportError::EndpointInvalid { .. }
            | TallyTransportError::PolicyInvalid { .. }
            | TallyTransportError::ClientInitializationFailed,
        ) => ReadFailureClass::Validation,
        None => ReadFailureClass::Validation,
    }
}

fn map_execution_error(error: ReadExecutionError<anyhow::Error>) -> anyhow::Error {
    match error {
        ReadExecutionError::Attempt(error) => error,
        ReadExecutionError::Cancelled => anyhow::Error::new(TallyRuntimeControlError::Cancelled),
        ReadExecutionError::QueueDeadline => {
            anyhow::Error::new(TallyRuntimeControlError::QueueDeadline)
        }
        ReadExecutionError::CircuitRejected {
            reason: crate::observability::CircuitRejectReason::Cooldown,
            ..
        } => anyhow::Error::new(TallyRuntimeControlError::CircuitCooldown),
        ReadExecutionError::CircuitRejected {
            reason: crate::observability::CircuitRejectReason::HalfOpenProbeInFlight,
            ..
        } => anyhow::Error::new(TallyRuntimeControlError::HalfOpenProbeInFlight),
        ReadExecutionError::EndpointSessionLimit => {
            anyhow::Error::new(TallyRuntimeControlError::EndpointSessionCapacity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::VerifiedCompanyIdentity;
    use crate::tally::TallyProduct;
    use anyhow::Context;
    use bridge_tally_core::CapabilityProfile;
    use std::collections::BTreeMap;
    use tally_protocol_simulator::Fixture;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn verified_identity(name: &str, guid: &str) -> VerifiedCompanyIdentity {
        VerifiedCompanyIdentity::test_fixture(name, guid)
    }

    fn utf16_xml_response(body: impl AsRef<str>) -> Vec<u8> {
        let body = bridge_tally_protocol::encode_tally_xml_request_utf16le(body.as_ref());
        utf16_xml_response_bytes(&body)
    }

    fn utf16_xml_response_bytes(body: &[u8]) -> Vec<u8> {
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-16\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [headers.as_bytes(), body].concat()
    }

    fn utf8_status_response(body: impl AsRef<str>) -> Vec<u8> {
        let body = body.as_ref().as_bytes();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [headers.as_bytes(), body].concat()
    }

    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let (header_end, content_length) = loop {
            let read = socket.read(&mut buffer).await.expect("read request");
            assert!(read > 0, "request ended before headers completed");
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
                continue;
            };
            let header = std::str::from_utf8(&request[..header_end])
                .expect("request headers are valid UTF-8");
            let content_length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then_some(value.trim())
                })
                .map(|value| {
                    value
                        .parse::<usize>()
                        .expect("request Content-Length is numeric")
                })
                .unwrap_or(0);
            break (header_end + 4, content_length);
        };
        while request.len() < header_end + content_length {
            let read = socket.read(&mut buffer).await.expect("read request body");
            assert!(read > 0, "request ended before declared body completed");
            request.extend_from_slice(&buffer[..read]);
        }
        request
    }

    #[test]
    fn ageing_anchor_serializes_as_an_explicit_wire_contract() {
        assert_eq!(
            serde_json::to_value(OutstandingsAgeingAnchor::DueDate).unwrap(),
            serde_json::json!("due_date")
        );
        assert_eq!(
            serde_json::to_value(OutstandingsAgeingAnchor::BillDate).unwrap(),
            serde_json::json!("bill_date")
        );
    }

    #[test]
    fn single_company_forex_ledger_capture_returns_a_typed_partial() {
        const FOREX_LEDGER_CAPTURE: &[u8] = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/ledgers_forex_composite_live.utf16le.xml"
        );
        let ledger_body = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
            FOREX_LEDGER_CAPTURE,
            "text/xml; charset=utf-16",
            bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
            FOREX_LEDGER_CAPTURE.len(),
        )
        .expect("captured forex ledger response decodes")
        .text;
        let admitted = admit_native_ledger_snapshot(
            parse_native_ledger_snapshot(&ledger_body)
                .map_err(anyhow::Error::from)
                .context("stable native ledger snapshot"),
        )
        .expect("a foreign-currency ledger is an in-band partial");

        assert!(matches!(
            admitted,
            NativeLedgerSnapshotAdmission::Partial(OutstandingsLoadResult::Partial { reason, .. })
                if reason.reason_code == "company_foreign_currency_ledger_balance"
                    && reason.foreign_currency_ledger_name.as_deref() == Some("FX USD Debtor 02")
        ));
    }

    #[tokio::test]
    async fn single_company_read_returns_the_forex_capture_partial() {
        const EXTENT: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
        );
        const FOREX_LEDGER_CAPTURE: &[u8] = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/ledgers_forex_composite_live.utf16le.xml"
        );
        const RECEIVABLE: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_aarav.xml"
        );
        const PAYABLE: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_payable_aarav.xml"
        );
        const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";

        let extent = EXTENT.replacen(
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>"#,
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID><ALTMSTID TYPE="Number">1</ALTMSTID>"#,
            1,
        );
        assert_ne!(extent, EXTENT, "the extent witness injection must apply");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic outstandings server");
        let address = listener.local_addr().expect("synthetic server address");
        let server = tokio::spawn(async move {
            for index in 0..24 {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                        .await
                        .expect("outstandings request timed out")
                        .expect("accept outstandings request");
                let request = read_http_request(&mut socket).await;
                let expected_method: &[u8] = if index % 2 == 0 {
                    b"POST /"
                } else {
                    b"GET /status"
                };
                assert!(
                    request.starts_with(expected_method),
                    "request {index} did not preserve paired-read health bracketing"
                );
                let response = match index {
                    0 | 2 | 20 | 22 => utf16_xml_response(&extent),
                    1 | 3 | 5 | 7 | 9 | 11 | 13 | 15 | 17 | 19 | 21 | 23 => {
                        utf8_status_response(STATUS)
                    }
                    4 | 6 => utf16_xml_response(RECEIVABLE),
                    12 | 14 => utf16_xml_response(PAYABLE),
                    _ => utf16_xml_response_bytes(FOREX_LEDGER_CAPTURE),
                };
                socket.write_all(&response).await.expect("write response");
            }
        });

        let result = TallyRuntime::default()
            .fetch_outstandings(
                TallyConfig {
                    host: address.ip().to_string(),
                    port: address.port(),
                },
                &verified_identity(
                    "Aarav Trading Company Demo",
                    "bb8ad19e-6aef-4239-a917-87fec0c6215e",
                ),
                TallyDate::parse("20260401").expect("captured book as-of"),
                OutstandingsCurrencyAssertion::Inr,
                OutstandingsAgeingAnchor::DueDate,
            )
            .await
            .expect("foreign-currency capture returns an in-band partial");

        assert!(matches!(
            result,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "company_foreign_currency_ledger_balance"
                    && reason.foreign_currency_ledger_name.as_deref() == Some("FX USD Debtor 02")
        ));
        server.await.expect("synthetic outstandings server task");
    }

    #[test]
    fn validation_lab_future_bill_stays_unaged_in_open_bill_output() {
        let receivable = parse_native_bill_rows(
            include_str!(
                "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_validation_lab.xml"
            ),
            &TallyDate::parse("20250401").expect("captured BooksFrom"),
            &TallyDate::parse("20260817").expect("capture as-of"),
        )
        .expect("captured validation-book rows parse");
        let rows = all_open_bill_rows(
            &receivable,
            &[],
            OutstandingsAgeingAnchor::DueDate,
            &TallyDate::parse("20260817").expect("capture as-of"),
        );
        let future = rows
            .iter()
            .find(|row| row.reference == "ALPHA-FUTURE")
            .expect("captured future-due bill remains present");
        assert_eq!(future.amount.as_str(), "22222.00");
        assert_eq!(future.age_days, None);

        let bill_date_rows = all_open_bill_rows(
            &receivable,
            &[],
            OutstandingsAgeingAnchor::BillDate,
            &TallyDate::parse("20260817").expect("capture as-of"),
        );
        let bill_date_future = bill_date_rows
            .iter()
            .find(|row| row.reference == "ALPHA-FUTURE")
            .expect("captured future-due bill remains present for bill-date ageing");
        assert!(
            bill_date_future.age_days.is_some(),
            "the selected bill-date basis must not reuse the future due date"
        );
    }

    #[test]
    fn settled_native_bill_rows_do_not_reach_statement_or_export_sources() {
        let books_from = TallyDate::parse("20250401").expect("synthetic BooksFrom");
        let as_of = TallyDate::parse("20260817").expect("synthetic as-of");
        let parsed = parse_native_bill_rows(
            "<ENVELOPE><BILLFIXED><BILLDATE>1-Aug-26</BILLDATE><BILLREF>SETTLED</BILLREF>\
             <BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED><BILLCL>0.00</BILLCL>\
             <BILLDUE>1-Aug-26</BILLDUE><BILLOVERDUE>16</BILLOVERDUE></ENVELOPE>",
            &books_from,
            &as_of,
        )
        .expect("synthetic settled row parses at the wire boundary");

        assert_eq!(parsed.len(), 1, "the fixture proves the native row existed");
        assert!(
            all_open_bill_rows(&parsed, &[], OutstandingsAgeingAnchor::DueDate, &as_of).is_empty(),
            "a zero closing balance is not an open bill"
        );
    }

    #[test]
    fn captured_validation_lab_bytes_render_a_complete_working_paper_end_to_end() {
        const COMPANY: &str = "Bridge Validation Lab";
        const COMPANY_GUID: &str = "c6afd306-00e1-4f51-802a-babe44daddd3";
        let books_from = TallyDate::parse("20250401").expect("captured BooksFrom");
        let as_of = TallyDate::parse("20260801").expect("captured as-of date");
        let receivable_xml = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_validation_lab.xml"
        );
        let payable_xml = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_payable_validation_lab.xml"
        );
        let groups_xml = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_validation_lab.xml"
        );
        let ledgers_xml = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/ledger_snapshot_validation_lab.xml"
        );

        let receivable = parse_native_bill_rows(receivable_xml, &books_from, &as_of)
            .expect("captured receivable rows parse");
        let payable = parse_native_bill_rows(payable_xml, &books_from, &as_of)
            .expect("captured payable rows parse");
        let groups = parse_native_group_snapshot(groups_xml, COMPANY_GUID)
            .expect("captured group identity and ancestry parse");
        let ledgers =
            parse_native_ledger_snapshot(ledgers_xml).expect("captured ledger controls parse");
        let source_bytes = receivable_xml
            .len()
            .checked_add(payable_xml.len())
            .and_then(|value| value.checked_add(groups_xml.len()))
            .and_then(|value| value.checked_add(ledgers_xml.len()))
            .expect("captured source byte count fits usize");
        let computed = compute_native_outstandings(
            COMPANY,
            &receivable,
            &payable,
            NativeMasterSnapshot {
                ledgers: &ledgers,
                groups: NativeGroupSnapshot::Complete(&groups),
            },
            NativeAgeingAnchor::DueDate,
            &as_of,
            source_bytes,
        )
        .expect("captured native controls compute");
        assert_eq!(
            native_crosscheck_partial_reason(&computed, &as_of),
            None,
            "captured overdue counters must positively prove the requested date"
        );

        let statement_open_bills = all_open_bill_rows(
            &receivable,
            &payable,
            OutstandingsAgeingAnchor::DueDate,
            &as_of,
        );
        assert_eq!(statement_open_bills.len(), 6);
        assert!(statement_open_bills.iter().all(|row| !row.amount.is_zero()));
        let statement_unallocated_by_party = all_unallocated_parties(&computed.residuals);
        let result = OutstandingsLoadResult::Complete {
            report: Box::new(computed.report),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            ageing_anchor: OutstandingsAgeingAnchor::DueDate,
            synced_at_unix_ms: 1_777_000_000_000,
            unallocated_total: Some(computed.residual_total),
            statement_unallocated_by_party,
            statement_open_bills,
        };
        let source = crate::reports::outstandings_working_paper_store::source_from_complete_result(
            &result,
            COMPANY_GUID,
        )
        .expect("captured source stays inside export budgets")
        .expect("captured complete result substantiates a source");
        assert_eq!(source.source_bytes, 33_575);
        assert_eq!(source.open_bills.len(), 6);
        let paper =
            crate::reports::outstandings_working_paper::build_outstandings_working_paper(source)
                .expect("captured exact controls reconcile into a working paper");
        let workbook = crate::reports::outstandings_working_paper_xlsx::render_outstandings_working_paper_xlsx(&paper)
            .expect("captured working paper renders");
        assert!(workbook.len() > 200);
        assert_eq!(&workbook[0..2], b"PK");
    }

    #[test]
    fn raw_overdue_disagreement_withholds_native_complete() {
        let rows = parse_native_bill_rows(
            "<ENVELOPE><BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>MISMATCH</BILLREF>\
             <BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED><BILLCL>-100.00</BILLCL>\
             <BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>31</BILLOVERDUE></ENVELOPE>",
            &TallyDate::parse("20250401").expect("synthetic BooksFrom"),
            &TallyDate::parse("20260817").expect("synthetic as-of"),
        )
        .expect("raw bill parses");
        let computed = compute_native_outstandings(
            "Synthetic Company",
            &rows,
            &[],
            NativeMasterSnapshot {
                ledgers: &[],
                groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
            },
            NativeAgeingAnchor::DueDate,
            &TallyDate::parse("20260817").expect("synthetic as-of"),
            0,
        )
        .expect("arithmetic still computes for diagnostic comparison");
        assert_eq!(
            computed.overdue_crosscheck,
            NativeOverdueCrosscheck::Inconsistent
        );
        assert_eq!(
            native_crosscheck_partial_reason(&computed, &TallyDate::parse("20260817").unwrap()),
            Some(OutstandingsPartialReason::code(
                "native_overdue_crosscheck_mismatch"
            ))
        );
    }

    #[test]
    fn zero_bill_rows_with_a_ledger_residual_withhold_native_complete() {
        let groups = parse_native_group_snapshot(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
            <GROUP NAME="Sundry Debtors" RESERVEDNAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><PARENT>Primary</PARENT></GROUP>
            </COLLECTION></DATA></BODY></ENVELOPE>"#,
            "11111111-1111-1111-1111-111111111111",
        )
        .expect("synthetic group snapshot parses");
        let ledgers = parse_native_ledger_snapshot(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
            <LEDGER NAME="Synthetic Customer"><PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>-100.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>No</ISBILLWISEON></LEDGER>
            </COLLECTION></DATA></BODY></ENVELOPE>"#,
        )
        .expect("synthetic ledger snapshot parses");
        let requested_as_of = TallyDate::parse("20260817").expect("synthetic as-of");
        let computed = compute_native_outstandings(
            "Synthetic Company",
            &[],
            &[],
            NativeMasterSnapshot {
                ledgers: &ledgers,
                groups: NativeGroupSnapshot::Complete(&groups),
            },
            NativeAgeingAnchor::DueDate,
            &requested_as_of,
            0,
        )
        .expect("the residual is a partial-result diagnostic, not a parser failure");

        assert_eq!(
            computed.overdue_crosscheck,
            NativeOverdueCrosscheck::UnconfirmedAsOfWithoutBillReferences
        );
        assert_eq!(
            native_crosscheck_partial_reason(&computed, &requested_as_of),
            Some(OutstandingsPartialReason::code(
                WarningCode::NativeOutstandingsAsOfUnconfirmedWithoutBillReferences.as_str(),
            ))
        );
    }

    #[test]
    fn raw_empty_bill_references_preserve_each_amount_and_get_explicit_label() {
        let raw_bills = "<ENVELOPE>\
            <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF></BILLREF><BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED>\
            <BILLCL>-40.00</BILLCL><BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>30</BILLOVERDUE>\
            <BILLFIXED><BILLDATE>2-Jul-26</BILLDATE><BILLREF> \n\t </BILLREF><BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED>\
            <BILLCL>-60.00</BILLCL><BILLDUE>2-Jul-26</BILLDUE><BILLOVERDUE>29</BILLOVERDUE>\
            </ENVELOPE>";
        let as_of = TallyDate::parse("20260731").expect("synthetic as-of");
        let parsed = parse_native_bill_rows(
            raw_bills,
            &TallyDate::parse("20250401").expect("synthetic BooksFrom"),
            &as_of,
        )
        .expect("paired empty BILLREF values remain parseable");
        let rows = all_open_bill_rows(&parsed, &[], OutstandingsAgeingAnchor::DueDate, &as_of);

        assert_eq!(rows.len(), 2, "empty identities must not collapse rows");
        let total = rows
            .iter()
            .try_fold(ExactDecimal::zero(), |sum, row| {
                sum.checked_add(&row.amount)
            })
            .expect("synthetic bill total remains exact");
        assert_eq!(total.as_str(), "100", "neither amount may be lost");
        assert_eq!(
            rows.iter()
                .map(|row| row.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["No reference reported", "No reference reported"],
            "client-facing rows must disclose the missing identity",
        );
    }

    #[tokio::test]
    async fn detect_base_currency_rejects_book_drift_after_the_currency_read() {
        const EXTENT: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
        );
        const CURRENCY: &[u8] = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        );
        const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";

        let currency = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
            CURRENCY,
            "text/xml; charset=utf-16",
            bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
            CURRENCY.len(),
        )
        .expect("captured currency response decodes")
        .text;

        // The captured fixture predates the ALTMSTID fetch. The outstandings bracket
        // (`fetch_company_book_extent`) now requires that witness, so inject it into this
        // in-memory copy -- the committed fixture bytes are left untouched.
        let opening_extent = EXTENT.replacen(
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>"#,
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID><ALTMSTID TYPE="Number">1</ALTMSTID>"#,
            1,
        );
        assert_ne!(
            opening_extent, EXTENT,
            "the injection must actually change the fixture for this test to prove anything"
        );
        let closing_extent = opening_extent.replace(
            "<LASTVOUCHERDATE TYPE=\"Date\">20260401</LASTVOUCHERDATE>",
            "<LASTVOUCHERDATE TYPE=\"Date\">20260402</LASTVOUCHERDATE>",
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic currency server");
        let address = listener.local_addr().expect("synthetic server address");
        let server = tokio::spawn(async move {
            let responses = [
                opening_extent.as_str(),
                STATUS,
                opening_extent.as_str(),
                STATUS,
                currency.as_str(),
                STATUS,
                currency.as_str(),
                STATUS,
                closing_extent.as_str(),
                STATUS,
                closing_extent.as_str(),
                STATUS,
            ];
            for (index, body) in responses.into_iter().enumerate() {
                let (mut socket, _) =
                    tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                        .await
                        .expect("currency request timed out")
                        .expect("accept currency request");
                let mut request = [0_u8; 16 * 1024];
                let bytes_read = socket.read(&mut request).await.expect("read request");
                let expected_method = if index % 2 == 0 {
                    "POST /"
                } else {
                    "GET /status"
                };
                assert!(
                    String::from_utf8_lossy(&request[..bytes_read]).starts_with(expected_method),
                    "request {index} did not preserve paired-read health bracketing"
                );
                let response = if index % 2 == 0 {
                    utf16_xml_response(body)
                } else {
                    utf8_status_response(body)
                };
                socket.write_all(&response).await.expect("write response");
            }
        });

        let runtime = TallyRuntime::default();
        let result = runtime
            .detect_base_currency(
                TallyConfig {
                    host: address.ip().to_string(),
                    port: address.port(),
                },
                &verified_identity(
                    "Aarav Trading Company Demo",
                    "bb8ad19e-6aef-4239-a917-87fec0c6215e",
                ),
            )
            .await;

        let error = result.expect_err("closing extent drift must reject the currency");
        assert!(
            error
                .to_string()
                .contains("book changed during currency detection"),
            "unexpected error: {error:#}"
        );
        server.await.expect("synthetic currency server task");
    }

    fn synthetic_probe_result() -> TallyProbeResult {
        TallyProbeResult {
            connection: ConnectionStatus {
                reachable: true,
                compatible: false,
                server_text: "Synthetic status".to_string(),
                product: TallyProduct::Unknown,
                error: None,
            },
            companies: vec![TallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("synthetic-guid".to_string()),
                company_number: None,
                books_from: None,
            }],
            profile: CapabilityProfile {
                profile_version: 2,
                product: "Unknown".to_string(),
                release: None,
                mode: None,
                transports: BTreeMap::new(),
                features: BTreeMap::new(),
                packs: BTreeMap::new(),
            },
            selected_read_scope: None,
            passport_snapshot_id: None,
        }
    }

    #[test]
    fn future_due_bill_from_raw_native_bytes_reaches_the_statement_source() {
        let books_from = TallyDate::parse("20260401").expect("synthetic book start");
        let as_of = TallyDate::parse("20260731").expect("synthetic as-of date");
        let bills_xml = "<ENVELOPE>\
            <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>FUTURE-1</BILLREF><BILLPARTY>Synthetic Party</BILLPARTY></BILLFIXED>\
            <BILLCL>-100.00</BILLCL><BILLDUE>1-Aug-26</BILLDUE><BILLOVERDUE>0</BILLOVERDUE>\
            </ENVELOPE>";
        let ledger_xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION></COLLECTION></DATA></BODY></ENVELOPE>";
        let receivable = parse_native_bill_rows(bills_xml, &books_from, &as_of)
            .expect("a future due date in raw native bytes must parse");
        let ledgers = parse_native_ledger_snapshot(ledger_xml)
            .expect("the synthetic raw ledger response must parse");
        let computed = compute_native_outstandings(
            "Synthetic Company",
            &receivable,
            &[],
            NativeMasterSnapshot {
                ledgers: &ledgers,
                groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
            },
            NativeAgeingAnchor::DueDate,
            &as_of,
            bills_xml.len() + ledger_xml.len(),
        )
        .expect("a future-due bill must not abort the native computation");
        assert_eq!(computed.report.receivable_total.as_str(), "100");
        assert_eq!(computed.report.top_parties[0].oldest_bill_age_days, None);
        assert_eq!(computed.report.ageing.days_0_30.as_str(), "100");
        assert_eq!(computed.report.ageing.days_31_60, ExactDecimal::zero());
        assert_eq!(computed.report.ageing.days_61_90, ExactDecimal::zero());
        assert_eq!(computed.report.ageing.days_90_plus, ExactDecimal::zero());
        assert_eq!(computed.report.open_receivable_bill_count, 1);

        let statement_rows =
            all_open_bill_rows(&receivable, &[], OutstandingsAgeingAnchor::DueDate, &as_of);
        assert_eq!(
            statement_rows.len(),
            1,
            "a future-due bill must remain available to the statement source"
        );
        assert_eq!(statement_rows[0].amount.as_str(), "100.00");
        assert_eq!(statement_rows[0].age_days, None);
        let statement = crate::reports::party_statement::build_party_statement(
            "Synthetic Company",
            as_of.as_str(),
            "Synthetic Party",
            &statement_rows,
            &[],
        )
        .expect("the future-due bill must build a party statement");
        assert_eq!(statement.bills.len(), 1);
        assert_eq!(statement.bills[0].reference, "FUTURE-1");
        assert_eq!(statement.bills[0].age_days, None);
        assert_eq!(statement.bills[0].bucket, None);
        assert_eq!(statement.bill_total.as_str(), "100");

        let destination = tempfile::tempdir().expect("synthetic destination");
        let approvals =
            crate::reports::bulk_party_statement::PartyStatementDestinationApprovals::default();
        let approval_id = approvals
            .issue(destination.path().to_path_buf())
            .expect("approve synthetic destination");
        let approved_destination = approvals
            .consume(&approval_id, destination.path())
            .expect("consume synthetic approval");
        let bulk = crate::reports::bulk_party_statement::write_bulk_party_statements(
            &approved_destination,
            "Synthetic Company",
            as_of.as_str(),
            "xlsx",
            &statement_rows,
            &[],
            |party_statement| {
                crate::reports::party_statement_xlsx::render_party_statement_xlsx(party_statement)
                    .map_err(|error| error.to_string())
            },
        )
        .expect("a bulk run must write the future-due party statement");
        assert_eq!(bulk.written.len(), 1);
        let workbook = std::fs::File::open(destination.path().join(&bulk.written[0].file_name))
            .expect("the bulk statement file exists");
        let mut archive = zip::ZipArchive::new(workbook).expect("bulk output is an XLSX archive");
        let mut workbook_text = String::new();
        for entry_name in ["xl/worksheets/sheet1.xml", "xl/sharedStrings.xml"] {
            let mut entry = archive
                .by_name(entry_name)
                .expect("the XLSX statement entry exists");
            std::io::Read::read_to_string(&mut entry, &mut workbook_text)
                .expect("the workbook XML is readable");
        }
        assert!(workbook_text.contains("FUTURE-1"));
        assert!(workbook_text.contains("Not due"));
        assert!(workbook_text.contains("Unaged"));
    }

    #[test]
    fn education_snapshot_period_refusal_is_an_in_band_partial() {
        let result = match admit_native_ledger_snapshot_period(
            DateBoundaryProfile::EducationRestricted,
            TallyDate::parse("20260401").expect("Education-valid book start"),
            TallyDate::parse("20260415").expect("ordinary calendar as-of"),
        ) {
            NativeLedgerSnapshotPeriodAdmission::Period(_) => {
                panic!("an Education-refused as-of must not construct a snapshot period")
            }
            NativeLedgerSnapshotPeriodAdmission::Partial(partial) => partial,
        };
        assert!(matches!(
            result,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "as_of_has_no_valid_window_boundary"
        ));
    }

    #[cfg(feature = "voucher-scan")]
    #[test]
    fn outstandings_read_failures_are_partial_and_deadlines_recommend_restart() {
        assert_eq!(
            outstandings_read_failure_reason(&anyhow::Error::new(
                TallyTransportError::RequestTimedOut
            )),
            "tally_segment_deadline_restart_recommended"
        );
        assert_eq!(
            outstandings_read_failure_reason(&anyhow::Error::new(
                TallyTransportError::RequestFailed
            )),
            "segment_request_failed"
        );
        assert_eq!(
            outstandings_read_failure_reason(&anyhow::Error::new(
                TallyTransportError::HttpStatus { status: 500 }
            )),
            "segment_http_status_failure"
        );
        assert_eq!(
            outstandings_read_failure_reason(&anyhow::anyhow!("connection refused")),
            "segment_read_failed"
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[tokio::test]
    async fn outstandings_read_transport_failure_feeds_breaker_and_preserves_typed_partial() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9120,
        };
        let result = runtime
            .execute(
                config,
                ReadOperation::VoucherExport,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |_client| async {
                    Err::<OutstandingsLoadResult, _>(anyhow::Error::new(
                        OutstandingsReadTransportFailure {
                            reason_code: "tally_segment_deadline_restart_recommended",
                            source: anyhow::Error::new(TallyTransportError::RequestTimedOut),
                        },
                    ))
                },
            )
            .await;
        let caller_value = partial_after_outstandings_read_transport_failure(result)
            .expect("outstandings read transport failures stay in-band for the caller");
        assert!(matches!(
            caller_value,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "tally_segment_deadline_restart_recommended"
        ));
        assert_eq!(
            runtime.snapshots().expect("runtime snapshot")[0].consecutive_failures,
            1,
            "the breaker must observe the transport failure before UI mapping"
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[tokio::test]
    async fn verification_partial_stays_partial_without_feeding_breaker() {
        let runtime = TallyRuntime::default();
        let result = runtime
            .execute(
                TallyConfig {
                    host: "localhost".to_string(),
                    port: 9121,
                },
                ReadOperation::VoucherExport,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |_client| async {
                    Ok::<_, anyhow::Error>(partial_result("paired_segment_mismatch"))
                },
            )
            .await;
        let caller_value = partial_after_outstandings_read_transport_failure(result)
            .expect("verification partial remains an in-band result");
        assert!(matches!(
            caller_value,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "paired_segment_mismatch"
        ));
        assert_eq!(
            runtime.snapshots().expect("runtime snapshot")[0].consecutive_failures,
            0,
            "a verification failure reached a responder and must not poison endpoint health"
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[test]
    fn closing_coverage_drift_is_not_reported_as_uncovered_opening_bills() {
        assert_eq!(
            closing_coverage_partial_reason(false, true),
            Some("ledger_master_identity_changed_during_scan")
        );
        assert_eq!(
            closing_coverage_partial_reason(true, false),
            Some("ledger_opening_bills_not_covered")
        );
        assert_eq!(closing_coverage_partial_reason(true, true), None);
    }

    #[cfg(feature = "voucher-scan")]
    #[test]
    fn intra_pair_ledger_coverage_drift_is_an_in_band_partial() {
        assert_eq!(
            paired_coverage_partial_reason(&LedgerOpeningCoverageRead::Drifted),
            Some("ledger_master_identity_changed_during_scan")
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[tokio::test]
    async fn witness_transport_failure_feeds_breaker_and_preserves_typed_partial() {
        let error = fetch_empty_partition_witness(
            VoucherAlterIdHighWater::parse("1").expect("positive high-water"),
            || async { Err(anyhow::Error::new(TallyTransportError::RequestTimedOut)) },
        )
        .await
        .expect_err("a witness transport failure must cross the runtime health boundary");
        assert!(error.downcast_ref::<TallyTransportError>().is_some());

        let runtime = TallyRuntime::default();
        let result = runtime
            .execute(
                TallyConfig {
                    host: "localhost".to_string(),
                    port: 9122,
                },
                ReadOperation::VoucherExport,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |_client| async {
                    Err::<OutstandingsLoadResult, _>(outstandings_read_transport_failure(
                        anyhow::Error::new(TallyTransportError::RequestTimedOut),
                    ))
                },
            )
            .await;
        let caller_value = partial_after_outstandings_read_transport_failure(result)
            .expect("witness transport failure stays in-band only after health accounting");

        assert!(matches!(
            caller_value,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "tally_segment_deadline_restart_recommended"
        ));
        assert_eq!(
            runtime.snapshots().expect("runtime snapshot")[0].consecutive_failures,
            1,
            "the breaker must observe the witness transport failure"
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[tokio::test]
    async fn witness_non_transport_failure_remains_an_in_band_partial() {
        let partial = fetch_empty_partition_witness(
            VoucherAlterIdHighWater::parse("1").expect("positive high-water"),
            || async { Err(anyhow::anyhow!("witness response was malformed")) },
        )
        .await
        .expect("non-transport witness failure stays in-band")
        .expect_err("a malformed witness cannot complete a non-empty book");

        assert_eq!(
            partial.reason_code,
            "empty_date_witness_profile_unavailable"
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[test]
    fn outstandings_date_boundaries_follow_detected_mode_and_fallback_to_i12() {
        let mut profile = synthetic_probe_result().profile;
        profile.product = "TallyPrime Edit Log".to_string();
        profile.mode = Some("Education".to_string());
        assert_eq!(
            select_date_boundary_profile(Some(&profile)),
            DateBoundaryProfile::EducationRestricted
        );

        profile.mode = Some("Licensed".to_string());
        assert_eq!(
            select_date_boundary_profile(Some(&profile)),
            DateBoundaryProfile::ModeAgnostic
        );
        assert_eq!(
            select_date_boundary_profile(None),
            DateBoundaryProfile::ModeAgnostic
        );

        profile.product = "Unknown".to_string();
        profile.mode = Some("Education".to_string());
        assert_eq!(
            select_date_boundary_profile(Some(&profile)),
            DateBoundaryProfile::ModeAgnostic,
            "inconsistent or incomplete detection must rely on I12 rather than inventing compatibility evidence"
        );
    }

    /// An uncalibrated segment width no longer refuses the read -- it selects
    /// the native bills path, which needs no width. What must NOT change is
    /// that a non-loopback endpoint is still refused.
    ///
    /// This test previously asserted `outstandings_segment_sizing_uncalibrated`
    /// and, by using a non-loopback host, proved the refusal happened BEFORE
    /// endpoint admission. That refusal was the defect: production has no
    /// calibrated width by construction, so every shipped build returned it for
    /// every company on every book and the screen could only ever say "No Tally
    /// data was read". The reason code is gone with it.
    ///
    /// The loopback guard is unchanged and still fails closed; it is simply now
    /// the first guard the native path reaches. That is the property worth
    /// pinning, so this asserts it directly rather than inferring it from an
    /// ordering that no longer exists.
    #[tokio::test]
    async fn uncalibrated_outstandings_takes_the_native_path_and_still_refuses_a_non_loopback_endpoint(
    ) {
        let runtime = TallyRuntime::default();
        #[cfg(feature = "voucher-scan")]
        assert!(
            runtime.outstandings_segment_policy.is_none(),
            "a default runtime must have no calibrated width -- that is what routes to the native path"
        );

        let error = runtime
            .fetch_outstandings(
                TallyConfig {
                    host: "not-a-loopback-endpoint".to_string(),
                    port: 9000,
                },
                &verified_identity("Synthetic Company", "synthetic-guid"),
                TallyDate::parse("20260731").unwrap(),
                OutstandingsCurrencyAssertion::Inr,
                OutstandingsAgeingAnchor::DueDate,
            )
            .await
            .expect_err("a non-loopback endpoint must never be contacted");
        assert!(
            error.to_string().contains("non_loopback_forbidden"),
            "loopback-only admission must still fail closed on the native path, got: {error}"
        );
    }

    #[cfg(feature = "live-calibration-harness")]
    #[tokio::test]
    async fn calibrated_voucher_scan_withholds_totals_before_endpoint_admission_without_residual_coverage(
    ) {
        let result = TallyRuntime::for_billwise_lab_reconciliation_exit_check()
            .fetch_outstandings(
                TallyConfig {
                    host: "not-a-loopback-endpoint".to_string(),
                    port: 9000,
                },
                &verified_identity("Synthetic Company", "synthetic-guid"),
                TallyDate::parse("20260731").unwrap(),
                OutstandingsCurrencyAssertion::Inr,
                OutstandingsAgeingAnchor::DueDate,
            )
            .await
            .expect("missing coverage is an in-band partial result");
        assert!(matches!(
            result,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "unallocated_direct_postings_not_covered"
        ));
    }

    #[cfg(all(feature = "voucher-scan", not(feature = "live-calibration-harness")))]
    #[test]
    fn default_build_has_no_outstandings_width_admission() {
        let runtime = TallyRuntime::default();
        assert!(runtime.outstandings_segment_policy.is_none());
        assert!(runtime.outstandings_boundary_profile_override.is_none());
    }

    #[cfg(feature = "live-calibration-harness")]
    #[test]
    fn billwise_lab_exit_harness_uses_only_education_valid_boundaries() {
        let runtime = TallyRuntime::for_billwise_lab_reconciliation_exit_check();
        assert_eq!(
            runtime.outstandings_boundary_profile_override,
            Some(DateBoundaryProfile::EducationRestricted)
        );
        let reporting_window = DateWindow::parse(
            runtime
                .outstandings_boundary_profile_override
                .expect("exit profile is fixed"),
            "20240401",
            "20260702",
        )
        .expect("accepted corpus extent uses Education-valid boundaries");
        let partitions = reporting_window
            .narrow_partitions()
            .expect("Education-valid corpus partitions without day-3 synthesis");
        assert!(partitions.iter().all(|partition| {
            matches!(&partition.from().as_str()[6..8], "01" | "02" | "31")
                && matches!(&partition.to().as_str()[6..8], "01" | "02" | "31")
        }));

        let unprofiled =
            DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20240401", "20260702")
                .unwrap()
                .narrow_partitions()
                .unwrap();
        assert!(unprofiled.iter().any(|partition| {
            !matches!(&partition.from().as_str()[6..8], "01" | "02" | "31")
                || !matches!(&partition.to().as_str()[6..8], "01" | "02" | "31")
        }));
    }

    #[test]
    fn local_rejections_and_client_http_statuses_do_not_poison_endpoint_health() {
        assert_eq!(
            classify_failure(&anyhow::Error::new(
                TallyRuntimeReadError::ApplicationResponseRejected,
            )),
            ReadFailureClass::Application
        );
        for error in [
            TallyTransportError::RequestTooLarge { limit: 1024 },
            TallyTransportError::PolicyInvalid { code: "test" },
            TallyTransportError::EndpointInvalid { code: "test" },
            TallyTransportError::ClientInitializationFailed,
            TallyTransportError::HttpStatus { status: 400 },
        ] {
            assert_eq!(
                classify_error(&anyhow::Error::new(error)),
                HealthOutcome::ApplicationRejected
            );
        }
        for error in [
            TallyTransportError::ConnectionFailed,
            TallyTransportError::RequestTimedOut,
            TallyTransportError::HttpStatus { status: 503 },
        ] {
            assert_eq!(
                classify_error(&anyhow::Error::new(error)),
                HealthOutcome::TransportFailure
            );
        }
    }

    #[test]
    fn endpoint_identity_aliases_only_localhost_to_ipv4_loopback() {
        let runtime = TallyRuntime::default();
        let first = runtime
            .session(TallyConfig {
                host: "localhost".to_string(),
                port: 9000,
            })
            .expect("localhost session");
        let second = runtime
            .session(TallyConfig {
                host: "127.0.0.1".to_string(),
                port: 9000,
            })
            .expect("IPv4 loopback session");
        let third = runtime
            .session(TallyConfig {
                host: "::1".to_string(),
                port: 9000,
            })
            .expect("IPv6 loopback session");
        let fourth = runtime
            .session(TallyConfig {
                host: "127.0.0.2".to_string(),
                port: 9000,
            })
            .expect("alternate IPv4 loopback session");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &third));
        assert!(!Arc::ptr_eq(&first, &fourth));
        assert!(!Arc::ptr_eq(&third, &fourth));
        let snapshots = runtime.snapshots().expect("runtime snapshots");
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].canonical_endpoint, "http://127.0.0.1:9000");
        assert_eq!(snapshots[1].canonical_endpoint, "http://127.0.0.2:9000");
        assert_eq!(snapshots[2].canonical_endpoint, "http://[::1]:9000");
    }

    #[test]
    fn reviewed_probe_cache_preserves_observation_time_and_is_single_use() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9000,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let review_id = "review-current";
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: review_id.to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });

        let mut reservation = runtime
            .reserve_cached_probe_fresh(&config, review_id, 300_000)
            .expect("reserve cache")
            .expect("fresh reviewed probe");
        assert_eq!(reservation.observed_at_unix_ms(), observed_at_unix_ms);
        assert_eq!(reservation.result().companies[0].name, "Synthetic Company");
        assert!(runtime
            .reserve_cached_probe_fresh(&config, review_id, 300_000)
            .expect("second reserve")
            .is_none());
        assert!(reservation.consume().expect("consume reservation"));
        assert!(!reservation
            .consume()
            .expect("consuming an already consumed lease is inert"));
        assert!(runtime
            .reserve_cached_probe_fresh(&config, review_id, 300_000)
            .expect("reserve consumed cache")
            .is_none());
    }

    #[test]
    fn stale_review_id_cannot_consume_or_reserve_a_newer_probe() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9002,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-b".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });

        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-a", 300_000)
            .expect("reject stale review")
            .is_none());
        let mut reservation = runtime
            .reserve_cached_probe_fresh(&config, "review-b", 300_000)
            .expect("reserve current review")
            .expect("current review exists");
        assert!(reservation.release().expect("release current review"));
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-b", 300_000)
            .expect("retry current review")
            .is_some());
    }

    #[test]
    fn reviewed_probe_cache_rejects_future_expired_and_invalid_freshness() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9001,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        for observed_at_unix_ms in [
            chrono::Utc::now().timestamp_millis() + 1_000,
            chrono::Utc::now().timestamp_millis() - 301_000,
        ] {
            *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
                review_id: "review-expiry".to_string(),
                observed_at_unix_ms,
                freshness_origin_unix_ms: observed_at_unix_ms,
                result: synthetic_probe_result(),
                reserved: false,
            });
            assert!(runtime
                .reserve_cached_probe_fresh(&config, "review-expiry", 300_000)
                .expect("reserve cache")
                .is_none());
        }
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-expiry", 0)
            .is_err());
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-expiry", 600_001)
            .is_err());
    }

    #[test]
    fn replacing_a_qualified_review_does_not_renew_its_freshness_origin() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9003,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let freshness_origin_unix_ms = chrono::Utc::now().timestamp_millis() - 299_000;
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-original".to_string(),
            observed_at_unix_ms: freshness_origin_unix_ms,
            freshness_origin_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        let mut reservation = runtime
            .reserve_cached_probe_fresh(&config, "review-original", 300_000)
            .expect("reserve original")
            .expect("original remains barely fresh");
        assert!(reservation
            .replace(
                "review-qualified".to_string(),
                chrono::Utc::now().timestamp_millis(),
                synthetic_probe_result(),
            )
            .expect("replace reservation"));
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-qualified", 298_000)
            .expect("check inherited freshness")
            .is_none());
    }

    #[test]
    fn ordinary_read_admission_and_review_reservation_are_mutually_exclusive() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9004,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-lease".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });

        let read_lease = runtime
            .begin_ordinary_read(&config)
            .expect("admit ordinary read");
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-lease", 300_000)
            .is_err());
        drop(read_lease);

        let reservation = runtime
            .reserve_cached_probe_fresh(&config, "review-lease", 300_000)
            .expect("reserve after read")
            .expect("fresh review");
        assert!(runtime.begin_ordinary_read(&config).is_err());
        assert!(reservation.authorize(&runtime, &config).is_ok());
        assert!(reservation
            .authorize(
                &runtime,
                &TallyConfig {
                    host: "127.0.0.2".to_string(),
                    port: 9004,
                },
            )
            .is_err());
    }

    #[tokio::test]
    async fn qualification_rejects_a_reservation_from_another_runtime_before_dispatch() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind cross-runtime qualification server");
        let address = listener.local_addr().expect("qualification server address");
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let owner_runtime = TallyRuntime::default();
        let executing_runtime = TallyRuntime::default();
        let session = owner_runtime
            .session(config.clone())
            .expect("owner runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-cross-runtime".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        drop(session);
        let reservation = owner_runtime
            .reserve_cached_probe_fresh(&config, "review-cross-runtime", 300_000)
            .expect("reserve owner review")
            .expect("fresh owner review");

        let error = executing_runtime
            .qualify_selected_ledgers(
                config,
                &reservation,
                &verified_identity("Synthetic Company", "synthetic-guid"),
            )
            .await
            .expect_err("another runtime must not borrow the reservation");
        assert!(error
            .to_string()
            .contains("reviewed setup operation ownership changed"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept(),)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reservation_owner_rechecks_the_tuple_then_qualifies_selected_ledgers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind qualification server");
        let address = listener.local_addr().expect("qualification server address");
        let company_list = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="BRIDGE SYNTHETIC BOOK"><GUID TYPE="String">00000000-0000-4000-8000-000000000001</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><BOOKSFROM TYPE="Date">20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let ledger_export = Fixture::NormalExport.body().into_owned();
        let server = tokio::spawn(async move {
            for body in [company_list.to_string(), ledger_export] {
                let (mut socket, _) = listener.accept().await.expect("accept reserved read");
                let request = read_http_request(&mut socket).await;
                assert!(request.starts_with(b"POST /"));
                socket
                    .write_all(&utf16_xml_response(body))
                    .await
                    .expect("write reserved read response");
            }
        });
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-qualified-tuple".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        drop(session);
        let reservation = runtime
            .reserve_cached_probe_fresh(&config, "review-qualified-tuple", 300_000)
            .expect("reserve reviewed setup")
            .expect("fresh review");

        let companies = runtime
            .fetch_companies_for_reservation(config.clone(), &reservation)
            .await
            .expect("reservation owner may recheck company tuple");
        assert_eq!(companies.len(), 1);
        let observation = runtime
            .qualify_selected_ledgers(
                config,
                &reservation,
                &verified_identity(
                    "BRIDGE SYNTHETIC BOOK",
                    "00000000-0000-4000-8000-000000000001",
                ),
            )
            .await
            .expect("qualification must run after the reserved tuple recheck");
        assert_eq!(observation.result_bucket, "non_empty_observed");
        server.await.expect("finish reserved qualification server");
    }

    #[test]
    fn dropping_a_review_reservation_restores_the_same_fresh_review() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9005,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-drop".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        drop(
            runtime
                .reserve_cached_probe_fresh(&config, "review-drop", 300_000)
                .expect("reserve review")
                .expect("fresh review"),
        );
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-drop", 300_000)
            .expect("reserve after drop")
            .is_some());
    }

    #[tokio::test]
    async fn aborting_a_task_drops_and_releases_its_review_reservation() {
        let runtime = Arc::new(TallyRuntime::default());
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9006,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-abort".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        let (held_tx, held_rx) = tokio::sync::oneshot::channel();
        let task_runtime = Arc::clone(&runtime);
        let task_config = config.clone();
        let task = tokio::spawn(async move {
            let _reservation = task_runtime
                .reserve_cached_probe_fresh(&task_config, "review-abort", 300_000)
                .expect("reserve review")
                .expect("fresh review");
            held_tx.send(()).expect("announce held reservation");
            std::future::pending::<()>().await;
        });
        held_rx.await.expect("reservation was held");
        task.abort();
        let _ = task.await;
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-abort", 300_000)
            .expect("reserve after abort")
            .is_some());
    }

    #[tokio::test]
    async fn aborting_pending_qualification_releases_review_and_active_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind pending qualification server");
        let address = listener.local_addr().expect("pending server address");
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.expect("accept qualification");
            accepted_tx.send(()).expect("announce accepted request");
            std::future::pending::<()>().await;
        });
        let runtime = Arc::new(TallyRuntime::default());
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-pending".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        drop(session);
        let task_runtime = Arc::clone(&runtime);
        let task_config = config.clone();
        let task = tokio::spawn(async move {
            let reservation = task_runtime
                .reserve_cached_probe_fresh(&task_config, "review-pending", 300_000)
                .expect("reserve pending review")
                .expect("fresh pending review");
            let _ = task_runtime
                .qualify_selected_ledgers(
                    task_config,
                    &reservation,
                    &verified_identity("Synthetic Company", "synthetic-guid"),
                )
                .await;
        });
        accepted_rx.await.expect("qualification reached server");
        task.abort();
        let _ = task.await;
        server.abort();
        let snapshots = runtime.snapshots().expect("runtime snapshots after abort");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].active_requests, 0);
        assert!(snapshots[0].active_request_ids.is_empty());
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-pending", 300_000)
            .expect("reserve after pending abort")
            .is_some());
    }

    #[test]
    fn stale_guard_cannot_release_or_consume_a_newer_reserved_review() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9007,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-old".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        let mut stale = runtime
            .reserve_cached_probe_fresh(&config, "review-old", 300_000)
            .expect("reserve old")
            .expect("old review");
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-new".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: true,
        });
        assert!(!stale.consume().expect("stale consume is inert"));
        drop(stale);
        let cache = session.cached_probe.read().expect("capability cache");
        let current = cache.as_ref().expect("new review remains");
        assert_eq!(current.review_id, "review-new");
        assert!(current.reserved);
    }

    #[test]
    fn stale_guard_cannot_replace_a_newer_reserved_review() {
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9008,
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-old".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        let mut stale = runtime
            .reserve_cached_probe_fresh(&config, "review-old", 300_000)
            .expect("reserve old")
            .expect("old review");
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-new".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: true,
        });

        assert!(!stale
            .replace(
                "review-illegal-replacement".to_string(),
                observed_at_unix_ms,
                synthetic_probe_result(),
            )
            .expect("stale replace is inert"));
        drop(stale);
        let cache = session.cached_probe.read().expect("capability cache");
        let current = cache.as_ref().expect("new review remains");
        assert_eq!(current.review_id, "review-new");
        assert!(current.reserved);
    }

    #[test]
    fn held_review_reservation_prevents_endpoint_session_eviction() {
        let runtime = TallyRuntime::default();
        let reserved_config = TallyConfig {
            host: "127.0.0.1".to_string(),
            port: 9200,
        };
        let reserved_endpoint = EndpointKey::from_config(&reserved_config).unwrap();
        let session = runtime
            .session(reserved_config.clone())
            .expect("reserved session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-capacity".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        drop(session);
        let _reservation = runtime
            .reserve_cached_probe_fresh(&reserved_config, "review-capacity", 300_000)
            .expect("reserve capacity review")
            .expect("fresh capacity review");
        for host_suffix in 2..=MAX_ENDPOINT_SESSIONS {
            runtime
                .session(TallyConfig {
                    host: format!("127.0.0.{host_suffix}"),
                    port: 9200,
                })
                .expect("fill endpoint capacity");
        }
        runtime
            .session(TallyConfig {
                host: "127.0.0.254".to_string(),
                port: 9200,
            })
            .expect("evict one unreserved session");
        assert!(runtime
            .sessions
            .lock()
            .expect("session registry")
            .contains_key(&reserved_endpoint));
    }

    #[tokio::test]
    async fn cancellation_registry_cancels_and_releases_requests() {
        let runtime = Arc::new(TallyRuntime::default());
        let config = TallyConfig {
            host: "localhost".to_string(),
            port: 9100,
        };
        let runtime_task = Arc::clone(&runtime);
        let task = tokio::spawn(async move {
            runtime_task
                .execute(
                    config,
                    ReadOperation::OtherRead,
                    ReadRetryPolicy::SINGLE_ATTEMPT,
                    |_client| async {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        Ok::<_, anyhow::Error>(())
                    },
                )
                .await
        });
        tokio::task::yield_now().await;
        let snapshot = runtime
            .snapshots()
            .expect("runtime snapshots")
            .pop()
            .expect("active session");
        assert_eq!(snapshot.active_requests, 1);
        let session = runtime
            .sessions
            .lock()
            .expect("sessions lock")
            .values()
            .next()
            .expect("session")
            .session
            .clone();
        let request_id = session
            .active_requests
            .lock()
            .expect("request lock")
            .keys()
            .next()
            .expect("request ID")
            .clone();
        assert!(runtime.cancel_request(&request_id).expect("cancel request"));
        assert!(task.await.expect("request task").is_err());
        assert_eq!(
            runtime.snapshots().expect("runtime snapshots")[0].active_requests,
            0
        );
        assert_eq!(
            runtime.snapshots().expect("runtime snapshots")[0].consecutive_failures,
            0,
            "operator cancellation must not degrade endpoint health"
        );
    }

    #[test]
    fn telemetry_preview_is_privacy_reduced_and_checksummed() {
        let preview = TallyRuntime::default()
            .telemetry_preview()
            .expect("telemetry preview");
        assert_eq!(preview.schema, "bridge.tally.telemetry-preview/2");
        assert_eq!(preview.payload_sha256.len(), 64);
        let preview_value: serde_json::Value =
            serde_json::from_str(&preview.preview_json).expect("valid preview JSON");
        assert_eq!(
            preview_value["privacy_profile"],
            "fixed_dimensions_bucketed_values_v1"
        );
        assert_eq!(preview_value["authenticity_claim"], "none");
    }
}
