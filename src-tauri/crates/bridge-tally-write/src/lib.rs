//! Portable, network-free qualification for controlled Tally ledger writes.
//!
//! This crate cannot dispatch HTTP. It binds a deterministic import preview to
//! typed intent, strict preflight state, parser-derived import evidence, and a
//! company-bound readback before it can return an exact verdict.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::{
    parse_import_evidence, parse_ledger_write_readback_with_evidence, ParsedImportEvidence,
    TallyImportApplicationStatus, TallyImportResult, BRIDGE_LEDGER_WRITE_READBACK_SCHEMA,
};
#[cfg(feature = "fixture-canary-runtime-dispatch")]
use bridge_tally_transport::{TallyHttpTransport, TallyTransportError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_LEDGER_WRITE_BATCH: usize = 10;
pub const LEDGER_WRITE_PROJECTION: &str = "bridge.tally.ledger-write-state/1";
pub const LEDGER_READBACK_PROFILE: &str = BRIDGE_LEDGER_WRITE_READBACK_SCHEMA;
pub const FIXTURE_CANARY_MAPPING_VERSION: &str = "bridge.fixture-canary/v1";
pub const FIXTURE_CANARY_LEDGER_NAME: &str = "BRIDGE-CANARY-LEDGER-V1";

const FIXTURE_CANARY_REMOTE_ID: &str = "bridge-fixture-canary-ledger-v1";
const FIXTURE_CANARY_PARENT: &str = "Indirect Expenses";
const FIXTURE_CANARY_OPENING_BALANCE: &str = "0";

macro_rules! digest_type {
    ($name:ident) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn as_hex(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }
    };
}

digest_type!(WirePayloadDigest);
digest_type!(IntendedStateDigest);
digest_type!(ImportResponseDigest);
digest_type!(ReadbackStateDigest);
digest_type!(IdentityCoverageDigest);
digest_type!(LineErrorDigest);
digest_type!(ApprovalEvidenceDigest);
digest_type!(IdentityQueryDigest);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerOperation {
    Create,
    Alter,
}

impl LedgerOperation {
    fn tally_action(self) -> &'static str {
        match self {
            Self::Create => "Create",
            Self::Alter => "Alter",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerState {
    name: String,
    parent: Option<String>,
    party_gstin: Option<String>,
    opening_balance: Option<String>,
}

impl LedgerState {
    pub fn new(
        name: impl Into<String>,
        parent: Option<String>,
        party_gstin: Option<String>,
        opening_balance: Option<String>,
    ) -> Result<Self, QualificationError> {
        let state = Self {
            name: name.into(),
            parent,
            party_gstin,
            opening_balance,
        };
        validate_value(&state.name, "ledger_name")?;
        for (value, field) in [
            (state.parent.as_deref(), "parent"),
            (state.party_gstin.as_deref(), "party_gstin"),
            (state.opening_balance.as_deref(), "opening_balance"),
        ] {
            if let Some(value) = value {
                validate_value(value, field)?;
            }
        }
        if let Some(value) = state.party_gstin.as_deref() {
            validate_gstin(value)?;
        }
        if let Some(value) = state.opening_balance.as_deref() {
            ExactDecimal::parse(value.to_owned())
                .map_err(|_| QualificationError::InvalidField("opening_balance"))?;
        }
        Ok(state)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLineage {
    system: String,
    record_id: String,
    version: String,
}

impl SourceLineage {
    pub fn new(
        system: impl Into<String>,
        record_id: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, QualificationError> {
        let lineage = Self {
            system: system.into(),
            record_id: record_id.into(),
            version: version.into(),
        };
        validate_value(&lineage.system, "source_system")?;
        validate_value(&lineage.record_id, "source_record_id")?;
        validate_value(&lineage.version, "source_version")?;
        Ok(lineage)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerMutation {
    operation: LedgerOperation,
    remote_id: String,
    before: Option<LedgerState>,
    after: LedgerState,
    source_lineage: SourceLineage,
}

impl LedgerMutation {
    pub fn create(
        remote_id: impl Into<String>,
        after: LedgerState,
        source_lineage: SourceLineage,
    ) -> Result<Self, QualificationError> {
        if after.parent.is_none() {
            return Err(QualificationError::CreateParentRequired);
        }
        Self::new(
            LedgerOperation::Create,
            remote_id.into(),
            None,
            after,
            source_lineage,
        )
    }

    pub fn alter(
        remote_id: impl Into<String>,
        before: LedgerState,
        after: LedgerState,
        source_lineage: SourceLineage,
    ) -> Result<Self, QualificationError> {
        if before == after {
            return Err(QualificationError::NoOpMutation);
        }
        for (before, after, field) in [
            (before.parent.as_ref(), after.parent.as_ref(), "parent"),
            (
                before.party_gstin.as_ref(),
                after.party_gstin.as_ref(),
                "party_gstin",
            ),
            (
                before.opening_balance.as_ref(),
                after.opening_balance.as_ref(),
                "opening_balance",
            ),
        ] {
            if before.is_some() && after.is_none() {
                return Err(QualificationError::UnsupportedFieldClear(field));
            }
        }
        Self::new(
            LedgerOperation::Alter,
            remote_id.into(),
            Some(before),
            after,
            source_lineage,
        )
    }

    fn new(
        operation: LedgerOperation,
        remote_id: String,
        before: Option<LedgerState>,
        after: LedgerState,
        source_lineage: SourceLineage,
    ) -> Result<Self, QualificationError> {
        validate_value(&remote_id, "remote_id")?;
        Ok(Self {
            operation,
            remote_id,
            before,
            after,
            source_lineage,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticCompany {
    name: String,
    guid: String,
}

impl SyntheticCompany {
    pub fn new(
        name: impl Into<String>,
        guid: impl Into<String>,
    ) -> Result<Self, QualificationError> {
        let company = Self {
            name: name.into(),
            guid: guid.into(),
        };
        validate_value(&company.name, "company_name")?;
        validate_value(&company.guid, "company_guid")?;
        Ok(company)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteCapability {
    Observed,
    Documented,
    Unknown,
    Unsupported,
}

#[derive(Clone)]
pub struct WriteAuthorizationRequest {
    pub explicit_opt_in: bool,
    pub synthetic_company_confirmed: bool,
    pub company_guid: String,
    pub capability: WriteCapability,
    pub backup_guidance_acknowledged: bool,
    pub approval_evidence_sha256: String,
    pub approved_wire_sha256: String,
    pub approved_intended_state_sha256: String,
    pub approved_identity_query_sha256: String,
    pub idempotency_key: String,
    pub outbox_id: String,
    pub mapping_version: String,
}

impl std::fmt::Debug for WriteAuthorizationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteAuthorizationRequest")
            .field("explicit_opt_in", &self.explicit_opt_in)
            .field(
                "synthetic_company_confirmed",
                &self.synthetic_company_confirmed,
            )
            .field("company_guid", &"<redacted>")
            .field("capability", &self.capability)
            .field(
                "backup_guidance_acknowledged",
                &self.backup_guidance_acknowledged,
            )
            .field("approval_evidence_sha256", &"<redacted>")
            .field("approved_wire_sha256", &"<redacted>")
            .field("approved_intended_state_sha256", &"<redacted>")
            .field("approved_identity_query_sha256", &"<redacted>")
            .field("idempotency_key", &"<redacted>")
            .field("outbox_id", &"<redacted>")
            .field("mapping_version", &self.mapping_version)
            .finish()
    }
}

#[derive(Clone)]
pub struct WriteAuthorization {
    company_guid: String,
    approval_evidence_sha256: String,
    idempotency_key: String,
    outbox_id: String,
    mapping_version: String,
    approved_wire_sha256: String,
    approved_intended_state_sha256: String,
    approved_identity_query_sha256: String,
}

/// The only authority that can prepare the initial fixture canary.
///
/// Unlike `WriteAuthorizationRequest`, it deliberately has no capability
/// field: the canary establishes the first observed write capability. It is
/// still unusable without an external durable reservation and exact preview
/// commitments, both of which are checked by the application coordinator
/// before any future transport is introduced.
#[derive(Clone)]
pub struct FixtureCanaryAuthorizationRequest {
    pub explicit_opt_in: bool,
    pub synthetic_company_confirmed: bool,
    pub company_guid: String,
    pub backup_guidance_acknowledged: bool,
    pub review_commitment_sha256: String,
    pub reservation_id: String,
    pub reservation_payload_sha256: String,
    pub approved_wire_sha256: String,
    pub approved_intended_state_sha256: String,
    pub approved_identity_query_sha256: String,
    pub idempotency_key: String,
}

impl std::fmt::Debug for FixtureCanaryAuthorizationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FixtureCanaryAuthorizationRequest")
            .field("explicit_opt_in", &self.explicit_opt_in)
            .field(
                "synthetic_company_confirmed",
                &self.synthetic_company_confirmed,
            )
            .field("company_guid", &"<redacted>")
            .field(
                "backup_guidance_acknowledged",
                &self.backup_guidance_acknowledged,
            )
            .field("review_commitment_sha256", &"<redacted>")
            .field("reservation_id", &"<redacted>")
            .field("reservation_payload_sha256", &"<redacted>")
            .field("approved_wire_sha256", &"<redacted>")
            .field("approved_intended_state_sha256", &"<redacted>")
            .field("approved_identity_query_sha256", &"<redacted>")
            .field("idempotency_key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone)]
pub struct FixtureCanaryAuthorization {
    authorization: WriteAuthorization,
    reservation_id: String,
    reservation_payload_sha256: String,
    review_commitment_sha256: String,
}

impl std::fmt::Debug for FixtureCanaryAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FixtureCanaryAuthorization")
            .field("reservation_id", &"<redacted>")
            .field("reservation_payload_sha256", &"<redacted>")
            .field("review_commitment_sha256", &"<redacted>")
            .finish()
    }
}

impl FixtureCanaryAuthorization {
    pub fn reservation_id(&self) -> &str {
        &self.reservation_id
    }

    pub fn reservation_payload_sha256(&self) -> &str {
        &self.reservation_payload_sha256
    }

    pub fn review_commitment_sha256(&self) -> &str {
        &self.review_commitment_sha256
    }
}

impl std::fmt::Debug for WriteAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteAuthorization")
            .field("company_guid", &"<redacted>")
            .field("approval_evidence_sha256", &"<redacted>")
            .field("idempotency_key", &"<redacted>")
            .field("outbox_id", &"<redacted>")
            .field("mapping_version", &self.mapping_version)
            .finish()
    }
}

pub fn authorize_synthetic_write(
    request: WriteAuthorizationRequest,
) -> Result<WriteAuthorization, QualificationError> {
    if !request.explicit_opt_in {
        return Err(QualificationError::ExplicitOptInRequired);
    }
    if !request.synthetic_company_confirmed {
        return Err(QualificationError::SyntheticCompanyRequired);
    }
    if request.capability != WriteCapability::Observed {
        return Err(QualificationError::ObservedCapabilityRequired);
    }
    if !request.backup_guidance_acknowledged {
        return Err(QualificationError::BackupAcknowledgementRequired);
    }
    validate_value(&request.company_guid, "company_guid")?;
    validate_sha256(
        &request.approval_evidence_sha256,
        "approval_evidence_sha256",
    )?;
    validate_sha256(&request.approved_wire_sha256, "approved_wire_sha256")?;
    validate_sha256(
        &request.approved_intended_state_sha256,
        "approved_intended_state_sha256",
    )?;
    validate_sha256(
        &request.approved_identity_query_sha256,
        "approved_identity_query_sha256",
    )?;
    validate_value(&request.idempotency_key, "idempotency_key")?;
    validate_value(&request.outbox_id, "outbox_id")?;
    validate_value(&request.mapping_version, "mapping_version")?;
    Ok(WriteAuthorization {
        company_guid: request.company_guid,
        approval_evidence_sha256: request.approval_evidence_sha256,
        idempotency_key: request.idempotency_key,
        outbox_id: request.outbox_id,
        mapping_version: request.mapping_version,
        approved_wire_sha256: request.approved_wire_sha256,
        approved_intended_state_sha256: request.approved_intended_state_sha256,
        approved_identity_query_sha256: request.approved_identity_query_sha256,
    })
}

/// Builds the distinct authority for one fixture-defined canary. No caller can
/// supply a ledger mutation, mapping version, or observed write capability.
pub fn authorize_fixture_canary(
    request: FixtureCanaryAuthorizationRequest,
) -> Result<FixtureCanaryAuthorization, QualificationError> {
    if !request.explicit_opt_in {
        return Err(QualificationError::ExplicitOptInRequired);
    }
    if !request.synthetic_company_confirmed {
        return Err(QualificationError::SyntheticCompanyRequired);
    }
    if !request.backup_guidance_acknowledged {
        return Err(QualificationError::BackupAcknowledgementRequired);
    }
    validate_value(&request.company_guid, "company_guid")?;
    validate_sha256(
        &request.review_commitment_sha256,
        "review_commitment_sha256",
    )?;
    validate_value(&request.reservation_id, "reservation_id")?;
    validate_sha256(
        &request.reservation_payload_sha256,
        "reservation_payload_sha256",
    )?;
    validate_sha256(&request.approved_wire_sha256, "approved_wire_sha256")?;
    validate_sha256(
        &request.approved_intended_state_sha256,
        "approved_intended_state_sha256",
    )?;
    validate_sha256(
        &request.approved_identity_query_sha256,
        "approved_identity_query_sha256",
    )?;
    validate_value(&request.idempotency_key, "idempotency_key")?;
    Ok(FixtureCanaryAuthorization {
        authorization: WriteAuthorization {
            company_guid: request.company_guid,
            approval_evidence_sha256: request.review_commitment_sha256.clone(),
            idempotency_key: request.idempotency_key,
            outbox_id: format!("fixture-canary:{}", request.reservation_id),
            mapping_version: FIXTURE_CANARY_MAPPING_VERSION.to_owned(),
            approved_wire_sha256: request.approved_wire_sha256,
            approved_intended_state_sha256: request.approved_intended_state_sha256,
            approved_identity_query_sha256: request.approved_identity_query_sha256,
        },
        reservation_id: request.reservation_id,
        reservation_payload_sha256: request.reservation_payload_sha256,
        review_commitment_sha256: request.review_commitment_sha256,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReservationState {
    Reserved,
    Sent,
    AwaitingReadback,
    OutcomeUnknown,
    Terminal,
}

#[derive(Debug, Clone)]
struct Reservation {
    wire_digest: WirePayloadDigest,
    outbox_id: String,
    state: ReservationState,
}

#[derive(Debug, Default)]
pub struct IdempotencyRegistry {
    reservations: HashMap<String, Reservation>,
}

impl IdempotencyRegistry {
    fn reserve(
        &mut self,
        authorization: &WriteAuthorization,
        wire_digest: &WirePayloadDigest,
    ) -> Result<(), QualificationError> {
        if let Some(existing) = self.reservations.get(&authorization.idempotency_key) {
            if existing.wire_digest != *wire_digest || existing.outbox_id != authorization.outbox_id
            {
                return Err(QualificationError::IdempotencyConflict);
            }
            return Err(QualificationError::DuplicateSubmission);
        }
        self.reservations.insert(
            authorization.idempotency_key.clone(),
            Reservation {
                wire_digest: wire_digest.clone(),
                outbox_id: authorization.outbox_id.clone(),
                state: ReservationState::Reserved,
            },
        );
        Ok(())
    }

    fn transition(
        &mut self,
        key: &str,
        expected: ReservationState,
        next: ReservationState,
    ) -> Result<(), QualificationError> {
        let reservation = self
            .reservations
            .get_mut(key)
            .ok_or(QualificationError::MissingIdempotencyReservation)?;
        if reservation.state != expected {
            return Err(QualificationError::DuplicateSubmission);
        }
        reservation.state = next;
        Ok(())
    }
}

#[derive(Clone)]
pub struct PreparedLedgerImport {
    company: SyntheticCompany,
    mutations: Vec<LedgerMutation>,
    wire_digest: WirePayloadDigest,
    intended_state_digest: IntendedStateDigest,
    expected_before: BTreeMap<String, Projection>,
    expected_after: BTreeMap<String, Projection>,
    authorization: WriteAuthorization,
    approval_evidence_digest: ApprovalEvidenceDigest,
    identity_query_digest: IdentityQueryDigest,
}

/// A fixed fixture canary preview that deliberately has no progression into
/// the generic import lifecycle. A separately reviewed coordinator may derive
/// sealed, non-dispatchable preflight evidence from an exact readback, but it
/// can never obtain a transport, receipt, or generic qualified-import state.
pub struct PreparedFixtureCanary {
    prepared: PreparedLedgerImport,
}

/// An opaque, non-dispatchable capability capsule for the fixed fixture-canary
/// payload commitment.
///
/// This is deliberately feature-gated and unavailable to Bridge's normal
/// build. It carries no transport, endpoint, retry policy, or persistence
/// hook, and it does not expose the XML. A separately reviewed runtime
/// coordinator must bind this capsule to a durable dispatch claim before it
/// introduces the one-send operation.
#[cfg(feature = "fixture-canary-dispatch-seam")]
pub struct SealedFixtureCanaryDispatch {
    prepared: PreparedLedgerImport,
    wire_xml: String,
    wire_digest: WirePayloadDigest,
}

/// A one-send canary receipt that remains sealed until it is correlated with
/// the exact closed readback profile. It never exposes the Tally response.
#[cfg(feature = "fixture-canary-runtime-dispatch")]
pub struct SealedFixtureCanaryReceipt {
    prepared: PreparedLedgerImport,
    receipt_xml: String,
    wire_digest: WirePayloadDigest,
}

/// Transport failure for the closed synthetic-canary path. A failure consumes
/// the dispatch capsule; callers must treat the outcome as unknown and must
/// not retry the import.
#[cfg(feature = "fixture-canary-runtime-dispatch")]
#[derive(Debug, thiserror::Error)]
pub enum FixtureCanaryDispatchError {
    #[error("sealed fixture-canary transport failed")]
    Transport(#[source] TallyTransportError),
    #[error("sealed fixture-canary runtime is unavailable")]
    RuntimeUnavailable,
}

#[cfg(feature = "fixture-canary-dispatch-seam")]
impl std::fmt::Debug for SealedFixtureCanaryDispatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedFixtureCanaryDispatch")
            .field("wire_digest", &self.wire_digest)
            .field("payload", &"[redacted]")
            .field("transport", &"absent")
            .finish()
    }
}

#[cfg(feature = "fixture-canary-dispatch-seam")]
impl SealedFixtureCanaryDispatch {
    pub fn wire_digest(&self) -> &WirePayloadDigest {
        &self.wire_digest
    }

    /// Sends the fixed, exact canary once through Bridge's bounded loopback
    /// transport. Consuming `self` prevents retrying or reusing its payload;
    /// the raw response remains sealed in the returned receipt.
    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    pub async fn dispatch_once(
        self,
        transport: &TallyHttpTransport,
    ) -> Result<SealedFixtureCanaryReceipt, FixtureCanaryDispatchError> {
        let receipt_xml = transport
            .post_xml_decoded(self.wire_xml)
            .await
            .map_err(FixtureCanaryDispatchError::Transport)?
            .into_text();
        Ok(SealedFixtureCanaryReceipt {
            prepared: self.prepared,
            receipt_xml,
            wire_digest: self.wire_digest,
        })
    }
}

#[cfg(feature = "fixture-canary-runtime-dispatch")]
impl std::fmt::Debug for SealedFixtureCanaryReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SealedFixtureCanaryReceipt")
            .field("wire_digest", &self.wire_digest)
            .field("receipt", &"[redacted]")
            .finish()
    }
}

#[cfg(feature = "fixture-canary-runtime-dispatch")]
impl SealedFixtureCanaryReceipt {
    /// Rejects a malformed import response before any follow-up readback. The
    /// parsed counters remain sealed and are checked again with the readback
    /// when deriving the final digest-only observation.
    pub fn validate_receipt(&self) -> Result<(), QualificationError> {
        parse_import_receipt(&self.receipt_xml).map(|_| ())
    }

    /// Validates the sealed receipt and a caller-owned closed readback, then
    /// returns only digest evidence. Neither raw XML document can escape.
    pub fn observe_with_readback(
        self,
        readback_xml: &str,
    ) -> Result<FixtureCanaryPostDispatchObservation, QualificationError> {
        observe_fixture_canary_post_dispatch(
            &PreparedFixtureCanary {
                prepared: self.prepared,
            },
            &self.receipt_xml,
            readback_xml,
        )
    }
}

impl std::fmt::Debug for PreparedFixtureCanary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedFixtureCanary")
            .field("wire_digest", self.prepared.wire_digest())
            .field(
                "intended_state_digest",
                self.prepared.intended_state_digest(),
            )
            .field(
                "identity_query_digest",
                self.prepared.identity_query_digest(),
            )
            .field("dispatch_eligible", &false)
            .finish()
    }
}

impl PreparedFixtureCanary {
    pub fn wire_digest(&self) -> &WirePayloadDigest {
        self.prepared.wire_digest()
    }

    pub fn intended_state_digest(&self) -> &IntendedStateDigest {
        self.prepared.intended_state_digest()
    }

    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        self.prepared.identity_query_digest()
    }

    pub const fn dispatch_eligible(&self) -> bool {
        false
    }

    /// Seals the immutable fixture payload commitment only in the separately
    /// opted-in dispatch-seam build. The normal Bridge build cannot name this
    /// type or invoke this method.
    #[cfg(feature = "fixture-canary-dispatch-seam")]
    pub fn seal_for_dispatch(self) -> Result<SealedFixtureCanaryDispatch, QualificationError> {
        let prepared = self.prepared;
        let wire_xml = build_import_xml(&prepared.company, &prepared.mutations);
        let wire_digest = WirePayloadDigest(domain_digest(
            b"bridge.tally.ledger-import-wire/1\0",
            wire_xml.as_bytes(),
        ));
        if wire_digest != prepared.wire_digest {
            return Err(QualificationError::FixtureCanaryPayloadMismatch);
        }
        Ok(SealedFixtureCanaryDispatch {
            wire_digest: prepared.wire_digest.clone(),
            prepared,
            wire_xml,
        })
    }
}

/// Digest-only proof that the fixed fixture canary is absent before a possible
/// write. It is not a dispatch authorization and cannot enter the generic
/// import lifecycle.
#[derive(Clone)]
pub struct FixtureCanaryPreflightEvidence {
    readback_state_digest: ReadbackStateDigest,
    identity_coverage_digest: IdentityCoverageDigest,
}

impl std::fmt::Debug for FixtureCanaryPreflightEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FixtureCanaryPreflightEvidence")
            .field("readback_state_digest", &self.readback_state_digest)
            .field("identity_coverage_digest", &self.identity_coverage_digest)
            .field("dispatch_eligible", &false)
            .finish()
    }
}

impl FixtureCanaryPreflightEvidence {
    pub fn readback_state_digest(&self) -> &ReadbackStateDigest {
        &self.readback_state_digest
    }

    pub fn identity_coverage_digest(&self) -> &IdentityCoverageDigest {
        &self.identity_coverage_digest
    }

    pub const fn dispatch_eligible(&self) -> bool {
        false
    }
}

/// Digest-only semantic observation for the fixed fixture canary. Receipt and
/// readback alone cannot correlate an import to a durable dispatch claim, so
/// this is explicitly not capability evidence or an exact-applied proof.
#[derive(Clone)]
pub struct FixtureCanaryPostDispatchObservation {
    import_response_digest: ImportResponseDigest,
    readback_state_digest: ReadbackStateDigest,
    identity_coverage_digest: IdentityCoverageDigest,
}

impl FixtureCanaryPostDispatchObservation {
    pub fn import_response_digest(&self) -> &ImportResponseDigest {
        &self.import_response_digest
    }

    pub fn readback_state_digest(&self) -> &ReadbackStateDigest {
        &self.readback_state_digest
    }

    pub fn identity_coverage_digest(&self) -> &IdentityCoverageDigest {
        &self.identity_coverage_digest
    }

    pub const fn dispatch_eligible(&self) -> bool {
        false
    }

    pub const fn capability_observed(&self) -> bool {
        false
    }
}

/// Validates a sealed fixture-canary readback and derives only its digest
/// evidence. The caller must keep the XML sealed; this function neither
/// exposes it nor returns any write-capable object.
pub fn verify_fixture_canary_preflight(
    prepared: &PreparedFixtureCanary,
    readback_xml: &str,
) -> Result<FixtureCanaryPreflightEvidence, QualificationError> {
    let observed = parse_readback(&prepared.prepared, readback_xml)?;
    if observed.projections != prepared.prepared.expected_before {
        return Err(QualificationError::PreflightMismatch);
    }
    Ok(FixtureCanaryPreflightEvidence {
        readback_state_digest: observed.state_digest,
        identity_coverage_digest: observed.coverage_digest,
    })
}

/// Parses an exact-looking receipt/readback pair for later correlation. Both
/// XML inputs stay caller-owned; only digests are returned. A durable sealed
/// coordinator must bind this observation to one dispatch claim before it can
/// record a capability or final verdict.
pub fn observe_fixture_canary_post_dispatch(
    prepared: &PreparedFixtureCanary,
    receipt_xml: &str,
    readback_xml: &str,
) -> Result<FixtureCanaryPostDispatchObservation, QualificationError> {
    let receipt = parse_import_receipt(receipt_xml)?;
    let observed = parse_readback(&prepared.prepared, readback_xml)?;
    let exact_after = observed.projections == prepared.prepared.expected_after;
    let exact_counters = receipt.is_clean()
        && receipt.counters.created == 1
        && receipt.counters.altered == 0
        && receipt.counters.deleted == 0;
    if !exact_after || !exact_counters {
        return Err(QualificationError::PostDispatchMismatch);
    }
    Ok(FixtureCanaryPostDispatchObservation {
        import_response_digest: receipt.response_digest,
        readback_state_digest: observed.state_digest,
        identity_coverage_digest: observed.coverage_digest,
    })
}

impl std::fmt::Debug for PreparedLedgerImport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedLedgerImport")
            .field("mutation_count", &self.mutations.len())
            .field("wire_digest", &self.wire_digest)
            .field("intended_state_digest", &self.intended_state_digest)
            .field("approval_evidence_digest", &self.approval_evidence_digest)
            .field("identity_query_digest", &self.identity_query_digest)
            .field("dispatch_eligible", &false)
            .finish()
    }
}

impl PreparedLedgerImport {
    pub fn wire_digest(&self) -> &WirePayloadDigest {
        &self.wire_digest
    }

    pub fn intended_state_digest(&self) -> &IntendedStateDigest {
        &self.intended_state_digest
    }

    pub fn approval_evidence_digest(&self) -> &ApprovalEvidenceDigest {
        &self.approval_evidence_digest
    }

    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        &self.identity_query_digest
    }

    pub const fn dispatch_eligible(&self) -> bool {
        false
    }

    pub fn qualify_preflight(
        self,
        readback_xml: &str,
    ) -> Result<QualifiedLedgerImport, QualificationError> {
        let observed = parse_readback(&self, readback_xml)?;
        if observed.projections != self.expected_before {
            return Err(QualificationError::PreflightMismatch);
        }
        Ok(QualifiedLedgerImport {
            prepared: self,
            before_readback_digest: observed.state_digest,
            before_coverage_digest: observed.coverage_digest,
        })
    }
}

#[derive(Clone)]
pub struct QualifiedLedgerImport {
    prepared: PreparedLedgerImport,
    before_readback_digest: ReadbackStateDigest,
    before_coverage_digest: IdentityCoverageDigest,
}

impl std::fmt::Debug for QualifiedLedgerImport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QualifiedLedgerImport")
            .field("prepared", &self.prepared)
            .field("before_readback_digest", &self.before_readback_digest)
            .field("before_coverage_digest", &self.before_coverage_digest)
            .finish()
    }
}

impl QualifiedLedgerImport {
    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        &self.prepared.identity_query_digest
    }

    pub fn record_dispatch_attempt(
        self,
        request_id: impl Into<String>,
        registry: &mut IdempotencyRegistry,
    ) -> Result<SentLedgerImport, QualificationError> {
        let request_id = request_id.into();
        validate_value(&request_id, "request_id")?;
        registry.transition(
            &self.prepared.authorization.idempotency_key,
            ReservationState::Reserved,
            ReservationState::Sent,
        )?;
        Ok(SentLedgerImport {
            qualified: self,
            request_id,
        })
    }
}

#[derive(Clone)]
pub struct SentLedgerImport {
    qualified: QualifiedLedgerImport,
    request_id: String,
}

impl SentLedgerImport {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        self.qualified.identity_query_digest()
    }

    pub fn record_import_receipt(
        self,
        response_xml: &str,
        registry: &mut IdempotencyRegistry,
    ) -> Result<AwaitingLedgerReadback, QualificationError> {
        let receipt = parse_import_receipt(response_xml)?;
        registry.transition(
            &self.qualified.prepared.authorization.idempotency_key,
            ReservationState::Sent,
            ReservationState::AwaitingReadback,
        )?;
        Ok(AwaitingLedgerReadback {
            sent: self,
            receipt,
        })
    }

    pub fn record_outcome_unknown(
        self,
        registry: &mut IdempotencyRegistry,
    ) -> Result<OutcomeUnknownLedgerImport, QualificationError> {
        registry.transition(
            &self.qualified.prepared.authorization.idempotency_key,
            ReservationState::Sent,
            ReservationState::OutcomeUnknown,
        )?;
        Ok(OutcomeUnknownLedgerImport { sent: self })
    }
}

#[derive(Clone)]
pub struct AwaitingLedgerReadback {
    sent: SentLedgerImport,
    receipt: ParsedImportReceipt,
}

impl AwaitingLedgerReadback {
    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        self.sent.identity_query_digest()
    }

    pub fn verify_readback(
        self,
        readback_xml: &str,
        registry: &mut IdempotencyRegistry,
    ) -> Result<DerivedWriteVerdict, QualificationError> {
        let prepared = &self.sent.qualified.prepared;
        let observed = parse_readback(prepared, readback_xml)?;
        let exact_after = observed.projections == prepared.expected_after;
        let exact_before = observed.projections == prepared.expected_before;
        let expected_created = prepared
            .mutations
            .iter()
            .filter(|mutation| mutation.operation == LedgerOperation::Create)
            .count() as u64;
        let expected_altered = prepared.mutations.len() as u64 - expected_created;
        let exact_counters = self.receipt.is_clean()
            && self.receipt.counters.created == expected_created
            && self.receipt.counters.altered == expected_altered
            && self.receipt.counters.deleted == 0;
        let zero_counters = self.receipt.counters.created == 0
            && self.receipt.counters.altered == 0
            && self.receipt.counters.deleted == 0;
        let outcome = if exact_after && exact_counters {
            WriteOutcome::ExactApplied
        } else if exact_before && zero_counters {
            WriteOutcome::ExactNotApplied
        } else {
            WriteOutcome::Mismatch
        };
        registry.transition(
            &prepared.authorization.idempotency_key,
            ReservationState::AwaitingReadback,
            ReservationState::Terminal,
        )?;
        Ok(verdict(prepared, Some(&self.receipt), observed, outcome))
    }
}

#[derive(Clone)]
pub struct OutcomeUnknownLedgerImport {
    sent: SentLedgerImport,
}

impl OutcomeUnknownLedgerImport {
    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        self.sent.identity_query_digest()
    }

    /// A readback may add commitments for investigation, but without a parsed
    /// import receipt the terminal write outcome remains unknown.
    pub fn observe_readback(
        &self,
        readback_xml: &str,
    ) -> Result<DerivedWriteVerdict, QualificationError> {
        let prepared = &self.sent.qualified.prepared;
        let observed = parse_readback(prepared, readback_xml)?;
        Ok(verdict(
            prepared,
            None,
            observed,
            WriteOutcome::OutcomeUnknown,
        ))
    }
}

fn verdict(
    prepared: &PreparedLedgerImport,
    receipt: Option<&ParsedImportReceipt>,
    observed: ParsedReadback,
    outcome: WriteOutcome,
) -> DerivedWriteVerdict {
    DerivedWriteVerdict {
        outcome,
        wire_digest: prepared.wire_digest.clone(),
        intended_state_digest: prepared.intended_state_digest.clone(),
        import_response_digest: receipt.map(|value| value.response_digest.clone()),
        readback_state_digest: observed.state_digest,
        identity_coverage_digest: observed.coverage_digest,
        auto_retry_allowed: false,
    }
}

#[derive(Clone)]
pub struct ParsedImportReceipt {
    application_status: TallyImportApplicationStatus,
    counters: TallyImportResult,
    exceptions_were_reported: bool,
    response_digest: ImportResponseDigest,
    line_error_digests: Vec<LineErrorDigest>,
}

impl std::fmt::Debug for ParsedImportReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParsedImportReceipt")
            .field("application_status", &self.application_status)
            .field("counters", &self.counters)
            .field("exceptions_were_reported", &self.exceptions_were_reported)
            .field("response_digest", &self.response_digest)
            .field("line_error_count", &self.line_error_digests.len())
            .finish()
    }
}

impl ParsedImportReceipt {
    pub fn application_status(&self) -> TallyImportApplicationStatus {
        self.application_status
    }

    pub fn counters(&self) -> &TallyImportResult {
        &self.counters
    }

    pub fn exceptions_were_reported(&self) -> bool {
        self.exceptions_were_reported
    }

    pub fn response_digest(&self) -> &ImportResponseDigest {
        &self.response_digest
    }

    pub fn line_error_digests(&self) -> &[LineErrorDigest] {
        &self.line_error_digests
    }

    fn is_clean(&self) -> bool {
        self.application_status != TallyImportApplicationStatus::Failure
            && self.counters.is_clean_success()
    }
}

pub fn parse_import_receipt(xml: &str) -> Result<ParsedImportReceipt, QualificationError> {
    let evidence =
        parse_import_evidence(xml).map_err(|_| QualificationError::InvalidImportReceipt)?;
    Ok(receipt_from_evidence(evidence))
}

fn receipt_from_evidence(evidence: ParsedImportEvidence) -> ParsedImportReceipt {
    ParsedImportReceipt {
        application_status: evidence.application_status(),
        counters: evidence.counters().clone(),
        exceptions_were_reported: evidence.exceptions_were_reported(),
        response_digest: ImportResponseDigest(evidence.response_sha256().to_owned()),
        line_error_digests: evidence
            .line_error_sha256()
            .iter()
            .cloned()
            .map(LineErrorDigest)
            .collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    ExactApplied,
    ExactNotApplied,
    Mismatch,
    OutcomeUnknown,
}

#[derive(Clone)]
pub struct DerivedWriteVerdict {
    outcome: WriteOutcome,
    wire_digest: WirePayloadDigest,
    intended_state_digest: IntendedStateDigest,
    import_response_digest: Option<ImportResponseDigest>,
    readback_state_digest: ReadbackStateDigest,
    identity_coverage_digest: IdentityCoverageDigest,
    auto_retry_allowed: bool,
}

impl std::fmt::Debug for DerivedWriteVerdict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DerivedWriteVerdict")
            .field("outcome", &self.outcome)
            .field("wire_digest", &self.wire_digest)
            .field("intended_state_digest", &self.intended_state_digest)
            .field("import_response_digest", &self.import_response_digest)
            .field("readback_state_digest", &self.readback_state_digest)
            .field("identity_coverage_digest", &self.identity_coverage_digest)
            .field("auto_retry_allowed", &self.auto_retry_allowed)
            .finish()
    }
}

impl DerivedWriteVerdict {
    pub fn outcome(&self) -> WriteOutcome {
        self.outcome
    }

    pub const fn auto_retry_allowed(&self) -> bool {
        self.auto_retry_allowed
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QualificationError {
    #[error("explicit operator opt-in is required")]
    ExplicitOptInRequired,
    #[error("controlled writes require an explicitly confirmed synthetic company")]
    SyntheticCompanyRequired,
    #[error("controlled writes require an observed capability")]
    ObservedCapabilityRequired,
    #[error("backup guidance acknowledgement is required")]
    BackupAcknowledgementRequired,
    #[error("approval evidence does not bind the exact preview commitments")]
    ApprovalMismatch,
    #[error("fixture canary payload does not match its approved commitment")]
    FixtureCanaryPayloadMismatch,
    #[error("controlled ledger write batch must contain between one and ten items")]
    InvalidBatchSize,
    #[error("controlled ledger write input is invalid: {0}")]
    InvalidField(&'static str),
    #[error("controlled ledger write contains a duplicate remote identity")]
    DuplicateIdentity,
    #[error("idempotency key is already bound to another write")]
    IdempotencyConflict,
    #[error("duplicate write submission is blocked")]
    DuplicateSubmission,
    #[error("idempotency reservation is missing")]
    MissingIdempotencyReservation,
    #[error("controlled ledger alter must change at least one verified field")]
    NoOpMutation,
    #[error("controlled ledger create requires an explicit parent")]
    CreateParentRequired,
    #[error("clearing controlled ledger field is not yet qualified: {0}")]
    UnsupportedFieldClear(&'static str),
    #[error("preflight readback did not exactly match the declared before state")]
    PreflightMismatch,
    #[error("post-dispatch receipt or readback did not exactly apply the fixture canary")]
    PostDispatchMismatch,
    #[error("Tally import receipt was invalid")]
    InvalidImportReceipt,
    #[error("Tally ledger readback was invalid or outside the expected company/profile")]
    InvalidReadback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Projection {
    contract: &'static str,
    company_guid: String,
    profile: &'static str,
    operation: LedgerOperation,
    remote_id: String,
    source_lineage: SourceLineage,
    mapping_version: String,
    state: LedgerState,
}

#[derive(Serialize)]
struct MutationIntentPreimage<'a> {
    contract: &'static str,
    before: &'a BTreeMap<String, Projection>,
    after: &'a BTreeMap<String, Projection>,
}

#[derive(Debug)]
struct ParsedReadback {
    projections: BTreeMap<String, Projection>,
    state_digest: ReadbackStateDigest,
    coverage_digest: IdentityCoverageDigest,
}

#[derive(Clone, Debug)]
pub struct LedgerImportPreview {
    wire_digest: WirePayloadDigest,
    intended_state_digest: IntendedStateDigest,
    identity_query_digest: IdentityQueryDigest,
}

impl LedgerImportPreview {
    pub fn wire_digest(&self) -> &WirePayloadDigest {
        &self.wire_digest
    }

    pub fn intended_state_digest(&self) -> &IntendedStateDigest {
        &self.intended_state_digest
    }

    pub fn identity_query_digest(&self) -> &IdentityQueryDigest {
        &self.identity_query_digest
    }
}

pub fn preview_ledger_import(
    company: &SyntheticCompany,
    mutations: &[LedgerMutation],
    mapping_version: &str,
) -> Result<LedgerImportPreview, QualificationError> {
    validate_value(mapping_version, "mapping_version")?;
    if mutations.is_empty() || mutations.len() > MAX_LEDGER_WRITE_BATCH {
        return Err(QualificationError::InvalidBatchSize);
    }
    let identities = mutations
        .iter()
        .map(|mutation| mutation.remote_id.clone())
        .collect::<BTreeSet<_>>();
    if identities.len() != mutations.len() {
        return Err(QualificationError::DuplicateIdentity);
    }
    let expected_after = mutations
        .iter()
        .map(|mutation| {
            (
                mutation.remote_id.clone(),
                projection(company, mutation, mutation.after.clone(), mapping_version),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected_before = mutations
        .iter()
        .filter_map(|mutation| {
            mutation.before.as_ref().map(|before| {
                (
                    mutation.remote_id.clone(),
                    projection(company, mutation, before.clone(), mapping_version),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let wire_bytes = build_import_xml(company, mutations).into_bytes();
    let intended_bytes = serde_json::to_vec(&MutationIntentPreimage {
        contract: "bridge.tally.ledger-mutation-intent/1",
        before: &expected_before,
        after: &expected_after,
    })
    .expect("versioned ledger projections are always serializable");
    let identity_bytes =
        serde_json::to_vec(&identities).expect("identity query sets are always serializable");
    Ok(LedgerImportPreview {
        wire_digest: WirePayloadDigest(domain_digest(
            b"bridge.tally.ledger-import-wire/1\0",
            &wire_bytes,
        )),
        intended_state_digest: IntendedStateDigest(domain_digest(
            b"bridge.tally.ledger-intended-state/1\0",
            &intended_bytes,
        )),
        identity_query_digest: IdentityQueryDigest(domain_digest(
            b"bridge.tally.ledger-readback-query-identities/1\0",
            &identity_bytes,
        )),
    })
}

pub fn prepare_ledger_import(
    company: SyntheticCompany,
    mutations: Vec<LedgerMutation>,
    authorization: WriteAuthorization,
    registry: &mut IdempotencyRegistry,
) -> Result<PreparedLedgerImport, QualificationError> {
    if mutations.is_empty() || mutations.len() > MAX_LEDGER_WRITE_BATCH {
        return Err(QualificationError::InvalidBatchSize);
    }
    if authorization.company_guid != company.guid {
        return Err(QualificationError::SyntheticCompanyRequired);
    }
    let mut identities = BTreeSet::new();
    let mut expected_before = BTreeMap::new();
    let mut expected_after = BTreeMap::new();
    for mutation in &mutations {
        if !identities.insert(mutation.remote_id.clone()) {
            return Err(QualificationError::DuplicateIdentity);
        }
        if let Some(before) = &mutation.before {
            expected_before.insert(
                mutation.remote_id.clone(),
                projection(
                    &company,
                    mutation,
                    before.clone(),
                    &authorization.mapping_version,
                ),
            );
        }
        expected_after.insert(
            mutation.remote_id.clone(),
            projection(
                &company,
                mutation,
                mutation.after.clone(),
                &authorization.mapping_version,
            ),
        );
    }
    let wire_bytes = build_import_xml(&company, &mutations).into_bytes();
    let wire_digest = WirePayloadDigest(domain_digest(
        b"bridge.tally.ledger-import-wire/1\0",
        &wire_bytes,
    ));
    let intended_bytes = serde_json::to_vec(&MutationIntentPreimage {
        contract: "bridge.tally.ledger-mutation-intent/1",
        before: &expected_before,
        after: &expected_after,
    })
    .expect("versioned ledger projections are always serializable");
    let intended_state_digest = IntendedStateDigest(domain_digest(
        b"bridge.tally.ledger-intended-state/1\0",
        &intended_bytes,
    ));
    let identity_bytes =
        serde_json::to_vec(&identities).expect("identity query sets are always serializable");
    let identity_query_digest = IdentityQueryDigest(domain_digest(
        b"bridge.tally.ledger-readback-query-identities/1\0",
        &identity_bytes,
    ));
    if authorization.approved_wire_sha256 != wire_digest.as_hex()
        || authorization.approved_intended_state_sha256 != intended_state_digest.as_hex()
        || authorization.approved_identity_query_sha256 != identity_query_digest.as_hex()
    {
        return Err(QualificationError::ApprovalMismatch);
    }
    let approval_evidence_digest =
        ApprovalEvidenceDigest(authorization.approval_evidence_sha256.clone());
    registry.reserve(&authorization, &wire_digest)?;
    Ok(PreparedLedgerImport {
        company,
        mutations,
        wire_digest,
        intended_state_digest,
        expected_before,
        expected_after,
        authorization,
        approval_evidence_digest,
        identity_query_digest,
    })
}

/// Returns the one immutable ledger create used to qualify a disposable
/// fixture. The canary deliberately carries no GSTIN and a zero balance.
pub fn fixture_canary_ledger_mutation() -> Result<LedgerMutation, QualificationError> {
    LedgerMutation::create(
        FIXTURE_CANARY_REMOTE_ID,
        LedgerState::new(
            FIXTURE_CANARY_LEDGER_NAME,
            Some(FIXTURE_CANARY_PARENT.to_owned()),
            None,
            Some(FIXTURE_CANARY_OPENING_BALANCE.to_owned()),
        )?,
        SourceLineage::new(
            "bridge-fixture-canary",
            FIXTURE_CANARY_REMOTE_ID,
            FIXTURE_CANARY_MAPPING_VERSION,
        )?,
    )
}

/// Prepares exactly one fixed, non-dispatchable fixture-canary preview. It has
/// no caller-supplied mutation or mapping escape hatch, and its returned type
/// intentionally cannot progress into the generic import lifecycle.
pub fn prepare_fixture_canary_ledger_import(
    company: SyntheticCompany,
    authorization: FixtureCanaryAuthorization,
    registry: &mut IdempotencyRegistry,
) -> Result<PreparedFixtureCanary, QualificationError> {
    let prepared = prepare_ledger_import(
        company,
        vec![fixture_canary_ledger_mutation()?],
        authorization.authorization,
        registry,
    )?;
    Ok(PreparedFixtureCanary { prepared })
}

fn projection(
    company: &SyntheticCompany,
    mutation: &LedgerMutation,
    state: LedgerState,
    mapping_version: &str,
) -> Projection {
    Projection {
        contract: LEDGER_WRITE_PROJECTION,
        company_guid: company.guid.clone(),
        profile: LEDGER_READBACK_PROFILE,
        operation: mutation.operation,
        remote_id: mutation.remote_id.clone(),
        source_lineage: mutation.source_lineage.clone(),
        mapping_version: mapping_version.to_owned(),
        state,
    }
}

fn parse_readback(
    prepared: &PreparedLedgerImport,
    xml: &str,
) -> Result<ParsedReadback, QualificationError> {
    let parsed = parse_ledger_write_readback_with_evidence(xml)
        .map_err(|_| QualificationError::InvalidReadback)?;
    let context = parsed
        .evidence
        .company_context
        .as_ref()
        .ok_or(QualificationError::InvalidReadback)?;
    if context.guid.as_deref() != Some(prepared.company.guid.as_str())
        || context.query_identity_set_sha256.as_deref()
            != Some(prepared.identity_query_digest.as_hex())
        || parsed.evidence.schema.as_deref() != Some(LEDGER_READBACK_PROFILE)
        || parsed.evidence.object_type.as_deref() != Some("LEDGER")
        || parsed.evidence.source_record_count != Some(parsed.records.len() as u64)
        || !parsed.evidence.duplicate_identities.is_empty()
    {
        return Err(QualificationError::InvalidReadback);
    }

    let by_id: BTreeMap<&str, &LedgerMutation> = prepared
        .mutations
        .iter()
        .map(|mutation| (mutation.remote_id.as_str(), mutation))
        .collect();
    let mut projections = BTreeMap::new();
    for record in parsed.records {
        let remote_id = record
            .identities
            .remote_id
            .ok_or(QualificationError::InvalidReadback)?;
        let mutation = by_id
            .get(remote_id.as_str())
            .ok_or(QualificationError::InvalidReadback)?;
        if projections.contains_key(&remote_id) {
            return Err(QualificationError::InvalidReadback);
        }
        let state = LedgerState::new(
            record.record.name,
            record.record.parent,
            record.record.party_gstin,
            record.record.opening_balance,
        )?;
        projections.insert(
            remote_id.clone(),
            projection(
                &prepared.company,
                mutation,
                state,
                &prepared.authorization.mapping_version,
            ),
        );
    }
    let state_bytes = serde_json::to_vec(&projections)
        .expect("versioned ledger projections are always serializable");
    let identities = projections.keys().cloned().collect::<Vec<_>>();
    let coverage_bytes =
        serde_json::to_vec(&identities).expect("identity coverage is always serializable");
    Ok(ParsedReadback {
        projections,
        state_digest: ReadbackStateDigest(domain_digest(
            b"bridge.tally.ledger-readback-state/1\0",
            &state_bytes,
        )),
        coverage_digest: IdentityCoverageDigest(domain_digest(
            b"bridge.tally.ledger-identity-coverage/1\0",
            &coverage_bytes,
        )),
    })
}

fn build_import_xml(company: &SyntheticCompany, mutations: &[LedgerMutation]) -> String {
    let mut xml = String::from(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Import</TALLYREQUEST><TYPE>Data</TYPE><ID>All Masters</ID></HEADER><BODY><DESC><STATICVARIABLES><SVCURRENTCOMPANY>",
    );
    push_xml_text(&mut xml, &company.name);
    xml.push_str("</SVCURRENTCOMPANY></STATICVARIABLES></DESC><DATA><TALLYMESSAGE>");
    for mutation in mutations {
        xml.push_str("<LEDGER REMOTEID=\"");
        push_xml_attribute(&mut xml, &mutation.remote_id);
        let selector_name = mutation
            .before
            .as_ref()
            .map(|before| &before.name)
            .unwrap_or(&mutation.after.name);
        xml.push_str("\" NAME=\"");
        push_xml_attribute(&mut xml, selector_name);
        xml.push_str("\" ACTION=\"");
        xml.push_str(mutation.operation.tally_action());
        xml.push_str("\">");
        if mutation.operation == LedgerOperation::Create
            || mutation
                .before
                .as_ref()
                .is_some_and(|before| before.name != mutation.after.name)
        {
            push_element(&mut xml, "NAME", &mutation.after.name);
        }
        if let Some(value) = &mutation.after.parent {
            push_element(&mut xml, "PARENT", value);
        }
        if let Some(value) = &mutation.after.party_gstin {
            push_element(&mut xml, "PARTYGSTIN", value);
        }
        if let Some(value) = &mutation.after.opening_balance {
            push_element(&mut xml, "OPENINGBALANCE", value);
        }
        xml.push_str("</LEDGER>");
    }
    xml.push_str("</TALLYMESSAGE></DATA></BODY></ENVELOPE>");
    xml
}

fn push_element(xml: &mut String, name: &str, value: &str) {
    xml.push('<');
    xml.push_str(name);
    xml.push('>');
    push_xml_text(xml, value);
    xml.push_str("</");
    xml.push_str(name);
    xml.push('>');
}

fn push_xml_text(xml: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => xml.push_str("&amp;"),
            '<' => xml.push_str("&lt;"),
            '>' => xml.push_str("&gt;"),
            _ => xml.push(character),
        }
    }
}

fn push_xml_attribute(xml: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '&' => xml.push_str("&amp;"),
            '<' => xml.push_str("&lt;"),
            '>' => xml.push_str("&gt;"),
            '\"' => xml.push_str("&quot;"),
            '\'' => xml.push_str("&apos;"),
            _ => xml.push(character),
        }
    }
}

fn validate_value(value: &str, field: &'static str) -> Result<(), QualificationError> {
    if value.trim().is_empty()
        || value.len() > 255
        || value.chars().any(|character| character.is_control())
    {
        return Err(QualificationError::InvalidField(field));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &'static str) -> Result<(), QualificationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(QualificationError::InvalidField(field));
    }
    Ok(())
}

fn validate_gstin(value: &str) -> Result<(), QualificationError> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 15
        && bytes[0..2].iter().all(u8::is_ascii_digit)
        && bytes[2..7].iter().all(u8::is_ascii_uppercase)
        && bytes[7..11].iter().all(u8::is_ascii_digit)
        && bytes[11].is_ascii_uppercase()
        && bytes[12].is_ascii_alphanumeric()
        && bytes[13] == b'Z'
        && bytes[14].is_ascii_alphanumeric();
    if !valid {
        return Err(QualificationError::InvalidField("party_gstin"));
    }
    Ok(())
}

fn domain_digest(domain: &[u8], value: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(value);
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    use bridge_tally_transport::TallyEndpointConfig;
    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator};

    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    const FIXTURE_COMPANY_GUID: &str = "00000000-0000-4000-8000-000000000001";
    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    const FIXTURE_COMMITMENT: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    fn sealed_fixture_canary() -> PreparedFixtureCanary {
        let company = SyntheticCompany::new("BRIDGE SYNTHETIC BOOK", FIXTURE_COMPANY_GUID)
            .expect("synthetic company");
        let mutation = fixture_canary_ledger_mutation().expect("fixed canary mutation");
        let preview = preview_ledger_import(&company, &[mutation], FIXTURE_CANARY_MAPPING_VERSION)
            .expect("fixed canary preview");
        let authorization = authorize_fixture_canary(FixtureCanaryAuthorizationRequest {
            explicit_opt_in: true,
            synthetic_company_confirmed: true,
            company_guid: FIXTURE_COMPANY_GUID.to_owned(),
            backup_guidance_acknowledged: true,
            review_commitment_sha256: FIXTURE_COMMITMENT.to_owned(),
            reservation_id: "fixture-runtime-dispatch-reservation".to_owned(),
            reservation_payload_sha256: FIXTURE_COMMITMENT.to_owned(),
            approved_wire_sha256: preview.wire_digest().as_hex().to_owned(),
            approved_intended_state_sha256: preview.intended_state_digest().as_hex().to_owned(),
            approved_identity_query_sha256: preview.identity_query_digest().as_hex().to_owned(),
            idempotency_key: "fixture-runtime-dispatch-idempotency".to_owned(),
        })
        .expect("authorize fixed canary");
        prepare_fixture_canary_ledger_import(
            company,
            authorization,
            &mut IdempotencyRegistry::default(),
        )
        .expect("prepare fixed canary")
    }

    #[cfg(feature = "fixture-canary-runtime-dispatch")]
    #[tokio::test]
    async fn sealed_fixture_canary_dispatch_posts_exactly_once_without_xml_escape() {
        let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::ImportCounters))
            .expect("spawn synthetic loopback server");
        let transport = TallyHttpTransport::new(TallyEndpointConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        })
        .expect("construct bounded loopback transport");
        let receipt = sealed_fixture_canary()
            .seal_for_dispatch()
            .expect("seal exact canary")
            .dispatch_once(&transport)
            .await
            .expect("single synthetic import response");

        assert!(format!("{receipt:?}").contains("[redacted]"));
        let observed = simulator.finish().expect("observe one request");
        assert_eq!(observed.method, "POST");
        assert_eq!(observed.path, "/");
        assert!(observed.bytes_received > 0);
    }

    #[test]
    fn private_wire_builder_escapes_text_and_attributes() {
        let company = SyntheticCompany::new("BRIDGE & BOOK", "company-guid").unwrap();
        let mutation = LedgerMutation::create(
            "remote-\"<&",
            LedgerState::new(
                "LEDGER <&",
                Some("PARENT <&".to_owned()),
                None,
                Some("0".to_owned()),
            )
            .unwrap(),
            SourceLineage::new("synthetic", "record", "v1").unwrap(),
        )
        .unwrap();
        let xml = build_import_xml(&company, &[mutation]);
        assert!(xml.contains("BRIDGE &amp; BOOK"));
        assert!(xml.contains("remote-&quot;&lt;&amp;"));
        assert!(xml
            .contains("NAME=\"LEDGER &lt;&amp;\" ACTION=\"Create\"><NAME>LEDGER &lt;&amp;</NAME>"));
        assert!(xml.contains("<PARENT>PARENT &lt;&amp;</PARENT>"));
    }

    #[test]
    fn private_wire_builder_uses_existing_name_to_select_a_rename() {
        let company = SyntheticCompany::new("BRIDGE SYNTHETIC BOOK", "company-guid").unwrap();
        let mutation = LedgerMutation::alter(
            "bridge-remote-id",
            LedgerState::new("OLD LEDGER", None, None, None).unwrap(),
            LedgerState::new("NEW LEDGER", None, None, None).unwrap(),
            SourceLineage::new("synthetic", "record", "v1").unwrap(),
        )
        .unwrap();

        let xml = build_import_xml(&company, &[mutation]);

        assert!(xml.contains("NAME=\"OLD LEDGER\" ACTION=\"Alter\"><NAME>NEW LEDGER</NAME>"));
        assert!(!xml.contains("NAME=\"NEW LEDGER\" ACTION=\"Alter\""));
    }
}
