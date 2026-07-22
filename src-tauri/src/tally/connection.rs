use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicU64, AtomicU8, Ordering},
    Arc,
};

use super::xml_parser::{TallyLedger, TallyVoucher};
use super::{
    tdl_engine,
    validators::{normalize_company_guid, normalize_company_name},
    xml_parser::{self, TallyCompany},
};
use bridge_tally_core::{
    CapabilityEvidence, CapabilityFeatureId, CapabilityPackId, CapabilityProfile, CapabilityState,
    EvidenceConfidence, TransportId,
};
use bridge_tally_protocol::{
    parse_companies_for_interactive_discovery, parse_ledger_source_records_with_evidence,
    parse_ledger_write_readback_with_evidence, parse_selected_voucher_source_records_with_evidence,
    parse_standard_ledger_catalog, parse_standard_ledger_identity_observation,
    verify_company_context, verify_selected_voucher_window_context,
    xml_read_profiles::{
        ReadOnlyProfile, ValidatedCanaryLedgerName, ValidatedCompanyName,
        ValidatedIdentityQuerySha256,
    },
    TallyTextEncoding, BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA,
};
use bridge_tally_transport::{
    canonical_loopback_origin as transport_canonical_origin, TallyEndpointConfig,
    TallyHttpTransport, TallyTransportError,
};

pub type TallyConfig = TallyEndpointConfig;

/// An exact, validated write-canary readback. Its XML remains crate-private to
/// the future write coordinator and is never returned to the UI or persisted.
#[allow(
    dead_code,
    reason = "the sealed runtime seam is intentionally staged before the write coordinator"
)]
pub(crate) struct LedgerCanaryReadbackXml(String);

impl LedgerCanaryReadbackXml {
    #[allow(
        dead_code,
        reason = "only the future crate-internal write coordinator may inspect sealed XML"
    )]
    pub(crate) fn as_xml(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for LedgerCanaryReadbackXml {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LedgerCanaryReadbackXml([redacted])")
    }
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
const ENCODING_UTF16_LE_BOM: u8 = 3;
const ENCODING_UTF16_BE_BOM: u8 = 4;

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

        let xml_evidence = match self.post_xml(tdl_engine::company_list_request()).await {
            Ok(xml) => match xml_parser::parse_companies(&xml) {
                Ok(discovered) => {
                    connection.reachable = true;
                    if connection.error.is_some() {
                        connection.error = Some("status_heuristic_unavailable".to_string());
                    }
                    match normalize_discovered_companies(discovered) {
                        Ok(normalized) => {
                            companies = normalized;
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
            },
            Err(error) => return Err(error),
        };
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
        } else if unique_company_guids(&companies) {
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("stable_company_guid_observed".to_string()),
            }
        } else if companies.iter().all(|company| company.guid.is_some()) {
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
                profile_version: 2,
                // Product/release/mode require separate evidence authority;
                // `/status` text cannot promote them.
                product: "Unknown".to_string(),
                release: None,
                mode: None,
                transports,
                features,
                packs,
            },
            selected_read_scope: None,
            passport_snapshot_id: None,
        })
    }

    pub(super) async fn post_xml(&self, xml: String) -> anyhow::Result<String> {
        let response = self.http.post_xml_decoded(xml).await?;
        self.record_observed_body_bytes(response.encoded_bytes());
        self.record_observed_encoding(response.encoding());
        Ok(response.into_text())
    }

    pub async fn fetch_companies(&self) -> anyhow::Result<Vec<TallyCompany>> {
        let xml = self.post_xml(tdl_engine::company_list_request()).await?;
        let companies = xml_parser::parse_companies_for_interactive_discovery(&xml)?;
        normalize_discovered_companies(companies).map_err(|_| {
            anyhow::anyhow!("Tally returned an invalid company identity for interactive discovery")
        })
    }

    /// Re-enumerates an untrusted direct report, then proves one user-chosen
    /// name with a separate shaped standard collection response. The direct
    /// report's GUID is deliberately discarded; only the collection's computed
    /// context may construct the returned company identity.
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
        Ok(TallyCompany {
            name: candidate.name.clone(),
            guid: Some(guid),
        })
    }

    pub async fn fetch_ledgers(
        &self,
        company: &str,
        expected_company_guid: &str,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let xml = self.post_xml(tdl_engine::ledgers_request(company)).await?;
        let parsed = xml_parser::parse_ledgers_with_evidence(&xml)?;
        xml_parser::verify_company_context(&parsed.evidence, expected_company_guid)?;
        Ok(parsed.records)
    }

    /// Executes the closed canary-readback profile and admits its response only
    /// when the company, query commitment, and at-most-one exact ledger agree.
    #[allow(
        dead_code,
        reason = "the sealed runtime seam is intentionally staged before the write coordinator"
    )]
    pub(crate) async fn fetch_ledger_canary_readback(
        &self,
        company: ValidatedCompanyName,
        ledger_name: ValidatedCanaryLedgerName,
        identity_query_sha256: ValidatedIdentityQuerySha256,
        expected_company_guid: &str,
    ) -> anyhow::Result<LedgerCanaryReadbackXml> {
        let xml = self
            .post_xml(
                ReadOnlyProfile::LedgerCanaryReadbackV1 {
                    company: &company,
                    ledger_name: &ledger_name,
                    identity_query_sha256: &identity_query_sha256,
                }
                .render(),
            )
            .await?;
        validate_ledger_canary_readback(
            &xml,
            ledger_name.as_str(),
            identity_query_sha256.as_str(),
            expected_company_guid,
        )?;
        Ok(LedgerCanaryReadbackXml(xml))
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

    pub async fn qualify_selected_ledgers(
        &self,
        company: &str,
        expected_company_guid: &str,
    ) -> anyhow::Result<SelectedReadObservation> {
        let request = tdl_engine::ledgers_request(company);
        let request_sha256 = sha256_hex(request.as_bytes());
        let xml = self.post_xml(request).await?;
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
        let xml = self
            .post_xml(tdl_engine::vouchers_request(company, from, to))
            .await?;
        let parsed = xml_parser::parse_vouchers_with_evidence(&xml)?;
        xml_parser::verify_company_context(&parsed.evidence, expected_company_guid)?;
        Ok(parsed.records)
    }

    pub async fn qualify_selected_vouchers(
        &self,
        company: &str,
        expected_company_guid: &str,
        from: &str,
        to: &str,
    ) -> anyhow::Result<SelectedReadObservation> {
        let request = tdl_engine::selected_vouchers_request(company, from, to);
        let request_sha256 = sha256_hex(request.as_bytes());
        let xml = self.post_xml(request).await?;
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
        bridge_tally_canonical::validate_selected_voucher_window(from, to, &parsed)
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
        self.observed_body_bytes.store(
            u64::try_from(bytes).unwrap_or(u64::MAX - 1),
            Ordering::Release,
        );
    }

    fn record_observed_encoding(&self, encoding: TallyTextEncoding) {
        let value = match encoding {
            TallyTextEncoding::Utf8 => ENCODING_UTF8,
            TallyTextEncoding::Utf8Bom => ENCODING_UTF8_BOM,
            TallyTextEncoding::Utf16LeBom => ENCODING_UTF16_LE_BOM,
            TallyTextEncoding::Utf16BeBom => ENCODING_UTF16_BE_BOM,
        };
        self.observed_encoding.store(value, Ordering::Release);
    }

    fn observed_encoding_evidence(&self) -> CapabilityEvidence {
        let reason = match self.observed_encoding.load(Ordering::Acquire) {
            ENCODING_UTF8 => "utf8_observed",
            ENCODING_UTF8_BOM => "utf8_bom_observed",
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
            ENCODING_UTF16_LE_BOM => Ok("utf16le_bom"),
            ENCODING_UTF16_BE_BOM => Ok("utf16be_bom"),
            _ => anyhow::bail!("response_encoding_not_observed"),
        }
    }
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
            Ok(TallyCompany { name, guid })
        })
        .collect()
}

fn unique_company_guids(companies: &[TallyCompany]) -> bool {
    let mut seen = BTreeSet::new();
    companies.iter().all(|company| {
        company
            .guid
            .as_deref()
            .is_some_and(|guid| seen.insert(guid.to_ascii_lowercase()))
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
        let name = bridge_tally_core::CanonicalText::parse(source.record.name.clone())?;
        if !names.insert(name.as_str().to_string()) {
            anyhow::bail!("Selected ledger response repeated a normalized name");
        }
        for value in [
            source.record.parent.as_ref(),
            source.record.party_gstin.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        {
            bridge_tally_core::CanonicalText::parse(value.clone())?;
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

#[allow(
    dead_code,
    reason = "the sealed runtime seam is intentionally staged before the write coordinator"
)]
fn validate_ledger_canary_readback(
    xml: &str,
    expected_ledger_name: &str,
    expected_identity_query_sha256: &str,
    expected_company_guid: &str,
) -> anyhow::Result<()> {
    let parsed = parse_ledger_write_readback_with_evidence(xml)?;
    verify_company_context(&parsed.evidence, expected_company_guid)?;
    if parsed
        .evidence
        .company_context
        .as_ref()
        .and_then(|context| context.query_identity_set_sha256.as_deref())
        != Some(expected_identity_query_sha256)
    {
        anyhow::bail!("Tally canary readback query commitment did not match the request");
    }
    if parsed.records.len() > 1 {
        anyhow::bail!("Tally canary readback returned more than one ledger");
    }
    if parsed
        .records
        .first()
        .is_some_and(|record| record.record.name != expected_ledger_name)
    {
        anyhow::bail!("Tally canary readback ledger name did not match the request");
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

pub(super) fn canonical_loopback_origin(config: &TallyConfig) -> anyhow::Result<String> {
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
    use super::{
        canonical_loopback_origin, decode_xml_bytes, detect_product,
        normalize_discovered_companies, tally_endpoint, unique_company_guids,
        validate_ledger_canary_readback, TallyClient, TallyConfig, TallyProduct,
    };
    use bridge_tally_core::{
        CapabilityFeatureId, CapabilityPackId, CapabilityState, EvidenceConfidence, TransportId,
    };
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const CANARY_QUERY_DIGEST: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn canary_readback(ledger_name: &str, company_guid: &str, query_digest: &str) -> String {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="bridge.tally.ledger-write-readback/1" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="{company_guid}" RECORDCOUNT="1" QUERYIDENTITYSETSHA256="{query_digest}"/><LEDGER REMOTEID="bridge-canary-remote-id" NAME="{ledger_name}"><PARENT>BRIDGE SYNTHETIC GROUP</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></BODY></ENVELOPE>"#
        )
    }

    #[test]
    fn canary_readback_requires_exact_company_commitment_and_ledger_name() {
        let ledger_name = "BRIDGE-CANARY-LEDGER-001";
        let xml = canary_readback(ledger_name, "company-guid", CANARY_QUERY_DIGEST);
        validate_ledger_canary_readback(&xml, ledger_name, CANARY_QUERY_DIGEST, "company-guid")
            .expect("exact synthetic canary readback is accepted");

        assert!(validate_ledger_canary_readback(
            &xml,
            "BRIDGE-CANARY-LEDGER-002",
            CANARY_QUERY_DIGEST,
            "company-guid",
        )
        .is_err());
        assert!(validate_ledger_canary_readback(
            &xml,
            ledger_name,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "company-guid",
        )
        .is_err());
        assert!(validate_ledger_canary_readback(
            &xml,
            ledger_name,
            CANARY_QUERY_DIGEST,
            "other-company-guid",
        )
        .is_err());
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

    #[test]
    fn company_identity_normalization_rejects_invalid_and_ambiguous_guids() {
        let normalized = normalize_discovered_companies(vec![
            crate::tally::TallyCompany {
                name: "  Synthetic A  ".to_string(),
                guid: Some("  GUID-1  ".to_string()),
            },
            crate::tally::TallyCompany {
                name: "Synthetic B".to_string(),
                guid: Some("guid-1".to_string()),
            },
        ])
        .expect("normalize company identities");
        assert_eq!(normalized[0].name, "Synthetic A");
        assert_eq!(normalized[0].guid.as_deref(), Some("GUID-1"));
        assert!(!unique_company_guids(&normalized));

        assert!(
            normalize_discovered_companies(vec![crate::tally::TallyCompany {
                name: "Synthetic\nCompany".to_string(),
                guid: Some("guid-2".to_string()),
            }])
            .is_err()
        );
        assert!(
            normalize_discovered_companies(vec![crate::tally::TallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("guid\n2".to_string()),
            }])
            .is_err()
        );
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
            let mut request = [0_u8; 2048];
            let bytes_read = socket.read(&mut request).await.expect("read Tally request");
            assert!(
                String::from_utf8_lossy(&request[..bytes_read]).starts_with("GET /status HTTP/1.1"),
                "request should go directly to the Tally endpoint"
            );
            let body = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
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

    #[tokio::test]
    async fn http_success_with_tally_status_zero_is_not_an_empty_success() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let mut request = [0_u8; 8192];
            let bytes_read = socket.read(&mut request).await.expect("read Tally request");
            assert!(
                String::from_utf8_lossy(&request[..bytes_read]).starts_with("POST / HTTP/1.1"),
                "ledger fetch should use Tally's XML POST endpoint"
            );
            let body = "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY /></ENVELOPE>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write Tally response");
        });

        let client = TallyClient::new(TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        })
        .expect("build synthetic Tally client");
        let error = client
            .fetch_ledgers("Synthetic Company", "synthetic-company-guid")
            .await
            .expect_err("STATUS 0 must not become an empty ledger result");
        server.await.expect("synthetic Tally server task");
        assert!(error.to_string().contains("export request failed"));
    }

    #[tokio::test]
    async fn capability_probe_reports_only_observed_xml_support() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            for body in [
                "<RESPONSE>LOCAL STATUS HEURISTIC UNRECOGNIZED</RESPONSE>",
                "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></BODY></ENVELOPE>",
            ] {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let mut request = [0_u8; 8192];
                let bytes_read = socket.read(&mut request).await.expect("read Tally request");
                assert!(bytes_read > 0, "synthetic Tally request must not be empty");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
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
        assert_eq!(probe.profile.product, "Unknown");
        assert!(probe.profile.release.is_none());
        assert!(probe.profile.mode.is_none());
        assert_eq!(probe.profile.profile_version, 2);
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
            Some("utf8_observed")
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
    async fn interactive_company_fetch_accepts_the_strict_direct_report_variant() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let body = "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>  Synthetic Company  </COMPANYNAMEFIELD><COMPANYGUIDFIELD>  guid-1  </COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>";
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let mut request = [0_u8; 8192];
            let bytes_read = socket.read(&mut request).await.expect("read Tally request");
            assert!(bytes_read > 0, "synthetic Tally request must not be empty");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
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
        .expect("interactive company discovery accepts the exact direct form");
        server.await.expect("synthetic Tally server task");

        assert_eq!(companies.len(), 1);
        assert_eq!(companies[0].name, "Synthetic Company");
        assert_eq!(companies[0].guid.as_deref(), Some("guid-1"));
    }

    #[tokio::test]
    async fn interactive_company_fetch_normalizes_a_shaped_success_response() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            let body = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYINFO><COMPANYNAMEFIELD>  Synthetic Company  </COMPANYNAMEFIELD><COMPANYGUIDFIELD>  guid-1  </COMPANYGUIDFIELD></COMPANYINFO></BODY></ENVELOPE>";
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let mut request = [0_u8; 8192];
            let bytes_read = socket.read(&mut request).await.expect("read Tally request");
            assert!(bytes_read > 0, "synthetic Tally request must not be empty");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
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
        .expect("standard company discovery normalizes identities");
        server.await.expect("synthetic Tally server task");

        assert_eq!(companies.len(), 1);
        assert_eq!(companies[0].name, "Synthetic Company");
        assert_eq!(companies[0].guid.as_deref(), Some("guid-1"));
    }

    #[tokio::test]
    async fn direct_company_bootstrap_uses_only_the_shaped_collection_identity() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let direct = "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>direct-guid-must-not-escape</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>";
        let standard = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO /></DESC><DATA><COLLECTION MSTDEPTYPE=\"Ledger\" ISMSTDEPTYPE=\"Yes\"><SyntheticLedger NAME=\"synthetic-ledger\" RESERVEDNAME=\"\"><GUID TYPE=\"String\">ledger-guid</GUID><PARENT TYPE=\"String\">Primary</PARENT><BRIDGECOMPANYGUID TYPE=\"String\">scoped-guid</BRIDGECOMPANYGUID><BRIDGECOMPANYNAME TYPE=\"String\">Synthetic Company</BRIDGECOMPANYNAME><LANGUAGENAME.LIST><LANGUAGEID>1033</LANGUAGEID></LANGUAGENAME.LIST></SyntheticLedger></COLLECTION></DATA></BODY></ENVELOPE>";
        let server = tokio::spawn(async move {
            for body in [direct, standard] {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let mut request = [0_u8; 8192];
                assert!(socket.read(&mut request).await.expect("read Tally request") > 0);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
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
    }

    #[tokio::test]
    async fn capability_probe_does_not_promote_a_direct_company_report() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind synthetic Tally server");
        let address = listener.local_addr().expect("synthetic Tally address");
        let server = tokio::spawn(async move {
            for body in [
                "<RESPONSE>LOCAL STATUS HEURISTIC UNRECOGNIZED</RESPONSE>",
                "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
            ] {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let mut request = [0_u8; 8192];
                let bytes_read = socket.read(&mut request).await.expect("read Tally request");
                assert!(bytes_read > 0, "synthetic Tally request must not be empty");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
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
            for body in [
                "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
                "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Could not find Company ''</LINEERROR></DATA></BODY></ENVELOPE>",
            ] {
                let (mut socket, _) = listener.accept().await.expect("accept Tally request");
                let mut request = [0_u8; 8192];
                let bytes_read = socket.read(&mut request).await.expect("read Tally request");
                assert!(bytes_read > 0, "synthetic Tally request must not be empty");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
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
}
