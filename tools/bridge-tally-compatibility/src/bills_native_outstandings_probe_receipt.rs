//! Qualification-only receipt for the dormant native Ledger Outstandings probe.
//!
//! This artifact records bounded observation facts. It is intentionally not a
//! [`crate::LiveCompatibilityReceipt`], cannot satisfy the support gate, and
//! carries no parser, accounting, runtime, mirror, or support authority.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    checksum, invalid, validate_commit, validate_exact_release, validate_label, validate_safe_code,
    validate_sha256, validate_slug, ApplicationStatus, Architecture, CompatibilityError,
    LocaleProfile, LoopbackFamily, Platform, ProductFamily, TallyMode, TextEncoding,
};

pub const BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_SCHEMA_VERSION: u16 = 0;
pub const BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES: usize = 256 * 1024;
pub const BILLS_NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES: u64 = 1024 * 1024;
pub const BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID: &str = "native_ledger_outstandings_candidate_v0";
pub const BILLS_NATIVE_OUTSTANDINGS_REPORT_ID: &str = "ledger_outstandings";

const RECEIPT_CHECKSUM_DOMAIN: &[u8] = b"bridge.tally.native-ledger-outstandings.probe-receipt/0\0";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeEndpointV0 {
    pub family: LoopbackFamily,
    pub port: u16,
    pub canonical_origin: String,
}

impl fmt::Debug for ProbeEndpointV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeEndpointV0")
            .field("family", &self.family)
            .field("port", &self.port)
            .field("canonical_origin", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeAttestationAuthority {
    User,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeAttestedEnvironmentV0 {
    pub authority: ProbeAttestationAuthority,
    pub product: ProductFamily,
    pub release: String,
    pub mode: TallyMode,
    pub locale: LocaleProfile,
    pub configured_tdl_count: u32,
    pub no_customer_data: bool,
}

impl fmt::Debug for ProbeAttestedEnvironmentV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeAttestedEnvironmentV0")
            .field("authority", &self.authority)
            .field("product", &self.product)
            .field("release", &"[redacted]")
            .field("mode", &self.mode)
            .field("locale", &self.locale)
            .field("configured_tdl_count", &self.configured_tdl_count)
            .field("no_customer_data", &self.no_customer_data)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeCommitmentsV0 {
    pub fixture_id: String,
    pub fixture_manifest_sha256: String,
    pub profile_id: String,
    pub template_sha256: String,
    pub request_sha256: String,
    pub scope_sha256: String,
}

impl fmt::Debug for ProbeCommitmentsV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeCommitmentsV0")
            .field("fixture_id", &self.fixture_id)
            .field("profile_id", &self.profile_id)
            .field("hashes", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeInitialPreflightV0 {
    pub observed_at_unix_ms: i64,
    pub company_identity_commitment_sha256: String,
    pub party_identity_commitment_sha256: String,
    pub company_response_sha256: String,
    pub party_response_sha256: String,
}

impl fmt::Debug for ProbeInitialPreflightV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeInitialPreflightV0")
            .field("observed_at_unix_ms", &self.observed_at_unix_ms)
            .field("commitments", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySnapshotId {
    B0,
    B1,
    B2,
    B3,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeIdentitySnapshotV0 {
    pub snapshot_id: IdentitySnapshotId,
    pub observed_at_unix_ms: i64,
    pub company_identity_commitment_sha256: String,
    pub party_identity_commitment_sha256: String,
    pub company_response_sha256: String,
    pub party_response_sha256: String,
}

impl fmt::Debug for ProbeIdentitySnapshotV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeIdentitySnapshotV0")
            .field("snapshot_id", &self.snapshot_id)
            .field("observed_at_unix_ms", &self.observed_at_unix_ms)
            .field("commitments", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateAttemptId {
    A1,
    A2,
    A3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateAttemptOutcome {
    ResponseObserved,
    HttpRejected,
    TransportFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityBracketState {
    Unchanged,
    Changed,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeCandidateAttemptV0 {
    pub attempt_id: CandidateAttemptId,
    pub observed_at_unix_ms: i64,
    pub outcome: CandidateAttemptOutcome,
    pub http_status: Option<u16>,
    pub application_status: ApplicationStatus,
    pub encoding: TextEncoding,
    pub exact_encoded_bytes: u64,
    pub encoded_body_sha256: Option<String>,
    pub decoded_text_sha256: Option<String>,
    pub safe_reason_code: Option<String>,
    pub bracket_state: IdentityBracketState,
}

impl fmt::Debug for ProbeCandidateAttemptV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeCandidateAttemptV0")
            .field("attempt_id", &self.attempt_id)
            .field("observed_at_unix_ms", &self.observed_at_unix_ms)
            .field("outcome", &self.outcome)
            .field("http_status", &self.http_status)
            .field("application_status", &self.application_status)
            .field("encoding", &self.encoding)
            .field("exact_encoded_bytes", &self.exact_encoded_bytes)
            .field("response_hashes", &"[redacted]")
            .field("safe_reason_code", &self.safe_reason_code)
            .field("bracket_state", &self.bracket_state)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByteRepeatability {
    Identical,
    Different,
    NotEstablished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiObservationPosition {
    Before,
    After,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeUiObservationV0 {
    pub position: UiObservationPosition,
    pub observed_at_unix_ms: i64,
    pub product: ProductFamily,
    pub release: String,
    pub mode: TallyMode,
    pub report_id: String,
    pub opening_column_visible: bool,
    pub pending_column_visible: bool,
    pub due_column_visible: bool,
    pub overdue_column_visible: bool,
    pub company_identity_commitment_sha256: String,
    pub party_identity_commitment_sha256: String,
    pub structured_observation_sha256: String,
    pub screenshot_sha256: String,
}

impl fmt::Debug for ProbeUiObservationV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeUiObservationV0")
            .field("position", &self.position)
            .field("observed_at_unix_ms", &self.observed_at_unix_ms)
            .field("product", &self.product)
            .field("release", &"[redacted]")
            .field("mode", &self.mode)
            .field("report_id", &self.report_id)
            .field("commitments", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserAttestedUiChange {
    Unchanged,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeReceiptAuthorityV0 {
    pub read_only: bool,
    pub writes_attempted: bool,
    pub raw_response_retained: bool,
    pub consent_replay_allowed: bool,
    pub responder_authenticity_established: bool,
    pub response_scope_bound: bool,
    pub response_grammar_established: bool,
    pub accounting_semantics_established: bool,
    pub source_completeness_established: bool,
    pub source_atomicity_established: bool,
    pub performance_budget_established: bool,
    pub native_runtime_observed: bool,
    pub mirror_written: bool,
    pub support_claim_eligible: bool,
}

impl ProbeReceiptAuthorityV0 {
    pub fn observation_only() -> Self {
        Self {
            read_only: true,
            writes_attempted: false,
            raw_response_retained: false,
            consent_replay_allowed: false,
            responder_authenticity_established: false,
            response_scope_bound: false,
            response_grammar_established: false,
            accounting_semantics_established: false,
            source_completeness_established: false,
            source_atomicity_established: false,
            performance_budget_established: false,
            native_runtime_observed: false,
            mirror_written: false,
            support_claim_eligible: false,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillsNativeOutstandingsProbeReceiptV0 {
    pub schema_version: u16,
    pub observed_at_unix_ms: i64,
    pub bridge_commit_sha: String,
    pub working_tree_dirty: bool,
    pub compatibility_surface_sha256: String,
    pub executable_sha256: String,
    pub cargo_lock_sha256: String,
    pub platform: Platform,
    pub architecture: Architecture,
    pub endpoint: ProbeEndpointV0,
    pub attested_environment: ProbeAttestedEnvironmentV0,
    pub commitments: ProbeCommitmentsV0,
    pub initial_preflight: ProbeInitialPreflightV0,
    pub identity_snapshots: [ProbeIdentitySnapshotV0; 4],
    pub attempts: [ProbeCandidateAttemptV0; 3],
    pub byte_repeatability: ByteRepeatability,
    pub ui_before: ProbeUiObservationV0,
    pub ui_after: ProbeUiObservationV0,
    pub user_attested_ui_change: UserAttestedUiChange,
    pub authority: ProbeReceiptAuthorityV0,
    pub receipt_sha256: String,
}

impl fmt::Debug for BillsNativeOutstandingsProbeReceiptV0 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BillsNativeOutstandingsProbeReceiptV0")
            .field("schema_version", &self.schema_version)
            .field("observed_at_unix_ms", &self.observed_at_unix_ms)
            .field("working_tree_dirty", &self.working_tree_dirty)
            .field("platform", &self.platform)
            .field("architecture", &self.architecture)
            .field("endpoint", &self.endpoint)
            .field("attested_environment", &self.attested_environment)
            .field("commitments", &self.commitments)
            .field("initial_preflight", &self.initial_preflight)
            .field("identity_snapshot_count", &self.identity_snapshots.len())
            .field("attempt_count", &self.attempts.len())
            .field("byte_repeatability", &self.byte_repeatability)
            .field("ui_before", &self.ui_before)
            .field("ui_after", &self.ui_after)
            .field("user_attested_ui_change", &self.user_attested_ui_change)
            .field("authority", &self.authority)
            .field("environment_and_receipt_hashes", &"[redacted]")
            .finish()
    }
}

impl BillsNativeOutstandingsProbeReceiptV0 {
    pub fn seal(mut self) -> Result<Self, CompatibilityError> {
        self.receipt_sha256.clear();
        self.validate_shape(false)?;
        self.receipt_sha256 = checksum(RECEIPT_CHECKSUM_DOMAIN, &self)?;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), CompatibilityError> {
        self.validate_shape(true)?;
        let mut unsigned = self.clone();
        let supplied = std::mem::take(&mut unsigned.receipt_sha256);
        let expected = checksum(RECEIPT_CHECKSUM_DOMAIN, &unsigned)?;
        if supplied != expected {
            return Err(invalid("native_probe_receipt_checksum_mismatch"));
        }
        Ok(())
    }

    pub fn to_pretty_json(&self) -> Result<Vec<u8>, CompatibilityError> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| invalid("serialization_failed"))?;
        if bytes.len() > BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES {
            return Err(invalid("native_probe_receipt_too_large"));
        }
        Ok(bytes)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, CompatibilityError> {
        if bytes.is_empty() || bytes.len() > BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES {
            return Err(invalid("native_probe_receipt_size_invalid"));
        }
        let receipt: Self = serde_json::from_slice(bytes)
            .map_err(|_| invalid("native_probe_receipt_json_invalid"))?;
        receipt.validate()?;
        Ok(receipt)
    }

    fn validate_shape(&self, require_checksum: bool) -> Result<(), CompatibilityError> {
        if self.schema_version != BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_SCHEMA_VERSION {
            return Err(invalid("native_probe_receipt_schema_unsupported"));
        }
        if self.observed_at_unix_ms <= 0 {
            return Err(invalid("native_probe_receipt_time_invalid"));
        }
        validate_commit(&self.bridge_commit_sha)?;
        for digest in [
            &self.compatibility_surface_sha256,
            &self.executable_sha256,
            &self.cargo_lock_sha256,
        ] {
            validate_sha256(digest)?;
        }
        if require_checksum {
            validate_sha256(&self.receipt_sha256)?;
        } else if !self.receipt_sha256.is_empty() {
            return Err(invalid("native_probe_receipt_checksum_must_start_empty"));
        }

        validate_endpoint(&self.endpoint)?;
        validate_attested_environment(&self.attested_environment)?;
        validate_commitments(&self.commitments)?;
        validate_initial_preflight(&self.initial_preflight)?;
        validate_identity_snapshots(&self.identity_snapshots)?;
        validate_attempts(&self.attempts, &self.identity_snapshots)?;
        validate_ui_observations(
            &self.attested_environment,
            &self.initial_preflight,
            &self.identity_snapshots,
            &self.ui_before,
            &self.ui_after,
            self.user_attested_ui_change,
        )?;

        let timeline = [
            self.ui_before.observed_at_unix_ms,
            self.initial_preflight.observed_at_unix_ms,
            self.identity_snapshots[0].observed_at_unix_ms,
            self.attempts[0].observed_at_unix_ms,
            self.identity_snapshots[1].observed_at_unix_ms,
            self.attempts[1].observed_at_unix_ms,
            self.identity_snapshots[2].observed_at_unix_ms,
            self.attempts[2].observed_at_unix_ms,
            self.identity_snapshots[3].observed_at_unix_ms,
            self.ui_after.observed_at_unix_ms,
            self.observed_at_unix_ms,
        ];
        if timeline.iter().any(|value| *value <= 0)
            || timeline.windows(2).any(|window| window[0] > window[1])
        {
            return Err(invalid("native_probe_receipt_timeline_invalid"));
        }

        if self.byte_repeatability != expected_repeatability(&self.attempts) {
            return Err(invalid("native_probe_byte_repeatability_inconsistent"));
        }
        if self.authority != ProbeReceiptAuthorityV0::observation_only() {
            return Err(invalid("native_probe_receipt_authority_invalid"));
        }
        Ok(())
    }
}

fn validate_endpoint(endpoint: &ProbeEndpointV0) -> Result<(), CompatibilityError> {
    if endpoint.port == 0 {
        return Err(invalid("native_probe_endpoint_invalid"));
    }
    let expected = match endpoint.family {
        LoopbackFamily::LocalhostAlias | LoopbackFamily::Ipv4 => {
            format!("http://127.0.0.1:{}", endpoint.port)
        }
        LoopbackFamily::Ipv6 => format!("http://[::1]:{}", endpoint.port),
    };
    if endpoint.canonical_origin != expected {
        return Err(invalid("native_probe_endpoint_not_canonical"));
    }
    Ok(())
}

fn validate_attested_environment(
    environment: &ProbeAttestedEnvironmentV0,
) -> Result<(), CompatibilityError> {
    validate_label(&environment.release)?;
    validate_exact_release(&environment.release)?;
    if environment.authority != ProbeAttestationAuthority::User
        || environment.product == ProductFamily::Unknown
        || environment.mode != TallyMode::Education
        || environment.locale == LocaleProfile::Unknown
        || environment.configured_tdl_count != 0
        || !environment.no_customer_data
    {
        return Err(invalid("native_probe_environment_attestation_invalid"));
    }
    Ok(())
}

fn validate_commitments(commitments: &ProbeCommitmentsV0) -> Result<(), CompatibilityError> {
    validate_slug(&commitments.fixture_id)?;
    if commitments.profile_id != BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID {
        return Err(invalid("native_probe_profile_invalid"));
    }
    for digest in [
        &commitments.fixture_manifest_sha256,
        &commitments.template_sha256,
        &commitments.request_sha256,
        &commitments.scope_sha256,
    ] {
        validate_sha256(digest)?;
    }
    Ok(())
}

fn validate_initial_preflight(
    preflight: &ProbeInitialPreflightV0,
) -> Result<(), CompatibilityError> {
    if preflight.observed_at_unix_ms <= 0 {
        return Err(invalid("native_probe_preflight_time_invalid"));
    }
    for digest in [
        &preflight.company_identity_commitment_sha256,
        &preflight.party_identity_commitment_sha256,
        &preflight.company_response_sha256,
        &preflight.party_response_sha256,
    ] {
        validate_sha256(digest)?;
    }
    Ok(())
}

fn validate_identity_snapshots(
    snapshots: &[ProbeIdentitySnapshotV0; 4],
) -> Result<(), CompatibilityError> {
    let expected = [
        IdentitySnapshotId::B0,
        IdentitySnapshotId::B1,
        IdentitySnapshotId::B2,
        IdentitySnapshotId::B3,
    ];
    for (snapshot, expected_id) in snapshots.iter().zip(expected) {
        if snapshot.snapshot_id != expected_id || snapshot.observed_at_unix_ms <= 0 {
            return Err(invalid("native_probe_identity_snapshot_order_invalid"));
        }
        for digest in [
            &snapshot.company_identity_commitment_sha256,
            &snapshot.party_identity_commitment_sha256,
            &snapshot.company_response_sha256,
            &snapshot.party_response_sha256,
        ] {
            validate_sha256(digest)?;
        }
    }
    Ok(())
}

fn validate_attempts(
    attempts: &[ProbeCandidateAttemptV0; 3],
    snapshots: &[ProbeIdentitySnapshotV0; 4],
) -> Result<(), CompatibilityError> {
    let expected = [
        CandidateAttemptId::A1,
        CandidateAttemptId::A2,
        CandidateAttemptId::A3,
    ];
    for (index, (attempt, expected_id)) in attempts.iter().zip(expected).enumerate() {
        if attempt.attempt_id != expected_id || attempt.observed_at_unix_ms <= 0 {
            return Err(invalid("native_probe_attempt_order_invalid"));
        }
        if let Some(code) = &attempt.safe_reason_code {
            validate_safe_code(code)?;
        }
        let has_hashes =
            attempt.encoded_body_sha256.is_some() && attempt.decoded_text_sha256.is_some();
        match attempt.outcome {
            CandidateAttemptOutcome::ResponseObserved => {
                if !attempt
                    .http_status
                    .is_some_and(|status| (200..=299).contains(&status))
                    || attempt.application_status == ApplicationStatus::NotApplicable
                    || attempt.encoding == TextEncoding::Unknown
                    || attempt.exact_encoded_bytes > BILLS_NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES
                    || !has_hashes
                {
                    return Err(invalid("native_probe_observed_attempt_invalid"));
                }
                validate_sha256(
                    attempt
                        .encoded_body_sha256
                        .as_deref()
                        .ok_or_else(|| invalid("native_probe_body_hash_missing"))?,
                )?;
                validate_sha256(
                    attempt
                        .decoded_text_sha256
                        .as_deref()
                        .ok_or_else(|| invalid("native_probe_decoded_hash_missing"))?,
                )?;
                let needs_reason = attempt.application_status != ApplicationStatus::Success;
                if needs_reason != attempt.safe_reason_code.is_some() {
                    return Err(invalid("native_probe_application_reason_inconsistent"));
                }
            }
            CandidateAttemptOutcome::HttpRejected => {
                if !attempt.http_status.is_some_and(|status| {
                    (100..=599).contains(&status) && !(200..=299).contains(&status)
                }) || attempt.application_status != ApplicationStatus::NotApplicable
                    || attempt.encoding != TextEncoding::Unknown
                    || attempt.exact_encoded_bytes != 0
                    || attempt.encoded_body_sha256.is_some()
                    || attempt.decoded_text_sha256.is_some()
                    || attempt.safe_reason_code.is_none()
                {
                    return Err(invalid("native_probe_http_rejected_attempt_invalid"));
                }
            }
            CandidateAttemptOutcome::TransportFailed => {
                if attempt.http_status.is_some()
                    || attempt.application_status != ApplicationStatus::NotApplicable
                    || attempt.encoding != TextEncoding::Unknown
                    || attempt.exact_encoded_bytes != 0
                    || attempt.encoded_body_sha256.is_some()
                    || attempt.decoded_text_sha256.is_some()
                    || attempt.safe_reason_code.is_none()
                {
                    return Err(invalid("native_probe_transport_failed_attempt_invalid"));
                }
            }
        }

        let expected_bracket = if same_identity(&snapshots[index], &snapshots[index + 1]) {
            IdentityBracketState::Unchanged
        } else {
            IdentityBracketState::Changed
        };
        if attempt.bracket_state != expected_bracket {
            return Err(invalid("native_probe_attempt_bracket_inconsistent"));
        }
    }

    for left in attempts {
        if left.outcome != CandidateAttemptOutcome::ResponseObserved {
            continue;
        }
        for right in attempts {
            if right.outcome == CandidateAttemptOutcome::ResponseObserved
                && left.encoded_body_sha256 == right.encoded_body_sha256
                && (left.exact_encoded_bytes != right.exact_encoded_bytes
                    || left.encoding != right.encoding
                    || left.decoded_text_sha256 != right.decoded_text_sha256)
            {
                return Err(invalid("native_probe_identical_body_facts_inconsistent"));
            }
        }
    }
    Ok(())
}

fn validate_ui_observations(
    environment: &ProbeAttestedEnvironmentV0,
    preflight: &ProbeInitialPreflightV0,
    snapshots: &[ProbeIdentitySnapshotV0; 4],
    before: &ProbeUiObservationV0,
    after: &ProbeUiObservationV0,
    attested_change: UserAttestedUiChange,
) -> Result<(), CompatibilityError> {
    if before.position != UiObservationPosition::Before
        || after.position != UiObservationPosition::After
        || before.observed_at_unix_ms <= 0
        || after.observed_at_unix_ms <= 0
    {
        return Err(invalid("native_probe_ui_position_invalid"));
    }
    for observation in [before, after] {
        validate_label(&observation.release)?;
        validate_exact_release(&observation.release)?;
        if observation.product == ProductFamily::Unknown
            || observation.mode == TallyMode::Unknown
            || observation.report_id != BILLS_NATIVE_OUTSTANDINGS_REPORT_ID
            || !observation.opening_column_visible
            || !observation.pending_column_visible
            || !observation.due_column_visible
            || !observation.overdue_column_visible
        {
            return Err(invalid("native_probe_ui_observation_invalid"));
        }
        for digest in [
            &observation.company_identity_commitment_sha256,
            &observation.party_identity_commitment_sha256,
            &observation.structured_observation_sha256,
            &observation.screenshot_sha256,
        ] {
            validate_sha256(digest)?;
        }
    }
    if before.product != environment.product
        || before.release != environment.release
        || before.mode != environment.mode
        || before.company_identity_commitment_sha256 != preflight.company_identity_commitment_sha256
        || before.party_identity_commitment_sha256 != preflight.party_identity_commitment_sha256
        || before.company_identity_commitment_sha256
            != snapshots[0].company_identity_commitment_sha256
        || before.party_identity_commitment_sha256 != snapshots[0].party_identity_commitment_sha256
        || after.company_identity_commitment_sha256
            != snapshots[3].company_identity_commitment_sha256
        || after.party_identity_commitment_sha256 != snapshots[3].party_identity_commitment_sha256
    {
        return Err(invalid("native_probe_ui_identity_or_environment_mismatch"));
    }
    if before.screenshot_sha256 == after.screenshot_sha256 {
        return Err(invalid("native_probe_ui_screenshot_reused"));
    }

    let expected_change = if same_ui_observation(before, after) {
        UserAttestedUiChange::Unchanged
    } else {
        UserAttestedUiChange::Changed
    };
    if attested_change != expected_change {
        return Err(invalid("native_probe_ui_change_attestation_inconsistent"));
    }
    Ok(())
}

fn same_identity(left: &ProbeIdentitySnapshotV0, right: &ProbeIdentitySnapshotV0) -> bool {
    left.company_identity_commitment_sha256 == right.company_identity_commitment_sha256
        && left.party_identity_commitment_sha256 == right.party_identity_commitment_sha256
}

fn same_ui_observation(left: &ProbeUiObservationV0, right: &ProbeUiObservationV0) -> bool {
    left.product == right.product
        && left.release == right.release
        && left.mode == right.mode
        && left.report_id == right.report_id
        && left.opening_column_visible == right.opening_column_visible
        && left.pending_column_visible == right.pending_column_visible
        && left.due_column_visible == right.due_column_visible
        && left.overdue_column_visible == right.overdue_column_visible
        && left.company_identity_commitment_sha256 == right.company_identity_commitment_sha256
        && left.party_identity_commitment_sha256 == right.party_identity_commitment_sha256
        && left.structured_observation_sha256 == right.structured_observation_sha256
}

fn expected_repeatability(attempts: &[ProbeCandidateAttemptV0; 3]) -> ByteRepeatability {
    if attempts
        .iter()
        .any(|attempt| attempt.outcome != CandidateAttemptOutcome::ResponseObserved)
    {
        return ByteRepeatability::NotEstablished;
    }
    let first = &attempts[0];
    if attempts[1..].iter().all(|attempt| {
        attempt.exact_encoded_bytes == first.exact_encoded_bytes
            && attempt.encoding == first.encoding
            && attempt.encoded_body_sha256 == first.encoded_body_sha256
            && attempt.decoded_text_sha256 == first.decoded_text_sha256
    }) {
        ByteRepeatability::Identical
    } else {
        ByteRepeatability::Different
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LiveCompatibilityReceipt;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const COMMIT: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const T0: i64 = 1_800_000_000_000;

    fn snapshot(id: IdentitySnapshotId, time: i64) -> ProbeIdentitySnapshotV0 {
        ProbeIdentitySnapshotV0 {
            snapshot_id: id,
            observed_at_unix_ms: time,
            company_identity_commitment_sha256: SHA_A.to_string(),
            party_identity_commitment_sha256: SHA_A.to_string(),
            company_response_sha256: SHA_A.to_string(),
            party_response_sha256: SHA_A.to_string(),
        }
    }

    fn attempt(id: CandidateAttemptId, time: i64) -> ProbeCandidateAttemptV0 {
        ProbeCandidateAttemptV0 {
            attempt_id: id,
            observed_at_unix_ms: time,
            outcome: CandidateAttemptOutcome::ResponseObserved,
            http_status: Some(200),
            application_status: ApplicationStatus::Success,
            encoding: TextEncoding::Utf8,
            exact_encoded_bytes: 1024,
            encoded_body_sha256: Some(SHA_A.to_string()),
            decoded_text_sha256: Some(SHA_A.to_string()),
            safe_reason_code: None,
            bracket_state: IdentityBracketState::Unchanged,
        }
    }

    fn ui(position: UiObservationPosition, time: i64, screenshot: &str) -> ProbeUiObservationV0 {
        ProbeUiObservationV0 {
            position,
            observed_at_unix_ms: time,
            product: ProductFamily::TallyPrime,
            release: "7.1".to_string(),
            mode: TallyMode::Education,
            report_id: BILLS_NATIVE_OUTSTANDINGS_REPORT_ID.to_string(),
            opening_column_visible: true,
            pending_column_visible: true,
            due_column_visible: true,
            overdue_column_visible: true,
            company_identity_commitment_sha256: SHA_A.to_string(),
            party_identity_commitment_sha256: SHA_A.to_string(),
            structured_observation_sha256: SHA_A.to_string(),
            screenshot_sha256: screenshot.to_string(),
        }
    }

    fn receipt() -> BillsNativeOutstandingsProbeReceiptV0 {
        BillsNativeOutstandingsProbeReceiptV0 {
            schema_version: BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_SCHEMA_VERSION,
            observed_at_unix_ms: T0 + 10,
            bridge_commit_sha: COMMIT.to_string(),
            working_tree_dirty: true,
            compatibility_surface_sha256: SHA_A.to_string(),
            executable_sha256: SHA_A.to_string(),
            cargo_lock_sha256: SHA_A.to_string(),
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            endpoint: ProbeEndpointV0 {
                family: LoopbackFamily::Ipv4,
                port: 9000,
                canonical_origin: "http://127.0.0.1:9000".to_string(),
            },
            attested_environment: ProbeAttestedEnvironmentV0 {
                authority: ProbeAttestationAuthority::User,
                product: ProductFamily::TallyPrime,
                release: "7.1".to_string(),
                mode: TallyMode::Education,
                locale: LocaleProfile::EnglishIndia,
                configured_tdl_count: 0,
                no_customer_data: true,
            },
            commitments: ProbeCommitmentsV0 {
                fixture_id: "education-small-v1".to_string(),
                fixture_manifest_sha256: SHA_A.to_string(),
                profile_id: BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID.to_string(),
                template_sha256: SHA_A.to_string(),
                request_sha256: SHA_A.to_string(),
                scope_sha256: SHA_A.to_string(),
            },
            initial_preflight: ProbeInitialPreflightV0 {
                observed_at_unix_ms: T0 + 1,
                company_identity_commitment_sha256: SHA_A.to_string(),
                party_identity_commitment_sha256: SHA_A.to_string(),
                company_response_sha256: SHA_A.to_string(),
                party_response_sha256: SHA_A.to_string(),
            },
            identity_snapshots: [
                snapshot(IdentitySnapshotId::B0, T0 + 2),
                snapshot(IdentitySnapshotId::B1, T0 + 4),
                snapshot(IdentitySnapshotId::B2, T0 + 6),
                snapshot(IdentitySnapshotId::B3, T0 + 8),
            ],
            attempts: [
                attempt(CandidateAttemptId::A1, T0 + 3),
                attempt(CandidateAttemptId::A2, T0 + 5),
                attempt(CandidateAttemptId::A3, T0 + 7),
            ],
            byte_repeatability: ByteRepeatability::Identical,
            ui_before: ui(UiObservationPosition::Before, T0, SHA_A),
            ui_after: ui(UiObservationPosition::After, T0 + 9, SHA_B),
            user_attested_ui_change: UserAttestedUiChange::Unchanged,
            authority: ProbeReceiptAuthorityV0::observation_only(),
            receipt_sha256: String::new(),
        }
    }

    fn reseal(
        mut value: BillsNativeOutstandingsProbeReceiptV0,
    ) -> Result<BillsNativeOutstandingsProbeReceiptV0, CompatibilityError> {
        value.receipt_sha256.clear();
        value.seal()
    }

    #[test]
    fn round_trip_is_bounded_redacted_and_structurally_separate() {
        let receipt = receipt().seal().unwrap();
        let bytes = receipt.to_pretty_json().unwrap();
        assert!(bytes.len() < BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES);
        assert_eq!(
            BillsNativeOutstandingsProbeReceiptV0::from_json(&bytes).unwrap(),
            receipt
        );
        assert!(LiveCompatibilityReceipt::from_json(&bytes).is_err());
        let debug = format!("{receipt:?}");
        for forbidden in [SHA_A, SHA_B, COMMIT, "http://127.0.0.1:9000", "7.1"] {
            assert!(!debug.contains(forbidden));
        }
    }

    #[test]
    fn checksum_tampering_and_unknown_json_fields_fail_closed() {
        let receipt = receipt().seal().unwrap();
        let mut tampered = receipt.clone();
        tampered.working_tree_dirty = !tampered.working_tree_dirty;
        assert_eq!(
            tampered.validate(),
            Err(invalid("native_probe_receipt_checksum_mismatch"))
        );

        let mut value = serde_json::to_value(receipt).unwrap();
        value.as_object_mut().unwrap().insert(
            "support_claim_eligible".to_string(),
            serde_json::Value::Bool(true),
        );
        assert!(BillsNativeOutstandingsProbeReceiptV0::from_json(
            &serde_json::to_vec(&value).unwrap()
        )
        .is_err());
    }

    #[test]
    fn ordering_timeline_and_bracket_consistency_are_enforced() {
        let mut value = receipt();
        value.identity_snapshots.swap(0, 1);
        assert!(reseal(value).is_err());

        let mut value = receipt();
        value.attempts.swap(0, 1);
        assert!(reseal(value).is_err());

        let mut value = receipt();
        value.identity_snapshots[1].party_identity_commitment_sha256 = SHA_B.to_string();
        assert!(reseal(value.clone()).is_err());
        value.attempts[0].bracket_state = IdentityBracketState::Changed;
        value.attempts[1].bracket_state = IdentityBracketState::Changed;
        assert!(reseal(value).is_ok());

        let mut value = receipt();
        value.attempts[1].observed_at_unix_ms = T0;
        assert!(reseal(value).is_err());
    }

    #[test]
    fn attempt_fact_shapes_and_repeatability_cannot_be_laundered() {
        let mut value = receipt();
        value.attempts[1].outcome = CandidateAttemptOutcome::TransportFailed;
        value.attempts[1].http_status = None;
        value.attempts[1].application_status = ApplicationStatus::NotApplicable;
        value.attempts[1].encoding = TextEncoding::Unknown;
        value.attempts[1].exact_encoded_bytes = 0;
        value.attempts[1].encoded_body_sha256 = None;
        value.attempts[1].decoded_text_sha256 = None;
        value.attempts[1].safe_reason_code = Some("request_failed".to_string());
        assert!(reseal(value.clone()).is_err());
        value.byte_repeatability = ByteRepeatability::NotEstablished;
        assert!(reseal(value).is_ok());

        let mut value = receipt();
        value.attempts[2].encoded_body_sha256 = Some(SHA_B.to_string());
        value.attempts[2].decoded_text_sha256 = Some(SHA_B.to_string());
        assert!(reseal(value.clone()).is_err());
        value.byte_repeatability = ByteRepeatability::Different;
        assert!(reseal(value).is_ok());

        let mut value = receipt();
        value.attempts[0].exact_encoded_bytes = BILLS_NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES + 1;
        assert!(reseal(value).is_err());
    }

    #[test]
    fn environment_ui_and_authority_cannot_promote_the_observation() {
        let mut value = receipt();
        value.attested_environment.no_customer_data = false;
        assert!(reseal(value).is_err());

        let mut value = receipt();
        value.attested_environment.configured_tdl_count = 1;
        assert!(reseal(value).is_err());

        let mut value = receipt();
        value.ui_after.screenshot_sha256 = value.ui_before.screenshot_sha256.clone();
        assert!(reseal(value).is_err());

        let mut value = receipt();
        value.ui_after.structured_observation_sha256 = SHA_B.to_string();
        assert!(reseal(value.clone()).is_err());
        value.user_attested_ui_change = UserAttestedUiChange::Changed;
        assert!(reseal(value).is_ok());

        for mutate in [
            |authority: &mut ProbeReceiptAuthorityV0| authority.support_claim_eligible = true,
            |authority: &mut ProbeReceiptAuthorityV0| authority.response_scope_bound = true,
            |authority: &mut ProbeReceiptAuthorityV0| {
                authority.accounting_semantics_established = true
            },
            |authority: &mut ProbeReceiptAuthorityV0| authority.raw_response_retained = true,
        ] {
            let mut value = receipt();
            mutate(&mut value.authority);
            assert!(reseal(value).is_err());
        }
    }

    #[test]
    fn endpoint_profile_and_artifact_size_are_strict() {
        let mut value = receipt();
        value.endpoint.canonical_origin = "http://localhost:9000".to_string();
        assert!(reseal(value).is_err());

        let mut value = receipt();
        value.commitments.profile_id = "native_ledger_outstandings_candidate_v1".to_string();
        assert!(reseal(value).is_err());

        assert_eq!(
            BillsNativeOutstandingsProbeReceiptV0::from_json(&vec![
                b' ';
                BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES
                    + 1
            ]),
            Err(invalid("native_probe_receipt_size_invalid"))
        );
    }
}
