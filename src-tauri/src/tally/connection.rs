use anyhow::Context as _;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc,
};
#[cfg(feature = "voucher-scan")]
use std::time::{Duration, Instant};

use super::xml_parser::{TallyLedger, TallyVoucher};
use super::{
    tdl_engine,
    validators::{normalize_company_guid, normalize_company_name},
    xml_parser::{self, TallyCompany},
    VerifiedCompanyIdentity,
};
use crate::reports::party_ledger_master::{PartyLedgerMasterRow, PartyLedgerMasterSource};
use crate::tally::runtime::{
    with_read_evidence, PartyLedgerMasterCurrencyAssertion, RuntimeReadEvidence,
};
use bridge_tally_core::{
    CapabilityEvidence, CapabilityFeatureId, CapabilityPackId, CapabilityProfile, CapabilityState,
    EvidenceConfidence, LicenseTier, TransportId,
};
#[cfg(feature = "voucher-scan")]
use bridge_tally_protocol::outstandings::{
    parse_ledger_opening_coverage, verify_empty_partition_witness_pair_with_wire_evidence,
    verify_segment_pair_with_wire_evidence, voucher_empty_partition_witness_request,
    voucher_outstandings_request, AlterIdRange, LedgerOpeningCoverage, NarrowDateWindow,
    PinnedCompany, SegmentVerification, SegmentWireEvidence, VoucherOutstandingsRequestXml,
    WitnessPairVerification,
};
use bridge_tally_protocol::{
    native_outstandings::{
        parse_native_group_snapshot_with_evidence, parse_native_ledger_snapshot_for_company,
        render_native_group_snapshot_request, render_native_ledger_export_request,
        render_native_ledger_snapshot_request, render_native_voucher_export_request,
        render_party_ledger_master_request, NativeLedgerExportPeriod, NativeLedgerSnapshotPeriod,
        NativeOutstandingsError,
    },
    outstandings_shared::{
        parse_company_book_extent_v2, require_master_witness, CompanyBookExtent,
        DateBoundaryProfile,
    },
    parse_companies_for_interactive_discovery, parse_company_gateway_capability_observation,
    parse_ledger_source_records_with_evidence, parse_native_ledger_source_records_with_evidence,
    parse_native_party_ledger_master_records_with_evidence,
    parse_native_voucher_source_records_with_evidence,
    parse_selected_voucher_source_records_with_evidence, parse_standard_ledger_catalog,
    parse_standard_ledger_identity_observation, verify_selected_voucher_window_context,
    xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName},
    TallyTextEncoding, BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA,
};
use bridge_tally_transport::{
    canonical_loopback_origin as transport_canonical_origin, TallyEndpointConfig,
    TallyHttpTransport, TallyTransportError,
};

pub type TallyConfig = TallyEndpointConfig;

fn ledger_display_key(name: &str, parent: Option<&str>) -> String {
    let parent = parent.unwrap_or_default();
    format!("{}:{name}{}:{parent}", name.len(), parent.len())
}

fn party_ledger_master_balance_snapshot_error(error: NativeOutstandingsError) -> anyhow::Error {
    match error {
        NativeOutstandingsError::InvalidResponse(
            "ledger_response_company_guid_missing" | "ledger_response_company_guid_mismatch",
        ) => anyhow::Error::new(
            PartyLedgerMasterSourceValidationError::BalanceCompanyIdentityUnverified,
        ),
        error => anyhow::Error::new(error),
    }
}

fn party_ledger_master_group_snapshot_error(error: NativeOutstandingsError) -> anyhow::Error {
    match error {
        NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_missing" | "group_response_company_guid_mismatch",
        ) => anyhow::Error::new(
            PartyLedgerMasterSourceValidationError::GroupCompanyIdentityUnverified,
        ),
        error => anyhow::Error::new(error),
    }
}

/// The paired master request reached Tally successfully. Any parser failure
/// after that point is response validation, never endpoint reachability.
fn party_ledger_master_master_snapshot_error(source: anyhow::Error) -> anyhow::Error {
    anyhow::Error::new(PartyLedgerMasterSourceValidationError::MasterResponseInvalid { source })
}

fn party_ledger_master_openings_agree(
    master_opening: &str,
    balance_opening: &bridge_tally_core::ExactDecimal,
) -> anyhow::Result<bool> {
    let master_opening = bridge_tally_core::ExactDecimal::parse(master_opening.to_owned())?;
    Ok(master_opening.numeric_eq(balance_opening))
}

/// The paired sources answered successfully but cannot be reconciled into one
/// safe workbook source. This is distinct from endpoint or XML failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PartyLedgerMasterSourceValidationError {
    #[error("Tally master ledger export period is unsupported")]
    MasterPeriod,
    #[error("Tally closing-balance period is unsupported")]
    BalancePeriod,
    #[error("Tally ledger master omitted GUID")]
    MasterGuid,
    #[error("Tally ledger master omitted MASTERID")]
    MasterId,
    #[error("Tally ledger master omitted ALTERID")]
    MasterAlterId,
    #[error("Tally ledger master omitted OPENINGBALANCE")]
    MasterOpeningBalance,
    #[error("Tally ledger master repeated a stable source identity")]
    DuplicateMasterIdentity,
    #[error("Tally balance snapshot omitted a ledger master")]
    BalanceMissingMasterLedger,
    #[error("Tally ledger opening balances disagreed across the paired sources")]
    OpeningBalancesDisagreed,
    #[error("Tally balance snapshot contained a ledger absent from master evidence")]
    BalanceLedgerAbsentFromMasterEvidence,
    #[error("Tally balance snapshot repeated a ledger display key")]
    DuplicateBalanceDisplayKey,
    #[error("Tally balance snapshot did not prove the selected company identity")]
    BalanceCompanyIdentityUnverified,
    #[error("Tally Group snapshot did not prove the selected company identity")]
    GroupCompanyIdentityUnverified,
    #[error("Tally party/ledger master response failed validation")]
    MasterResponseInvalid {
        #[source]
        source: anyhow::Error,
    },
}

impl PartyLedgerMasterSourceValidationError {
    /// A stable, data-free name for this refusal, safe to return to an agent.
    pub(crate) fn safe_code(&self) -> &'static str {
        match self {
            Self::MasterPeriod => "master_period_unsupported",
            Self::BalancePeriod => "balance_period_unsupported",
            Self::MasterGuid => "master_guid_missing",
            Self::MasterId => "master_id_missing",
            Self::MasterAlterId => "master_alter_id_missing",
            Self::MasterOpeningBalance => "master_opening_balance_missing",
            Self::DuplicateMasterIdentity => "duplicate_master_identity",
            Self::BalanceMissingMasterLedger => "balance_missing_master_ledger",
            Self::OpeningBalancesDisagreed => "opening_balances_disagreed",
            Self::BalanceLedgerAbsentFromMasterEvidence => {
                "balance_ledger_absent_from_master_evidence"
            }
            Self::DuplicateBalanceDisplayKey => "duplicate_balance_display_key",
            Self::BalanceCompanyIdentityUnverified => "balance_company_identity_unverified",
            Self::GroupCompanyIdentityUnverified => "group_company_identity_unverified",
            Self::MasterResponseInvalid { .. } => "master_response_invalid",
        }
    }
}

/// A paired or bracketed read observed movement in the endpoint's data. This
/// is response validation, not an endpoint failure: Tally answered, but
/// Bridge must withhold the unstable result.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PairedReadValidationError {
    #[error("Tally native ledger collection changed between paired reads")]
    NativeLedgerCollection,
    #[error("Tally company book changed during native ledger read")]
    NativeLedgerExtent,
    #[error("Tally group hierarchy changed between paired reads beside a ledger read")]
    NativeLedgerGroup,
    #[error("Tally ledger master changed between paired reads")]
    PartyLedgerMaster,
    #[error("Tally ledger balances changed between paired reads")]
    PartyLedgerBalance,
    #[error("Tally group hierarchy changed between paired reads")]
    PartyLedgerGroup,
    #[error("Tally company book changed during party/ledger master read")]
    PartyLedgerExtent,
    #[error("Tally company book extent changed between paired reads")]
    CompanyBookExtent,
    #[error("Tally currency masters changed between paired reads")]
    CurrencyMaster,
    #[error("Tally company currency name changed between paired reads")]
    CompanyCurrencyName,
    #[error("Tally company book changed during currency detection")]
    CurrencyExtent,
    #[error("Tally company changed between the currency read and the master read")]
    CurrencyToMasterExtent,
}

impl PairedReadValidationError {
    /// A stable, data-free name for this refusal, safe to return to an agent.
    pub(crate) fn safe_code(&self) -> &'static str {
        match self {
            Self::NativeLedgerCollection => "native_ledger_collection_changed",
            Self::NativeLedgerExtent => "native_ledger_extent_changed",
            Self::NativeLedgerGroup => "native_ledger_group_changed",
            Self::PartyLedgerMaster => "party_ledger_master_changed",
            Self::PartyLedgerBalance => "party_ledger_balance_changed",
            Self::PartyLedgerGroup => "party_ledger_group_changed",
            Self::PartyLedgerExtent => "party_ledger_extent_changed",
            Self::CompanyBookExtent => "company_book_extent_changed",
            Self::CurrencyMaster => "currency_master_changed",
            Self::CompanyCurrencyName => "company_currency_name_changed",
            Self::CurrencyExtent => "currency_extent_changed",
            Self::CurrencyToMasterExtent => "currency_to_master_extent_changed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DirectCompanyBootstrapError {
    #[error("Tally direct company identity did not match its enumerated candidate")]
    CandidateGuidMismatch,
    #[error("Tally direct company candidate omitted a complete identity tuple")]
    IncompleteTuple,
}

#[cfg(feature = "voucher-scan")]
#[derive(Debug)]
pub(crate) struct OutstandingsSegmentObservation {
    pub(crate) verification: SegmentVerification,
    pub(crate) first_read_elapsed: Duration,
    pub(crate) second_read_elapsed: Duration,
}

#[cfg(feature = "voucher-scan")]
pub(crate) enum LedgerOpeningCoverageRead {
    Stable(LedgerOpeningCoverage),
    Drifted,
}

/// An HTTP response entity kept byte for byte, with its decoded text.
#[derive(Debug, Clone)]
pub(crate) struct RawTallyResponse {
    pub(crate) text: String,
    pub(crate) encoded_body: Vec<u8>,
    pub(crate) encoded_sha256: String,
}

#[derive(Debug, thiserror::Error)]
#[error("Tally native report changed between paired reads")]
pub(crate) struct NativeReportPairDrift;

impl NativeReportPairDrift {
    /// A stable, data-free name for this refusal, safe to return to an agent.
    pub(crate) const SAFE_CODE: &'static str = "native_report_pair_changed";
}

/// Tags a failure as coming from one of the two POST responses inside
/// `fetch_native_report_paired_with_evidence` -- the paired native report
/// request itself, never the health checks bracketing it or any stage
/// outside this function. A catalogue read's classifier applies its
/// bounds/malformed split only when this marker is present in the error
/// chain; everything untagged -- the identity bracket, both health checks,
/// and any stage added later -- falls back to the conservative `Transport`
/// code by default. That inversion is deliberate: tagging every non-response
/// stage is unbounded (there is always another stage to remember), while a
/// positive marker on the one response that is actually being classified
/// means a stage added later inherits the safe code automatically instead of
/// silently inheriting a confidently wrong one.
///
/// Deliberately `transparent`: `tally_runtime_command_error` classifies some
/// failures by substring-matching `error.to_string()`, which is the *top-level*
/// message, so any wrapper with a message of its own silently rewrites how
/// every reader's errors classify. A message naming this stage would have
/// contained "report", whose "port" substring routes straight to
/// `endpoint_configuration_invalid` -- telling operators their endpoint is
/// misconfigured when a connection merely dropped. Forwarding Display leaves
/// every existing message byte-identical and adds only a type to downcast to.
#[derive(Debug)]
pub(crate) struct PairedNativeReportResponseFailure(anyhow::Error);

// Display and Error are written out rather than derived because this marker has
// to be invisible in two different ways at once, and no single derive gives
// both.
//
// Display forwards: `tally_runtime_command_error` classifies some failures by
// substring-matching `error.to_string()`, which is the *top-level* message, so a
// marker with a message of its own silently rewrites how every reader's errors
// classify. A message naming this stage would have contained "report", whose
// "port" substring routes to `endpoint_configuration_invalid` -- blaming the
// endpoint configuration for a dropped connection.
//
// `source` returns the wrapped error's own head rather than delegating to its
// source. `#[error(transparent)]` would delegate, which skips the head and hides
// the `TallyTransportError` from everything that downcasts while walking the
// chain. Returning the head keeps the chain exactly as it was, with this marker
// inserted ahead of it rather than replacing anything.
impl std::fmt::Display for PairedNativeReportResponseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for PairedNativeReportResponseFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

impl PairedNativeReportResponseFailure {
    /// Only constructor: forces every paired-report response failure through
    /// this one marking point rather than each call site improvising its own
    /// wrap. `pub(crate)` so tests outside this module can construct a
    /// tagged failure directly rather than driving a real request.
    pub(crate) fn new(error: anyhow::Error) -> Self {
        Self(error)
    }

    /// The `TallyTransportError` behind this marker, if any. Looked up
    /// through the wrapped `anyhow::Error`'s own chain rather than the outer
    /// error's chain: nesting an `anyhow::Error` behind `#[source]` does not
    /// expose the wrapped error as its own link when walked through
    /// `std::error::Error::source()`, only through `anyhow::Error::chain()`
    /// on that inner value directly.
    pub(crate) fn transport_error(&self) -> Option<&TallyTransportError> {
        self.0
            .chain()
            .find_map(|cause| cause.downcast_ref::<TallyTransportError>())
    }
}

/// Outcome of a paired native-report read. `Drifted` means the two reads
/// disagreed, so the book moved between them and no total may be reported.
pub(crate) enum NativePairedRead {
    Stable {
        body: String,
        encoded_bytes: usize,
        encoded_sha256: String,
    },
    Drifted(RuntimeReadEvidence),
}

impl NativePairedRead {
    pub(crate) fn require_stable(
        self,
        error: PairedReadValidationError,
    ) -> anyhow::Result<(String, usize, String)> {
        match self {
            Self::Stable {
                body,
                encoded_bytes,
                encoded_sha256,
            } => Ok((body, encoded_bytes, encoded_sha256)),
            Self::Drifted(evidence) => {
                Err(super::runtime::with_read_evidence(error.into(), evidence))
            }
        }
    }
}

#[cfg(feature = "voucher-scan")]
struct OutstandingsWireResponse {
    text: String,
    encoded_bytes: usize,
    encoded_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub enum TallyProduct {
    TallyPrime,
    #[serde(rename = "Tally ERP 9")]
    TallyErp9,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionStatus {
    pub reachable: bool,
    pub compatible: bool,
    pub server_text: String,
    pub product: TallyProduct,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TallyProbeResult {
    pub connection: ConnectionStatus,
    pub companies: Vec<TallyCompany>,
    pub profile: CapabilityProfile,
    pub selected_read_scope: Option<SelectedReadScopeEvidence>,
    pub passport_snapshot_id: Option<String>,
}

struct GatewayProductModeEvidence {
    product: String,
    release: Option<String>,
    license_tier: Option<LicenseTier>,
    mode: Option<String>,
    capability: CapabilityEvidence,
}

impl GatewayProductModeEvidence {
    fn unavailable() -> Self {
        Self {
            product: "Unknown".to_string(),
            release: None,
            license_tier: None,
            mode: None,
            capability: CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("product_mode_evidence_unavailable".to_string()),
            },
        }
    }

    fn from_observation(
        observation: bridge_tally_protocol::CompanyGatewayCapabilityObservation,
    ) -> Self {
        // Observed live on 22 Sep 2026 (a TallyPrime 7.1 lab instance in
        // Education): `EDUMODE=Yes` with `SILVER=Yes` and `GOLD=No`. Education
        // still reports Silver, so the mode is read from `EDUMODE` first, and
        // no licence tier is inferred when it is set (bridge#581).
        let mode = if observation.educational_mode {
            Some("Education".to_string())
        } else if observation.silver || observation.gold {
            Some("Licensed".to_string())
        } else {
            None
        };
        let capability = if mode.is_some() {
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: None,
            }
        } else {
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("license_mode_not_established".to_string()),
            }
        };
        let license_tier = match (
            observation.educational_mode,
            observation.silver,
            observation.gold,
        ) {
            (false, true, false) => Some(LicenseTier::Silver),
            (false, false, true) => Some(LicenseTier::Gold),
            _ => None,
        };
        Self {
            product: observation.product,
            release: observation.release,
            license_tier,
            mode,
            capability,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SelectedReadScopeEvidence {
    pub scope_version: u16,
    pub ledger_profile_id: String,
    pub voucher_profile_id: String,
    pub voucher_from_yyyymmdd: String,
    pub voucher_to_yyyymmdd: String,
    pub scope_commitment_sha256: String,
    #[serde(skip_serializing)]
    pub(crate) parent_review_sha256: String,
    #[serde(skip_serializing)]
    pub(crate) company_guid_ascii_casefolded: String,
    #[serde(skip_serializing)]
    pub(crate) company_number: String,
    #[serde(skip_serializing)]
    pub(crate) books_from_yyyymmdd: String,
    #[serde(skip_serializing)]
    pub(crate) observations: Vec<SelectedReadCapabilityObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedReadCapabilityObservation {
    pub capability_key: &'static str,
    pub state: CapabilityState,
    pub confidence: EvidenceConfidence,
    pub safe_reason_code: &'static str,
    pub result_bucket: &'static str,
    pub request_sha256: Option<String>,
    pub decoded_response_sha256: Option<String>,
    pub response_encoding: Option<&'static str>,
    pub company_context_verified: bool,
    pub schema_verified: bool,
    pub record_count_verified: bool,
    pub identity_evidence_state: &'static str,
    pub date_window_verified: bool,
}

pub const SELECTED_LEDGER_QUERY_PROFILE_ID: &str = BRIDGE_LEDGER_EXPORT_SCHEMA;
pub const SELECTED_VOUCHER_QUERY_PROFILE_ID: &str = BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedReadObservation {
    /// SHA-256 of the exact HTTP entity bytes dispatched for the selected read.
    /// Pre-U1 UTF-8 observations and current UTF-16LE observations therefore
    /// retain one stable meaning even though their wire encodings differ.
    pub request_sha256: String,
    /// SHA-256 of the decoded XML re-encoded as UTF-8, not of the wire bytes.
    pub decoded_response_sha256: String,
    pub response_encoding: &'static str,
    pub result_bucket: &'static str,
}

#[derive(Clone)]
pub struct TallyClient {
    config: TallyConfig,
    http: TallyHttpTransport,
    observed_body_bytes: Arc<AtomicU64>,
    observed_encoding: Arc<AtomicU8>,
}

const BODY_BYTES_UNAVAILABLE: u64 = u64::MAX;
const ENCODING_UNAVAILABLE: u8 = 0;
const ENCODING_UTF8: u8 = 1;
const ENCODING_UTF8_BOM: u8 = 2;
const ENCODING_UTF16_LE: u8 = 3;
const ENCODING_UTF16_LE_BOM: u8 = 4;
const ENCODING_UTF16_BE_BOM: u8 = 5;

impl TallyClient {
    pub fn new(config: TallyConfig) -> anyhow::Result<Self> {
        let http = TallyHttpTransport::new(config.clone())?;
        Ok(Self {
            config,
            http,
            observed_body_bytes: Arc::new(AtomicU64::new(BODY_BYTES_UNAVAILABLE)),
            observed_encoding: Arc::new(AtomicU8::new(ENCODING_UNAVAILABLE)),
        })
    }

    pub fn canonical_origin(&self) -> anyhow::Result<String> {
        canonical_loopback_origin(&self.config)
    }

    #[cfg(test)]
    fn with_http_builder(config: TallyConfig, builder: reqwest::ClientBuilder) -> Self {
        let http = TallyHttpTransport::with_builder(
            config.clone(),
            bridge_tally_transport::TransportPolicy::default(),
            builder,
        )
        .expect("build synthetic Tally HTTP transport");
        Self {
            config,
            http,
            observed_body_bytes: Arc::new(AtomicU64::new(BODY_BYTES_UNAVAILABLE)),
            observed_encoding: Arc::new(AtomicU8::new(ENCODING_UNAVAILABLE)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_transport_policy(
        config: TallyConfig,
        policy: bridge_tally_transport::TransportPolicy,
    ) -> anyhow::Result<Self> {
        let http =
            TallyHttpTransport::with_builder(config.clone(), policy, reqwest::Client::builder())?;
        Ok(Self {
            config,
            http,
            observed_body_bytes: Arc::new(AtomicU64::new(BODY_BYTES_UNAVAILABLE)),
            observed_encoding: Arc::new(AtomicU8::new(ENCODING_UNAVAILABLE)),
        })
    }

    pub async fn check_connection(&self) -> anyhow::Result<ConnectionStatus> {
        match self.check_connection_strict().await {
            Ok(status) => Ok(status),
            Err(error) => Ok(ConnectionStatus {
                reachable: false,
                compatible: false,
                server_text: String::new(),
                product: TallyProduct::Unknown,
                error: Some(safe_connection_failure_code(&error).to_string()),
            }),
        }
    }

    pub(crate) async fn check_connection_strict(&self) -> anyhow::Result<ConnectionStatus> {
        self.check_connection_strict_with_wire_evidence()
            .await
            .map(|(status, _)| status)
    }

    async fn check_connection_strict_with_wire_evidence(
        &self,
    ) -> anyhow::Result<(ConnectionStatus, RuntimeReadEvidence)> {
        let response = self.http.get_status_decoded().await?;
        let wire_evidence = RuntimeReadEvidence {
            // GET /status has an empty request body. This is a body commitment,
            // matching POST request_body_sha256, not an invented operation label.
            request_sha256: Sha256::digest([])
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            response_sha256: response.encoded_sha256().to_string(),
            bytes: response.encoded_bytes(),
        };
        self.record_observed_body_bytes(response.encoded_bytes());
        self.record_observed_encoding(response.encoding());
        let response_text = response.into_text();
        let product = detect_product(&response_text);
        let compatible = matches!(product, TallyProduct::TallyPrime | TallyProduct::TallyErp9);
        let server_text = match product {
            TallyProduct::TallyPrime => "TallyPrime Server is Running",
            TallyProduct::TallyErp9 => "Tally ERP 9 Server is Running",
            TallyProduct::Unknown => "Endpoint responded with an unrecognized status document",
        };
        Ok((
            ConnectionStatus {
                reachable: true,
                compatible,
                product,
                server_text: server_text.to_string(),
                error: None,
            },
            wire_evidence,
        ))
    }

    pub async fn probe(&self) -> anyhow::Result<TallyProbeResult> {
        self.probe_with_wire_evidence()
            .await
            .map(|(probe, _)| probe)
    }

    pub(crate) async fn probe_with_wire_evidence(
        &self,
    ) -> anyhow::Result<(TallyProbeResult, RuntimeReadEvidence)> {
        // `/status` is useful local diagnostics but is not part of Tally's
        // documented third-party XML contract. Never gate the POST probe or
        // authoritative product metadata on this unauthenticated heuristic.
        let (mut connection, mut wire_evidence) =
            match self.check_connection_strict_with_wire_evidence().await {
                Ok(observation) => observation,
                Err(error) => (
                    ConnectionStatus {
                        reachable: false,
                        compatible: false,
                        server_text: String::new(),
                        product: TallyProduct::Unknown,
                        error: Some(safe_connection_failure_code(&error).to_string()),
                    },
                    RuntimeReadEvidence::empty(),
                ),
            };
        let mut transports = BTreeMap::new();
        let mut features = BTreeMap::new();
        let mut packs = BTreeMap::new();
        let mut companies = Vec::new();

        let mut gateway_product_mode = GatewayProductModeEvidence::unavailable();
        let xml_evidence = self
            .company_discovery_evidence(
                &mut connection,
                &mut companies,
                &mut gateway_product_mode,
                &mut wire_evidence,
            )
            .await
            .map_err(|error| super::runtime::with_read_evidence(error, wire_evidence.clone()))?;
        transports.insert(TransportId::XmlHttp, xml_evidence.clone());
        transports.insert(
            TransportId::JsonEx,
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Unknown,
                safe_reason_code: Some("transport_not_probed".to_string()),
            },
        );
        for transport in [TransportId::TdlCompanion, TransportId::Odbc] {
            transports.insert(
                transport,
                CapabilityEvidence {
                    state: CapabilityState::Unknown,
                    confidence: EvidenceConfidence::Unknown,
                    safe_reason_code: Some("configuration_not_observed".to_string()),
                },
            );
        }

        features.insert(
            CapabilityFeatureId::EndpointReachability,
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("xml_endpoint_responded".to_string()),
            },
        );
        features.insert(
            CapabilityFeatureId::ProductAndMode,
            gateway_product_mode.capability.clone(),
        );
        let empty_company_reason = || {
            if xml_evidence.state == CapabilityState::Supported {
                "company_not_loaded".to_string()
            } else {
                xml_evidence
                    .safe_reason_code
                    .clone()
                    .unwrap_or_else(|| "company_list_not_established".to_string())
            }
        };
        let company_state = if companies.is_empty() {
            CapabilityEvidence {
                state: if xml_evidence.state == CapabilityState::Supported {
                    CapabilityState::NotConfigured
                } else {
                    CapabilityState::Unknown
                },
                confidence: xml_evidence.confidence,
                safe_reason_code: Some(empty_company_reason()),
            }
        } else {
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("loaded_company_observed".to_string()),
            }
        };
        features.insert(CapabilityFeatureId::LoadedCompanies, company_state);
        let identity_evidence = if companies.is_empty() {
            CapabilityEvidence {
                state: if xml_evidence.state == CapabilityState::Supported {
                    CapabilityState::NotConfigured
                } else {
                    CapabilityState::Unknown
                },
                confidence: xml_evidence.confidence,
                safe_reason_code: Some(empty_company_reason()),
            }
        } else if has_presentation_equivalent_guid_siblings(&companies) {
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("company_identity_display_scope_ambiguous".to_string()),
            }
        } else if unique_company_identities(&companies) {
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("stable_company_identity_observed".to_string()),
            }
        } else if companies.iter().all(has_complete_company_identity) {
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("company_identity_ambiguous".to_string()),
            }
        } else {
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("stable_company_identity_not_observed".to_string()),
            }
        };
        features.insert(
            CapabilityFeatureId::StableCompanyIdentity,
            identity_evidence,
        );
        features.insert(
            CapabilityFeatureId::EncodingBehaviour,
            self.observed_encoding_evidence(),
        );
        features.insert(
            CapabilityFeatureId::PracticalResponseLimit,
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Unknown,
                safe_reason_code: Some("practical_limit_not_measured".to_string()),
            },
        );
        features.insert(CapabilityFeatureId::CompanyRead, xml_evidence);
        for feature in [
            CapabilityFeatureId::LedgerRead,
            CapabilityFeatureId::VoucherRead,
            CapabilityFeatureId::SelectedLedgerRead,
            CapabilityFeatureId::SelectedVoucherWindowRead,
        ] {
            features.insert(
                feature,
                CapabilityEvidence {
                    state: CapabilityState::Unknown,
                    confidence: EvidenceConfidence::Unknown,
                    safe_reason_code: Some("selected_read_probe_not_run".to_string()),
                },
            );
        }
        features.insert(
            CapabilityFeatureId::Write,
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Unknown,
                safe_reason_code: Some("write_probe_not_run".to_string()),
            },
        );

        for pack in [
            CapabilityPackId::CoreAccounting,
            CapabilityPackId::IndiaTax,
            CapabilityPackId::BillsAndPayments,
            CapabilityPackId::Inventory,
        ] {
            packs.insert(
                pack,
                CapabilityEvidence {
                    state: CapabilityState::Unknown,
                    confidence: EvidenceConfidence::Unknown,
                    safe_reason_code: Some("verified_snapshot_not_run".to_string()),
                },
            );
        }

        Ok((
            TallyProbeResult {
                connection,
                companies,
                profile: CapabilityProfile {
                    // Version 4 adds observed release and licence tier, invalidating
                    // reuse of version-3 snapshots without those observations.
                    profile_version: 4,
                    product: gateway_product_mode.product,
                    release: gateway_product_mode.release,
                    license_tier: gateway_product_mode.license_tier,
                    mode: gateway_product_mode.mode,
                    transports,
                    features,
                    packs,
                },
                selected_read_scope: None,
                passport_snapshot_id: None,
            },
            wire_evidence,
        ))
    }

    /// Discovers companies through Tally's documented `Company` collection
    /// (`ReadOnlyProfile::CompanyListV2`). Unlike the legacy custom TDL
    /// report, its response is Tally's ordinary shaped `HEADER/STATUS=1`
    /// success envelope, so a successful parse directly satisfies the export
    /// trust check instead of requiring the narrower, explicitly-untrusted
    /// interactive compatibility parse.
    ///
    /// Responders that reject the collection outright — a shaped failure, an
    /// unrecognized shape, or anything else the collection parser cannot
    /// read — fall back to `legacy_company_discovery_evidence`, off the happy
    /// path but otherwise unchanged.
    async fn company_discovery_evidence(
        &self,
        connection: &mut ConnectionStatus,
        companies: &mut Vec<TallyCompany>,
        gateway_product_mode: &mut GatewayProductModeEvidence,
        wire_evidence: &mut RuntimeReadEvidence,
    ) -> anyhow::Result<CapabilityEvidence> {
        let xml = self
            .post_probe_xml(ReadOnlyProfile::CompanyListV2.render(), wire_evidence)
            .await?;
        match xml_parser::parse_companies_from_collection(&xml) {
            Ok(discovered) => {
                *gateway_product_mode = parse_company_gateway_capability_observation(&xml)
                    .map(GatewayProductModeEvidence::from_observation)
                    .unwrap_or_else(|_| GatewayProductModeEvidence::unavailable());
                connection.reachable = true;
                if connection.error.is_some() {
                    connection.error = Some("status_heuristic_unavailable".to_string());
                }
                Ok(match normalize_discovered_companies(discovered) {
                    Ok(normalized) => {
                        *companies = normalized;
                        CapabilityEvidence {
                            state: CapabilityState::Supported,
                            confidence: EvidenceConfidence::Observed,
                            safe_reason_code: None,
                        }
                    }
                    Err(()) => CapabilityEvidence {
                        state: CapabilityState::Unknown,
                        confidence: EvidenceConfidence::Observed,
                        safe_reason_code: Some("company_identity_invalid".to_string()),
                    },
                })
            }
            Err(_) => {
                self.legacy_company_discovery_evidence(connection, companies, wire_evidence)
                    .await
            }
        }
    }

    /// The pre-`CompanyListV2` company discovery path: the custom
    /// `CompanyListV1` TDL report, which most Tally responders answer with a
    /// bare `<ENVELOPE><COMPANYINFO>...` document carrying no
    /// `HEADER`/`STATUS` at all. That bare shape is accepted only through the
    /// narrow, explicitly-untrusted interactive discovery parse; it can never
    /// promote `CapabilityState::Supported`.
    async fn legacy_company_discovery_evidence(
        &self,
        connection: &mut ConnectionStatus,
        companies: &mut Vec<TallyCompany>,
        wire_evidence: &mut RuntimeReadEvidence,
    ) -> anyhow::Result<CapabilityEvidence> {
        let xml = self
            .post_probe_xml(tdl_engine::company_list_request(), wire_evidence)
            .await?;
        Ok(match xml_parser::parse_companies(&xml) {
            Ok(discovered) => {
                connection.reachable = true;
                if connection.error.is_some() {
                    connection.error = Some("status_heuristic_unavailable".to_string());
                }
                match normalize_discovered_companies(discovered) {
                    Ok(normalized) => {
                        *companies = normalized;
                        CapabilityEvidence {
                            state: CapabilityState::Supported,
                            confidence: EvidenceConfidence::Observed,
                            safe_reason_code: None,
                        }
                    }
                    Err(()) => CapabilityEvidence {
                        state: CapabilityState::Unknown,
                        confidence: EvidenceConfidence::Observed,
                        safe_reason_code: Some("company_identity_invalid".to_string()),
                    },
                }
            }
            Err(_) => match xml_parser::export_status(&xml) {
                Ok(xml_parser::TallyExportStatus::Failure) => CapabilityEvidence {
                    // A shaped failure is an endpoint claim, not responder
                    // authenticity or proof that the read profile works.
                    state: CapabilityState::Unknown,
                    confidence: EvidenceConfidence::Observed,
                    safe_reason_code: Some(
                        xml_parser::export_failure_reason_code(&xml).to_string(),
                    ),
                },
                _ if parse_companies_for_interactive_discovery(&xml).is_ok() => {
                    connection.reachable = true;
                    if connection.error.is_some() {
                        connection.error = Some("status_heuristic_unavailable".to_string());
                    }
                    CapabilityEvidence {
                        state: CapabilityState::Unknown,
                        confidence: EvidenceConfidence::Observed,
                        safe_reason_code: Some("direct_company_report_untrusted".to_string()),
                    }
                }
                _ => CapabilityEvidence {
                    state: CapabilityState::Unknown,
                    confidence: EvidenceConfidence::Observed,
                    safe_reason_code: Some("xml_export_shape_unrecognized".to_string()),
                },
            },
        })
    }

    pub(super) async fn post_probe_xml(
        &self,
        xml: String,
        evidence: &mut RuntimeReadEvidence,
    ) -> anyhow::Result<String> {
        let response = self.http.post_xml_decoded(xml).await?;
        let wire = RuntimeReadEvidence {
            request_sha256: response
                .request_body_sha256()
                .ok_or_else(|| anyhow::anyhow!("Tally POST omitted request wire commitment"))?
                .to_string(),
            response_sha256: response.encoded_sha256().to_string(),
            bytes: response.encoded_bytes(),
        };
        self.record_observed_body_bytes(response.encoded_bytes());
        self.record_observed_encoding(response.encoding());
        *evidence = evidence.clone().combine(wire);
        Ok(response.into_text())
    }

    pub(super) async fn post_xml(&self, xml: String) -> anyhow::Result<String> {
        self.post_xml_with_encoded_bytes(xml)
            .await
            .map(|(xml, _, _)| xml)
    }

    async fn post_xml_with_encoded_bytes(
        &self,
        xml: String,
    ) -> anyhow::Result<(String, usize, String)> {
        let response = self.http.post_xml_decoded(xml).await?;
        let encoded_bytes = response.encoded_bytes();
        let encoded_sha256 = response.encoded_sha256().to_string();
        self.record_observed_body_bytes(encoded_bytes);
        self.record_observed_encoding(response.encoding());
        Ok((response.into_text(), encoded_bytes, encoded_sha256))
    }

    async fn post_xml_with_request_wire_sha256(
        &self,
        xml: String,
    ) -> anyhow::Result<(String, String)> {
        let response = self.http.post_xml_decoded(xml).await?;
        let request_sha256 = response
            .request_body_sha256()
            .ok_or_else(|| anyhow::anyhow!("Tally POST omitted request wire commitment"))?
            .to_owned();
        self.record_observed_body_bytes(response.encoded_bytes());
        self.record_observed_encoding(response.encoding());
        Ok((response.into_text(), request_sha256))
    }

    #[cfg(feature = "voucher-scan")]
    async fn post_outstandings_xml_with_encoded_bytes(
        &self,
        request: VoucherOutstandingsRequestXml,
    ) -> anyhow::Result<OutstandingsWireResponse> {
        let response = self.http.post_outstandings_xml_decoded(request).await?;
        let encoded_bytes = response.encoded_bytes();
        let encoded_sha256 = response.encoded_sha256().to_string();
        self.record_observed_body_bytes(encoded_bytes);
        self.record_observed_encoding(response.encoding());
        Ok(OutstandingsWireResponse {
            text: response.into_text(),
            encoded_bytes,
            encoded_sha256,
        })
    }

    /// Uses the ordinary 32 MiB XML cap. Only the wildcard outstandings
    /// profile is allowed through `post_outstandings_xml_decoded`.
    #[cfg(feature = "voucher-scan")]
    async fn post_xml_with_wire_evidence(
        &self,
        request: String,
    ) -> anyhow::Result<OutstandingsWireResponse> {
        let response = self.http.post_xml_decoded(request).await?;
        let encoded_bytes = response.encoded_bytes();
        let encoded_sha256 = response.encoded_sha256().to_string();
        self.record_observed_body_bytes(encoded_bytes);
        self.record_observed_encoding(response.encoding());
        Ok(OutstandingsWireResponse {
            text: response.into_text(),
            encoded_bytes,
            encoded_sha256,
        })
    }

    /// Discovers companies through Tally's documented `Company` collection
    /// (`ReadOnlyProfile::CompanyListV2`) rather than the legacy `CompanyListV1`
    /// custom TDL report. Unlike the legacy report -- which one Tally instance
    /// answers with a bare, unwrapped `<COMPANYINFO>` document and another is
    /// known to simply hang on -- the collection always answers with the
    /// ordinary shaped `HEADER/STATUS=1` envelope, so `parse_companies_from_collection`
    /// can require that shape outright.
    pub async fn fetch_companies(&self) -> anyhow::Result<Vec<TallyCompany>> {
        self.fetch_companies_with_wire_evidence()
            .await
            .map(|(companies, _)| companies)
    }

    /// Enumerates the complete Company collection together with the exact
    /// request/response commitment. Write admission retains this observation
    /// instead of treating a parsed tuple as sufficient evidence.
    pub(crate) async fn fetch_companies_with_wire_evidence(
        &self,
    ) -> anyhow::Result<(Vec<TallyCompany>, RuntimeReadEvidence)> {
        self.fetch_companies_observing_education_mode()
            .await
            .map(|(companies, evidence, _)| (companies, evidence))
    }

    /// As [`Self::fetch_companies_with_wire_evidence`], also returning whether
    /// the same `CompanyListV2` response may come from an Education-mode
    /// endpoint: by the full capability observation, or by any `EDUMODE` field
    /// saying anything but `No` when that observation does not parse. No
    /// further request is made.
    pub(crate) async fn fetch_companies_observing_education_mode(
        &self,
    ) -> anyhow::Result<(Vec<TallyCompany>, RuntimeReadEvidence, bool)> {
        let mut evidence = RuntimeReadEvidence::empty();
        let xml = self
            .post_probe_xml(ReadOnlyProfile::CompanyListV2.render(), &mut evidence)
            .await?;
        let education = parse_company_gateway_capability_observation(&xml)
            .map(|observation| observation.educational_mode)
            .unwrap_or(false)
            || bridge_tally_protocol::company_list_may_be_in_educational_mode(&xml);
        let discovered = xml_parser::parse_companies_from_collection(&xml)
            .map_err(|error| with_read_evidence(error, evidence.clone()))?;
        let companies = normalize_discovered_companies(discovered).map_err(|_| {
            with_read_evidence(
                anyhow::anyhow!(
                    "Tally returned an invalid company identity for interactive discovery"
                ),
                evidence.clone(),
            )
        })?;
        Ok((companies, evidence, education))
    }

    /// Re-enumerates the trusted `Company` collection, then proves one
    /// user-chosen name with a separate shaped standard collection response.
    /// The collection's GUID is deliberately discarded; only the standard
    /// ledger identity collection's computed context may construct the
    /// returned company identity -- that binding proves Tally will actually
    /// scope subsequent reads to this exact company, which matching a name
    /// in a list can never prove by itself.
    pub async fn bootstrap_direct_company(
        &self,
        candidate_name: &str,
    ) -> anyhow::Result<TallyCompany> {
        let candidate_name = normalize_company_name(candidate_name)
            .map_err(|_| anyhow::anyhow!("Tally direct company candidate was invalid"))?;
        let discovered = self.fetch_companies().await?;
        let candidates = discovered
            .into_iter()
            .filter(|company| company.name == candidate_name)
            .collect::<Vec<_>>();
        let [candidate] = candidates.as_slice() else {
            anyhow::bail!("Tally direct company candidate was absent or ambiguous");
        };
        let xml = self
            .post_xml(tdl_engine::standard_ledger_identity_request(
                &candidate.name,
            ))
            .await?;
        let observed = parse_standard_ledger_identity_observation(&xml, &candidate.name)?;
        let guid = normalize_company_guid(&observed.company_guid)
            .map_err(|_| anyhow::anyhow!("Tally standard ledger identity was invalid"))?;
        if candidate
            .guid
            .as_deref()
            .is_none_or(|listed_guid| !listed_guid.eq_ignore_ascii_case(&guid))
        {
            return Err(DirectCompanyBootstrapError::CandidateGuidMismatch.into());
        }
        let Some(company_number) = candidate.company_number.clone() else {
            return Err(DirectCompanyBootstrapError::IncompleteTuple.into());
        };
        let Some(books_from) = candidate.books_from.clone() else {
            return Err(DirectCompanyBootstrapError::IncompleteTuple.into());
        };
        Ok(TallyCompany {
            name: candidate.name.clone(),
            guid: Some(guid),
            company_number: Some(company_number),
            books_from: Some(books_from),
        })
    }

    pub async fn fetch_ledgers(
        &self,
        identity: &VerifiedCompanyIdentity,
        boundary_profile: DateBoundaryProfile,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let opening_extent = self.fetch_company_book_extent(identity).await?;
        let period = NativeLedgerExportPeriod::new(
            boundary_profile,
            opening_extent.books_from().clone(),
            opening_extent.last_voucher_date().clone(),
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "Tally master ledger export period is not supported by the endpoint compatibility profile"
            )
        })?;
        let paired = self
            .fetch_native_report_paired(render_native_ledger_export_request(
                identity.display_name(),
                &period,
            ))
            .await?;
        let (body, _, _) =
            paired.require_stable(PairedReadValidationError::NativeLedgerCollection)?;
        let parsed =
            parse_native_ledger_source_records_with_evidence(&body, identity.company_guid())?;
        let closing_extent = self.fetch_company_book_extent(identity).await?;
        if closing_extent != opening_extent {
            return Err(anyhow::Error::new(
                PairedReadValidationError::NativeLedgerExtent,
            ));
        }
        Ok(parsed
            .records
            .into_iter()
            .map(|record| record.record)
            .collect())
    }

    /// Reads the identity-bearing ledger master and the existing period-bound
    /// balance snapshot as one bracketed source for a customer workbook. The
    /// balance parser requires row GUID evidence for the selected company
    /// before any `(name, parent)` join can attach money to a master.
    pub(crate) async fn fetch_party_ledger_master_source(
        &self,
        identity: &VerifiedCompanyIdentity,
        boundary_profile: DateBoundaryProfile,
        currency_assertion: PartyLedgerMasterCurrencyAssertion,
    ) -> anyhow::Result<PartyLedgerMasterSource> {
        let mut evidence = RuntimeReadEvidence::empty();
        let result = async {
            let opening_extent = self.fetch_company_book_extent(identity).await?;
            let currency = currency_assertion.require_opening_extent(&opening_extent)?;
            let master_period = NativeLedgerExportPeriod::new(
                boundary_profile,
                opening_extent.books_from().clone(),
                opening_extent.last_voucher_date().clone(),
            )
            .map_err(|_| {
                anyhow::Error::new(PartyLedgerMasterSourceValidationError::MasterPeriod)
            })?;
            let balance_period = party_ledger_master_balance_period(
                boundary_profile,
                opening_extent.books_from().clone(),
                opening_extent.last_voucher_date().clone(),
            )
            .map_err(|_| {
                anyhow::Error::new(PartyLedgerMasterSourceValidationError::BalancePeriod)
            })?;
            let requests = [
                render_party_ledger_master_request(identity.display_name(), &master_period),
                render_native_ledger_snapshot_request(identity.display_name(), &balance_period),
                render_native_group_snapshot_request(identity.display_name()),
            ];
            let request_sha256 = party_ledger_request_commitment(&requests);
            let [master_request, balance_request, group_request] = requests;
            let master_pair = self
                .fetch_native_report_paired(master_request.clone())
                .await?;
            let (master_body, master_response_bytes, master_response_sha256) =
                master_pair.require_stable(PairedReadValidationError::PartyLedgerMaster)?;
            evidence = evidence.clone().combine(RuntimeReadEvidence::paired(
                &master_request,
                master_response_sha256.clone(),
                master_response_bytes,
            ));
            let master = parse_native_party_ledger_master_records_with_evidence(
                &master_body,
                identity.company_guid(),
            )
            .map_err(party_ledger_master_master_snapshot_error)?;
            if !master.evidence.duplicate_identities.is_empty() {
                return Err(anyhow::Error::new(
                    PartyLedgerMasterSourceValidationError::DuplicateMasterIdentity,
                ));
            }
            let balance_pair = self
                .fetch_native_report_paired(balance_request.clone())
                .await?;
            let (balance_body, balance_response_bytes, balance_response_sha256) =
                balance_pair.require_stable(PairedReadValidationError::PartyLedgerBalance)?;
            evidence = evidence.clone().combine(RuntimeReadEvidence::paired(
                &balance_request,
                balance_response_sha256.clone(),
                balance_response_bytes,
            ));
            let balances =
                parse_native_ledger_snapshot_for_company(&balance_body, identity.company_guid())
                    .map_err(party_ledger_master_balance_snapshot_error)?;
            let group_pair = self
                .fetch_native_report_paired(group_request.clone())
                .await?;
            let (group_body, group_response_bytes, group_response_sha256) =
                group_pair.require_stable(PairedReadValidationError::PartyLedgerGroup)?;
            evidence = evidence.clone().combine(RuntimeReadEvidence::paired(
                &group_request,
                group_response_sha256.clone(),
                group_response_bytes,
            ));
            let groups =
                parse_native_group_snapshot_with_evidence(&group_body, identity.company_guid())
                    .map_err(party_ledger_master_group_snapshot_error)?
                    .into_iter()
                    .map(|entry| entry.record)
                    .collect();
            let closing_extent = self.fetch_company_book_extent(identity).await?;
            if closing_extent != opening_extent {
                return Err(anyhow::Error::new(
                    PairedReadValidationError::PartyLedgerExtent,
                ));
            }

            let mut balances_by_key = HashMap::new();
            for balance in balances {
                let key = ledger_display_key(&balance.name, balance.parent.as_deref());
                if balances_by_key.insert(key, balance).is_some() {
                    return Err(anyhow::Error::new(
                        PartyLedgerMasterSourceValidationError::DuplicateBalanceDisplayKey,
                    ));
                }
            }
            let mut rows = Vec::with_capacity(master.records.len());
            for source in master.records {
                let key = ledger_display_key(
                    &source.record.ledger.name,
                    source.record.ledger.parent.nonempty_returned_text(),
                );
                let balance = balances_by_key.remove(&key).ok_or_else(|| {
                    anyhow::Error::new(
                        PartyLedgerMasterSourceValidationError::BalanceMissingMasterLedger,
                    )
                })?;
                let guid = source.identities.guid.ok_or_else(|| {
                    anyhow::Error::new(PartyLedgerMasterSourceValidationError::MasterGuid)
                })?;
                let master_id = source.identities.master_id.ok_or_else(|| {
                    anyhow::Error::new(PartyLedgerMasterSourceValidationError::MasterId)
                })?;
                let alter_id = source.alter_id.ok_or_else(|| {
                    anyhow::Error::new(PartyLedgerMasterSourceValidationError::MasterAlterId)
                })?;
                let master_opening =
                    source
                        .record
                        .ledger
                        .opening_balance
                        .as_deref()
                        .ok_or_else(|| {
                            anyhow::Error::new(
                                PartyLedgerMasterSourceValidationError::MasterOpeningBalance,
                            )
                        })?;
                if !party_ledger_master_openings_agree(master_opening, &balance.opening_balance)? {
                    return Err(anyhow::Error::new(
                        PartyLedgerMasterSourceValidationError::OpeningBalancesDisagreed,
                    ));
                }
                rows.push(PartyLedgerMasterRow {
                    name: source.record.ledger.name,
                    parent: source.record.ledger.parent,
                    party_gstin: source.record.ledger.party_gstin,
                    fields: source.record.fields,
                    guid,
                    master_id,
                    alter_id,
                    opening_balance: balance.opening_balance,
                    closing_balance: balance.closing_balance,
                });
            }
            if !balances_by_key.is_empty() {
                return Err(anyhow::Error::new(
                    PartyLedgerMasterSourceValidationError::BalanceLedgerAbsentFromMasterEvidence,
                ));
            }
            rows.sort_by(|left, right| left.name.cmp(&right.name).then(left.guid.cmp(&right.guid)));
            Ok(PartyLedgerMasterSource {
                company: identity.display_name().to_string(),
                company_guid: identity.company_guid().to_string(),
                currency_assertion: currency.assertion,
                currency_decimal_places: currency.decimal_places,
                from: master_period.from().clone(),
                // The snapshot period is the balance evidence. Its derived end is
                // the date Tally was actually asked to honor, not merely the last
                // voucher date used by the identity/master read.
                to: balance_period.to().clone(),
                rows,
                request_sha256,
                master_response_sha256,
                balance_response_sha256,
                group_response_sha256,
                master_response_bytes,
                balance_response_bytes,
                group_response_bytes,
                groups,
            })
        }
        .await;
        result.map_err(|error| crate::tally::runtime::with_read_evidence(error, evidence))
    }

    /// Reads the documented standard ledger collection as an explicitly limited
    /// compatibility catalog. It is not a fallback for Bridge's custom export
    /// and cannot establish snapshot, voucher, or write capability.
    pub async fn fetch_standard_ledger_catalog(
        &self,
        company: &str,
        expected_company_guid: &str,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let xml = self
            .post_xml(tdl_engine::standard_ledger_catalog_request(company))
            .await?;
        Ok(parse_standard_ledger_catalog(
            &xml,
            company,
            expected_company_guid,
        )?)
    }

    /// One extra paired read per scan: bill-wise OPENING balances live on
    /// ledger masters, so a voucher-only scan is blind to them.
    ///
    /// Takes the already GUID-verified `PinnedCompany` rather than a bare name.
    /// The ledger profile fetches every master GUID and verifies its company
    /// GUID prefix, so a name-only selection cannot make another loaded
    /// company's coverage look like the pinned book.
    #[cfg(feature = "voucher-scan")]
    pub(crate) async fn fetch_ledger_opening_coverage(
        &self,
        company: &PinnedCompany,
    ) -> anyhow::Result<LedgerOpeningCoverageRead> {
        let company_name = ValidatedCompanyName::new(company.name().to_string())?;
        let request = ReadOnlyProfile::LedgerOpeningCoverageV1 {
            company: &company_name,
        }
        .render();
        let first = self.post_xml(request.clone()).await?;
        self.http
            .get_status_decoded()
            .await
            .context("Tally health check between ledger opening reads failed")?;
        let second = self.post_xml(request).await?;
        self.http
            .get_status_decoded()
            .await
            .context("Tally health check after ledger opening reads failed")?;
        let first = parse_ledger_opening_coverage(&first, company)?;
        let second = parse_ledger_opening_coverage(&second, company)?;
        if first != second {
            return Ok(LedgerOpeningCoverageRead::Drifted);
        }
        Ok(LedgerOpeningCoverageRead::Stable(first))
    }

    pub async fn fetch_company_book_extent(
        &self,
        identity: &VerifiedCompanyIdentity,
    ) -> anyhow::Result<CompanyBookExtent> {
        let expectation = identity.company_book_extent_expectation()?;
        let company_name = ValidatedCompanyName::new(identity.display_name().to_owned())?;
        let request = ReadOnlyProfile::CompanyBookExtentV2 {
            company: &company_name,
        }
        .render();
        let first = self.post_xml(request.clone()).await?;
        self.http
            .get_status_decoded()
            .await
            .context("Tally health check between company extent reads failed")?;
        let second = self.post_xml(request).await?;
        self.http
            .get_status_decoded()
            .await
            .context("Tally health check after company extent reads failed")?;
        let first = parse_company_book_extent_v2(&first, &expectation)?;
        let second = parse_company_book_extent_v2(&second, &expectation)?;
        if first != second {
            return Err(anyhow::Error::new(
                PairedReadValidationError::CompanyBookExtent,
            ));
        }
        // The parser stays tolerant of an absent ALTMSTID (older captures still parse), but this
        // is the outstandings bracket itself: fail closed here so a witness-less pair -- which
        // would otherwise compare equal regardless of a mid-window master edit -- can never be
        // mistaken for a stable one. See `require_master_witness` for why.
        require_master_witness(&first)?;
        Ok(first)
    }

    /// Paired read for the native `TYPE=Data` bills reports and the ledger
    /// closing snapshot.
    ///
    /// These responses are small — measured 11 KB for 48 bills and 41 KB for 88
    /// ledgers — so the whole-response byte comparison this performs is cheap,
    /// and it replaces the date/AlterID partition-completeness machinery the
    /// voucher scan needs. A drift between the two reads means the book moved
    /// mid-sequence; the caller must treat that as Partial rather than pick a
    /// side.
    ///
    /// Health checks bracket both requests and sit between them, so a gateway
    /// that stalls mid-pair is distinguishable from a clean pair. See the
    /// identical discipline in `fetch_company_book_extent`.
    pub(crate) async fn fetch_native_report_paired(
        &self,
        request_xml: String,
    ) -> anyhow::Result<NativePairedRead> {
        match self
            .fetch_native_report_paired_with_evidence(request_xml)
            .await
        {
            Ok((body, encoded_bytes, encoded_sha256)) => Ok(NativePairedRead::Stable {
                body,
                encoded_bytes,
                encoded_sha256,
            }),
            // Preserve existing financial callers' explicit Partial verdict.
            Err(error)
                if error
                    .chain()
                    .any(|cause| cause.is::<NativeReportPairDrift>()) =>
            {
                let failure = error.downcast::<super::runtime::RuntimeReadFailure>()?;
                Ok(NativePairedRead::Drifted(failure.evidence))
            }
            Err(error) => Err(error),
        }
    }

    // Both adapters and the financial verdict wrapper retain completed source
    // commitments on pair drift; the wrapper preserves its Partial classification.
    pub(crate) async fn fetch_native_report_paired_with_evidence(
        &self,
        request_xml: String,
    ) -> anyhow::Result<(String, usize, String)> {
        let (first, first_bytes, first_sha256) = self
            .post_xml_with_encoded_bytes(request_xml.clone())
            .await
            .map_err(|error| anyhow::Error::new(PairedNativeReportResponseFailure::new(error)))?;
        let mut evidence =
            RuntimeReadEvidence::single(&request_xml, first_sha256.clone(), first_bytes);
        let result = async {
            self.http
                .get_status_decoded()
                .await
                .context("Tally health check between paired native report reads failed")?;
            let (second, second_bytes, second_sha256) = self
                .post_xml_with_encoded_bytes(request_xml.clone())
                .await
                .map_err(|error| {
                    anyhow::Error::new(PairedNativeReportResponseFailure::new(error))
                })?;
            if first_bytes == second_bytes && first_sha256 == second_sha256 {
                evidence.bytes = evidence.bytes.saturating_add(second_bytes);
            } else {
                evidence = evidence.clone().combine(RuntimeReadEvidence::single(
                    &request_xml,
                    second_sha256.clone(),
                    second_bytes,
                ));
            }
            self.http
                .get_status_decoded()
                .await
                .context("Tally health check after paired native report reads failed")?;
            if first != second || first_bytes != second_bytes || first_sha256 != second_sha256 {
                return Err(NativeReportPairDrift.into());
            }
            Ok((first, first_bytes, first_sha256))
        }
        .await;
        result.map_err(|error| super::runtime::with_read_evidence(error, evidence))
    }

    /// One XML POST whose HTTP response entity is kept byte for byte, for
    /// audit_read, which seals those bytes rather than a re-encoding of the
    /// decoded text. The entity is what the transport received after chunked
    /// framing is removed; content encodings other than identity are refused.
    /// The decoded text is returned beside it for admission.
    pub(crate) async fn post_xml_raw(&self, xml: String) -> anyhow::Result<RawTallyResponse> {
        let response = self.http.post_xml(xml).await?;
        self.record_observed_body_bytes(response.encoded_bytes());
        self.record_observed_encoding(response.encoding());
        let encoded_body = response.encoded_body().to_vec();
        let encoded_sha256 = sha256_hex(&encoded_body);
        Ok(RawTallyResponse {
            text: response.into_text(),
            encoded_body,
            encoded_sha256,
        })
    }

    /// [`Self::post_xml_raw`] twice, with the same health checks as
    /// [`Self::fetch_native_report_paired_with_evidence`], admitted only when
    /// the two response entities are byte-identical. On drift both completed
    /// bodies are accounted for and neither is released, and the refusal is
    /// returned before the trailing health check, so a drift is never reported
    /// as that check's failure.
    pub(crate) async fn post_xml_raw_paired(
        &self,
        xml: String,
    ) -> anyhow::Result<RawTallyResponse> {
        let first = self.post_xml_raw(xml.clone()).await?;
        let mut evidence = RuntimeReadEvidence::single(
            &xml,
            first.encoded_sha256.clone(),
            first.encoded_body.len(),
        );
        let result = async {
            self.http
                .get_status_decoded()
                .await
                .context("Tally health check between paired audit reads failed")?;
            let second = self.post_xml_raw(xml.clone()).await?;
            if second.encoded_body == first.encoded_body {
                evidence.bytes = evidence.bytes.saturating_add(second.encoded_body.len());
            } else {
                evidence = evidence.clone().combine(RuntimeReadEvidence::single(
                    &xml,
                    second.encoded_sha256.clone(),
                    second.encoded_body.len(),
                ));
            }
            if second.encoded_body != first.encoded_body {
                return Err(NativeReportPairDrift.into());
            }
            self.http
                .get_status_decoded()
                .await
                .context("Tally health check after paired audit reads failed")?;
            Ok(())
        }
        .await;
        result.map_err(|error| super::runtime::with_read_evidence(error, evidence))?;
        Ok(first)
    }

    /// The smallest request Tally answers: its status page. Used to learn
    /// whether a responder that was left building an abandoned response has
    /// finished; it proves nothing else.
    pub(crate) async fn status_probe(&self) -> anyhow::Result<()> {
        self.http.get_status_decoded().await?;
        Ok(())
    }

    #[cfg(feature = "voucher-scan")]
    pub(crate) async fn fetch_outstandings_segment_pair(
        &self,
        company: &PinnedCompany,
        segment_window: NarrowDateWindow,
        alter_id_range: AlterIdRange,
    ) -> anyhow::Result<OutstandingsSegmentObservation> {
        let request = voucher_outstandings_request(company, &segment_window, alter_id_range);
        let range_label = format!(
            "{}..{}",
            alter_id_range.exclusive_start(),
            alter_id_range.inclusive_end()
        );
        let first_started = Instant::now();
        let first = self
            .post_outstandings_xml_with_encoded_bytes(request.clone())
            .await
            .with_context(|| {
                format!("outstandings first segment read failed for AlterID {range_label}")
            })?;
        let first_read_elapsed = first_started.elapsed();
        self.http.get_status_decoded().await.with_context(|| {
            format!(
                "Tally health check between outstandings reads failed for AlterID {range_label}"
            )
        })?;
        let second_started = Instant::now();
        let second = self
            .post_outstandings_xml_with_encoded_bytes(request)
            .await
            .with_context(|| {
                format!("outstandings second segment read failed for AlterID {range_label}")
            })?;
        let second_read_elapsed = second_started.elapsed();
        self.http.get_status_decoded().await.with_context(|| {
            format!("Tally health check after outstandings reads failed for AlterID {range_label}")
        })?;
        let verification = verify_segment_pair_with_wire_evidence(
            SegmentWireEvidence::new(&first.text, first.encoded_bytes, &first.encoded_sha256),
            SegmentWireEvidence::new(&second.text, second.encoded_bytes, &second.encoded_sha256),
            company,
            segment_window.into_date_window(),
            alter_id_range,
        )?;
        Ok(OutstandingsSegmentObservation {
            verification,
            first_read_elapsed,
            second_read_elapsed,
        })
    }

    /// Executes one paired, date-only I5 witness read. This is intentionally
    /// separate from `fetch_outstandings_segment_pair`: it has no AlterID
    /// predicate and uses the ordinary 32 MiB transport cap. Its supervised
    /// live qualification is recorded in TALLY_PROTOCOL_REFERENCE.md §12.7;
    /// runtime may use it only for a primary-empty partition's control or
    /// shifted cover.
    #[cfg(feature = "voucher-scan")]
    pub(crate) async fn fetch_empty_partition_witness_pair(
        &self,
        company: &PinnedCompany,
        window: NarrowDateWindow,
    ) -> anyhow::Result<WitnessPairVerification> {
        let request = voucher_empty_partition_witness_request(company, &window).into_xml();
        let label = format!("{}..{}", window.from().as_str(), window.to().as_str());
        let first = self
            .post_xml_with_wire_evidence(request.clone())
            .await
            .with_context(|| format!("empty-date witness first read failed for {label}"))?;
        self.http.get_status_decoded().await.with_context(|| {
            format!("Tally health check between empty-date witness reads for {label}")
        })?;
        let second = self
            .post_xml_with_wire_evidence(request)
            .await
            .with_context(|| format!("empty-date witness second read failed for {label}"))?;
        self.http.get_status_decoded().await.with_context(|| {
            format!("Tally health check after empty-date witness reads for {label}")
        })?;
        verify_empty_partition_witness_pair_with_wire_evidence(
            SegmentWireEvidence::new(&first.text, first.encoded_bytes, &first.encoded_sha256),
            SegmentWireEvidence::new(&second.text, second.encoded_bytes, &second.encoded_sha256),
            company,
            window.into_date_window(),
        )
        .map_err(anyhow::Error::from)
    }

    pub async fn qualify_selected_ledgers(
        &self,
        company: &str,
        expected_company_guid: &str,
    ) -> anyhow::Result<SelectedReadObservation> {
        let request = tdl_engine::ledgers_request(company);
        let (xml, request_sha256) = self.post_xml_with_request_wire_sha256(request).await?;
        let decoded_response_sha256 = sha256_hex(xml.as_bytes());
        bridge_tally_protocol::validate_exact_selected_export_structure(&xml, "LEDGER")?;
        let parsed = parse_ledger_source_records_with_evidence(&xml)?;
        xml_parser::verify_company_context(&parsed.evidence, expected_company_guid)?;
        verify_selected_company_name(&parsed.evidence, company)?;
        validate_selected_read_identity_evidence(
            parsed.records.len(),
            parsed.evidence.identified_record_count,
            parsed.evidence.duplicate_identities.len(),
        )?;
        validate_selected_ledgers(&parsed.records)?;
        Ok(SelectedReadObservation {
            request_sha256,
            decoded_response_sha256,
            response_encoding: self.observed_encoding_label()?,
            result_bucket: if parsed.records.is_empty() {
                "empty_observed"
            } else {
                "non_empty_observed"
            },
        })
    }

    pub async fn fetch_vouchers(
        &self,
        identity: &VerifiedCompanyIdentity,
        from: &str,
        to: &str,
    ) -> anyhow::Result<Vec<TallyVoucher>> {
        // Fail closed: `from`/`to` feed a quoted `$$Date:"..."` TDL formula
        // argument, where XML escaping alone cannot contain an embedded
        // quote (Tally decodes `&quot;` back to `"` before evaluating the
        // formula). Requiring a validated `TallyDate` -- exactly 8 ASCII
        // digits -- closes that off at the source instead of sanitising.
        let from = bridge_tally_core::TallyDate::parse(from)
            .context("voucher export from-date must be a valid YYYYMMDD date")?;
        let to = bridge_tally_core::TallyDate::parse(to)
            .context("voucher export to-date must be a valid YYYYMMDD date")?;
        let xml = self
            .post_xml(render_native_voucher_export_request(
                identity.display_name(),
                &from,
                &to,
            ))
            .await?;
        let parsed =
            parse_native_voucher_source_records_with_evidence(&xml, identity.company_guid())?;
        if parsed.records.is_empty() {
            // A native Voucher collection carries no envelope company GUID,
            // so a zero-row response has no per-row identity to bind to the
            // pinned company either -- `parse_native_voucher_source_records_with_evidence`
            // accepts it unauthenticated. Since the voucher request is now
            // filtered by date, a refused period boundary yields the exact
            // same zero-row, byte-identical response as a genuinely empty
            // window. Confirm the pinned company out-of-band with the same
            // GUID-verified, paired book-extent bracket the core window uses
            // for exactly this situation (see `RuntimeTallyConnector::extract_core_window`
            // in connector.rs), instead of accepting the empty result as-is.
            // Paid only here: a non-empty response keeps its existing
            // row-GUID binding and issues no extra request.
            self.fetch_company_book_extent(identity).await.context(
                "empty voucher response could not confirm the pinned company book extent",
            )?;
        }
        Ok(parsed
            .records
            .into_iter()
            .map(|record| record.record)
            .collect())
    }

    pub async fn qualify_selected_vouchers(
        &self,
        company: &str,
        expected_company_guid: &str,
        from: &str,
        to: &str,
    ) -> anyhow::Result<SelectedReadObservation> {
        let request = tdl_engine::selected_vouchers_request(company, from, to);
        let (xml, request_sha256) = self.post_xml_with_request_wire_sha256(request).await?;
        let decoded_response_sha256 = sha256_hex(xml.as_bytes());
        bridge_tally_protocol::validate_exact_selected_export_structure(&xml, "VOUCHER")?;
        let parsed = parse_selected_voucher_source_records_with_evidence(&xml)?;
        xml_parser::verify_company_context(&parsed.evidence, expected_company_guid)?;
        verify_selected_company_name(&parsed.evidence, company)?;
        verify_selected_voucher_window_context(&parsed.evidence, from, to)?;
        validate_selected_read_identity_evidence(
            parsed.records.len(),
            parsed.evidence.identified_record_count,
            parsed.evidence.duplicate_identities.len(),
        )?;
        crate::tally::canonical_window::validate_selected_voucher_window(from, to, &parsed)
            .map_err(anyhow::Error::new)?;
        Ok(SelectedReadObservation {
            request_sha256,
            decoded_response_sha256,
            response_encoding: self.observed_encoding_label()?,
            result_bucket: if parsed.records.is_empty() {
                "empty_observed"
            } else {
                "non_empty_observed"
            },
        })
    }

    pub(crate) fn reset_observed_body_bytes(&self) {
        self.observed_body_bytes
            .store(BODY_BYTES_UNAVAILABLE, Ordering::Release);
    }

    pub(crate) fn observed_body_bytes(&self) -> Option<u64> {
        match self.observed_body_bytes.load(Ordering::Acquire) {
            BODY_BYTES_UNAVAILABLE => None,
            bytes => Some(bytes),
        }
    }

    fn record_observed_body_bytes(&self, bytes: usize) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX - 1);
        let _ = self.observed_body_bytes.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |observed| {
                Some(if observed == BODY_BYTES_UNAVAILABLE {
                    bytes
                } else {
                    observed.max(bytes)
                })
            },
        );
    }

    fn record_observed_encoding(&self, encoding: TallyTextEncoding) {
        let value = match encoding {
            TallyTextEncoding::Utf8 => ENCODING_UTF8,
            TallyTextEncoding::Utf8Bom => ENCODING_UTF8_BOM,
            TallyTextEncoding::Utf16Le => ENCODING_UTF16_LE,
            TallyTextEncoding::Utf16LeBom => ENCODING_UTF16_LE_BOM,
            TallyTextEncoding::Utf16BeBom => ENCODING_UTF16_BE_BOM,
        };
        self.observed_encoding.store(value, Ordering::Release);
    }

    fn observed_encoding_evidence(&self) -> CapabilityEvidence {
        let reason = match self.observed_encoding.load(Ordering::Acquire) {
            ENCODING_UTF8 => "utf8_observed",
            ENCODING_UTF8_BOM => "utf8_bom_observed",
            ENCODING_UTF16_LE => "utf16_le_observed",
            ENCODING_UTF16_LE_BOM => "utf16_le_bom_observed",
            ENCODING_UTF16_BE_BOM => "utf16_be_bom_observed",
            _ => {
                return CapabilityEvidence {
                    state: CapabilityState::Unknown,
                    confidence: EvidenceConfidence::Unknown,
                    safe_reason_code: Some("encoding_not_observed".to_string()),
                };
            }
        };
        CapabilityEvidence {
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
            safe_reason_code: Some(reason.to_string()),
        }
    }

    fn observed_encoding_label(&self) -> anyhow::Result<&'static str> {
        match self.observed_encoding.load(Ordering::Acquire) {
            ENCODING_UTF8 => Ok("utf8"),
            ENCODING_UTF8_BOM => Ok("utf8_bom"),
            ENCODING_UTF16_LE => Ok("utf16le"),
            ENCODING_UTF16_LE_BOM => Ok("utf16le_bom"),
            ENCODING_UTF16_BE_BOM => Ok("utf16be_bom"),
            _ => anyhow::bail!("response_encoding_not_observed"),
        }
    }
}

fn party_ledger_master_balance_period(
    boundary_profile: DateBoundaryProfile,
    books_from: bridge_tally_core::TallyDate,
    last_voucher_date: bridge_tally_core::TallyDate,
) -> Result<
    NativeLedgerSnapshotPeriod,
    bridge_tally_protocol::native_outstandings::NativeLedgerSnapshotPeriodError,
> {
    // This workbook must be safe when a capability profile is not cached.
    // The strict Education boundary set is the known common admissible set;
    // choosing the next such date includes the final voucher rather than
    // silently requesting a refused boundary or shrinking the period.
    let closing_boundary = DateBoundaryProfile::EducationRestricted
        .earliest_boundary_at_or_after(&last_voucher_date)
        .ok_or(
            bridge_tally_protocol::native_outstandings::NativeLedgerSnapshotPeriodError::UnsupportedBoundary,
        )?;
    NativeLedgerSnapshotPeriod::new(boundary_profile, books_from, closing_boundary)
}

fn normalize_discovered_companies(companies: Vec<TallyCompany>) -> Result<Vec<TallyCompany>, ()> {
    companies
        .into_iter()
        .map(|company| {
            let name = normalize_company_name(&company.name).map_err(|_| ())?;
            let guid = company
                .guid
                .as_deref()
                .map(normalize_company_guid)
                .transpose()
                .map_err(|_| ())?;
            let company_number = company
                .company_number
                .as_deref()
                .map(normalize_company_number)
                .transpose()
                .map_err(|_| ())?;
            let books_from = company
                .books_from
                .as_deref()
                .map(normalize_books_from)
                .transpose()
                .map_err(|_| ())?;
            Ok(TallyCompany {
                name,
                guid,
                company_number,
                books_from,
            })
        })
        .collect()
}

fn normalize_company_number(value: &str) -> Result<String, ()> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 16
        || !trimmed.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(());
    }
    Ok(trimmed.to_string())
}

fn normalize_books_from(value: &str) -> Result<String, ()> {
    let trimmed = value.trim();
    bridge_tally_core::TallyDate::parse(trimmed)
        .map(|_| trimmed.to_string())
        .map_err(|_| ())
}

fn has_complete_company_identity(company: &TallyCompany) -> bool {
    company
        .guid
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        && company
            .company_number
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && company
            .books_from
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && !company.name.is_empty()
}

fn unique_company_identities(companies: &[TallyCompany]) -> bool {
    let mut seen = BTreeSet::new();
    companies.iter().all(|company| {
        let (Some(guid), Some(company_number), Some(books_from)) = (
            company.guid.as_deref(),
            company.company_number.as_deref(),
            company.books_from.as_deref(),
        ) else {
            return false;
        };
        seen.insert((
            guid.to_ascii_lowercase(),
            company_number.to_string(),
            company.name.clone(),
            books_from.to_string(),
        ))
    })
}

/// Tally scopes reads by display name, so presentation-equivalent same-GUID
/// books with distinct observed tuples cannot be safely selected.
fn has_presentation_equivalent_guid_siblings(companies: &[TallyCompany]) -> bool {
    companies.iter().enumerate().any(|(index, company)| {
        let Some(guid) = company.guid.as_deref() else {
            return false;
        };
        companies[..index].iter().any(|other| {
            other
                .guid
                .as_deref()
                .is_some_and(|other_guid| other_guid.eq_ignore_ascii_case(guid))
                && company.name.trim().eq_ignore_ascii_case(other.name.trim())
                && (company.name != other.name
                    || company.company_number != other.company_number
                    || company.books_from != other.books_from)
        })
    })
}

fn validate_selected_read_identity_evidence(
    parsed_record_count: usize,
    identified_record_count: u64,
    duplicate_identity_count: usize,
) -> anyhow::Result<()> {
    let parsed_record_count = u64::try_from(parsed_record_count)
        .map_err(|_| anyhow::anyhow!("Selected Tally read exceeded the supported record count"))?;
    if identified_record_count != parsed_record_count {
        anyhow::bail!("Selected Tally read omitted stable record identity");
    }
    if duplicate_identity_count != 0 {
        anyhow::bail!("Selected Tally read repeated stable record identity");
    }
    Ok(())
}

fn validate_selected_ledgers(
    records: &[bridge_tally_protocol::ParsedSourceRecord<TallyLedger>],
) -> anyhow::Result<()> {
    let mut names = BTreeSet::new();
    for source in records {
        let source_id = source
            .source_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Selected ledger omitted stable identity"))?;
        if source.identity_kind.is_none() {
            anyhow::bail!("Selected ledger omitted identity kind");
        }
        bridge_tally_core::SourceRecordId::parse(source_id.clone())?;
        bridge_tally_core::RawSourceSha256::parse(source.raw_source_sha256.clone())?;
        if let Some(alter_id) = &source.alter_id {
            bridge_tally_core::SourceAlterId::parse(alter_id.clone())?;
        }
        let name = bridge_tally_core::ForeignText::from_tally(source.record.name.clone());
        if !names.insert(name.as_str().to_string()) {
            anyhow::bail!("Selected ledger response repeated a normalized name");
        }
        let party_gstin = match &source.record.party_gstin {
            bridge_tally_protocol::PartyLedgerMasterFieldObservation::Returned(value)
                if !value.trim().is_empty() =>
            {
                Some(value)
            }
            bridge_tally_protocol::PartyLedgerMasterFieldObservation::Returned(_)
            | bridge_tally_protocol::PartyLedgerMasterFieldObservation::NotObserved => None,
        };
        for value in [
            source.record.parent.nonempty_returned_text(),
            party_gstin.map(String::as_str),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        {
            bridge_tally_core::ForeignText::from_tally(value);
        }
        if let Some(opening_balance) = source
            .record
            .opening_balance
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            bridge_tally_core::ExactDecimal::parse(opening_balance.clone())?;
        }
    }
    Ok(())
}

fn verify_selected_company_name(
    evidence: &bridge_tally_protocol::ExportEvidence,
    expected_name: &str,
) -> anyhow::Result<()> {
    let actual_name = evidence
        .company_context
        .as_ref()
        .and_then(|context| context.name.as_deref())
        .ok_or_else(|| anyhow::anyhow!("Selected Tally read omitted company name context"))?;
    let actual_name = normalize_company_name(actual_name).map_err(anyhow::Error::msg)?;
    let expected_name = normalize_company_name(expected_name).map_err(anyhow::Error::msg)?;
    if actual_name != expected_name {
        anyhow::bail!("Selected Tally read company name context did not match the request");
    }
    Ok(())
}

fn party_ledger_request_commitment(requests: &[String; 3]) -> String {
    let hashes = requests
        .iter()
        .map(|request| {
            sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
                request,
            ))
        })
        .collect::<Vec<_>>();
    sha256_hex(hashes.join(":").as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn safe_connection_failure_code(error: &anyhow::Error) -> &'static str {
    if let Some(transport) = error.downcast_ref::<TallyTransportError>() {
        return transport.safe_code();
    }
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("cancel") {
        "request_cancelled"
    } else if message.contains("queue deadline") {
        "endpoint_queue_deadline_exceeded"
    } else if message.contains("circuit") {
        "endpoint_circuit_open"
    } else if message.contains("response exceeded") {
        "response_size_limit_exceeded"
    } else if message.contains("decode") || message.contains("utf") {
        "response_encoding_invalid"
    } else {
        "endpoint_unreachable"
    }
}

pub(crate) fn canonical_loopback_origin(config: &TallyConfig) -> anyhow::Result<String> {
    Ok(transport_canonical_origin(config)?)
}

#[cfg(test)]
fn tally_endpoint(config: &TallyConfig, path: &str) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&canonical_loopback_origin(config)?)?;
    url.set_path(path);
    Ok(url)
}

#[cfg(test)]
fn decode_xml_bytes(bytes: Vec<u8>) -> anyhow::Result<String> {
    bridge_tally_protocol::decode_xml_bytes(bytes)
}

fn detect_product(text: &str) -> TallyProduct {
    let trimmed = text.trim();
    let marker = |expected: &str| {
        trimmed.eq_ignore_ascii_case(expected)
            || trimmed.eq_ignore_ascii_case(&format!("<RESPONSE>{expected}</RESPONSE>"))
    };
    if marker("TallyPrime Server is Running") {
        TallyProduct::TallyPrime
    } else if marker("Tally ERP 9 Server is Running") || marker("Tally.ERP 9 Server is Running") {
        TallyProduct::TallyErp9
    } else {
        TallyProduct::Unknown
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
