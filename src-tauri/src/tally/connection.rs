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
};
use crate::reports::party_ledger_master::{
    PartyLedgerMasterGroup, PartyLedgerMasterRow, PartyLedgerMasterSource,
};
use crate::tally::runtime::PartyLedgerMasterCurrencyAssertion;
use bridge_tally_core::{
    CapabilityEvidence, CapabilityFeatureId, CapabilityPackId, CapabilityProfile, CapabilityState,
    EvidenceConfidence, TransportId,
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
        parse_native_group_snapshot_with_evidence, parse_native_ledger_snapshot,
        render_native_group_snapshot_request, render_native_ledger_export_request,
        render_native_ledger_snapshot_request, render_native_voucher_export_request,
        render_party_ledger_master_request, NativeLedgerExportPeriod, NativeLedgerSnapshotPeriod,
    },
    outstandings_shared::{
        parse_company_book_extent, require_master_witness, CompanyBookExtent, DateBoundaryProfile,
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
    #[error("Tally company book changed during currency detection")]
    CurrencyExtent,
    #[error("Tally company changed between the currency read and the master read")]
    CurrencyToMasterExtent,
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

/// Outcome of a paired native-report read. `Drifted` means the two reads
/// disagreed, so the book moved between them and no total may be reported.
pub(crate) enum NativePairedRead {
    Stable {
        body: String,
        encoded_bytes: usize,
        encoded_sha256: String,
    },
    Drifted,
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
    mode: Option<String>,
    capability: CapabilityEvidence,
}

impl GatewayProductModeEvidence {
    fn unavailable() -> Self {
        Self {
            product: "Unknown".to_string(),
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
        // `IsEducationalMode=Yes` has not been captured from a live Education
        // instance: that inference is the complement of the observed licensed
        // response (`No`, `IsSilver=Yes`), not a second live observation.
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
        Self {
            product: observation.product,
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
        let response = self.http.get_status_decoded().await?;
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
        Ok(ConnectionStatus {
            reachable: true,
            compatible,
            product,
            server_text: server_text.to_string(),
            error: None,
        })
    }

    pub async fn probe(&self) -> anyhow::Result<TallyProbeResult> {
        // `/status` is useful local diagnostics but is not part of Tally's
        // documented third-party XML contract. Never gate the POST probe or
        // authoritative product metadata on this unauthenticated heuristic.
        let mut connection = self.check_connection().await?;
        let mut transports = BTreeMap::new();
        let mut features = BTreeMap::new();
        let mut packs = BTreeMap::new();
        let mut companies = Vec::new();

        let mut gateway_product_mode = GatewayProductModeEvidence::unavailable();
        let xml_evidence = self
            .company_discovery_evidence(&mut connection, &mut companies, &mut gateway_product_mode)
            .await?;
        transports.insert(TransportId::XmlHttp, xml_evidence.clone());
        transports.insert(
            TransportId::JsonEx,
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Unknown,
                safe_reason_code: Some("release_not_observed".to_string()),
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

        Ok(TallyProbeResult {
            connection,
            companies,
            profile: CapabilityProfile {
                // Version 3 adds observed gateway product/mode evidence. It
                // intentionally invalidates persisted version-2 snapshots,
                // whose literal Unknown/None values had a weaker meaning.
                profile_version: 3,
                product: gateway_product_mode.product,
                release: None,
                mode: gateway_product_mode.mode,
                transports,
                features,
                packs,
            },
            selected_read_scope: None,
            passport_snapshot_id: None,
        })
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
    ) -> anyhow::Result<CapabilityEvidence> {
        let xml = self
            .post_xml(ReadOnlyProfile::CompanyListV2.render())
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
                self.legacy_company_discovery_evidence(connection, companies)
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
    ) -> anyhow::Result<CapabilityEvidence> {
        let xml = self.post_xml(tdl_engine::company_list_request()).await?;
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
        let xml = self
            .post_xml(ReadOnlyProfile::CompanyListV2.render())
            .await?;
        let companies = xml_parser::parse_companies_from_collection(&xml)?;
        normalize_discovered_companies(companies).map_err(|_| {
            anyhow::anyhow!("Tally returned an invalid company identity for interactive discovery")
        })
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
        company: &str,
        expected_company_guid: &str,
        boundary_profile: DateBoundaryProfile,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let opening_extent = self
            .fetch_company_book_extent(company, expected_company_guid)
            .await?;
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
            .fetch_native_report_paired(render_native_ledger_export_request(company, &period))
            .await?;
        let NativePairedRead::Stable { body, .. } = paired else {
            return Err(anyhow::Error::new(
                PairedReadValidationError::NativeLedgerCollection,
            ));
        };
        let parsed =
            parse_native_ledger_source_records_with_evidence(&body, expected_company_guid)?;
        let closing_extent = self
            .fetch_company_book_extent(company, expected_company_guid)
            .await?;
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
    /// balance snapshot as one bracketed source for a customer workbook.
    /// Balances expose no GUID, so a row may be joined only where both reads
    /// contain one unique exact `(name, parent)` pair; any ambiguity withholds
    /// the whole export rather than attaching money to the wrong master.
    pub(crate) async fn fetch_party_ledger_master_source(
        &self,
        company: &str,
        expected_company_guid: &str,
        boundary_profile: DateBoundaryProfile,
        currency_assertion: PartyLedgerMasterCurrencyAssertion,
    ) -> anyhow::Result<PartyLedgerMasterSource> {
        let opening_extent = self
            .fetch_company_book_extent(company, expected_company_guid)
            .await?;
        let currency = currency_assertion.require_opening_extent(&opening_extent)?;
        let master_period = NativeLedgerExportPeriod::new(
            boundary_profile,
            opening_extent.books_from().clone(),
            opening_extent.last_voucher_date().clone(),
        )
        .map_err(|_| anyhow::Error::new(PartyLedgerMasterSourceValidationError::MasterPeriod))?;
        let balance_period = party_ledger_master_balance_period(
            boundary_profile,
            opening_extent.books_from().clone(),
            opening_extent.last_voucher_date().clone(),
        )
        .map_err(|_| anyhow::Error::new(PartyLedgerMasterSourceValidationError::BalancePeriod))?;
        let master_pair = self
            .fetch_native_report_paired(render_party_ledger_master_request(company, &master_period))
            .await?;
        let NativePairedRead::Stable {
            body: master_body,
            encoded_bytes: master_response_bytes,
            encoded_sha256: master_response_sha256,
        } = master_pair
        else {
            return Err(anyhow::Error::new(
                PairedReadValidationError::PartyLedgerMaster,
            ));
        };
        let master = parse_native_party_ledger_master_records_with_evidence(
            &master_body,
            expected_company_guid,
        )?;
        if !master.evidence.duplicate_identities.is_empty() {
            return Err(anyhow::Error::new(
                PartyLedgerMasterSourceValidationError::DuplicateMasterIdentity,
            ));
        }
        let balance_pair = self
            .fetch_native_report_paired(render_native_ledger_snapshot_request(
                company,
                &balance_period,
            ))
            .await?;
        let NativePairedRead::Stable {
            body: balance_body,
            encoded_bytes: balance_response_bytes,
            encoded_sha256: balance_response_sha256,
        } = balance_pair
        else {
            return Err(anyhow::Error::new(
                PairedReadValidationError::PartyLedgerBalance,
            ));
        };
        let balances = parse_native_ledger_snapshot(&balance_body)?;
        let group_pair = self
            .fetch_native_report_paired(render_native_group_snapshot_request(company))
            .await?;
        let NativePairedRead::Stable {
            body: group_body,
            encoded_bytes: group_response_bytes,
            encoded_sha256: group_response_sha256,
        } = group_pair
        else {
            return Err(anyhow::Error::new(
                PairedReadValidationError::PartyLedgerGroup,
            ));
        };
        let groups = parse_native_group_snapshot_with_evidence(&group_body, expected_company_guid)?
            .into_iter()
            .map(|entry| PartyLedgerMasterGroup {
                name: entry.record.name,
                parent: entry.record.parent,
                reserved_name: entry.record.reserved_name,
            })
            .collect();
        let closing_extent = self
            .fetch_company_book_extent(company, expected_company_guid)
            .await?;
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
            company: company.to_string(),
            company_guid: expected_company_guid.to_string(),
            currency_assertion: currency.assertion,
            currency_decimal_places: currency.decimal_places,
            from: master_period.from().clone(),
            // The snapshot period is the balance evidence. Its derived end is
            // the date Tally was actually asked to honor, not merely the last
            // voucher date used by the identity/master read.
            to: balance_period.to().clone(),
            rows,
            master_response_sha256,
            balance_response_sha256,
            group_response_sha256,
            master_response_bytes,
            balance_response_bytes,
            group_response_bytes,
            groups,
        })
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
        parse_standard_ledger_catalog(&xml, company, expected_company_guid)
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
        company: &str,
        expected_company_guid: &str,
    ) -> anyhow::Result<CompanyBookExtent> {
        let company_name = ValidatedCompanyName::new(company.to_string())?;
        let request = ReadOnlyProfile::CompanyBookExtentV1 {
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
        let first = parse_company_book_extent(&first, company, expected_company_guid)?;
        let second = parse_company_book_extent(&second, company, expected_company_guid)?;
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
        let (first, first_bytes, first_sha256) = self
            .post_xml_with_encoded_bytes(request_xml.clone())
            .await?;
        self.http
            .get_status_decoded()
            .await
            .context("Tally health check between paired native report reads failed")?;
        let (second, second_bytes, second_sha256) =
            self.post_xml_with_encoded_bytes(request_xml).await?;
        self.http
            .get_status_decoded()
            .await
            .context("Tally health check after paired native report reads failed")?;
        if first != second || first_bytes != second_bytes || first_sha256 != second_sha256 {
            return Ok(NativePairedRead::Drifted);
        }
        Ok(NativePairedRead::Stable {
            body: first,
            encoded_bytes: first_bytes,
            encoded_sha256: first_sha256,
        })
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
        company: &str,
        expected_company_guid: &str,
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
            .post_xml(render_native_voucher_export_request(company, &from, &to))
            .await?;
        let parsed =
            parse_native_voucher_source_records_with_evidence(&xml, expected_company_guid)?;
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
            self.fetch_company_book_extent(company, expected_company_guid)
                .await
                .context(
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
mod tests {
    #[cfg(feature = "voucher-scan")]
    use super::LedgerOpeningCoverageRead;
    use super::{
        canonical_loopback_origin, decode_xml_bytes, detect_product,
        has_presentation_equivalent_guid_siblings, normalize_discovered_companies,
        party_ledger_master_balance_period, party_ledger_master_openings_agree, tally_endpoint,
        unique_company_identities, TallyClient, TallyConfig, TallyProduct,
    };
    use bridge_tally_core::{
        CapabilityFeatureId, CapabilityPackId, CapabilityState, EvidenceConfidence, TallyDate,
        TransportId,
    };
    use bridge_tally_protocol::native_outstandings::NativeLedgerExportPeriod;
    use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;
    use std::time::Duration;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator, WireEncoding};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn party_master_snapshot_uses_the_next_common_admissible_boundary() {
        let period = party_ledger_master_balance_period(
            DateBoundaryProfile::ModeAgnostic,
            TallyDate::parse("20260401").unwrap(),
            TallyDate::parse("20260415").unwrap(),
        )
        .expect("the derived common boundary is valid");
        assert_eq!(period.to().as_str(), "20260501");
        assert!(period.to() >= &TallyDate::parse("20260415").unwrap());
    }

    #[test]
    fn party_master_opening_balance_comparison_accepts_equivalent_decimal_scale() {
        assert!(party_ledger_master_openings_agree(
            "0",
            &bridge_tally_core::ExactDecimal::parse("0.00").unwrap(),
        )
        .unwrap());
    }

    #[test]
    fn party_master_opening_balance_comparison_withholds_a_real_numeric_mismatch() {
        assert!(!party_ledger_master_openings_agree(
            "0.01",
            &bridge_tally_core::ExactDecimal::parse("0.00").unwrap(),
        )
        .unwrap());
    }

    #[test]
    fn party_master_opening_balance_comparison_rejects_an_unparseable_master_value() {
        assert!(party_ledger_master_openings_agree(
            "not-an-amount",
            &bridge_tally_core::ExactDecimal::parse("0.00").unwrap(),
        )
        .is_err());
    }

    fn utf16_xml_response(body: impl AsRef<str>) -> Vec<u8> {
        let body = bridge_tally_protocol::encode_tally_xml_request_utf16le(body.as_ref());
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-16\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [headers.as_bytes(), &body].concat()
    }

    fn utf8_status_response(body: impl AsRef<str>) -> Vec<u8> {
        let body = body.as_ref().as_bytes();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [headers.as_bytes(), body].concat()
    }

    async fn read_complete_http_request(
        socket: &mut (impl tokio::io::AsyncRead + Unpin),
    ) -> Vec<u8> {
        const HEADER_TERMINATOR: &[u8] = b"\r\n\r\n";
        const MAX_TEST_HEADER_BYTES: usize = 64 * 1024;

        let mut request = Vec::new();
        let mut chunk = [0_u8; 4096];
        let header_end = loop {
            if let Some(offset) = request
                .windows(HEADER_TERMINATOR.len())
                .position(|window| window == HEADER_TERMINATOR)
            {
                break offset + HEADER_TERMINATOR.len();
            }
            let bytes_read = socket
                .read(&mut chunk)
                .await
                .expect("read synthetic HTTP request headers");
            assert!(
                bytes_read > 0,
                "synthetic HTTP request ended before its headers were complete"
            );
            request.extend_from_slice(&chunk[..bytes_read]);
            assert!(
                request.len() <= MAX_TEST_HEADER_BYTES,
                "synthetic HTTP request headers exceeded {MAX_TEST_HEADER_BYTES} bytes"
            );
        };

        let headers = std::str::from_utf8(&request[..header_end - HEADER_TERMINATOR.len()])
            .expect("synthetic HTTP request headers must be UTF-8");
        let mut lines = headers.split("\r\n");
        let request_line = lines
            .next()
            .expect("synthetic HTTP request must contain a request line");
        let mut request_line_parts = request_line.split_ascii_whitespace();
        let method = request_line_parts
            .next()
            .expect("synthetic HTTP request method is missing");
        request_line_parts
            .next()
            .expect("synthetic HTTP request target is missing");
        let version = request_line_parts
            .next()
            .expect("synthetic HTTP request version is missing");
        assert_eq!(version, "HTTP/1.1", "synthetic request must use HTTP/1.1");
        assert!(
            request_line_parts.next().is_none(),
            "synthetic HTTP request line has unexpected fields"
        );

        let mut content_length = None;
        for header in lines {
            let (name, value) = header
                .split_once(':')
                .expect("synthetic HTTP request contains a malformed header");
            assert!(
                !name.eq_ignore_ascii_case("transfer-encoding"),
                "synthetic HTTP request must use Content-Length framing"
            );
            if name.eq_ignore_ascii_case("content-length") {
                assert!(
                    content_length.is_none(),
                    "synthetic HTTP request contains duplicate Content-Length headers"
                );
                content_length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .expect("synthetic HTTP request has an invalid Content-Length"),
                );
            }
        }
        let content_length = match content_length {
            Some(length) => length,
            None if method == "GET" || method == "HEAD" => 0,
            None => panic!("synthetic HTTP request body is missing Content-Length"),
        };
        let request_end = header_end
            .checked_add(content_length)
            .expect("synthetic HTTP request length overflowed usize");
        assert!(
            request.len() <= request_end,
            "synthetic HTTP request contains bytes beyond Content-Length"
        );

        while request.len() < request_end {
            let remaining = request_end - request.len();
            let chunk_length = remaining.min(chunk.len());
            let bytes_read = socket
                .read(&mut chunk[..chunk_length])
                .await
                .expect("read synthetic HTTP request body");
            assert!(
                bytes_read > 0,
                "synthetic HTTP request ended before its Content-Length body was complete"
            );
            request.extend_from_slice(&chunk[..bytes_read]);
        }

        request
    }

    fn assert_company_collection_request_shape(request: &str) {
        assert!(request.contains("<TYPE>Collection</TYPE>"));
        for method in ["NAME", "GUID", "PRODUCTNAME"] {
            assert!(request.contains(&format!("<NATIVEMETHOD>{method}</NATIVEMETHOD>")));
        }
        for expression in [
            "<COMPUTE>EduMode : $$LicenseInfo:IsEducationalMode</COMPUTE>",
            "<COMPUTE>Silver : $$LicenseInfo:IsSilver</COMPUTE>",
            "<COMPUTE>Gold : $$LicenseInfo:IsGold</COMPUTE>",
        ] {
            assert!(request.contains(expression));
        }
    }

    #[test]
    fn detects_tallyprime_status() {
        assert!(matches!(
            detect_product("TallyPrime Server is Running"),
            TallyProduct::TallyPrime
        ));
    }

    #[test]
    fn detects_erp9_status() {
        assert!(matches!(
            detect_product("Tally ERP 9 Server is Running"),
            TallyProduct::TallyErp9
        ));
    }

    #[test]
    fn product_marker_is_not_accepted_inside_unrelated_content() {
        assert!(matches!(
            detect_product("<html><body>TallyPrime Server is Running</body></html>"),
            TallyProduct::Unknown
        ));
        assert!(matches!(
            detect_product("prefix Tally ERP 9 Server is Running suffix"),
            TallyProduct::Unknown
        ));
    }

    #[tokio::test]
    async fn selected_read_request_digest_matches_the_dispatched_wire_entity() {
        let plan = ScenarioPlan::new(Fixture::NormalExport).with_encoding(WireEncoding::Utf16Le);
        let simulator = Simulator::spawn(plan).expect("spawn synthetic Tally endpoint");
        let client = TallyClient::new(TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        })
        .expect("build synthetic Tally client");

        let observation = client
            .qualify_selected_ledgers(
                "BRIDGE SYNTHETIC BOOK",
                "00000000-0000-4000-8000-000000000001",
            )
            .await
            .expect("qualify synthetic selected-ledger read");
        let dispatched = simulator.finish().expect("finish synthetic Tally exchange");

        assert_eq!(observation.request_sha256, dispatched.request_body_sha256);
    }

    #[test]
    fn company_identity_normalization_rejects_invalid_and_ambiguous_guids() {
        let normalized = normalize_discovered_companies(vec![
            crate::tally::TallyCompany {
                name: "  Synthetic A  ".to_string(),
                guid: Some("  GUID-1  ".to_string()),
                company_number: Some("100005".to_string()),
                books_from: Some("20250401".to_string()),
            },
            crate::tally::TallyCompany {
                name: "Synthetic B".to_string(),
                guid: Some("guid-1".to_string()),
                company_number: Some("100014".to_string()),
                books_from: Some("20260401".to_string()),
            },
        ])
        .expect("normalize company identities");
        assert_eq!(normalized[0].name, "Synthetic A");
        assert_eq!(normalized[0].guid.as_deref(), Some("GUID-1"));
        assert!(unique_company_identities(&normalized));
        assert!(!has_presentation_equivalent_guid_siblings(&normalized));

        assert!(
            normalize_discovered_companies(vec![crate::tally::TallyCompany {
                name: "Synthetic\nCompany".to_string(),
                guid: Some("guid-2".to_string()),
                company_number: None,
                books_from: None,
            }])
            .is_err()
        );
        assert!(
            normalize_discovered_companies(vec![crate::tally::TallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("guid\n2".to_string()),
                company_number: None,
                books_from: None,
            }])
            .is_err()
        );
    }

    #[test]
    fn presentation_equivalent_guid_siblings_are_not_stable_company_identities() {
        let companies = vec![
            crate::tally::TallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("guid-3".to_string()),
                company_number: Some("100005".to_string()),
                books_from: Some("20250401".to_string()),
            },
            crate::tally::TallyCompany {
                name: " synthetic company ".to_string(),
                guid: Some("GUID-3".to_string()),
                company_number: Some("100014".to_string()),
                books_from: Some("20260401".to_string()),
            },
        ];

        assert!(unique_company_identities(&companies));
        assert!(has_presentation_equivalent_guid_siblings(&companies));
    }

    #[test]
    fn identical_name_same_guid_distinct_books_are_not_stable_company_identities() {
        let companies = vec![
            crate::tally::TallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("guid-3".to_string()),
                company_number: Some("100005".to_string()),
                books_from: Some("20250401".to_string()),
            },
            crate::tally::TallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("GUID-3".to_string()),
                company_number: Some("100014".to_string()),
                books_from: Some("20260401".to_string()),
            },
        ];

        assert!(unique_company_identities(&companies));
        assert!(has_presentation_equivalent_guid_siblings(&companies));
    }

    #[test]
    fn validates_tally_endpoint_components() {
        assert_eq!(
            tally_endpoint(&TallyConfig::default(), "/status")
                .expect("localhost endpoint")
                .as_str(),
            "http://127.0.0.1:9000/status"
        );
        let config = TallyConfig {
            host: "::1".to_string(),
            port: 9000,
        };
        assert_eq!(
            tally_endpoint(&config, "/status")
                .expect("IPv6 endpoint")
                .as_str(),
            "http://[::1]:9000/status"
        );
        for host in ["localhost", "127.0.0.1"] {
            assert_eq!(
                canonical_loopback_origin(&TallyConfig {
                    host: host.to_string(),
                    port: 9000,
                })
                .expect("canonical loopback origin"),
                "http://127.0.0.1:9000"
            );
        }
        assert_eq!(
            canonical_loopback_origin(&TallyConfig {
                host: "::1".to_string(),
                port: 9000,
            })
            .expect("canonical IPv6 loopback origin"),
            "http://[::1]:9000"
        );

        for host in ["http://localhost", "localhost/path", "user@localhost", ""] {
            let invalid = TallyConfig {
                host: host.to_string(),
                port: 9000,
            };
            assert!(tally_endpoint(&invalid, "/status").is_err());
        }

        for host in [
            "192.168.1.10",
            "10.0.0.5",
            "169.254.1.1",
            "224.0.0.1",
            "8.8.8.8",
            "tally.internal",
        ] {
            let remote = TallyConfig {
                host: host.to_string(),
                port: 9000,
            };
            assert!(tally_endpoint(&remote, "/status").is_err());
        }
    }

    #[test]
    fn decodes_supported_xml_byte_order_marks_and_rejects_invalid_sequences() {
        let utf8 = [b"\xEF\xBB\xBF".as_slice(), b"<ENVELOPE />"].concat();
        assert_eq!(decode_xml_bytes(utf8).expect("UTF-8 BOM"), "<ENVELOPE />");

        let document = "<ENVELOPE><NAME>नमस्ते</NAME></ENVELOPE>";
        let mut utf16le = vec![0xFF, 0xFE];
        utf16le.extend(document.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_xml_bytes(utf16le).expect("UTF-16LE"), document);

        let mut utf16be = vec![0xFE, 0xFF];
        utf16be.extend(document.encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_xml_bytes(utf16be).expect("UTF-16BE"), document);

        assert!(decode_xml_bytes(vec![0xFF, 0xFE, 0x00]).is_err());
        assert!(decode_xml_bytes(vec![0x80]).is_err());
    }

    #[tokio::test]
    async fn tally_requests_ignore_configured_proxy() {
        let tally_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let tally_address = tally_listener.local_addr().expect("Tally address");
        let proxy_listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic proxy");
        let proxy_address = proxy_listener.local_addr().expect("proxy address");

        let tally_server = tokio::spawn(async move {
            let accepted = tokio::time::timeout(Duration::from_secs(2), tally_listener.accept())
                .await
                .expect("Tally request timed out")
                .expect("accept Tally request");
            let (mut socket, _) = accepted;
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                String::from_utf8_lossy(&request).starts_with("GET /status HTTP/1.1"),
                "request should go directly to the Tally endpoint"
            );
            let body = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
            socket
                .write_all(&utf8_status_response(body))
                .await
                .expect("write Tally response");
        });

        let proxy_server = tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_millis(750), proxy_listener.accept()).await {
                Ok(Ok((mut socket, _))) => {
                    let response = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .expect("write proxy response");
                    true
                }
                Ok(Err(error)) => panic!("accept proxy request: {error}"),
                Err(_) => false,
            }
        });

        let client = TallyClient::with_http_builder(
            TallyConfig {
                host: tally_address.ip().to_string(),
                port: tally_address.port(),
            },
            reqwest::Client::builder().proxy(
                reqwest::Proxy::all(format!("http://{proxy_address}"))
                    .expect("synthetic proxy URL"),
            ),
        );

        let status = client
            .check_connection()
            .await
            .expect("check synthetic Tally connection");
        tally_server.await.expect("synthetic Tally server task");
        let proxy_received_request = proxy_server.await.expect("synthetic proxy task");

        assert!(status.reachable, "direct Tally response should be accepted");
        assert!(
            status.compatible,
            "synthetic Tally status should be recognized"
        );
        assert!(
            !proxy_received_request,
            "Tally traffic must never be sent through a configured proxy"
        );
    }

    #[cfg(feature = "voucher-scan")]
    #[tokio::test]
    async fn paired_outstandings_reads_health_check_between_and_after_requests() {
        const COMPANY_EXTENT: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
        );
        const OPTIONAL_VOUCHERS: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_optional_voucher_live.xml"
        );
        const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
        // The captured fixture predates the ALTMSTID fetch. The outstandings bracket
        // (`fetch_company_book_extent`) now requires that witness, so inject it into this
        // in-memory copy -- the committed fixture bytes are left untouched.
        let company_extent = COMPANY_EXTENT.replacen(
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>"#,
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID><ALTMSTID TYPE="Number">1</ALTMSTID>"#,
            1,
        );
        assert_ne!(
            company_extent, COMPANY_EXTENT,
            "the injection must actually change the fixture for this test to prove anything"
        );

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            for (index, expected_prefix) in [
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
                "POST / HTTP/1.1",
                "GET /status HTTP/1.1",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .expect("paired request timed out")
                        .expect("accept paired request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected_prefix),
                    "request {index} did not follow the required read/health-check sequence"
                );
                let body = match index {
                    0 | 2 | 12 | 14 => company_extent.as_str(),
                    4 | 6 | 8 | 10 => OPTIONAL_VOUCHERS,
                    _ => STATUS,
                };
                let response = if index % 2 == 0 {
                    utf16_xml_response(body)
                } else {
                    utf8_status_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write paired response");
            }
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let extent = client
            .fetch_company_book_extent(
                "Aarav Trading Company Demo",
                "bb8ad19e-6aef-4239-a917-87fec0c6215e",
            )
            .await
            .expect("paired extent reads remain stable");
        let reporting_window = bridge_tally_protocol::outstandings::DateWindow::parse(
            bridge_tally_protocol::outstandings::DateBoundaryProfile::ModeAgnostic,
            "20260401",
            "20260401",
        )
        .expect("synthetic one-day window");
        let segment_window = reporting_window
            .narrow_partitions()
            .expect("one narrow partition")
            .remove(0);
        let observation = client
            .fetch_outstandings_segment_pair(
                extent.company(),
                segment_window.clone(),
                bridge_tally_protocol::outstandings::AlterIdRange::new(0, 101603)
                    .expect("non-empty synthetic range"),
            )
            .await
            .expect("paired segment reads remain stable");
        let verification = observation.verification;
        let bridge_tally_protocol::outstandings::SegmentVerification::Complete(segment) =
            verification
        else {
            panic!("captured voucher response did not verify: {verification:?}");
        };
        assert!(
            !segment.vouchers().is_empty(),
            "captured voucher fixture unexpectedly had no vouchers"
        );
        let witness = client
            .fetch_empty_partition_witness_pair(extent.company(), segment_window)
            .await
            .expect("paired empty-date witness reads remain stable");
        let bridge_tally_protocol::outstandings::WitnessPairVerification::Complete(witness) =
            witness
        else {
            panic!("captured witness fixture did not verify: {witness:?}");
        };
        assert!(
            !witness.vouchers().is_empty(),
            "captured witness fixture unexpectedly had no identity rows"
        );
        let closing_extent = client
            .fetch_company_book_extent(
                "Aarav Trading Company Demo",
                "bb8ad19e-6aef-4239-a917-87fec0c6215e",
            )
            .await
            .expect("closing paired extent reads remain stable");
        assert_eq!(closing_extent, extent, "synthetic book did not change");
        assert_eq!(
            client.observed_body_bytes(),
            Some(
                u64::try_from(
                    bridge_tally_protocol::encode_tally_xml_request_utf16le(OPTIONAL_VOUCHERS)
                        .len(),
                )
                .expect("encoded fixture length fits u64"),
            ),
            "closing extent evidence must not overwrite the larger voucher payload"
        );
        server.await.expect("synthetic Tally server task");
    }

    #[cfg(feature = "voucher-scan")]
    #[tokio::test]
    async fn paired_ledger_opening_coverage_reports_intra_pair_drift() {
        const COMPANY_EXTENT: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
        );
        const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
        // The captured fixture predates the ALTMSTID fetch. The outstandings bracket
        // (`fetch_company_book_extent`) now requires that witness, so inject it into this
        // in-memory copy -- the committed fixture bytes are left untouched.
        let company_extent = COMPANY_EXTENT.replacen(
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>"#,
            r#"<GUID TYPE="String">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID><ALTMSTID TYPE="Number">1</ALTMSTID>"#,
            1,
        );
        assert_ne!(
            company_extent, COMPANY_EXTENT,
            "the injection must actually change the fixture for this test to prove anything"
        );

        let coverage = |name: &str| {
            format!(
                "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME=\"{name}\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000001</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
            )
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let responses = vec![
            company_extent.clone(),
            STATUS.to_string(),
            company_extent,
            STATUS.to_string(),
            coverage("Before Rename"),
            STATUS.to_string(),
            coverage("After Rename"),
            STATUS.to_string(),
        ];
        let server = tokio::spawn(async move {
            for (index, body) in responses.into_iter().enumerate() {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .expect("paired request timed out")
                        .expect("accept paired request");
                let request = read_complete_http_request(&mut socket).await;
                let expected_prefix = if index % 2 == 0 {
                    "POST / HTTP/1.1"
                } else {
                    "GET /status HTTP/1.1"
                };
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected_prefix),
                    "request {index} did not follow the required read/health-check sequence"
                );
                let response = if index % 2 == 0 {
                    utf16_xml_response(body)
                } else {
                    utf8_status_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write paired response");
            }
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let extent = client
            .fetch_company_book_extent(
                "Aarav Trading Company Demo",
                "bb8ad19e-6aef-4239-a917-87fec0c6215e",
            )
            .await
            .expect("paired extent reads remain stable");
        assert!(matches!(
            client
                .fetch_ledger_opening_coverage(extent.company())
                .await
                .expect("coverage responses parse"),
            LedgerOpeningCoverageRead::Drifted
        ));
        server.await.expect("synthetic Tally server task");
    }

    /// The outstandings bracket (`fetch_company_book_extent`, feeding both
    /// `fetch_outstandings_native` and `fetch_ledgers`) must fail closed with
    /// a typed error when both paired reads agree but neither carries
    /// `ALTMSTID`. This is the exact case the review flagged: two
    /// witness-less extents compare equal, so the ordinary `first != second`
    /// drift check alone cannot tell a stable book from one where a
    /// GROUP/LEDGER master moved mid-window without a signal to detect it.
    /// Uses the real, unmodified `unit_a_company_extent_live.xml` capture --
    /// from before `ALTMSTID` was added to the fetch list -- rather than a
    /// synthetic response, so the absence being tested is the one Tally has
    /// actually produced.
    #[tokio::test]
    async fn outstandings_bracket_fails_closed_when_altmstid_is_absent() {
        const COMPANY_EXTENT_WITHOUT_ALTMSTID: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
        );
        const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
        assert!(
            !COMPANY_EXTENT_WITHOUT_ALTMSTID.contains("ALTMSTID"),
            "this fixture must predate the ALTMSTID fetch for this test to prove anything"
        );

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let responses = [
            COMPANY_EXTENT_WITHOUT_ALTMSTID,
            STATUS,
            COMPANY_EXTENT_WITHOUT_ALTMSTID,
            STATUS,
        ];
        let server = tokio::spawn(async move {
            for (index, body) in responses.into_iter().enumerate() {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .expect("paired request timed out")
                        .expect("accept paired request");
                let request = read_complete_http_request(&mut socket).await;
                let expected_prefix = if index % 2 == 0 {
                    "POST / HTTP/1.1"
                } else {
                    "GET /status HTTP/1.1"
                };
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected_prefix),
                    "request {index} did not follow the required read/health-check sequence"
                );
                let response = if index % 2 == 0 {
                    utf16_xml_response(body)
                } else {
                    utf8_status_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write paired response");
            }
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let error = client
            .fetch_company_book_extent(
                "Aarav Trading Company Demo",
                "bb8ad19e-6aef-4239-a917-87fec0c6215e",
            )
            .await
            .expect_err("a stable pair without ALTMSTID must still fail closed");
        server.await.expect("synthetic Tally server task");
        assert!(
            error
                .downcast_ref::<bridge_tally_protocol::outstandings_shared::OutstandingsError>()
                .is_some_and(|error| {
                    *error
                        == bridge_tally_protocol::outstandings_shared::OutstandingsError::MasterWitnessAbsent
                }),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn http_success_with_tally_status_zero_is_not_an_empty_success() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let extent = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><LASTVOUCHERDATE TYPE="Date">20260701</LASTVOUCHERDATE><BOOKSFROM TYPE="Date">20240101</BOOKSFROM><NAME TYPE="String">Synthetic Company</NAME><GUID TYPE="String">synthetic-company-guid</GUID><ALTMSTID TYPE="Number">1</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
            for (index, body) in [
                extent,
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                extent,
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY /></ENVELOPE>",
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY /></ENVELOPE>",
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                let expected = if index == 1 || index == 3 || index == 5 || index == 7 {
                    "GET /status HTTP/1.1"
                } else {
                    "POST / HTTP/1.1"
                };
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected),
                    "ledger fetch did not preserve its extent/read sequence"
                );
                if index == 4 || index == 6 {
                    let body_start = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|offset| offset + 4)
                        .expect("native ledger POST has complete HTTP headers");
                    let request_xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
                        &request[body_start..],
                        request.len(),
                    )
                    .expect("native ledger POST uses decodable UTF-16 XML")
                    .text;
                    assert!(
                        request_xml.contains(r#"<SVFROMDATE TYPE="Date">20240101</SVFROMDATE>"#)
                    );
                    assert!(request_xml.contains(r#"<SVTODATE TYPE="Date">20260701</SVTODATE>"#));
                }
                let response = if index == 1 || index == 3 || index == 5 || index == 7 {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write Tally response");
            }
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let error = client
            .fetch_ledgers(
                "Synthetic Company",
                "synthetic-company-guid",
                DateBoundaryProfile::ModeAgnostic,
            )
            .await
            .expect_err("STATUS 0 must not become an empty ledger result");
        server.await.expect("synthetic Tally server task");
        assert!(
            error
                .to_string()
                .contains("native ledger collection did not report success"),
            "unexpected error: {error:#}"
        );
    }

    #[tokio::test]
    async fn invalid_book_extent_stops_ledger_export_without_a_date_fallback() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let invalid_extent = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><LASTVOUCHERDATE TYPE="Date">20260701</LASTVOUCHERDATE><NAME TYPE="String">Synthetic Company</NAME><GUID TYPE="String">synthetic-company-guid</GUID><ALTMSTID TYPE="Number">1</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
            for (index, body) in [
                invalid_extent,
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                invalid_extent,
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept extent request");
                let request = read_complete_http_request(&mut socket).await;
                let expected = if index % 2 == 0 {
                    "POST / HTTP/1.1"
                } else {
                    "GET /status HTTP/1.1"
                };
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected),
                    "request {index} did not follow the paired extent sequence"
                );
                let response = if index % 2 == 0 {
                    utf16_xml_response(body)
                } else {
                    utf8_status_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write extent response");
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err(),
                "an invalid extent must stop before any native ledger request"
            );
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        client
            .fetch_ledgers(
                "Synthetic Company",
                "synthetic-company-guid",
                DateBoundaryProfile::ModeAgnostic,
            )
            .await
            .expect_err("missing BOOKSFROM must fail closed");
        server.await.expect("synthetic Tally server task");
    }

    #[tokio::test]
    async fn education_profile_rejects_an_unsupported_books_from_before_ledger_export() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let extent = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><LASTVOUCHERDATE TYPE="Date">20260701</LASTVOUCHERDATE><BOOKSFROM TYPE="Date">20240115</BOOKSFROM><NAME TYPE="String">Synthetic Company</NAME><GUID TYPE="String">synthetic-company-guid</GUID><ALTMSTID TYPE="Number">1</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
            for (index, body) in [
                extent,
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                extent,
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept extent request");
                let request = read_complete_http_request(&mut socket).await;
                let expected = if index % 2 == 0 {
                    "POST / HTTP/1.1"
                } else {
                    "GET /status HTTP/1.1"
                };
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected),
                    "request {index} did not follow the paired extent sequence"
                );
                let response = if index % 2 == 0 {
                    utf16_xml_response(body)
                } else {
                    utf8_status_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write extent response");
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err(),
                "an Education-invalid BOOKSFROM must stop before the native ledger request"
            );
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let error = client
            .fetch_ledgers(
                "Synthetic Company",
                "synthetic-company-guid",
                DateBoundaryProfile::EducationRestricted,
            )
            .await
            .expect_err("unsupported boundary must not reach the ledger export");
        server.await.expect("synthetic Tally server task");
        assert!(
            error
                .to_string()
                .contains("not supported by the endpoint compatibility profile"),
            "unexpected error: {error:#}"
        );
    }

    fn native_voucher_collection_xml(rows: &[&str]) -> String {
        format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{}</COLLECTION></DATA></BODY></ENVELOPE>",
            rows.join("")
        )
    }

    /// Carries `ALTMSTID` so callers that need the production bracket to
    /// accept a stable pair (rather than exercise the master-witness guard
    /// itself) can use it as-is.
    fn synthetic_company_book_extent_xml(guid: &str) -> String {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><LASTVOUCHERDATE TYPE="Date">20260701</LASTVOUCHERDATE><BOOKSFROM TYPE="Date">20240101</BOOKSFROM><NAME TYPE="String">Synthetic Company</NAME><GUID TYPE="String">{guid}</GUID><ALTMSTID TYPE="Number">1</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    }

    const SYNTHETIC_VOUCHER_ROW: &str = r#"<VOUCHER REMOTEID="synthetic-company-guid-00000001"><DATE TYPE="Date">20260401</DATE><GUID TYPE="String">synthetic-company-guid-00000001</GUID><MASTERID TYPE="Number">1</MASTERID><ALTERID TYPE="Number">1</ALTERID><VOUCHERTYPENAME TYPE="String">Payment</VOUCHERTYPENAME><ISCANCELLED TYPE="String">No</ISCANCELLED><ISOPTIONAL TYPE="String">No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME TYPE="String">Cash</LEDGERNAME><AMOUNT TYPE="Amount">-100.00</AMOUNT><ISDEEMEDPOSITIVE TYPE="String">Yes</ISDEEMEDPOSITIVE></ALLLEDGERENTRIES.LIST></VOUCHER>"#;

    /// A native Voucher collection carries no envelope company GUID, and a
    /// zero-row response has no per-row GUID either -- there is nothing for
    /// `parse_native_voucher_source_records_with_evidence` to bind. Before
    /// the fix, `fetch_vouchers` accepted such a response unauthenticated,
    /// so a silently substituted or dropped company looked identical to a
    /// genuinely empty window. This reproduces that: the voucher read comes
    /// back empty, and the out-of-band book-extent bracket that should
    /// confirm the pinned company instead observes a different one.
    #[tokio::test]
    async fn empty_voucher_response_is_rejected_when_pinned_company_cannot_be_confirmed() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let empty_vouchers = native_voucher_collection_xml(&[]);
        let substituted_extent = synthetic_company_book_extent_xml("substituted-company-guid");
        let status = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
        let steps: Vec<(&str, String, bool)> = vec![
            ("POST / HTTP/1.1", empty_vouchers, false),
            ("POST / HTTP/1.1", substituted_extent.clone(), false),
            ("GET /status HTTP/1.1", status.to_string(), true),
            ("POST / HTTP/1.1", substituted_extent, false),
            ("GET /status HTTP/1.1", status.to_string(), true),
        ];
        let server = tokio::spawn(async move {
            for (index, (expected_prefix, body, is_status)) in steps.into_iter().enumerate() {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .unwrap_or_else(|_| {
                            panic!(
                                "request {index} timed out -- the empty-voucher rejection path \
                                 did not attempt to confirm the pinned company out of band"
                            )
                        })
                        .expect("accept synthetic Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected_prefix),
                    "request {index} did not follow the voucher-then-extent-bracket sequence"
                );
                let response = if is_status {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write synthetic Tally response");
            }
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let error = client
            .fetch_vouchers(
                "Synthetic Company",
                "synthetic-company-guid",
                "20260401",
                "20260401",
            )
            .await
            .expect_err(
                "an empty voucher response must not be accepted when the pinned company book \
                 extent cannot be confirmed",
            );
        server.await.expect("synthetic Tally server task");
        assert!(
            error.to_string().contains(
                "empty voucher response could not confirm the pinned company book extent"
            ),
            "unexpected error: {error:#}"
        );
    }

    /// Same empty voucher response as above, but this time the out-of-band
    /// book-extent bracket confirms the pinned company is still selected and
    /// stable -- so the empty result is accepted.
    #[tokio::test]
    async fn empty_voucher_response_is_accepted_when_bracket_confirms_pinned_company() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let empty_vouchers = native_voucher_collection_xml(&[]);
        let confirmed_extent = synthetic_company_book_extent_xml("synthetic-company-guid");
        let status = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
        let steps: Vec<(&str, String, bool)> = vec![
            ("POST / HTTP/1.1", empty_vouchers, false),
            ("POST / HTTP/1.1", confirmed_extent.clone(), false),
            ("GET /status HTTP/1.1", status.to_string(), true),
            ("POST / HTTP/1.1", confirmed_extent, false),
            ("GET /status HTTP/1.1", status.to_string(), true),
        ];
        let request_count = steps.len();
        let server = tokio::spawn(async move {
            for (index, (expected_prefix, body, is_status)) in steps.into_iter().enumerate() {
                let (mut socket, _) =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept())
                        .await
                        .unwrap_or_else(|_| panic!("request {index} timed out"))
                        .expect("accept synthetic Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(
                    String::from_utf8_lossy(&request).starts_with(expected_prefix),
                    "request {index} did not follow the voucher-then-extent-bracket sequence"
                );
                let response = if is_status {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write synthetic Tally response");
            }
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let vouchers = client
            .fetch_vouchers(
                "Synthetic Company",
                "synthetic-company-guid",
                "20260401",
                "20260401",
            )
            .await
            .expect("an empty voucher response confirmed by the extent bracket must be accepted");
        server.await.expect("synthetic Tally server task");
        assert!(vouchers.is_empty());
        assert_eq!(
            request_count, 5,
            "the empty path must pay for exactly one voucher read plus the extent bracket"
        );
    }

    /// A non-empty voucher response keeps its existing row-GUID binding and
    /// must not pay for the extent bracket -- the common case stays a single
    /// request.
    #[tokio::test]
    async fn non_empty_voucher_response_issues_no_extra_request() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let non_empty_vouchers = native_voucher_collection_xml(&[SYNTHETIC_VOUCHER_ROW]);

        let server = tokio::spawn(async move {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("voucher request timed out")
                .expect("accept synthetic Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                String::from_utf8_lossy(&request).starts_with("POST / HTTP/1.1"),
                "unexpected request for the non-empty voucher read"
            );
            socket
                .write_all(&utf16_xml_response(non_empty_vouchers))
                .await
                .expect("write synthetic Tally response");

            // A non-empty response must not pay for the extent bracket: no
            // further connection should ever arrive.
            let extra = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
            assert!(
                extra.is_err(),
                "non-empty voucher fetch issued an unexpected extra request"
            );
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let vouchers = client
            .fetch_vouchers(
                "Synthetic Company",
                "synthetic-company-guid",
                "20260401",
                "20260401",
            )
            .await
            .expect("non-empty voucher fetch must still succeed exactly as today");
        server.await.expect("synthetic Tally server task");
        assert_eq!(vouchers.len(), 1);
    }

    #[tokio::test]
    async fn capability_probe_reports_only_observed_xml_support() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            for (index, body) in [
                "<RESPONSE>LOCAL STATUS HEURISTIC UNRECOGNIZED</RESPONSE>",
                "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"Synthetic Company\"><GUID TYPE=\"String\">guid-1</GUID><COMPANYNUMBER TYPE=\"Number\">100001</COMPANYNUMBER><BOOKSFROM TYPE=\"Date\">20260401</BOOKSFROM><PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME><EDUMODE>No</EDUMODE><SILVER>Yes</SILVER><GOLD>No</GOLD></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(!request.is_empty(), "synthetic Tally request must not be empty");
                let response = if index == 0 {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write Tally response");
            }
        });

        let probe = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .probe()
        .await
        .expect("probe synthetic Tally endpoint");
        server.await.expect("synthetic Tally server task");

        assert!(probe.connection.reachable);
        assert!(!probe.connection.compatible);
        assert_eq!(probe.companies.len(), 1);
        assert_eq!(
            probe.profile.transports[&TransportId::XmlHttp].state,
            CapabilityState::Supported
        );
        assert_eq!(
            probe.profile.packs[&CapabilityPackId::CoreAccounting].state,
            CapabilityState::Unknown
        );
        assert_eq!(probe.profile.product, "TallyPrime");
        assert!(probe.profile.release.is_none());
        assert_eq!(probe.profile.mode.as_deref(), Some("Licensed"));
        assert_eq!(probe.profile.profile_version, 3);
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::ProductAndMode].state,
            CapabilityState::Supported
        );
        let boundary = crate::tally::TallyRuntime::default()
            .master_ledger_export_boundary_profile_from_profile(Some(&probe.profile));
        assert_eq!(boundary, DateBoundaryProfile::ModeAgnostic);
        assert!(NativeLedgerExportPeriod::new(
            boundary,
            TallyDate::parse("20240115").expect("valid mid-month date"),
            TallyDate::parse("20240115").expect("valid mid-month date"),
        )
        .is_ok());
        for transport in [TransportId::TdlCompanion, TransportId::Odbc] {
            let evidence = &probe.profile.transports[&transport];
            assert_eq!(evidence.state, CapabilityState::Unknown);
            assert_eq!(evidence.confidence, EvidenceConfidence::Unknown);
            assert_eq!(
                evidence.safe_reason_code.as_deref(),
                Some("configuration_not_observed")
            );
        }
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::EndpointReachability].state,
            CapabilityState::Supported
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::LoadedCompanies].state,
            CapabilityState::Supported
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity].state,
            CapabilityState::Supported
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::EncodingBehaviour]
                .safe_reason_code
                .as_deref(),
            Some("utf16_le_bom_observed")
        );
        for feature in [
            CapabilityFeatureId::PracticalResponseLimit,
            CapabilityFeatureId::LedgerRead,
            CapabilityFeatureId::VoucherRead,
            CapabilityFeatureId::Write,
        ] {
            assert_eq!(
                probe.profile.features[&feature].state,
                CapabilityState::Unknown
            );
        }
    }

    #[tokio::test]
    async fn capability_probe_records_unavailable_product_mode_evidence_without_refusing() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let responses = [
                utf8_status_response("<RESPONSE>TallyPrime Server is Running</RESPONSE>"),
                utf16_xml_response("<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Capability collection unavailable</LINEERROR></DATA></BODY></ENVELOPE>"),
                utf16_xml_response("<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>"),
            ];
            for response in responses {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(
                    !request.is_empty(),
                    "synthetic Tally request must not be empty"
                );
                socket
                    .write_all(&response)
                    .await
                    .expect("write Tally response");
            }
        });

        let probe = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .probe()
        .await
        .expect("unavailable product/mode evidence must not refuse the probe");
        server.await.expect("synthetic Tally server task");

        assert_eq!(probe.profile.product, "Unknown");
        assert!(probe.profile.mode.is_none());
        assert_eq!(
            crate::tally::TallyRuntime::default()
                .master_ledger_export_boundary_profile_from_profile(Some(&probe.profile)),
            DateBoundaryProfile::ModeAgnostic
        );
        let evidence = &probe.profile.features[&CapabilityFeatureId::ProductAndMode];
        assert_eq!(evidence.state, CapabilityState::Unknown);
        assert_eq!(evidence.confidence, EvidenceConfidence::Observed);
        assert_eq!(
            evidence.safe_reason_code.as_deref(),
            Some("product_mode_evidence_unavailable")
        );
    }

    /// `fetch_companies` now requests the native `Company` collection
    /// (`ReadOnlyProfile::CompanyListV2`) instead of the legacy `CompanyListV1`
    /// custom TDL report. This asserts the request itself carries that shape --
    /// `TYPE=Collection`, no `REPORT`/`FORM`/`PART`/`LINE`/`FIELD` stack, and no
    /// `SVCURRENTCOMPANY` scoping a discovery read to one company -- and that a
    /// company row's `NAME` attribute and nested `GUID` are trimmed and returned.
    #[tokio::test]
    async fn interactive_company_fetch_sends_the_native_collection_request() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let body = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="  Synthetic Company  "><GUID TYPE="String">  guid-1  </GUID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            let body_start = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
                .expect("POST request has complete HTTP headers");
            let post_xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
                &request[body_start..],
                request.len(),
            )
            .expect("POST request uses decodable UTF-16 XML")
            .text;
            assert_company_collection_request_shape(&post_xml);
            assert!(!post_xml.contains("<SVCURRENTCOMPANY"));
            assert!(!post_xml.contains("<REPORT"));
            assert!(!post_xml.contains("<FORM "));
            socket
                .write_all(&utf16_xml_response(body))
                .await
                .expect("write Tally response");
        });

        let companies = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .fetch_companies()
        .await
        .expect("interactive company discovery accepts the native collection response");
        server.await.expect("synthetic Tally server task");

        assert_eq!(companies.len(), 1);
        assert_eq!(companies[0].name, "Synthetic Company");
        assert_eq!(companies[0].guid.as_deref(), Some("guid-1"));
    }

    /// A collection response omitting a company's `GUID` must fail closed --
    /// the whole discovery read is rejected rather than silently returning an
    /// identity-less company, since identity is what every other read binds
    /// against.
    #[tokio::test]
    async fn interactive_company_fetch_fails_closed_without_a_guid() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let body = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                !request.is_empty(),
                "synthetic Tally request must not be empty"
            );
            socket
                .write_all(&utf16_xml_response(body))
                .await
                .expect("write Tally response");
        });

        let error = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .fetch_companies()
        .await
        .expect_err("a company row without a GUID must fail closed");
        server.await.expect("synthetic Tally server task");
        assert!(error.to_string().contains("GUID"));
    }

    #[tokio::test]
    async fn direct_company_bootstrap_uses_only_the_shaped_collection_identity() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        // `fetch_companies` (the first of the two reads below) now requests the
        // native `Company` collection (`ReadOnlyProfile::CompanyListV2`), so its
        // response carries that shape rather than the legacy `CompanyListV1`
        // direct report. Its GUID must still not escape into the returned
        // identity -- only the second, scoped `standard` read may do that.
        let discovered = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><GUID TYPE="String">scoped-guid</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><BOOKSFROM TYPE="Date">20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let standard = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO /></DESC><DATA><COLLECTION MSTDEPTYPE=\"Ledger\" ISMSTDEPTYPE=\"Yes\"><SyntheticLedger NAME=\"synthetic-ledger\" RESERVEDNAME=\"\"><GUID TYPE=\"String\">ledger-guid</GUID><PARENT TYPE=\"String\">Primary</PARENT><BRIDGECOMPANYGUID TYPE=\"String\">scoped-guid</BRIDGECOMPANYGUID><BRIDGECOMPANYNAME TYPE=\"String\">Synthetic Company</BRIDGECOMPANYNAME><LANGUAGENAME.LIST><LANGUAGEID>1033</LANGUAGEID></LANGUAGENAME.LIST></SyntheticLedger></COLLECTION></DATA></BODY></ENVELOPE>";
        let server = tokio::spawn(async move {
            for body in [discovered, standard] {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(!request.is_empty());
                socket
                    .write_all(&utf16_xml_response(body))
                    .await
                    .expect("write Tally response");
            }
        });

        let company = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .bootstrap_direct_company("Synthetic Company")
        .await
        .expect("strict scoped bootstrap should succeed");
        server.await.expect("synthetic Tally server task");

        assert_eq!(company.name, "Synthetic Company");
        assert_eq!(company.guid.as_deref(), Some("scoped-guid"));
        assert_eq!(company.company_number.as_deref(), Some("100001"));
        assert_eq!(company.books_from.as_deref(), Some("20260401"));
    }

    #[tokio::test]
    async fn capability_probe_does_not_promote_a_direct_company_report() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            // This responder gives the same untrusted bare direct company
            // report regardless of what is requested: once for the `V2`
            // collection attempt (which the collection parser rejects, since
            // it never satisfies `HEADER/STATUS`), and once more for the
            // `V1` fallback that follows.
            for (index, body) in [
                "<RESPONSE>LOCAL STATUS HEURISTIC UNRECOGNIZED</RESPONSE>",
                "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
                "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(!request.is_empty(), "synthetic Tally request must not be empty");
                let response = if index == 0 {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write Tally response");
            }
        });

        let probe = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .probe()
        .await
        .expect("probe synthetic Tally endpoint");
        server.await.expect("synthetic Tally server task");

        assert!(probe.connection.reachable);
        assert!(probe.companies.is_empty());
        assert_eq!(
            probe.profile.transports[&TransportId::XmlHttp].state,
            CapabilityState::Unknown
        );
        assert_eq!(
            probe.profile.transports[&TransportId::XmlHttp]
                .safe_reason_code
                .as_deref(),
            Some("direct_company_report_untrusted")
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::CompanyRead].state,
            CapabilityState::Unknown
        );
    }

    #[tokio::test]
    async fn capability_probe_does_not_promote_a_shaped_company_failure_to_xml_support() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            // Same shaped `STATUS=0` failure for both the `V2` collection
            // attempt and the `V1` fallback that follows it.
            for (index, body) in [
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Could not find Company ''</LINEERROR></DATA></BODY></ENVELOPE>",
                "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Could not find Company ''</LINEERROR></DATA></BODY></ENVELOPE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                assert!(!request.is_empty(), "synthetic Tally request must not be empty");
                let response = if index == 0 {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write Tally response");
            }
        });

        let probe = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .probe()
        .await
        .expect("probe synthetic Tally endpoint");
        server.await.expect("synthetic Tally server task");

        assert!(probe.companies.is_empty());
        let xml = &probe.profile.transports[&TransportId::XmlHttp];
        assert_eq!(xml.state, CapabilityState::Unknown);
        assert_eq!(xml.confidence, EvidenceConfidence::Observed);
        assert_eq!(xml.safe_reason_code.as_deref(), Some("company_not_loaded"));
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::LoadedCompanies].state,
            CapabilityState::Unknown
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity].state,
            CapabilityState::Unknown
        );
    }

    /// A gateway-shaped `Company` collection response with two loaded
    /// synthetic companies and the `CMPINFO` counter trap included. Proves
    /// `probe` requests `CompanyListV2` on the happy path
    /// and trusts its success without ever falling back to the legacy
    /// `CompanyListV1` report: the mock server has exactly one POST response
    /// queued, so a fallback request would hang and fail this test.
    #[tokio::test]
    async fn capability_probe_marks_presentation_equivalent_guid_siblings_ambiguous() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (index, body) in [
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                "<ENVELOPE>\n <HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER>\n <BODY><DESC><CMPINFO><COMPANY>0</COMPANY></CMPINFO></DESC>\n  <DATA><COLLECTION>\n   <COMPANY NAME=\"Synthetic Company A\" RESERVEDNAME=\"\"><NAME TYPE=\"String\">Synthetic Company A</NAME><GUID TYPE=\"String\">synthetic-guid-a</GUID><COMPANYNUMBER TYPE=\"Number\">100001</COMPANYNUMBER><BOOKSFROM TYPE=\"Date\">20260401</BOOKSFROM><PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME><EDUMODE TYPE=\"Logical\">No</EDUMODE><SILVER TYPE=\"Logical\">Yes</SILVER><GOLD TYPE=\"Logical\">No</GOLD></COMPANY>\n   <COMPANY NAME=\" synthetic company a \" RESERVEDNAME=\"\"><NAME TYPE=\"String\"> synthetic company a </NAME><GUID TYPE=\"String\">SYNTHETIC-GUID-A</GUID><COMPANYNUMBER TYPE=\"Number\">100002</COMPANYNUMBER><BOOKSFROM TYPE=\"Date\">20270401</BOOKSFROM><PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME><EDUMODE TYPE=\"Logical\">No</EDUMODE><SILVER TYPE=\"Logical\">Yes</SILVER><GOLD TYPE=\"Logical\">No</GOLD></COMPANY>\n  </COLLECTION></DATA>\n </BODY>\n</ENVELOPE>",
            ]
            .into_iter()
            .enumerate()
            {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let request = read_complete_http_request(&mut socket).await;
                requests.push(request);
                let response = if index == 0 {
                    utf8_status_response(body)
                } else {
                    utf16_xml_response(body)
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write Tally response");
            }
            requests
        });

        let probe = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client")
        .probe()
        .await
        .expect("probe synthetic Tally endpoint");
        let requests = server.await.expect("synthetic Tally server task");

        assert_eq!(probe.companies.len(), 2);
        assert_eq!(probe.companies[0].name, "Synthetic Company A");
        assert_eq!(probe.companies[0].guid.as_deref(), Some("synthetic-guid-a"));
        assert_eq!(probe.companies[1].name, "synthetic company a");
        assert_eq!(probe.companies[1].guid.as_deref(), Some("SYNTHETIC-GUID-A"));
        assert_eq!(probe.profile.product, "TallyPrime");
        assert_eq!(probe.profile.mode.as_deref(), Some("Licensed"));
        assert_eq!(
            probe.profile.transports[&TransportId::XmlHttp].state,
            CapabilityState::Supported
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity]
                .safe_reason_code
                .as_deref(),
            Some("company_identity_display_scope_ambiguous")
        );
        assert_eq!(
            probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity].state,
            CapabilityState::Unknown
        );

        let post_request = requests
            .iter()
            .find(|request| request.starts_with(b"POST"))
            .expect("exactly one POST request was sent");
        let body_start = post_request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
            .expect("POST request has complete HTTP headers");
        let post_xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
            &post_request[body_start..],
            post_request.len(),
        )
        .expect("POST request uses decodable UTF-16 XML");
        assert_company_collection_request_shape(&post_xml.text);
        assert!(post_xml.text.contains("<ID>BridgeCompanyExtent</ID>"));
    }
}
