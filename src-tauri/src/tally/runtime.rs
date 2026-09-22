use super::{
    ConnectionStatus, TallyClient, TallyCompany, TallyConfig, TallyLedger, VerifiedCompanyIdentity,
};
use super::{TallyProbeResult, TallyVoucher};
use crate::observability::BodyBytesObservation;
use crate::reports::party_ledger_master::PartyLedgerMasterSource;
use crate::tally::connection::{canonical_loopback_origin, SelectedReadObservation};
#[cfg(feature = "voucher-scan")]
use crate::tally::connection::{LedgerOpeningCoverageRead, OutstandingsSegmentObservation};
use crate::tally::connection::{NativePairedRead, PairedReadValidationError};
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
    render_native_ledger_export_request, render_native_ledger_snapshot_request,
    AgeingAnchor as NativeAgeingAnchor, CompanyCurrency, LedgerSnapshotEntry,
    NativeBillsReportKind, NativeGroupSnapshot, NativeLedgerExportPeriod,
    NativeLedgerExportPeriodError, NativeLedgerSnapshotPeriod, NativeMasterSnapshot,
    NativeOutstandingsError, NativeOverdueCrosscheck,
};
#[cfg(feature = "voucher-scan")]
use bridge_tally_protocol::outstandings::{
    assemble_partitioned_scan, assemble_scan, compute_outstandings_with_ageing_anchor,
    corroborate_empty_date_partition, nearest_non_empty_primary_partition,
    AgeingAnchor as LegacyAgeingAnchor, CompleteWitnessPair, CorroboratedDatePartition, DateWindow,
    NarrowDateWindow, PartialScan, ScanResult, SegmentVerification, StrictlyWiderDateCover,
    VoucherAlterIdHighWater, WitnessPairVerification,
};
use bridge_tally_protocol::outstandings_shared::{
    CompanyBookExtent, DateBoundaryProfile, OutstandingsReport,
};
use bridge_tally_protocol::{
    parse_companies_from_collection, parse_native_ledger_source_records_with_evidence,
    xml_read_profiles::ReadOnlyProfile,
};
use bridge_tally_transport::TallyTransportError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

const MAX_ENDPOINT_SESSIONS: usize = 32;

#[path = "runtime_trial_balance.rs"]
mod trial_balance;
pub(crate) use trial_balance::TrialBalanceReadError;
pub use trial_balance::{TrialBalancePeriod, TrialBalanceRead};

#[cfg(test)]
#[path = "runtime_trial_balance_tests.rs"]
mod trial_balance_tests;

/// A company-list response bound to the exact transport bytes that produced
/// it. The MCP adapter uses this rather than reserializing parsed companies.
#[derive(Debug, Clone)]
pub struct AgentCompanyList {
    pub companies: Vec<TallyCompany>,
    pub response_bytes: usize,
    pub response_sha256: String,
}

/// A custom agent read bound to the stable paired transport response that
/// produced it. The decoded body is only for parsing; evidence remains tied to
/// the original encoded wire bytes.
#[derive(Debug, Clone)]
pub struct AgentRead {
    pub body: String,
    pub encoded_bytes: usize,
    pub encoded_sha256: String,
    /// The stricter of the date-boundary profiles the read's opening and
    /// closing identity brackets observed: Education if either reported it.
    pub boundary_profile: DateBoundaryProfile,
}

async fn fetch_admitted_agent_read(
    client: &TallyClient,
    identity: &VerifiedCompanyIdentity,
    request: super::agent_read_request::AgentReadRequest,
) -> anyhow::Result<(AgentRead, RuntimeReadEvidence)> {
    let opening = bracket_verified_company_identity_observing_mode(client, identity).await?;
    if !request.window_accepted_by(opening) {
        return Err(EducationBoundaryRefusal.into());
    }
    let request_xml = request.clone().into_xml();
    let (body, encoded_bytes, encoded_sha256) = client
        .fetch_native_report_paired_with_evidence(request_xml.clone())
        .await?;
    let evidence = RuntimeReadEvidence::paired(&request_xml, encoded_sha256.clone(), encoded_bytes);
    let closing = bracket_verified_company_identity_observing_mode(client, identity)
        .await
        .map_err(|error| with_read_evidence(error, evidence.clone()))?;
    // Education reported only after the read may have served it: which mode
    // answered is unknown, so the read is refused as if it had been sent in
    // Education (a licence server dropping out mid-read does this).
    if !request.window_accepted_by(closing) {
        return Err(with_read_evidence(
            EducationBoundaryRefusal.into(),
            evidence.clone(),
        ));
    }
    let profile = if closing == DateBoundaryProfile::EducationRestricted {
        closing
    } else {
        opening
    };
    Ok((
        AgentRead {
            body,
            encoded_bytes,
            encoded_sha256,
            boundary_profile: profile,
        },
        evidence,
    ))
}

/// The per-HTTP-request deadline an audit_read part's requests get. It is the
/// transport's own session deadline, not a longer one: owner ruling 2
/// (`docs/tally/UNIT_A_RULING_2.md`) denied raising the 20-second deadline, and
/// the audit-read design's 90 s would reverse that ruling, so it waits for the
/// owner. Nothing here enforces it; the session transport policy does, and a
/// test keeps the two equal.
///
/// It bounds each request, not the part: a single part sends three requests
/// (bracket, data, bracket) and a paired part six, so a part can take several
/// times this long.
///
/// What keeping it costs, measured on licensed books: a stock master read of
/// 43.4 s, a narrow voucher window of 42.8 s and a one-day voucher read of
/// 31-34 s all exceed it. Each is refused as `audit_part_deadline_exceeded`
/// rather than admitted late, so the planner must divide voucher windows
/// smaller than one day's AlterIDs, and a master read that slow cannot be
/// taken at all until the owner rules.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read by the audit_read planner, plan step 7; pinned here by its test"
    )
)]
pub(crate) const AUDIT_PART_DEADLINE: std::time::Duration =
    bridge_tally_transport::DEFAULT_REQUEST_TIMEOUT;

/// Each drain probe gets its own short deadline. That makes an unanswered probe
/// give up sooner; it does not stop the probe reaching Tally, which is why
/// probes are also spaced and capped below.
const AUDIT_DRAIN_PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
/// A probe answered more slowly than this does not count towards the drain.
const AUDIT_DRAIN_PROBE_SLOW: std::time::Duration = std::time::Duration::from_secs(2);
/// Consecutive quick probe answers that clear a drain debt.
const AUDIT_DRAIN_QUICK_PROBES: u8 = 2;
/// The least time between two probes of one endpoint.
const AUDIT_DRAIN_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// Probes that went unanswered before the runtime stops probing and needs the
/// operator. Repeated silence is not "Tally is down": a modal dialog on the
/// Tally screen looks the same, and more requests only queue behind it.
const AUDIT_DRAIN_ABANDONED_PROBES: u8 = 2;
/// A probe started longer ago than this and never finished was dropped by its
/// caller: it counts as abandoned, and another may be sent. Generous, because a
/// live probe can wait up to the endpoint queue's 30 s before its own 5 s.
const AUDIT_DRAIN_PROBE_STALE: std::time::Duration = std::time::Duration::from_secs(60);

/// How an audit_read part is read. Voucher parts are read once; masters are
/// read twice and admitted only when the two responses match byte for byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the audit_read orchestrator, plan step 7")
)]
pub(crate) enum AuditPartShape {
    Single,
    Paired,
}

/// One admitted audit_read part: the HTTP response entity exactly as the
/// transport received it (after chunked framing is removed; content encodings
/// other than identity are refused), read between two identity brackets that
/// both matched, with an export status of success.
///
/// What the brackets prove is narrow: at each, the company list held exactly
/// one company with the complete identity tuple. They say nothing about the
/// state of the book between them, which another user may change; a caller
/// that needs a book-state witness brackets the part with the company's
/// AlterID marks itself.
#[derive(Debug, Clone)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the audit_read orchestrator, plan step 7")
)]
pub(crate) struct AuditPart {
    pub(crate) encoded_body: Vec<u8>,
    pub(crate) encoded_sha256: String,
    /// The decoded text, for admission only. What is sealed is `encoded_body`.
    pub(crate) body: String,
    /// Time spent on the data request or requests, excluding the brackets.
    pub(crate) elapsed: std::time::Duration,
    pub(crate) evidence: RuntimeReadEvidence,
    /// The stricter of the date-boundary profiles the two brackets observed:
    /// Education if either reported it.
    pub(crate) boundary_profile: DateBoundaryProfile,
}

/// Why a part was not admitted. No failure carries a body: a part is admitted
/// whole or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuditPartFailureKind {
    /// A previous response was abandoned and Tally may still be building it.
    /// Nothing was sent.
    DrainRequired,
    /// The request did not name exactly the verified company. Nothing was sent.
    RequestNotCompanyScoped,
    Deadline,
    SizeLimit,
    /// The request reached Tally and the connection ended before a complete
    /// response.
    ConnectionDropped,
    /// Tally's response was refused at its head (HTTP status, content type or
    /// content encoding), usually abandoning the body unread, or because a
    /// complete body could not be decoded. A drain is owed either way; in the
    /// second case it is merely conservative.
    HeadRejected(&'static str),
    /// The read was cancelled. If it had started, Tally may still be working.
    Cancelled,
    /// Nothing reached Tally.
    Unreachable,
    /// The runtime's queue or circuit breaker refused before sending anything.
    NotSent(&'static str),
    /// The two reads of a paired part differed: the book changed between them.
    PairDrift,
    /// The company was absent, ambiguous or changed at a bracket.
    IdentityChanged,
    /// Education mode was observed and would not honour this part's
    /// `SVFROMDATE`/`SVTODATE`: it answers such a window with a well-formed
    /// empty collection (bridge#581). Refused before sending when the opening
    /// bracket reports it, and after the read, discarding the body, when only
    /// the closing bracket does.
    EducationBoundary,
    /// A complete response whose export status was not success, such as an
    /// error envelope for a company Tally could not select.
    ResponseRejected,
    /// Any other refusal, by its existing safe code.
    Other(&'static str),
}

impl AuditPartFailureKind {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::DrainRequired => "audit_part_drain_required",
            Self::RequestNotCompanyScoped => "audit_part_request_not_company_scoped",
            Self::Deadline => "audit_part_deadline_exceeded",
            Self::SizeLimit => "audit_part_response_size_limit_exceeded",
            Self::ConnectionDropped => "audit_part_connection_dropped",
            Self::HeadRejected(_) => "audit_part_response_head_rejected",
            Self::Cancelled => "audit_part_cancelled",
            Self::Unreachable => "audit_part_endpoint_unreachable",
            Self::NotSent(code) | Self::Other(code) => code,
            Self::PairDrift => "audit_part_pair_drift",
            Self::IdentityChanged => "audit_part_company_identity_changed",
            Self::EducationBoundary => "audit_part_window_unsupported_in_education",
            Self::ResponseRejected => "audit_part_response_rejected",
        }
    }

    /// Whether the same request may be sent again later as it stands. A size
    /// refusal is not: the plan must divide the part. An identity change is
    /// not: the read must start again from the company list. A pair drift is,
    /// but a caller must cap how often, because a responder whose output is
    /// not deterministic drifts every time.
    pub(crate) const fn retryable(self) -> bool {
        matches!(
            self,
            Self::DrainRequired
                | Self::Deadline
                | Self::ConnectionDropped
                | Self::Cancelled
                | Self::Unreachable
                | Self::NotSent(_)
                | Self::PairDrift
        )
    }

    /// Whether Bridge may have stopped listening before Tally finished
    /// answering, so Tally may still be building or sending the response. A
    /// client deadline does not stop Tally working (the lab gateway stayed
    /// busy for over 40 minutes after one abandoned read), so no later audit
    /// part is sent to that endpoint until it has been drained.
    pub(crate) const fn owes_drain(self) -> bool {
        matches!(
            self,
            Self::Deadline
                | Self::ConnectionDropped
                | Self::SizeLimit
                | Self::HeadRejected(_)
                | Self::Cancelled
        )
    }
}

#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the audit_read orchestrator, plan step 7")
)]
pub(crate) struct AuditPartFailure {
    pub(crate) kind: AuditPartFailureKind,
    /// Completed source bodies read before the refusal, if any.
    pub(crate) evidence: Option<RuntimeReadEvidence>,
}

impl AuditPartFailure {
    fn new(kind: AuditPartFailureKind) -> Self {
        Self {
            kind,
            evidence: None,
        }
    }
}

/// Where an endpoint's drain stands after [`TallyRuntime::drain_probe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuditDrainStatus {
    Clear,
    Owed {
        quick_probes: u8,
        abandoned_probes: u8,
    },
    /// No probe was sent: the last one was too recent.
    Wait {
        retry_after: std::time::Duration,
    },
    /// No probe was sent, and none will be: probes went unanswered too often.
    /// Someone must look at the Tally screen, and restart Tally if it is
    /// stuck, before [`TallyRuntime::clear_audit_drain_after_operator_check`].
    OperatorRequired,
}

/// One endpoint's drain debt. `ticket` names the part that armed it; an armed
/// part clears only its own entry.
#[derive(Clone, Copy, Debug)]
struct AuditDrainDebt {
    ticket: u64,
    /// Armed by a part that has not settled. A part whose future was dropped
    /// never settles, so its debt stays owed.
    in_flight: bool,
    quick_probes: u8,
    abandoned_probes: u8,
    last_probe: Option<Instant>,
    /// When a probe for this debt was started and has not finished. A
    /// concurrent call sends nothing; a start older than
    /// `AUDIT_DRAIN_PROBE_STALE` was dropped by its caller.
    probe_started: Option<Instant>,
}

impl AuditDrainDebt {
    fn armed(ticket: u64) -> Self {
        Self {
            ticket,
            in_flight: true,
            quick_probes: 0,
            abandoned_probes: 0,
            last_probe: None,
            probe_started: None,
        }
    }
}

type AuditDrainRegistry = Arc<Mutex<HashMap<EndpointKey, AuditDrainDebt>>>;

/// The registry holds plain counters that no update leaves half-written, so a
/// poisoned lock is recovered rather than refusing every endpoint forever.
fn audit_drain_lock(
    registry: &AuditDrainRegistry,
) -> std::sync::MutexGuard<'_, HashMap<EndpointKey, AuditDrainDebt>> {
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Names each armed part, so a part settles only the debt it armed.
static AUDIT_DRAIN_TICKET: AtomicU64 = AtomicU64::new(1);

/// Raised inside the endpoint gate when a drain became owed while this part
/// was queued behind the part that abandoned a response.
#[derive(Debug, thiserror::Error)]
#[error("audit_part_drain_required")]
struct AuditDrainOwedAtDispatch;

/// A complete response whose export status was not success.
#[derive(Debug, thiserror::Error)]
#[error("audit_part_response_rejected")]
struct AuditResponseRejected;

/// Arm this endpoint's drain for `ticket` immediately before anything is sent,
/// inside the endpoint gate, refusing if any debt is already recorded.
fn arm_audit_drain(
    registry: &AuditDrainRegistry,
    endpoint: &EndpointKey,
    ticket: u64,
) -> anyhow::Result<()> {
    let mut owed = audit_drain_lock(registry);
    if owed.contains_key(endpoint) {
        return Err(AuditDrainOwedAtDispatch.into());
    }
    owed.insert(endpoint.clone(), AuditDrainDebt::armed(ticket));
    Ok(())
}

/// A part's armed debt. The part settles it with what it knows; if the part's
/// future is dropped first (a caller timeout, a cancel, an abort), dropping the
/// guard leaves the debt owed. So `in_flight` means a part that is still
/// running, never one that was abandoned.
struct ArmedAuditDrain {
    registry: AuditDrainRegistry,
    endpoint: EndpointKey,
    ticket: u64,
    settled: bool,
}

impl ArmedAuditDrain {
    fn settle(mut self, owed_now: bool) {
        self.settled = true;
        settle_audit_drain(&self.registry, &self.endpoint, self.ticket, owed_now);
    }
}

impl Drop for ArmedAuditDrain {
    fn drop(&mut self) {
        if !self.settled {
            settle_audit_drain(&self.registry, &self.endpoint, self.ticket, true);
        }
    }
}

/// Settle the entry `ticket` armed: remove it when nothing was left running in
/// Tally, or keep it as an owed debt when something may have been.
fn settle_audit_drain(
    registry: &AuditDrainRegistry,
    endpoint: &EndpointKey,
    ticket: u64,
    owed_now: bool,
) {
    let mut owed = audit_drain_lock(registry);
    match owed.get_mut(endpoint) {
        Some(debt) if debt.ticket == ticket && debt.in_flight => {
            if owed_now {
                debt.in_flight = false;
            } else {
                owed.remove(endpoint);
            }
        }
        _ => {}
    }
}

/// Whether `request` names exactly one `SVCURRENTCOMPANY`, in
/// `ENVELOPE/BODY/DESC/STATICVARIABLES` where Tally reads it, equal to the
/// verified company's name. Without one there, Tally reads whichever company
/// is active; with a different one, it reads that company. An element of that
/// name anywhere else is refused too, rather than trusted to be inert.
fn request_scopes_company(request: &str, company: &str) -> bool {
    use quick_xml::events::Event;
    const STATIC_VARIABLES: [&[u8]; 4] = [b"ENVELOPE", b"BODY", b"DESC", b"STATICVARIABLES"];
    let mut reader = quick_xml::Reader::from_str(request);
    let mut path = Vec::<Vec<u8>>::new();
    let mut named = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if name == b"SVCURRENTCOMPANY" {
                    let anchored = path.len() == STATIC_VARIABLES.len()
                        && path
                            .iter()
                            .zip(STATIC_VARIABLES)
                            .all(|(part, expected)| part.as_slice() == expected);
                    if !anchored {
                        return false;
                    }
                    let Ok(text) = reader.read_text(element.name()) else {
                        return false;
                    };
                    let Ok(raw) = text.decode() else {
                        return false;
                    };
                    let Ok(text) = quick_xml::escape::unescape(&raw) else {
                        return false;
                    };
                    named.push(text.into_owned());
                } else {
                    path.push(name);
                }
            }
            Ok(Event::Empty(element))
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"SVCURRENTCOMPANY") =>
            {
                return false;
            }
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    matches!(named.as_slice(), [only] if only == company)
}

fn classify_audit_part_failure(error: &anyhow::Error) -> AuditPartFailure {
    let kind = if error
        .chain()
        .any(|cause| cause.is::<AuditDrainOwedAtDispatch>())
    {
        AuditPartFailureKind::DrainRequired
    } else if error
        .chain()
        .any(|cause| cause.is::<EducationBoundaryRefusal>())
    {
        AuditPartFailureKind::EducationBoundary
    } else if error.chain().any(|cause| cause.is::<ToolCancelled>()) {
        // Withdrawn before the operation was queued: nothing was sent.
        AuditPartFailureKind::NotSent("request_cancelled")
    } else if let Some(transport) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TallyTransportError>())
    {
        match transport {
            TallyTransportError::RequestTimedOut => AuditPartFailureKind::Deadline,
            TallyTransportError::ResponseTooLarge { .. } => AuditPartFailureKind::SizeLimit,
            TallyTransportError::RequestFailed
            | TallyTransportError::ResponseReadFailed
            | TallyTransportError::ResponseTruncated => AuditPartFailureKind::ConnectionDropped,
            TallyTransportError::HttpStatus { .. }
            | TallyTransportError::InvalidEncoding { .. }
            | TallyTransportError::UnsupportedContentEncoding => {
                AuditPartFailureKind::HeadRejected(transport.safe_code())
            }
            TallyTransportError::ConnectionFailed => AuditPartFailureKind::Unreachable,
            other => AuditPartFailureKind::Other(other.safe_code()),
        }
    } else if error
        .chain()
        .any(|cause| cause.is::<crate::tally::connection::NativeReportPairDrift>())
    {
        AuditPartFailureKind::PairDrift
    } else if error
        .chain()
        .any(|cause| cause.is::<CompanyIdentityBracketError>())
    {
        AuditPartFailureKind::IdentityChanged
    } else if error
        .chain()
        .any(|cause| cause.is::<AuditResponseRejected>())
    {
        AuditPartFailureKind::ResponseRejected
    } else if let Some(control) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TallyRuntimeControlError>())
    {
        match control {
            TallyRuntimeControlError::Cancelled => AuditPartFailureKind::Cancelled,
            TallyRuntimeControlError::QueueDeadline => {
                AuditPartFailureKind::NotSent("endpoint_queue_deadline_exceeded")
            }
            TallyRuntimeControlError::CircuitCooldown => {
                AuditPartFailureKind::NotSent("endpoint_circuit_cooldown")
            }
            TallyRuntimeControlError::HalfOpenProbeInFlight => {
                AuditPartFailureKind::NotSent("endpoint_half_open_probe_in_flight")
            }
            TallyRuntimeControlError::EndpointSessionCapacity => {
                AuditPartFailureKind::NotSent("endpoint_session_capacity_reached")
            }
        }
    } else {
        AuditPartFailureKind::Other("audit_part_read_failed")
    };
    AuditPartFailure {
        kind,
        evidence: error
            .chain()
            .find_map(|cause| cause.downcast_ref::<RuntimeReadFailure>())
            .map(|failure| failure.evidence.clone()),
    }
}

async fn fetch_admitted_audit_part(
    client: &TallyClient,
    identity: &VerifiedCompanyIdentity,
    request: super::agent_read_request::AgentReadRequest,
    shape: AuditPartShape,
) -> anyhow::Result<AuditPart> {
    // As for every admitted agent read (bridge#581): the mode comes from the
    // bracket's own company list, and a window Education would serve empty is
    // refused before it is sent.
    let opening = bracket_verified_company_identity_observing_mode(client, identity).await?;
    if !request.window_accepted_by(opening) {
        return Err(EducationBoundaryRefusal.into());
    }
    let window = request.clone();
    let request_xml = request.into_xml();
    let started = Instant::now();
    let raw = match shape {
        AuditPartShape::Single => client.post_xml_raw(request_xml.clone()).await?,
        AuditPartShape::Paired => client.post_xml_raw_paired(request_xml.clone()).await?,
    };
    let elapsed = started.elapsed();
    let evidence = match shape {
        AuditPartShape::Single => RuntimeReadEvidence::single(
            &request_xml,
            raw.encoded_sha256.clone(),
            raw.encoded_body.len(),
        ),
        AuditPartShape::Paired => RuntimeReadEvidence::paired(
            &request_xml,
            raw.encoded_sha256.clone(),
            raw.encoded_body.len(),
        ),
    };
    if !matches!(
        bridge_tally_protocol::export_status(&raw.text),
        Ok(bridge_tally_protocol::TallyExportStatus::Success)
    ) {
        return Err(with_read_evidence(
            AuditResponseRejected.into(),
            evidence.clone(),
        ));
    }
    let closing = bracket_verified_company_identity_observing_mode(client, identity)
        .await
        .map_err(|error| with_read_evidence(error, evidence.clone()))?;
    // Education reported only after the part may have been served by it: which
    // mode answered is unknown, so the part is refused as if sent in Education.
    if !window.window_accepted_by(closing) {
        return Err(with_read_evidence(
            EducationBoundaryRefusal.into(),
            evidence.clone(),
        ));
    }
    let boundary_profile = if closing == DateBoundaryProfile::EducationRestricted {
        closing
    } else {
        opening
    };
    Ok(AuditPart {
        encoded_body: raw.encoded_body,
        encoded_sha256: raw.encoded_sha256,
        body: raw.text,
        elapsed,
        evidence,
        boundary_profile,
    })
}

/// An approved import response retains the import wire separately from the
/// source observations that admitted it. The ledger's request/response hashes
/// must remain the raw import bytes, not a composite admission digest.
#[derive(Debug, Clone)]
pub(crate) struct ApprovedImportDispatch {
    pub(crate) body: String,
    pub(crate) response_evidence: RuntimeReadEvidence,
    pub(crate) admission_evidence: RuntimeReadEvidence,
}

/// Commitments to completed runtime source observations, using actual encoded
/// request and response bodies. Stable pairs share a hash and count both bodies;
/// a failed pair may retain only its first body, or combine differing bodies.
/// Auxiliary health and identity guards are not native-report source bodies.
/// Retried operations retain their terminal attempt only, not total traffic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeReadEvidence {
    pub request_sha256: String,
    pub response_sha256: String,
    pub bytes: usize,
}

tokio::task_local! {
    /// The cancellation of the one agent tool call running in this task, when
    /// its caller can withdraw it (an MCP `notifications/cancelled`, or the host
    /// closing its input). Checked before each queued operation starts, never
    /// during one: an operation already sent to Tally runs to completion, since
    /// abandoning a request does not stop Tally (protocol reference §11b.2).
    pub(crate) static TOOL_CANCELLATION: CancellationToken;
}

/// The tool call was withdrawn before this operation started; nothing was sent.
#[derive(Debug, thiserror::Error)]
#[error("request_cancelled")]
pub(crate) struct ToolCancelled;

/// Retains admitted source commitments when a runtime read cannot be released.
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
pub(crate) struct RuntimeReadFailure {
    pub(crate) evidence: RuntimeReadEvidence,
    #[source]
    source: anyhow::Error,
}

pub(crate) fn with_read_evidence(
    source: anyhow::Error,
    prior: RuntimeReadEvidence,
) -> anyhow::Error {
    let evidence = match source.downcast_ref::<RuntimeReadFailure>() {
        Some(failure) => prior.combine(failure.evidence.clone()),
        None => prior,
    };
    RuntimeReadFailure { evidence, source }.into()
}

impl RuntimeReadEvidence {
    pub(crate) fn empty() -> Self {
        Self {
            request_sha256: String::new(),
            response_sha256: String::new(),
            bytes: 0,
        }
    }

    pub(crate) fn paired(request: &str, response_sha256: String, encoded_bytes: usize) -> Self {
        // Stable pairs share one commitment and count both completed bodies.
        Self::single(request, response_sha256, encoded_bytes.saturating_mul(2))
    }

    pub(crate) fn single(request: &str, response_sha256: String, encoded_bytes: usize) -> Self {
        Self {
            request_sha256: sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
                request,
            )),
            response_sha256,
            bytes: encoded_bytes,
        }
    }

    pub(crate) fn combine(self, other: Self) -> Self {
        if self.request_sha256.is_empty() && self.response_sha256.is_empty() {
            return other;
        }
        if other.request_sha256.is_empty() && other.response_sha256.is_empty() {
            return self;
        }
        Self {
            request_sha256: sha256_hex(
                format!("{}:{}", self.request_sha256, other.request_sha256).as_bytes(),
            ),
            response_sha256: sha256_hex(
                format!("{}:{}", self.response_sha256, other.response_sha256).as_bytes(),
            ),
            bytes: self.bytes.saturating_add(other.bytes),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn agent_company_list_from_response(
    response: String,
    encoded_bytes: usize,
    encoded_sha256: String,
) -> anyhow::Result<AgentCompanyList> {
    let companies = parse_companies_from_collection(&response)?;
    Ok(AgentCompanyList {
        response_bytes: encoded_bytes,
        response_sha256: encoded_sha256,
        companies,
    })
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CompanyIdentityBracketError {
    #[error(
        "Tally returned a presentation-equivalent same-GUID company with a distinct book tuple"
    )]
    PresentationCollision,
    #[error("Tally complete company identity was absent or ambiguous")]
    AbsentOrAmbiguous,
}

/// Re-enumerate the complete identity immediately before or after a scoped
/// read. Tally accepts a company name as the scope selector, so the GUID alone
/// is not a sufficient witness when company names differ only by presentation.
async fn bracket_verified_company_identity(
    client: &TallyClient,
    identity: &VerifiedCompanyIdentity,
) -> anyhow::Result<()> {
    let companies = client.fetch_companies().await?;
    admit_company_identity(&companies, identity)
}

/// As [`bracket_verified_company_identity`], also returning the date-boundary
/// profile the same company-list response reports. Education mode is read from
/// the `EDUMODE` field every `CompanyListV2` row carries, so observing it here
/// costs no request. Any `EDUMODE` field that does not say `No` is enough, even
/// when the other capability fields do not parse. A response with no `EDUMODE`
/// field at all keeps the mode-agnostic profile, as before bridge#581.
async fn bracket_verified_company_identity_observing_mode(
    client: &TallyClient,
    identity: &VerifiedCompanyIdentity,
) -> anyhow::Result<DateBoundaryProfile> {
    let (companies, _, education) = client.fetch_companies_observing_education_mode().await?;
    admit_company_identity(&companies, identity)?;
    Ok(if education {
        DateBoundaryProfile::EducationRestricted
    } else {
        DateBoundaryProfile::ModeAgnostic
    })
}

/// A read refused because Education mode would not honour its
/// `SVFROMDATE`/`SVTODATE` (the 1st, 2nd or 31st only). Education answers such
/// a read with a well-formed empty collection, not an error (bridge#581), so it
/// is refused before sending when the opening identity bracket reports
/// Education, and after the read, discarding it, when only the closing bracket
/// does. Raised by agent reads (reported as this code) and audit parts
/// (reported as `audit_part_window_unsupported_in_education`).
#[derive(Debug, thiserror::Error)]
#[error("window_part_boundary_unsupported_in_education")]
pub(crate) struct EducationBoundaryRefusal;

/// A read was refused before it was sent: the endpoint reported Education mode,
/// and the request is one of Bridge's custom reports whose TDL passes a spaced
/// collection identifier to a `$$` function. Education answered one such report
/// (`ledgers_v1`) with a blocking "Bad formula!" dialog on the Tally screen,
/// which holds the XML gateway until someone dismisses it (bridge#45); the
/// others carry the same construct. Such a read needs a licensed
/// Tally until the Collection-based reads replace it.
#[derive(Debug, thiserror::Error)]
#[error("education_report_family_unsupported")]
pub(crate) struct EducationReportFamilyRefusal;

/// Refuses a report-formula read when the bracket that precedes it observed
/// Education mode ([`EducationReportFamilyRefusal`]).
fn refuse_report_formula_in_education(profile: DateBoundaryProfile) -> anyhow::Result<()> {
    if profile == DateBoundaryProfile::EducationRestricted {
        return Err(EducationReportFamilyRefusal.into());
    }
    Ok(())
}

fn admit_company_identity(
    companies: &[TallyCompany],
    identity: &VerifiedCompanyIdentity,
) -> anyhow::Result<()> {
    if companies
        .iter()
        .any(|company| identity.is_presentation_equivalent_guid_sibling(company))
    {
        return Err(CompanyIdentityBracketError::PresentationCollision.into());
    }
    let matches = companies
        .iter()
        .filter(|company| identity.matches_observed_company(company))
        .count();
    if matches != 1 {
        return Err(CompanyIdentityBracketError::AbsentOrAmbiguous.into());
    }
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

/// A typed INR admission for a monetary document. Outstandings may receive an
/// explicit operator assertion, while the party/ledger export constructs this
/// only after the existing Tally currency probe establishes one INR master.
/// No other currency can reach an INR formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum OutstandingsCurrencyAssertion {
    #[serde(rename = "INR")]
    Inr,
}

/// An INR admission that is inseparable from the company extent observed
/// during the currency read. Party/ledger masters and MCP outstandings consume
/// this witness; desktop outstandings retains its explicit operator assertion.
#[derive(Debug, Clone)]
pub(crate) struct PartyLedgerMasterCurrencyAssertion {
    assertion: OutstandingsCurrencyAssertion,
    decimal_places: u8,
    currency_read_extent: CompanyBookExtent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PartyLedgerMasterCurrency {
    pub(crate) assertion: OutstandingsCurrencyAssertion,
    pub(crate) decimal_places: u8,
}

impl PartyLedgerMasterCurrencyAssertion {
    /// Releases the INR assertion only when the monetary master read opens on
    /// the exact company extent that the existing currency probe observed.
    pub(crate) fn require_opening_extent(
        &self,
        opening_extent: &CompanyBookExtent,
    ) -> anyhow::Result<PartyLedgerMasterCurrency> {
        if self.currency_read_extent == *opening_extent {
            return Ok(PartyLedgerMasterCurrency {
                assertion: self.assertion,
                decimal_places: self.decimal_places,
            });
        }

        Err(anyhow::Error::new(
            PairedReadValidationError::CurrencyToMasterExtent,
        ))
    }
}

/// `CompanyCurrencyRead::admit_inr` refused to label this company's figures
/// as INR. The code is one of that function's static reasons, never data.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub(crate) struct CurrencyAdmissionRefusal(pub(crate) &'static str);

/// The result of the existing Tally currency probe, retaining the extent that
/// bracketed it so a monetary document cannot separate the two facts.
#[derive(Debug, Clone)]
pub(crate) struct CompanyCurrencyRead {
    currency: CompanyCurrency,
    extent: CompanyBookExtent,
    evidence: RuntimeReadEvidence,
}

impl CompanyCurrencyRead {
    pub(crate) fn evidence(&self) -> RuntimeReadEvidence {
        self.evidence.clone()
    }

    pub(crate) fn admit_inr(self) -> Result<PartyLedgerMasterCurrencyAssertion, &'static str> {
        match (self.currency_count(), self.is_inr()) {
            (1, true) => {
                Ok(self.bind_party_ledger_master_assertion(OutstandingsCurrencyAssertion::Inr))
            }
            (0, _) => Err("company_currency_probe_failed"),
            (1, false) => Err("company_base_currency_not_inr"),
            _ => Err("company_base_currency_undetermined"),
        }
    }

    pub(crate) fn currency_count(&self) -> usize {
        self.currency.currency_count
    }

    pub(crate) fn is_inr(&self) -> bool {
        self.currency.is_inr
    }

    pub(crate) fn bind_party_ledger_master_assertion(
        self,
        assertion: OutstandingsCurrencyAssertion,
    ) -> PartyLedgerMasterCurrencyAssertion {
        PartyLedgerMasterCurrencyAssertion {
            assertion,
            decimal_places: self.currency.decimal_places,
            currency_read_extent: self.extent,
        }
    }
}

#[derive(Clone)]
enum NativeOutstandingsCurrency {
    Operator(OutstandingsCurrencyAssertion),
    Observed(PartyLedgerMasterCurrencyAssertion),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutstandingsReadStrategy {
    NativeBills,
    VoucherScan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OutstandingsLoadResult {
    Complete {
        report: Box<OutstandingsReport>,
        /// The read path that produced `report`, preserved separately from
        /// every count so an empty result cannot be mislabeled as another
        /// strategy.
        read_strategy: OutstandingsReadStrategy,
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

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum OpeningBoundaryObservationError {
    #[error("opening_period_not_honoured")]
    Period(NativeLedgerExportPeriodError),
    #[error("opening_boundary_profile_not_observed")]
    Unobserved,
    #[error("opening_boundary_profile_changed")]
    Changed,
}

pub(crate) fn observed_opening_boundary(
    profile: &bridge_tally_core::CapabilityProfile,
) -> Result<DateBoundaryProfile, OpeningBoundaryObservationError> {
    use bridge_tally_core::{CapabilityFeatureId, CapabilityState, EvidenceConfidence};
    let product = profile
        .product
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let observed = profile
        .features
        .get(&CapabilityFeatureId::ProductAndMode)
        .is_some_and(|feature| {
            feature.state == CapabilityState::Supported
                && feature.confidence == EvidenceConfidence::Observed
        });
    if !observed || product != "tallyprime" {
        return Err(OpeningBoundaryObservationError::Unobserved);
    }
    match profile.mode.as_deref() {
        Some(mode)
            if mode.eq_ignore_ascii_case("education")
                || mode.eq_ignore_ascii_case("educational") =>
        {
            Ok(DateBoundaryProfile::EducationRestricted)
        }
        Some(mode) if mode.eq_ignore_ascii_case("licensed") => {
            Ok(DateBoundaryProfile::ModeAgnostic)
        }
        _ => Err(OpeningBoundaryObservationError::Unobserved),
    }
}

async fn observe_read_boundary(
    client: &TallyClient,
) -> anyhow::Result<(DateBoundaryProfile, RuntimeReadEvidence)> {
    let (probe, evidence) = client.probe_with_wire_evidence().await?;
    let boundary = observed_opening_boundary(&probe.profile)
        .map_err(|error| with_read_evidence(error.into(), evidence.clone()))?;
    Ok((boundary, evidence))
}

async fn confirm_read_boundary(
    client: &TallyClient,
    opening_profile: DateBoundaryProfile,
) -> anyhow::Result<RuntimeReadEvidence> {
    // Repeat the observed product/mode admission. A change to Education changes
    // the boundary rules for the same operation; release and licence tier are
    // retained as observed facts and do not categorically refuse the read.
    let (closing_profile, evidence) = observe_read_boundary(client).await?;
    if closing_profile != opening_profile {
        return Err(with_read_evidence(
            OpeningBoundaryObservationError::Changed.into(),
            evidence,
        ));
    }
    Ok(evidence)
}

fn ledger_opening_period(
    profile: DateBoundaryProfile,
    books_from: &TallyDate,
    last_voucher_date: &TallyDate,
    requested_from: Option<&TallyDate>,
) -> Result<NativeLedgerExportPeriod, NativeLedgerExportPeriodError> {
    let from = requested_from.unwrap_or(books_from);
    if from < books_from {
        return Err(NativeLedgerExportPeriodError::InvalidRange);
    }
    // The export does not fetch closing balances; `to` only avoids an inverted
    // request when the requested opening follows the last posted voucher.
    let to = if requested_from.is_some() {
        last_voucher_date.max(from)
    } else {
        last_voucher_date
    };
    NativeLedgerExportPeriod::new(profile, from.clone(), to.clone())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum NativeLedgerIdentityAdmissionError {
    #[error("native_ledger_identity_duplicate")]
    Duplicate,
    #[error("native_ledger_master_id_invalid")]
    InvalidMasterId,
}

fn admit_native_ledger_opening_rows(
    xml: &str,
    company_guid: &str,
) -> anyhow::Result<Vec<TallyLedger>> {
    let parsed = parse_native_ledger_source_records_with_evidence(xml, company_guid)?;
    if !parsed.evidence.duplicate_identities.is_empty() {
        return Err(NativeLedgerIdentityAdmissionError::Duplicate.into());
    }
    // The parser commits duplicate GUIDs and textual aliases. MASTERID is a
    // numeric identity, so leading-zero spellings must not create another row.
    let mut master_ids = std::collections::HashSet::new();
    for record in &parsed.records {
        let master_id = record
            .identities
            .master_id
            .as_deref()
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(NativeLedgerIdentityAdmissionError::InvalidMasterId)?;
        if !master_ids.insert(master_id) {
            return Err(NativeLedgerIdentityAdmissionError::Duplicate.into());
        }
    }
    Ok(parsed
        .records
        .into_iter()
        .map(|record| record.record)
        .collect())
}

#[cfg(test)]
#[path = "runtime_ledger_opening_tests.rs"]
mod ledger_opening_tests;

#[cfg(test)]
#[path = "runtime_financial_mode_tests.rs"]
mod financial_mode_tests;

#[cfg(test)]
#[path = "runtime_agent_read_evidence_tests.rs"]
mod agent_read_evidence_tests;

#[cfg(test)]
#[path = "runtime_audit_part_tests.rs"]
mod audit_part_tests;

#[cfg(test)]
#[path = "runtime_party_evidence_tests.rs"]
mod party_evidence_tests;

#[cfg(test)]
#[path = "runtime_import_admission_tests.rs"]
mod import_admission_tests;

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
    /// Endpoints owed a drain after an abandoned audit part, with the count of
    /// consecutive quick probe answers so far. Kept here, not on a session,
    /// because a session can be evicted while Tally is still busy.
    audit_drain: AuditDrainRegistry,
    audit_drain_probe_interval: std::time::Duration,
    audit_drain_probe_stale: std::time::Duration,
    audit_drain_probe_slow: std::time::Duration,
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
            audit_drain: Arc::new(Mutex::new(HashMap::new())),
            audit_drain_probe_interval: AUDIT_DRAIN_PROBE_INTERVAL,
            audit_drain_probe_stale: AUDIT_DRAIN_PROBE_STALE,
            audit_drain_probe_slow: AUDIT_DRAIN_PROBE_SLOW,
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
        // The only point a withdrawn agent tool call stops: before this
        // operation is queued, so nothing further is sent. Never mid-operation.
        if TOOL_CANCELLATION
            .try_with(CancellationToken::is_cancelled)
            .unwrap_or(false)
        {
            return Err(ToolCancelled.into());
        }
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
        self.probe_with_wire_observation(config)
            .await
            .map(|(review, at, result, _)| (review, at, result))
    }

    pub(crate) async fn probe_with_wire_evidence(
        &self,
        config: TallyConfig,
    ) -> anyhow::Result<(TallyProbeResult, RuntimeReadEvidence)> {
        self.probe_with_wire_observation(config)
            .await
            .map(|(_, _, result, evidence)| (result, evidence))
    }

    async fn probe_with_wire_observation(
        &self,
        config: TallyConfig,
    ) -> anyhow::Result<(String, i64, TallyProbeResult, RuntimeReadEvidence)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let session = self.session(config.clone())?;
        let (result, evidence) = self
            .execute(
                config,
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe_with_wire_evidence().await },
            )
            .await?;
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let review_id = uuid::Uuid::new_v4().to_string();
        let mut cache = session.cached_probe.write().map_err(|_| {
            with_read_evidence(
                anyhow::anyhow!("Tally capability cache is unavailable"),
                evidence.clone(),
            )
        })?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            return Err(with_read_evidence(
                anyhow::anyhow!("Tally reviewed setup save is in progress"),
                evidence,
            ));
        }
        *cache = Some(CachedProbe {
            review_id: review_id.clone(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: result.clone(),
            reserved: false,
        });
        Ok((review_id, observed_at_unix_ms, result, evidence))
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

    /// As [`Self::fetch_companies`], also returning whether the same
    /// `CompanyListV2` response may come from an Education-mode endpoint
    /// ([`TallyClient::fetch_companies_observing_education_mode`]). No further
    /// request is made.
    pub async fn fetch_companies_observing_education_mode(
        &self,
        config: TallyConfig,
    ) -> anyhow::Result<(Vec<TallyCompany>, bool)> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::CompanyList,
            ReadRetryPolicy::transient_default(),
            |client| async move {
                client
                    .fetch_companies_observing_education_mode()
                    .await
                    .map(|(companies, _, education)| (companies, education))
            },
        )
        .await
    }

    /// Reads the documented company collection and retains evidence for the
    /// exact raw response bytes used to produce the parsed company list. As in
    /// the shared retry runtime, only the terminal attempt contributes evidence.
    pub async fn fetch_agent_companies(
        &self,
        config: TallyConfig,
    ) -> anyhow::Result<AgentCompanyList> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::CompanyList,
            ReadRetryPolicy::transient_default(),
            |client| async move {
                let request = ReadOnlyProfile::CompanyListV2.render();
                let (body, encoded_bytes, encoded_sha256) = client
                    .fetch_native_report_paired_with_evidence(request.clone())
                    .await?;
                let evidence =
                    RuntimeReadEvidence::paired(&request, encoded_sha256.clone(), encoded_bytes);
                agent_company_list_from_response(body, encoded_bytes, encoded_sha256)
                    .map_err(|error| with_read_evidence(error, evidence))
            },
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
        self.fetch_ledgers_with_evidence(config, identity)
            .await
            .map(|(ledgers, _)| ledgers)
    }

    /// Reads the BOOKSFROM-pinned ledger export and retains the paired wire
    /// evidence used by agent-visible ledger responses.
    pub async fn fetch_ledgers_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<(Vec<TallyLedger>, RuntimeReadEvidence)> {
        self.fetch_ledger_opening_with_evidence(config, identity, None)
            .await
            .map(|(ledgers, _, evidence)| (ledgers, evidence))
    }

    /// As `fetch_ledgers_with_evidence`, also returning the `SVFROMDATE` the
    /// export was pinned to (the admitted BOOKSFROM). Each ledger's
    /// `OPENINGBALANCE` is the opening at that date (TALLY_PROTOCOL_REFERENCE
    /// §5.5), which on a multi-year book is not the current year's opening.
    pub async fn fetch_ledgers_with_opening_as_of_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<(Vec<TallyLedger>, TallyDate, RuntimeReadEvidence)> {
        self.fetch_ledger_opening_with_evidence(config, identity, None)
            .await
    }

    /// Reads the native period opening at `from`, retaining the existing paired
    /// wire, company identity and book-extent checks. TALLY_PROTOCOL_REFERENCE
    /// section 5.5 records the observed account-dependent period semantics;
    /// callers must not treat every native opening as a running balance.
    pub async fn fetch_ledger_opening_at_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        from: TallyDate,
    ) -> anyhow::Result<(Vec<TallyLedger>, RuntimeReadEvidence)> {
        self.fetch_ledger_opening_with_evidence(config, identity, Some(from))
            .await
            .map(|(ledgers, _, evidence)| (ledgers, evidence))
    }

    async fn fetch_ledger_opening_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        opening_date: Option<TallyDate>,
    ) -> anyhow::Result<(Vec<TallyLedger>, TallyDate, RuntimeReadEvidence)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let identity = identity.clone();
                let opening_date = opening_date.clone();
                async move {
                    let mut evidence = RuntimeReadEvidence::empty();
                    let result = async {
                        // A prior status call is not admission: the gateway's current
                        // licence mode can differ from the cached observation.
                        let (boundary_profile, opening_evidence) =
                            observe_read_boundary(&client).await?;
                        evidence = opening_evidence;
                        bracket_verified_company_identity(&client, &identity).await?;
                        let opening_extent = client.fetch_company_book_extent(&identity).await?;
                        let period = ledger_opening_period(
                            boundary_profile,
                            opening_extent.books_from(),
                            opening_extent.last_voucher_date(),
                            opening_date.as_ref(),
                        )
                        .map_err(OpeningBoundaryObservationError::Period)?;
                        let request =
                            render_native_ledger_export_request(identity.display_name(), &period);
                        let paired = client.fetch_native_report_paired(request.clone()).await?;
                        let (body, encoded_bytes, encoded_sha256) = paired
                            .require_stable(PairedReadValidationError::NativeLedgerCollection)?;
                        evidence = evidence.clone().combine(RuntimeReadEvidence::paired(
                            &request,
                            encoded_sha256,
                            encoded_bytes,
                        ));
                        let ledgers =
                            admit_native_ledger_opening_rows(&body, identity.company_guid())?;
                        let closing_extent = client.fetch_company_book_extent(&identity).await?;
                        if closing_extent != opening_extent {
                            return Err(anyhow::Error::new(
                                PairedReadValidationError::NativeLedgerExtent,
                            ));
                        }
                        bracket_verified_company_identity(&client, &identity).await?;
                        let closing_evidence =
                            confirm_read_boundary(&client, boundary_profile).await?;
                        evidence = evidence.clone().combine(closing_evidence);
                        Ok((ledgers, period.from().clone(), evidence.clone()))
                    }
                    .await;
                    result.map_err(|error| with_read_evidence(error, evidence))
                }
            },
        )
        .await
    }

    pub(crate) async fn fetch_party_ledger_master_source(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        currency_assertion: PartyLedgerMasterCurrencyAssertion,
    ) -> anyhow::Result<PartyLedgerMasterSource> {
        self.fetch_party_ledger_master_source_with_evidence(config, identity, currency_assertion)
            .await
            .map(|(source, _)| source)
    }

    async fn fetch_party_ledger_master_source_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        currency_assertion: PartyLedgerMasterCurrencyAssertion,
    ) -> anyhow::Result<(PartyLedgerMasterSource, RuntimeReadEvidence)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                let currency_assertion = currency_assertion.clone();
                async move {
                    let mut evidence = RuntimeReadEvidence::empty();
                    let result = async {
                        let (boundary_profile, opening_evidence) =
                            observe_read_boundary(&client).await?;
                        evidence = opening_evidence;
                        bracket_verified_company_identity(&client, &identity).await?;
                        let source = client
                            .fetch_party_ledger_master_source(
                                &identity,
                                boundary_profile,
                                currency_assertion,
                            )
                            .await?;
                        evidence =
                            Self::party_ledger_master_source_evidence(&source, evidence.clone());
                        bracket_verified_company_identity(&client, &identity).await?;
                        let closing_evidence =
                            confirm_read_boundary(&client, boundary_profile).await?;
                        evidence = evidence.clone().combine(closing_evidence);
                        Ok((source, evidence.clone()))
                    }
                    .await;
                    result.map_err(|error| with_read_evidence(error, evidence))
                }
            },
        )
        .await
    }

    /// Returns the existing paired, identity-bracketed party ledger source in
    /// a library-safe record form together with its retained response
    /// commitments. Currency admission remains inside the runtime so callers
    /// cannot label an unverified currency as INR or bypass the paired master
    /// read.
    ///
    /// The group collection is returned alongside the records rather than
    /// dropped: `fetch_party_ledger_master_source` already reads it, in the
    /// same company-bracketed triple as the master and balance rows, purely
    /// to let Schedule III classify the party rows it captures. A caller that
    /// needs ledger *ancestry* (ledger_masters' compliance path) can now
    /// build a `GroupIndex` from this without any additional Tally read.
    pub async fn fetch_agent_party_ledger_masters_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<(
        Vec<bridge_tally_protocol::PartyLedgerMasterRecord>,
        Vec<bridge_tally_protocol::TallyNamedMaster>,
        TallyDate,
        RuntimeReadEvidence,
    )> {
        let currency_read = self
            .detect_base_currency_with_extent(config.clone(), identity)
            .await?;
        let currency_evidence = currency_read.evidence.clone();
        let assertion = currency_read.admit_inr().map_err(|code| {
            with_read_evidence(
                anyhow::Error::new(CurrencyAdmissionRefusal(code)),
                currency_evidence.clone(),
            )
        })?;
        let (source, source_evidence) = self
            .fetch_party_ledger_master_source_with_evidence(config, identity, assertion)
            .await
            .map_err(|error| with_read_evidence(error, currency_evidence.clone()))?;
        let evidence = currency_evidence.combine(source_evidence);
        let groups = source.groups.clone();
        // The master request's SVFROMDATE (the admitted BOOKSFROM): each opening is as of it.
        let opening_as_of = source.from.clone();
        let records = source
            .rows
            .into_iter()
            .map(|row| bridge_tally_protocol::PartyLedgerMasterRecord {
                ledger: TallyLedger {
                    name: row.name,
                    parent: row.parent,
                    party_gstin: row.party_gstin,
                    opening_balance: Some(row.opening_balance.as_str().to_string()),
                },
                fields: row.fields,
            })
            .collect();
        Ok((records, groups, opening_as_of, evidence))
    }

    /// Retain the three actual request body commitments alongside their paired
    /// source responses, and include the currency observation that admitted them.
    fn party_ledger_master_source_evidence(
        source: &PartyLedgerMasterSource,
        currency_evidence: RuntimeReadEvidence,
    ) -> RuntimeReadEvidence {
        currency_evidence.combine(RuntimeReadEvidence {
            request_sha256: source.request_sha256.clone(),
            response_sha256: sha256_hex(
                format!(
                    "{}:{}:{}",
                    source.master_response_sha256,
                    source.balance_response_sha256,
                    source.group_response_sha256,
                )
                .as_bytes(),
            ),
            bytes: source
                .master_response_bytes
                .saturating_add(source.balance_response_bytes)
                .saturating_add(source.group_response_bytes)
                .saturating_mul(2),
        })
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
                    refuse_report_formula_in_education(
                        bracket_verified_company_identity_observing_mode(&client, &identity)
                            .await?,
                    )?;
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
                    let vouchers = client.fetch_vouchers(&identity, &from, &to).await?;
                    bracket_verified_company_identity(&client, &identity).await?;
                    Ok(vouchers)
                }
            },
        )
        .await
    }

    /// Dispatches a read-only adapter request through the same serialized,
    /// identity-bracketed runtime used by the product read paths.  The caller
    /// owns parsing the documented response shape; it cannot bypass the
    /// loopback policy, queue, or complete company-tuple witness.
    pub(crate) async fn fetch_agent_read(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        request: super::agent_read_request::AgentReadRequest,
    ) -> anyhow::Result<AgentRead> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::OtherRead,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                let request = request.clone();
                async move {
                    fetch_admitted_agent_read(&client, &identity, request)
                        .await
                        .map(|(read, _)| read)
                }
            },
        )
        .await
    }

    /// One audit_read part: a single attempt, bracketed by the complete company
    /// identity, whose response entity is kept byte-for-byte.
    ///
    /// Nothing partial is ever returned. The drain debt is armed inside the
    /// endpoint gate immediately before anything is sent, and cleared only when
    /// the part settles with nothing left running in Tally. So a part that
    /// abandons a response, is cancelled, or whose future is dropped leaves the
    /// endpoint owed, and every later audit part to it, including one already
    /// queued, is refused without sending anything until [`Self::drain_probe`]
    /// clears it.
    ///
    /// What this does not do, and a caller must:
    /// - Other runtime reads (`fetch_agent_read`, status, catalogue reads)
    ///   ignore the debt. A caller must send no other read to the endpoint
    ///   while a debt is owed.
    /// - The debt lives in this process's memory. It is not shared with another
    ///   Bridge process on the same machine and does not survive a restart.
    ///   It is keyed by the canonical loopback origin, so `127.0.0.1` and
    ///   `localhost` share one debt, but another spelling of the same instance
    ///   would not.
    /// - It proves nothing about the state of the book between the brackets
    ///   (see [`AuditPart`]).
    /// - A part whose window Education would not honour is refused as
    ///   `EducationBoundary`. An admitted part reports the stricter
    ///   `boundary_profile` its brackets saw; a caller reading a window in parts
    ///   must carry Education forward once seen and plan every later part on
    ///   days Education honours.
    /// - It checks the export status and the company the request names, not
    ///   the rows; admitting the rows is the caller's. The company binding
    ///   covers the `SVCURRENTCOMPANY` static variable only: TDL embedded in a
    ///   request is not proven unable to change which company Tally reads, so
    ///   requests must come from Bridge's pinned profiles.
    pub(crate) async fn fetch_audit_part(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        request: super::agent_read_request::AgentReadRequest,
        shape: AuditPartShape,
    ) -> Result<AuditPart, AuditPartFailure> {
        let endpoint = EndpointKey::from_config(&config)
            .map_err(|_| AuditPartFailure::new(AuditPartFailureKind::Other("endpoint_invalid")))?;
        if !request_scopes_company(&request.clone().into_xml(), identity.display_name()) {
            return Err(AuditPartFailure::new(
                AuditPartFailureKind::RequestNotCompanyScoped,
            ));
        }
        if self.audit_drain_owed(&endpoint) {
            return Err(AuditPartFailure::new(AuditPartFailureKind::DrainRequired));
        }
        let _lease = self
            .begin_ordinary_read(&config)
            .map_err(|error| classify_audit_part_failure(&error))?;
        let ticket = AUDIT_DRAIN_TICKET.fetch_add(1, Ordering::Relaxed);
        let registry = Arc::clone(&self.audit_drain);
        let identity = identity.clone();
        let armed_endpoint = endpoint;
        let result = self
            .execute(
                config,
                ReadOperation::OtherRead,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                move |client| {
                    let identity = identity.clone();
                    let request = request.clone();
                    let registry = Arc::clone(&registry);
                    let endpoint = armed_endpoint.clone();
                    async move {
                        arm_audit_drain(&registry, &endpoint, ticket)?;
                        let armed = ArmedAuditDrain {
                            registry,
                            endpoint,
                            ticket,
                            settled: false,
                        };
                        match fetch_admitted_audit_part(&client, &identity, request, shape).await {
                            Ok(part) => {
                                armed.settle(false);
                                Ok(part)
                            }
                            Err(error) => {
                                armed.settle(classify_audit_part_failure(&error).kind.owes_drain());
                                Err(error)
                            }
                        }
                    }
                },
            )
            .await;
        // An armed part has settled its own debt inside the gate, or left it
        // owed by being dropped; a failure before arming sent nothing.
        result.map_err(|error| classify_audit_part_failure(&error))
    }

    /// At most one status probe, with its own short deadline, towards clearing
    /// an endpoint's drain debt.
    ///
    /// - The debt clears only after `AUDIT_DRAIN_QUICK_PROBES` consecutive
    ///   probes each answered within `AUDIT_DRAIN_PROBE_SLOW`. A slow, failed
    ///   or unanswered probe starts the count again.
    /// - Probes are spaced at least `AUDIT_DRAIN_PROBE_INTERVAL` apart; a call
    ///   sooner sends nothing and returns [`AuditDrainStatus::Wait`].
    /// - After `AUDIT_DRAIN_ABANDONED_PROBES` unanswered probes, no more are
    ///   sent until the operator has looked at Tally
    ///   ([`AuditDrainStatus::OperatorRequired`]).
    /// - Sends nothing when nothing is owed.
    ///
    /// Unmeasured premise, for live qualification: that Tally answers
    /// `/status` quickly only once it has finished building an abandoned
    /// response. The brain notes record `/status` both dead during a modal
    /// hang and healthy just before one.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the audit_read orchestrator, plan step 7")
    )]
    pub(crate) async fn drain_probe(&self, config: TallyConfig) -> AuditDrainStatus {
        let Ok(endpoint) = EndpointKey::from_config(&config) else {
            return AuditDrainStatus::OperatorRequired;
        };
        let captured = {
            let mut owed = audit_drain_lock(&self.audit_drain);
            let Some(debt) = owed.get_mut(&endpoint) else {
                return AuditDrainStatus::Clear;
            };
            if debt.abandoned_probes >= AUDIT_DRAIN_ABANDONED_PROBES {
                return AuditDrainStatus::OperatorRequired;
            }
            // A part is still running and has not settled: there is nothing to
            // drain yet, and a probe now would only queue behind it.
            if debt.in_flight {
                return AuditDrainStatus::Wait {
                    retry_after: self.audit_drain_probe_interval,
                };
            }
            if let Some(started) = debt.probe_started {
                let running = started.elapsed();
                if running < self.audit_drain_probe_stale {
                    return AuditDrainStatus::Wait {
                        retry_after: self.audit_drain_probe_stale - running,
                    };
                }
                // Its caller stopped waiting; the request may still be queued
                // behind a busy responder. Count it, then carry on.
                debt.probe_started = None;
                debt.quick_probes = 0;
                debt.abandoned_probes = debt.abandoned_probes.saturating_add(1);
                if debt.abandoned_probes >= AUDIT_DRAIN_ABANDONED_PROBES {
                    return AuditDrainStatus::OperatorRequired;
                }
            }
            if let Some(last) = debt.last_probe {
                let since = last.elapsed();
                if since < self.audit_drain_probe_interval {
                    return AuditDrainStatus::Wait {
                        retry_after: self.audit_drain_probe_interval - since,
                    };
                }
            }
            debt.last_probe = Some(Instant::now());
            debt.probe_started = Some(Instant::now());
            (debt.ticket, debt.in_flight)
        };
        let outcome = match self.begin_ordinary_read(&config) {
            Ok(_lease) => self
                .execute(
                    config,
                    ReadOperation::Status,
                    ReadRetryPolicy::SINGLE_ATTEMPT,
                    |client| async move {
                        let started = Instant::now();
                        tokio::time::timeout(AUDIT_DRAIN_PROBE_DEADLINE, client.status_probe())
                            .await
                            .map_err(|_| TallyTransportError::RequestTimedOut)??;
                        Ok(started.elapsed())
                    },
                )
                .await
                .map_err(|error| classify_audit_part_failure(&error).kind),
            Err(_) => Err(AuditPartFailureKind::NotSent("endpoint_read_reserved")),
        };
        let mut owed = audit_drain_lock(&self.audit_drain);
        let Some(debt) = owed.get_mut(&endpoint) else {
            return AuditDrainStatus::Clear;
        };
        debt.probe_started = None;
        // A probe counts only towards the debt it was sent for.
        if (debt.ticket, debt.in_flight) != captured {
            return AuditDrainStatus::Owed {
                quick_probes: debt.quick_probes,
                abandoned_probes: debt.abandoned_probes,
            };
        }
        match outcome {
            Ok(elapsed) if elapsed <= self.audit_drain_probe_slow => {
                debt.quick_probes = debt.quick_probes.saturating_add(1);
            }
            Ok(_) => debt.quick_probes = 0,
            Err(kind) => {
                debt.quick_probes = 0;
                if kind.owes_drain() {
                    debt.abandoned_probes = debt.abandoned_probes.saturating_add(1);
                }
            }
        }
        if debt.quick_probes >= AUDIT_DRAIN_QUICK_PROBES {
            owed.remove(&endpoint);
            return AuditDrainStatus::Clear;
        }
        if debt.abandoned_probes >= AUDIT_DRAIN_ABANDONED_PROBES {
            return AuditDrainStatus::OperatorRequired;
        }
        AuditDrainStatus::Owed {
            quick_probes: debt.quick_probes,
            abandoned_probes: debt.abandoned_probes,
        }
    }

    /// Clear an endpoint's drain debt after the operator has looked at the
    /// Tally screen and, if it was stuck, restarted Tally. Only for a debt
    /// that reached [`AuditDrainStatus::OperatorRequired`].
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "used by the audit_read orchestrator, plan step 7")
    )]
    pub(crate) fn clear_audit_drain_after_operator_check(&self, config: &TallyConfig) -> bool {
        let Ok(endpoint) = EndpointKey::from_config(config) else {
            return false;
        };
        let mut owed = audit_drain_lock(&self.audit_drain);
        match owed.get(&endpoint) {
            Some(debt) if debt.abandoned_probes >= AUDIT_DRAIN_ABANDONED_PROBES => {
                owed.remove(&endpoint);
                true
            }
            _ => false,
        }
    }

    /// Whether a settled debt is owed. A part still in flight is not a debt
    /// yet: a later part waits behind it in the endpoint queue and is checked
    /// again at dispatch ([`arm_audit_drain`]), where an in-flight entry left
    /// by a dropped part also refuses.
    fn audit_drain_owed(&self, endpoint: &EndpointKey) -> bool {
        audit_drain_lock(&self.audit_drain)
            .get(endpoint)
            .is_some_and(|debt| !debt.in_flight)
    }

    #[cfg(test)]
    pub(crate) fn with_audit_drain_probe_slow(mut self, slow: std::time::Duration) -> Self {
        self.audit_drain_probe_slow = slow;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_audit_drain_probe_stale(mut self, stale: std::time::Duration) -> Self {
        self.audit_drain_probe_stale = stale;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_audit_drain_probe_interval(mut self, interval: std::time::Duration) -> Self {
        self.audit_drain_probe_interval = interval;
        self
    }

    /// One approved mutation through the shared endpoint queue. The durable
    /// intent is committed after identity admission and before any import bytes.
    /// Unlike paired reads, an import must never be repeated automatically.
    pub(crate) async fn post_approved_import<A, F>(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        request: super::approved_import::ApprovedImport,
        recheck_admission: A,
        before_dispatch: F,
    ) -> anyhow::Result<ApprovedImportDispatch>
    where
        A: Fn(
            &str,
            &str,
            &str,
            &bridge_tally_protocol::StandardLedgerCatalogBinding,
        ) -> anyhow::Result<()>,
        F: Fn() -> Result<(), String>,
    {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::Import,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            |client| {
                let identity = identity.clone();
                let request = request.clone();
                let xml = request.xml().to_string();
                let recheck_admission = &recheck_admission;
                let before_dispatch = &before_dispatch;
                async move {
                    // Admit the initial observed product/mode and company scope before
                    // any queued monetary source read. Those observations can become
                    // stale during queued source reads, so the same admission is
                    // repeated after the catalogue, before the final absence reads.
                    let (opening_profile, opening_mode_evidence) =
                        observe_read_boundary(&client).await?;
                    let (opening_companies, opening_company_evidence) = client
                        .fetch_companies_with_wire_evidence()
                        .await
                        .map_err(|error| {
                            with_read_evidence(error, opening_mode_evidence.clone())
                        })?;
                    let admission_evidence =
                        opening_mode_evidence.combine(opening_company_evidence);
                    admit_company_identity(&opening_companies, &identity)
                        .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    super::approved_import::require_unique_company_scope(
                        &opening_companies,
                        &identity,
                    )
                    .map_err(|error| {
                        with_read_evidence(error.into(), admission_evidence.clone())
                    })?;
                    request
                        .require_boundary_profile(opening_profile)
                        .map_err(|error| {
                            with_read_evidence(error.into(), admission_evidence.clone())
                        })?;
                    let (catalogue, catalogue_evidence) = fetch_admitted_agent_read(
                        &client,
                        &identity,
                        request.ledger_catalogue_request(),
                    )
                    .await
                    .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    let admission_evidence = admission_evidence.combine(catalogue_evidence);
                    let (profile, mode_evidence) = observe_read_boundary(&client)
                        .await
                        .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    let admission_evidence = admission_evidence.combine(mode_evidence);
                    let (companies, company_evidence) = client
                        .fetch_companies_with_wire_evidence()
                        .await
                        .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    let admission_evidence = admission_evidence.combine(company_evidence);
                    admit_company_identity(&companies, &identity)
                        .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    super::approved_import::require_unique_company_scope(&companies, &identity)
                        .map_err(|error| {
                            with_read_evidence(error.into(), admission_evidence.clone())
                        })?;
                    request.require_boundary_profile(profile).map_err(|error| {
                        with_read_evidence(error.into(), admission_evidence.clone())
                    })?;
                    // Keep duplicate absence as the final source admission. The
                    // helper retains its required identity/health brackets; no
                    // unrelated profile or catalogue read follows this verdict.
                    let (first_read, first_evidence) = fetch_admitted_agent_read(
                        &client,
                        &identity,
                        request.verification_request(),
                    )
                    .await
                    .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    let admission_evidence = admission_evidence.combine(first_evidence);
                    let (second_read, second_evidence) = fetch_admitted_agent_read(
                        &client,
                        &identity,
                        request.verification_request(),
                    )
                    .await
                    .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    let admission_evidence = admission_evidence.combine(second_evidence);
                    recheck_admission(
                        &first_read.body,
                        &second_read.body,
                        &catalogue.body,
                        request.ledger_binding(),
                    )
                    .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    before_dispatch().map_err(|error| {
                        with_read_evidence(anyhow::Error::msg(error), admission_evidence.clone())
                    })?;
                    let mut response_evidence = RuntimeReadEvidence::empty();
                    let body = client
                        .post_probe_xml(xml, &mut response_evidence)
                        .await
                        .map_err(|error| with_read_evidence(error, admission_evidence.clone()))?;
                    Ok(ApprovedImportDispatch {
                        body,
                        response_evidence,
                        admission_evidence,
                    })
                }
            },
        )
        .await
    }

    /// LAB-ONLY (audit-sprint 2026-09-14, Phase 3.4/3.5). Posts already-built
    /// import XML directly to the Tally XML gateway through the same
    /// serialized session queue as every other operation, but without the
    /// native-approval / durable-dispatch-ledger machinery
    /// `post_approved_import` layers on top for the production Journal path.
    /// That machinery *is* the production path's safety (one human-approved
    /// Journal at a time); the lab writer's safety is its caller's
    /// `admit_lab_target` re-check before every batch, not this method.
    /// Compiled only behind `lab-writes`; never called from, and never
    /// changes, `agent_import_post.rs` or `approved_import.rs`.
    #[cfg(feature = "lab-writes")]
    pub(crate) async fn post_lab_import(
        &self,
        config: TallyConfig,
        xml: String,
    ) -> anyhow::Result<(String, RuntimeReadEvidence)> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::Import,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let xml = xml.clone();
                async move {
                    let mut evidence = RuntimeReadEvidence::empty();
                    let body = client.post_probe_xml(xml, &mut evidence).await?;
                    Ok((body, evidence))
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
    ) -> anyhow::Result<(OutstandingsLoadResult, RuntimeReadEvidence)> {
        self.fetch_outstandings_native_with_currency(
            config,
            identity,
            as_of,
            NativeOutstandingsCurrency::Operator(currency_assertion),
            ageing_anchor,
        )
        .await
    }

    /// MCP monetary reads require the observed currency's company extent;
    /// the desktop operator assertion cannot be supplied through this entry point.
    pub(crate) async fn fetch_agent_outstandings_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: PartyLedgerMasterCurrencyAssertion,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<(OutstandingsLoadResult, RuntimeReadEvidence)> {
        #[cfg(feature = "voucher-scan")]
        if self.outstandings_segment_policy.is_some() {
            anyhow::bail!("outstandings_read_evidence_unavailable");
        }
        self.fetch_outstandings_native_with_currency(
            config,
            identity,
            as_of,
            NativeOutstandingsCurrency::Observed(currency_assertion),
            ageing_anchor,
        )
        .await
    }

    async fn fetch_outstandings_native_with_currency(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: NativeOutstandingsCurrency,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<(OutstandingsLoadResult, RuntimeReadEvidence)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                let as_of = as_of.clone();
                let currency_assertion = currency_assertion.clone();
                async move {
                    let mut read_evidence = RuntimeReadEvidence::empty();
                    let outcome = async {
                        let (boundary_profile, opening_evidence) =
                            observe_read_boundary(&client).await?;
                        read_evidence = opening_evidence;
                        bracket_verified_company_identity(&client, &identity).await?;
                        let company = identity.display_name();
                        let expected_company_guid = identity.company_guid();
                        let extent = client.fetch_company_book_extent(&identity).await?;
                        let currency_assertion = match &currency_assertion {
                            NativeOutstandingsCurrency::Operator(assertion) => *assertion,
                            NativeOutstandingsCurrency::Observed(witness) => {
                                witness.require_opening_extent(&extent)?.assertion
                            }
                        };
                        if &as_of < extent.books_from() {
                            return Ok((
                                partial_result("as_of_precedes_books_from"),
                                read_evidence.clone(),
                            ));
                        }
                        let books_from = extent.books_from().clone();
                        let snapshot_period = match admit_native_ledger_snapshot_period(
                            boundary_profile,
                            books_from.clone(),
                            as_of.clone(),
                        ) {
                            NativeLedgerSnapshotPeriodAdmission::Period(period) => period,
                            NativeLedgerSnapshotPeriodAdmission::Partial(partial) => {
                                return Ok((partial, read_evidence.clone()));
                            }
                        };
                        let mut total_bytes = 0usize;
                        let read =
                            |kind| render_native_bills_request(kind, company, &books_from, &as_of);
                        let receivable_request = read(NativeBillsReportKind::Receivable);
                        let receivable = client
                            .fetch_native_report_paired(receivable_request.clone())
                            .await?;
                        let (receivable_body, encoded_bytes, encoded_sha256) = match receivable {
                            NativePairedRead::Stable {
                                body,
                                encoded_bytes,
                                encoded_sha256,
                            } => (body, encoded_bytes, encoded_sha256),
                            NativePairedRead::Drifted(evidence) => {
                                read_evidence = read_evidence.clone().combine(evidence);
                                return Ok((
                                    partial_result("native_bills_report_drifted"),
                                    read_evidence.clone(),
                                ));
                            }
                        };
                        total_bytes += encoded_bytes;
                        read_evidence = read_evidence.clone().combine(RuntimeReadEvidence::paired(
                            &receivable_request,
                            encoded_sha256,
                            encoded_bytes,
                        ));

                        // A ledger's immediate parent can be an arbitrary custom
                        // subgroup. The native group snapshot resolves it all the
                        // way to Sundry Debtors/Creditors without importing the
                        // legacy custom-TDL profile.
                        let group_request = render_native_group_snapshot_request(company);
                        let groups = client
                            .fetch_native_report_paired(group_request.clone())
                            .await?;
                        let (group_body, encoded_bytes, encoded_sha256) = match groups {
                            NativePairedRead::Stable {
                                body,
                                encoded_bytes,
                                encoded_sha256,
                            } => (body, encoded_bytes, encoded_sha256),
                            NativePairedRead::Drifted(evidence) => {
                                read_evidence = read_evidence.clone().combine(evidence);
                                return Ok((
                                    partial_result("native_group_snapshot_drifted"),
                                    read_evidence.clone(),
                                ));
                            }
                        };
                        total_bytes += encoded_bytes;
                        read_evidence = read_evidence.clone().combine(RuntimeReadEvidence::paired(
                            &group_request,
                            encoded_sha256,
                            encoded_bytes,
                        ));

                        let payable_request = read(NativeBillsReportKind::Payable);
                        let payable = client
                            .fetch_native_report_paired(payable_request.clone())
                            .await?;
                        let (payable_body, encoded_bytes, encoded_sha256) = match payable {
                            NativePairedRead::Stable {
                                body,
                                encoded_bytes,
                                encoded_sha256,
                            } => (body, encoded_bytes, encoded_sha256),
                            NativePairedRead::Drifted(evidence) => {
                                read_evidence = read_evidence.clone().combine(evidence);
                                return Ok((
                                    partial_result("native_bills_report_drifted"),
                                    read_evidence.clone(),
                                ));
                            }
                        };
                        total_bytes += encoded_bytes;
                        read_evidence = read_evidence.clone().combine(RuntimeReadEvidence::paired(
                            &payable_request,
                            encoded_sha256,
                            encoded_bytes,
                        ));

                        let ledger_request =
                            render_native_ledger_snapshot_request(company, &snapshot_period);
                        let ledgers = client
                            .fetch_native_report_paired(ledger_request.clone())
                            .await?;
                        let (ledger_body, encoded_bytes, encoded_sha256) = match ledgers {
                            NativePairedRead::Stable {
                                body,
                                encoded_bytes,
                                encoded_sha256,
                            } => (body, encoded_bytes, encoded_sha256),
                            NativePairedRead::Drifted(evidence) => {
                                read_evidence = read_evidence.clone().combine(evidence);
                                return Ok((
                                    partial_result("native_ledger_snapshot_drifted"),
                                    read_evidence.clone(),
                                ));
                            }
                        };
                        total_bytes += encoded_bytes;
                        read_evidence = read_evidence.clone().combine(RuntimeReadEvidence::paired(
                            &ledger_request,
                            encoded_sha256,
                            encoded_bytes,
                        ));

                        // Re-pin after every read. The native rows cannot be
                        // GUID-checked individually, so an unchanged extent across
                        // the whole sequence is the only identity evidence
                        // available.
                        let closing_extent = client.fetch_company_book_extent(&identity).await?;
                        if closing_extent != extent {
                            return Ok((
                                partial_result("book_changed_during_read"),
                                read_evidence.clone(),
                            ));
                        }
                        bracket_verified_company_identity(&client, &identity).await?;

                        let closing_evidence =
                            confirm_read_boundary(&client, boundary_profile).await?;
                        read_evidence = read_evidence.clone().combine(closing_evidence);
                        let receivable_rows =
                            parse_native_bill_rows(&receivable_body, &books_from, &as_of)?;
                        let payable_rows =
                            parse_native_bill_rows(&payable_body, &books_from, &as_of)?;
                        let ledger_rows = match admit_native_ledger_snapshot(
                            parse_native_ledger_snapshot(&ledger_body).map_err(anyhow::Error::from),
                        )? {
                            NativeLedgerSnapshotAdmission::Snapshot(snapshot) => snapshot,
                            NativeLedgerSnapshotAdmission::Partial(partial) => {
                                return Ok((partial, read_evidence.clone()));
                            }
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
                            return Ok((partial_result(reason), read_evidence.clone()));
                        }

                        let statement_open_bills = all_open_bill_rows(
                            &receivable_rows,
                            &payable_rows,
                            ageing_anchor,
                            &as_of,
                        );
                        let statement_unallocated_by_party =
                            all_unallocated_parties(&result.residuals);
                        Ok((
                            OutstandingsLoadResult::Complete {
                                report: Box::new(result.report),
                                read_strategy: OutstandingsReadStrategy::NativeBills,
                                currency_assertion,
                                ageing_anchor,
                                synced_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                                unallocated_total: Some(result.residual_total),
                                statement_unallocated_by_party,
                                statement_open_bills,
                            },
                            read_evidence.clone(),
                        ))
                    }
                    .await;
                    outcome.map_err(|error| with_read_evidence(error, read_evidence))
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
        Ok(self
            .detect_base_currency_with_extent(config, identity)
            .await?
            .currency)
    }

    /// Reads the base-currency assertion together with the exact paired wire
    /// evidence that established it for an agent-visible monetary result.
    pub async fn detect_base_currency_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<(CompanyCurrency, RuntimeReadEvidence)> {
        let read = self
            .detect_base_currency_with_extent(config, identity)
            .await?;
        Ok((read.currency, read.evidence))
    }

    /// Runs the existing currency probe while retaining its stable company
    /// extent for the party/ledger master document boundary.
    pub(crate) async fn detect_party_ledger_master_currency(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<CompanyCurrencyRead> {
        self.detect_base_currency_with_extent(config, identity)
            .await
    }

    pub(crate) async fn detect_base_currency_with_extent(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<CompanyCurrencyRead> {
        let _lease = self.begin_ordinary_read(&config)?;
        let identity = identity.clone();
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let identity = identity.clone();
                async move {
                    let mut evidence = RuntimeReadEvidence::empty();
                    let result = async {
                        bracket_verified_company_identity(&client, &identity).await?;
                        // Pin identity first: a currency read against the wrong
                        // company is worse than none.
                        let extent = client.fetch_company_book_extent(&identity).await?;
                        let request = render_company_currency_request(identity.display_name());
                        let body = client.fetch_native_report_paired(request.clone()).await?;
                        let (body, encoded_bytes, encoded_sha256) =
                            body.require_stable(PairedReadValidationError::CurrencyMaster)?;
                        evidence =
                            RuntimeReadEvidence::paired(&request, encoded_sha256, encoded_bytes);
                        let closing_extent = client.fetch_company_book_extent(&identity).await?;
                        if closing_extent != extent {
                            return Err(anyhow::Error::new(
                                PairedReadValidationError::CurrencyExtent,
                            ));
                        }
                        bracket_verified_company_identity(&client, &identity).await?;
                        Ok(CompanyCurrencyRead {
                            currency: parse_company_currency(&body)?,
                            extent,
                            evidence: evidence.clone(),
                        })
                    }
                    .await;
                    result.map_err(|error| with_read_evidence(error, evidence))
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
            .map(|(result, _)| result)
    }

    #[cfg(not(feature = "voucher-scan"))]
    pub async fn fetch_outstandings_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: OutstandingsCurrencyAssertion,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<(OutstandingsLoadResult, RuntimeReadEvidence)> {
        self.fetch_outstandings_native(config, identity, as_of, currency_assertion, ageing_anchor)
            .await
    }

    #[cfg(feature = "voucher-scan")]
    pub async fn fetch_outstandings_with_evidence(
        &self,
        config: TallyConfig,
        identity: &VerifiedCompanyIdentity,
        as_of: TallyDate,
        currency_assertion: OutstandingsCurrencyAssertion,
        ageing_anchor: OutstandingsAgeingAnchor,
    ) -> anyhow::Result<(OutstandingsLoadResult, RuntimeReadEvidence)> {
        if self.outstandings_segment_policy.is_none() {
            return self
                .fetch_outstandings_native(
                    config,
                    identity,
                    as_of,
                    currency_assertion,
                    ageing_anchor,
                )
                .await;
        }

        anyhow::bail!("outstandings_read_evidence_unavailable")
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
                .await
                .map(|(result, _)| result);
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
                        let extent = client
                            .fetch_company_book_extent(&identity)
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
                            .fetch_company_book_extent(&identity)
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
                                read_strategy: OutstandingsReadStrategy::VoucherScan,
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
                    refuse_report_formula_in_education(
                        bracket_verified_company_identity_observing_mode(&client, &identity)
                            .await?,
                    )?;
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
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<TallyRuntimeReadError>()),
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
#[path = "runtime_tests.rs"]
mod tests;
