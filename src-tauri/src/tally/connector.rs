use bridge_tally_canonical::build_core_window;
use bridge_tally_core::report_tie_out::{LedgerPeriodBalance, LedgerPeriodBalanceReport};
use bridge_tally_core::{
    CanonicalPackWindow, CapabilityEvidence, CapabilityPackId, CapabilityState, CompanyRef,
    EvidenceConfidence, ExactDecimal, PackBatch, ProbeResult, ReadResponseScope, ReadWindow,
    RequestContext, SourceIdentity, TallyConnector, TallyError, CORE_ACCOUNTING_SCHEMA_VERSION,
};
use bridge_tally_protocol::{
    parse_companies, parse_group_source_records_with_evidence, parse_ledger_period_balance_report,
    parse_ledger_source_records_with_evidence, parse_voucher_source_records_with_evidence,
    parse_voucher_type_source_records_with_evidence, verify_company_context,
};
use bridge_tally_transport::TallyTransportError;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::runtime::{TallyRuntimeControlError, TallyRuntimeReadError};
use super::{tdl_engine, TallyConfig, TallyRuntime};

const CORE_QUERY_PROFILE: &str = "core_accounting_v2";

pub(super) struct SealedReadRequest(String);

impl SealedReadRequest {
    fn from_connector_profile(xml: String) -> Self {
        Self(xml)
    }

    pub(super) fn into_xml(self) -> String {
        self.0
    }
}

#[derive(Clone)]
pub struct RuntimeTallyConnector {
    runtime: TallyRuntime,
    config: TallyConfig,
    company: CompanyRef,
    canary_context: RequestContext,
    cancellation: CancellationToken,
}

impl RuntimeTallyConnector {
    pub fn new(
        runtime: TallyRuntime,
        config: TallyConfig,
        company: CompanyRef,
        canary_context: RequestContext,
    ) -> Result<Self, TallyError> {
        if canary_context.company != company
            || canary_context.pack != CapabilityPackId::CoreAccounting
            || canary_context.schema_version != CORE_ACCOUNTING_SCHEMA_VERSION
            || canary_context.query_profile.as_str() != CORE_QUERY_PROFILE
        {
            return Err(invalid_data("connector_context_invalid"));
        }
        Ok(Self {
            runtime,
            config,
            company,
            canary_context,
            cancellation: CancellationToken::new(),
        })
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    async fn post_xml_validated<P>(
        &self,
        request_xml: String,
        validate_application_response: P,
    ) -> Result<String, TallyError>
    where
        P: Fn(&str) -> bool + Send + Sync,
    {
        self.runtime
            .post_xml_cancellable_validated(
                self.config.clone(),
                SealedReadRequest::from_connector_profile(request_xml),
                self.cancellation.clone(),
                validate_application_response,
            )
            .await
            .map_err(map_transport_error)
    }

    async fn extract_core_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        if context.company.identity != self.company.identity {
            return Err(invalid_data("company_identity_mismatch"));
        }
        if context.pack != CapabilityPackId::CoreAccounting
            || context.schema_version != CORE_ACCOUNTING_SCHEMA_VERSION
            || context.query_profile.as_str() != CORE_QUERY_PROFILE
        {
            return Err(TallyError::Unsupported {
                code: "query_profile_not_supported".to_string(),
            });
        }

        let company_name = self.company.display_name.clone();
        let expected_guid = self.company.identity.company_guid.clone();
        let validation_guid = expected_guid.clone();
        let group_xml = self
            .post_xml_validated(tdl_engine::groups_request(&company_name), move |xml| {
                parse_group_source_records_with_evidence(xml)
                    .and_then(|parsed| verify_company_context(&parsed.evidence, &validation_guid))
                    .is_ok()
            })
            .await?;
        let groups = parse_group_source_records_with_evidence(&group_xml)
            .map_err(|_| protocol_error("group_export_invalid"))?;
        verify_company_context(&groups.evidence, &expected_guid)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;

        let validation_guid = expected_guid.clone();
        let ledger_xml = self
            .post_xml_validated(tdl_engine::ledgers_request(&company_name), move |xml| {
                parse_ledger_source_records_with_evidence(xml)
                    .and_then(|parsed| verify_company_context(&parsed.evidence, &validation_guid))
                    .is_ok()
            })
            .await?;
        let ledgers = parse_ledger_source_records_with_evidence(&ledger_xml)
            .map_err(|_| protocol_error("ledger_export_invalid"))?;
        verify_company_context(&ledgers.evidence, &expected_guid)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;

        let validation_guid = expected_guid.clone();
        let voucher_type_xml = self
            .post_xml_validated(
                tdl_engine::voucher_types_request(&company_name),
                move |xml| {
                    parse_voucher_type_source_records_with_evidence(xml)
                        .and_then(|parsed| {
                            verify_company_context(&parsed.evidence, &validation_guid)
                        })
                        .is_ok()
                },
            )
            .await?;
        let voucher_types = parse_voucher_type_source_records_with_evidence(&voucher_type_xml)
            .map_err(|_| protocol_error("voucher_type_export_invalid"))?;
        verify_company_context(&voucher_types.evidence, &expected_guid)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;

        let validation_guid = expected_guid.clone();
        let voucher_xml = self
            .post_xml_validated(
                tdl_engine::vouchers_request(
                    &company_name,
                    &context.window.from_yyyymmdd,
                    &context.window.to_yyyymmdd,
                ),
                move |xml| {
                    parse_voucher_source_records_with_evidence(xml)
                        .and_then(|parsed| {
                            verify_company_context(&parsed.evidence, &validation_guid)
                        })
                        .is_ok()
                },
            )
            .await
            .map_err(classify_voucher_window_error)?;
        let vouchers = parse_voucher_source_records_with_evidence(&voucher_xml)
            .map_err(|_| protocol_error("voucher_export_invalid"))?;
        verify_company_context(&vouchers.evidence, &expected_guid)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;

        build_core_window(context, groups, ledgers, voucher_types, vouchers)
    }
}

#[async_trait::async_trait]
impl TallyConnector for RuntimeTallyConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        let mut result = self
            .runtime
            .probe(self.config.clone())
            .await
            .map_err(map_transport_error)?;
        let canary_result = self.extract_core_window(&self.canary_context).await;
        let core_evidence = match canary_result {
            Ok(window) => core_canary_capability(&window),
            Err(error) => CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some(capability_failure_code(&error)),
            },
        };
        result
            .profile
            .packs
            .insert(CapabilityPackId::CoreAccounting, core_evidence);
        Ok(ProbeResult {
            reachable: result.connection.reachable,
            profile: result.profile,
        })
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        let mut result = self
            .runtime
            .probe(self.config.clone())
            .await
            .map_err(map_transport_error)?;
        let core_evidence = match self.extract_core_window(&self.canary_context).await {
            Ok(window) => core_canary_capability(&window),
            Err(error) => CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some(capability_failure_code(&error)),
            },
        };
        result
            .profile
            .packs
            .insert(CapabilityPackId::CoreAccounting, core_evidence);
        Ok(ProbeResult {
            reachable: result.connection.reachable,
            profile: result.profile,
        })
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        let origin = self
            .runtime
            .cached_probe(&self.config)
            .map_err(|_| protocol_error("capability_cache_unavailable"))?;
        if origin.is_none() {
            return Err(protocol_error("capability_probe_required"));
        }
        let lineage = source_lineage(&self.config)?;
        let companies = parse_companies(
            &self
                .post_xml_validated(tdl_engine::company_list_request(), |xml| {
                    parse_companies(xml).is_ok()
                })
                .await?,
        )
        .map_err(|_| protocol_error("company_export_invalid"))?;
        Ok(companies
            .into_iter()
            .filter_map(|company| {
                let guid = company.guid?;
                if guid.trim().is_empty() {
                    return None;
                }
                Some(CompanyRef {
                    identity: company_source_identity(&lineage, &guid),
                    display_name: company.name,
                })
            })
            .collect())
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        // Capability probes happen before a durable run receives its started_at timestamp.
        // Always perform a new source read here, including for the same canary context, so
        // pre-run observations can never enter the snapshot as if they were run data.
        self.extract_core_window(context).await
    }

    async fn read_core_period_balance_report(
        &self,
        context: &RequestContext,
    ) -> Result<LedgerPeriodBalanceReport, TallyError> {
        if context.company.identity != self.company.identity
            || context.pack != CapabilityPackId::CoreAccounting
            || context.schema_version != CORE_ACCOUNTING_SCHEMA_VERSION
            || context.query_profile.as_str() != CORE_QUERY_PROFILE
        {
            return Err(invalid_data("period_report_scope_mismatch"));
        }
        let expected_company_guid = self.company.identity.company_guid.clone();
        let expected_from = context.window.from_yyyymmdd.clone();
        let expected_to = context.window.to_yyyymmdd.clone();
        let validation_company_guid = expected_company_guid.clone();
        let validation_from = expected_from.clone();
        let validation_to = expected_to.clone();
        let xml = self
            .post_xml_validated(
                tdl_engine::ledger_period_balances_request(
                    &self.company.display_name,
                    &expected_from,
                    &expected_to,
                ),
                move |xml| {
                    parse_ledger_period_balance_report(xml).is_ok_and(|parsed| {
                        company_guids_equal(&parsed.context.company_guid, &validation_company_guid)
                            && parsed.context.from_yyyymmdd == validation_from
                            && parsed.context.to_yyyymmdd == validation_to
                            && parsed.context.ordinary_books_requested
                    })
                },
            )
            .await?;
        let parsed = parse_ledger_period_balance_report(&xml)
            .map_err(|_| protocol_error("period_report_invalid"))?;
        if !company_guids_equal(
            &parsed.context.company_guid,
            &self.company.identity.company_guid,
        ) || parsed.context.from_yyyymmdd != context.window.from_yyyymmdd
            || parsed.context.to_yyyymmdd != context.window.to_yyyymmdd
            || !parsed.context.ordinary_books_requested
        {
            return Err(invalid_data("period_report_scope_mismatch"));
        }
        let balances = parsed
            .records
            .into_iter()
            .map(|row| {
                Ok(LedgerPeriodBalance {
                    ledger_source_id: row
                        .source_id
                        .ok_or_else(|| invalid_data("period_report_identity_missing"))?,
                    opening_balance: ExactDecimal::parse(row.record.opening_balance)?,
                    closing_balance: ExactDecimal::parse(row.record.closing_balance)?,
                })
            })
            .collect::<Result<Vec<_>, TallyError>>()?;
        Ok(LedgerPeriodBalanceReport {
            source_identity: self.company.identity.clone(),
            window: ReadWindow {
                from_yyyymmdd: parsed.context.from_yyyymmdd,
                to_yyyymmdd: parsed.context.to_yyyymmdd,
            },
            // The report echoes Bridge's requested profile, but Tally does not
            // attest that TBalOpening/TBalClosing exclude every scenario,
            // optional, post-dated, or tracking-note effect. A live,
            // release-specific capability receipt must opt this in later.
            ordinary_books_scope_observed: false,
            source_reported_count: parsed.context.source_record_count,
            balances,
        })
    }
}

fn core_canary_capability(window: &CanonicalPackWindow) -> CapabilityEvidence {
    let PackBatch::CoreAccounting(_) = &window.batch else {
        return observed_core_capability(
            CapabilityState::Unknown,
            "sealed_profile_executed_unexpected_pack",
        );
    };
    // A successful extraction proves that every sealed export parsed and matched the pinned
    // company. Returned rows cannot prove that optional fields work when absent, nor that a field
    // observed in this particular date window is supported generally. Keep one stable, truthful
    // execution receipt regardless of incidental row population.
    observed_core_capability(CapabilityState::Unknown, "sealed_profile_executed")
}

/// Returns whether a fresh, identity-bound execution of the sealed Core Accounting profile is
/// sufficient to start a snapshot attempt.
///
/// `Unknown` is deliberately required: a successful sealed execution authorizes a run, but does
/// not claim that fields absent from the returned rows are supported. Reconciliation retains this
/// evidence and can therefore finish partial/unverified.
pub fn core_snapshot_start_authorized(evidence: &CapabilityEvidence) -> bool {
    evidence.state == CapabilityState::Unknown
        && evidence.confidence == EvidenceConfidence::Observed
        && evidence.safe_reason_code.as_deref() == Some("sealed_profile_executed")
}

fn observed_core_capability(state: CapabilityState, reason: &str) -> CapabilityEvidence {
    CapabilityEvidence {
        state,
        confidence: EvidenceConfidence::Observed,
        safe_reason_code: Some(reason.to_string()),
    }
}

pub fn source_lineage(config: &TallyConfig) -> Result<String, TallyError> {
    let endpoint =
        super::EndpointKey::from_config(config).map_err(|_| invalid_data("endpoint_invalid"))?;
    Ok(format!("tally_xml_http:{}", endpoint.as_str()))
}

pub fn company_source_identity(lineage: &str, company_guid: &str) -> SourceIdentity {
    let canonical_guid = company_guid.to_ascii_lowercase();
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-company-observation-v1\0");
    digest.update(lineage.as_bytes());
    digest.update(b"\0");
    digest.update(canonical_guid.as_bytes());
    SourceIdentity {
        bridge_source_lineage: lineage.to_string(),
        company_guid: canonical_guid,
        observed_fingerprint: hex_lower(&digest.finalize()),
    }
}

fn company_guids_equal(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn map_transport_error(error: anyhow::Error) -> TallyError {
    if let Some(control) = error.downcast_ref::<TallyRuntimeControlError>() {
        return match control {
            TallyRuntimeControlError::Cancelled => TallyError::Cancelled,
            TallyRuntimeControlError::QueueDeadline => TallyError::Unsupported {
                code: "endpoint_queue_deadline_exceeded".to_string(),
            },
            TallyRuntimeControlError::CircuitCooldown
            | TallyRuntimeControlError::HalfOpenProbeInFlight => TallyError::Unsupported {
                code: "endpoint_circuit_open".to_string(),
            },
            TallyRuntimeControlError::EndpointSessionCapacity => TallyError::Unsupported {
                code: "runtime_capacity_reached".to_string(),
            },
        };
    }
    if let Some(transport) = error.downcast_ref::<TallyTransportError>() {
        return match transport {
            TallyTransportError::EndpointInvalid { .. } => invalid_data("endpoint_invalid"),
            TallyTransportError::PolicyInvalid { .. }
            | TallyTransportError::ClientInitializationFailed => TallyError::Unsupported {
                code: transport.safe_code().to_string(),
            },
            TallyTransportError::RequestTooLarge { .. } => {
                invalid_data("request_size_limit_exceeded")
            }
            TallyTransportError::ResponseTooLarge { .. }
            | TallyTransportError::ResponseTruncated
            | TallyTransportError::ResponseReadFailed
            | TallyTransportError::UnsupportedContentEncoding
            | TallyTransportError::InvalidEncoding { .. }
            | TallyTransportError::HttpStatus { .. } => protocol_error(transport.safe_code()),
            TallyTransportError::ConnectionFailed
            | TallyTransportError::RequestTimedOut
            | TallyTransportError::RequestFailed => TallyError::Unreachable,
        };
    }
    if let Some(read) = error.downcast_ref::<TallyRuntimeReadError>() {
        return match read {
            TallyRuntimeReadError::ApplicationResponseRejected => {
                protocol_error("application_response_rejected")
            }
        };
    }
    protocol_error("unclassified_tally_error")
}

fn classify_voucher_window_error(error: TallyError) -> TallyError {
    match error {
        TallyError::Protocol { code } if code == "response_size_limit_exceeded" => {
            TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            }
        }
        error => error,
    }
}

fn invalid_data(code: &'static str) -> TallyError {
    TallyError::InvalidData {
        code: code.to_string(),
    }
}

fn protocol_error(code: &'static str) -> TallyError {
    TallyError::Protocol {
        code: code.to_string(),
    }
}

fn capability_failure_code(error: &TallyError) -> String {
    match error {
        TallyError::Protocol { code }
        | TallyError::InvalidData { code }
        | TallyError::Unsupported { code } => code.clone(),
        TallyError::Unreachable => "tally_unreachable".to_string(),
        TallyError::ReadResponseTooLarge { .. } => {
            "voucher_response_size_limit_exceeded".to_string()
        }
        TallyError::Cancelled => "canary_cancelled".to_string(),
        TallyError::OutcomeUnknown => "canary_outcome_unknown".to_string(),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, SequenceSimulator};

    #[test]
    fn company_source_identity_is_stable_across_guid_casing() {
        let lowercase = company_source_identity(
            "tally_xml_http:http://127.0.0.1:9000",
            "4c42a771-abcd-4def-8abc-001122aabbcc",
        );
        let mixed_case = company_source_identity(
            "tally_xml_http:http://127.0.0.1:9000",
            "4C42A771-AbCd-4DeF-8AbC-001122AaBbCc",
        );

        assert_eq!(mixed_case, lowercase);
        assert_eq!(
            mixed_case.company_guid,
            "4c42a771-abcd-4def-8abc-001122aabbcc"
        );
    }

    #[test]
    fn empty_sealed_canary_stays_unknown() {
        let window = CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
            bridge_tally_core::CoreAccountingBatch::default(),
        ));

        let evidence = core_canary_capability(&window);

        assert_eq!(evidence.state, CapabilityState::Unknown);
        assert_eq!(
            evidence.safe_reason_code.as_deref(),
            Some("sealed_profile_executed")
        );
        assert_eq!(evidence.confidence, EvidenceConfidence::Observed);
        assert!(core_snapshot_start_authorized(&evidence));
    }

    #[test]
    fn partial_sealed_canary_does_not_promote_the_whole_pack() {
        let window = CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
            bridge_tally_core::CoreAccountingBatch {
                groups: vec![bridge_tally_core::GroupRecord {
                    source_id: "group-guid".to_string(),
                    name: "Assets".to_string(),
                    parent_source_id: None,
                }],
                ..bridge_tally_core::CoreAccountingBatch::default()
            },
        ));

        let evidence = core_canary_capability(&window);

        assert_eq!(evidence.state, CapabilityState::Unknown);
        assert_eq!(
            evidence.safe_reason_code.as_deref(),
            Some("sealed_profile_executed")
        );
        assert!(core_snapshot_start_authorized(&evidence));
    }

    #[test]
    fn fully_populated_canary_does_not_overclaim_field_support() {
        let entry_source_id = "bridge-derived:ledger-entry:v1:synthetic".to_string();
        let window = CanonicalPackWindow {
            batch: PackBatch::CoreAccounting(bridge_tally_core::CoreAccountingBatch {
                groups: vec![
                    bridge_tally_core::GroupRecord {
                        source_id: "root-group".to_string(),
                        name: "Root".to_string(),
                        parent_source_id: None,
                    },
                    bridge_tally_core::GroupRecord {
                        source_id: "child-group".to_string(),
                        name: "Assets".to_string(),
                        parent_source_id: Some("root-group".to_string()),
                    },
                ],
                ledgers: vec![bridge_tally_core::LedgerRecord {
                    source_id: "ledger-guid".to_string(),
                    name: "Cash".to_string(),
                    parent_source_id: Some("child-group".to_string()),
                    opening_balance: Some(ExactDecimal::parse("0").unwrap()),
                }],
                voucher_types: vec![bridge_tally_core::VoucherTypeRecord {
                    source_id: "voucher-type-guid".to_string(),
                    name: "Receipt".to_string(),
                }],
                vouchers: vec![bridge_tally_core::VoucherRecord {
                    source_id: "voucher-guid".to_string(),
                    date_yyyymmdd: "20260716".to_string(),
                    voucher_type_source_id: "voucher-type-guid".to_string(),
                    voucher_number: Some("SYN-1".to_string()),
                    cancelled: false,
                    optional: false,
                }],
                ledger_entries: vec![bridge_tally_core::LedgerEntryRecord {
                    source_id: entry_source_id.clone(),
                    voucher_source_id: "voucher-guid".to_string(),
                    ledger_source_id: "ledger-guid".to_string(),
                    amount: ExactDecimal::parse("0").unwrap(),
                    polarity: bridge_tally_core::LedgerEntryPolarity::Debit,
                }],
            }),
            source_counts: None,
            record_evidence: Some(vec![bridge_tally_core::SourceRecordEvidence {
                object_type: bridge_tally_core::CanonicalText::parse("ledger_entry").unwrap(),
                source_id: bridge_tally_core::SourceRecordId::parse(entry_source_id).unwrap(),
                identity_kind: bridge_tally_core::SourceIdentityKind::Fallback,
                observed_identities: bridge_tally_core::ObservedSourceIdentities::default(),
                raw_source_sha256: bridge_tally_core::RawSourceSha256::parse("0".repeat(64))
                    .unwrap(),
                alter_id: None,
            }]),
        };

        let evidence = core_canary_capability(&window);

        assert_eq!(evidence.state, CapabilityState::Unknown);
        assert_eq!(
            evidence.safe_reason_code.as_deref(),
            Some("sealed_profile_executed")
        );
        assert!(core_snapshot_start_authorized(&evidence));
    }

    #[test]
    fn failed_or_unobserved_canary_cannot_authorize_snapshot_start() {
        for evidence in [
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("voucher_export_invalid".to_string()),
            },
            CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Unknown,
                safe_reason_code: Some("sealed_profile_executed".to_string()),
            },
            CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("release_claimed_support".to_string()),
            },
        ] {
            assert!(!core_snapshot_start_authorized(&evidence));
        }
    }

    #[test]
    fn period_report_company_guid_matching_is_ascii_case_insensitive_only() {
        assert!(company_guids_equal(
            "4C42A771-AbCd-4DeF-8AbC-001122AaBbCc",
            "4c42a771-abcd-4def-8abc-001122aabbcc"
        ));
        assert!(!company_guids_equal("company-guid-a", "company-guid-b"));
        assert!(!company_guids_equal("company-guid", " company-guid "));
    }

    #[tokio::test]
    async fn same_context_snapshot_read_does_not_reuse_pre_run_canary_rows() {
        let company_guid = "synthetic-company-guid";
        let empty_export = |schema: &str, object_type: &str| {
            format!(
                r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{schema}" OBJECTTYPE="{object_type}" NAME="Synthetic Company" GUID="{company_guid}" RECORDCOUNT="0"/></BODY></ENVELOPE>"#
            )
        };
        let second_group = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="GROUP" NAME="Synthetic Company" GUID="{company_guid}" RECORDCOUNT="1"/><GROUP NAME="Post-start Assets" GUID="post-start-group"><PARENT></PARENT></GROUP></BODY></ENVELOPE>"#,
            bridge_tally_protocol::BRIDGE_GROUP_EXPORT_SCHEMA
        );
        let plans = [
            empty_export(bridge_tally_protocol::BRIDGE_GROUP_EXPORT_SCHEMA, "GROUP"),
            empty_export(bridge_tally_protocol::BRIDGE_LEDGER_EXPORT_SCHEMA, "LEDGER"),
            empty_export(
                bridge_tally_protocol::BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
                "VOUCHERTYPE",
            ),
            empty_export(
                bridge_tally_protocol::BRIDGE_VOUCHER_EXPORT_SCHEMA,
                "VOUCHER",
            ),
            second_group,
            empty_export(bridge_tally_protocol::BRIDGE_LEDGER_EXPORT_SCHEMA, "LEDGER"),
            empty_export(
                bridge_tally_protocol::BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
                "VOUCHERTYPE",
            ),
            empty_export(
                bridge_tally_protocol::BRIDGE_VOUCHER_EXPORT_SCHEMA,
                "VOUCHER",
            ),
        ]
        .into_iter()
        .map(Fixture::SyntheticXml)
        .map(ScenarioPlan::new)
        .collect();
        let simulator = SequenceSimulator::spawn(plans).expect("spawn sequence simulator");
        let company = CompanyRef {
            identity: company_source_identity(
                &format!("tally_xml_http:http://{}", simulator.address()),
                company_guid,
            ),
            display_name: "Synthetic Company".to_string(),
        };
        let context = RequestContext {
            run_id: "run-canary-lifecycle".to_string(),
            company: company.clone(),
            pack: CapabilityPackId::CoreAccounting,
            schema_version: CORE_ACCOUNTING_SCHEMA_VERSION,
            window: ReadWindow {
                from_yyyymmdd: "20260701".to_string(),
                to_yyyymmdd: "20260701".to_string(),
            },
            query_profile: bridge_tally_core::CanonicalText::parse(CORE_QUERY_PROFILE).unwrap(),
            filters_sha256: bridge_tally_core::CanonicalText::parse("0".repeat(64)).unwrap(),
        };
        let connector = RuntimeTallyConnector::new(
            TallyRuntime::default(),
            TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            company,
            context.clone(),
        )
        .unwrap();

        let pre_run_canary = connector.extract_core_window(&context).await.unwrap();
        let PackBatch::CoreAccounting(pre_run_batch) = pre_run_canary.batch else {
            panic!("expected core canary batch");
        };
        assert!(pre_run_batch.groups.is_empty());

        let snapshot_window = connector.read_pack_window(&context).await.unwrap();
        let PackBatch::CoreAccounting(snapshot_batch) = snapshot_window.batch else {
            panic!("expected core snapshot batch");
        };
        assert_eq!(snapshot_batch.groups.len(), 1);
        assert_eq!(snapshot_batch.groups[0].source_id, "post-start-group");

        let requests = simulator.finish().expect("finish sequence simulator");
        assert_eq!(requests.len(), 8);
        assert!(requests.iter().all(|request| request.method == "POST"));
    }

    #[test]
    fn only_exact_voucher_response_limit_becomes_adaptive_split_authority() {
        assert!(matches!(
            classify_voucher_window_error(TallyError::Protocol {
                code: "response_size_limit_exceeded".to_string(),
            }),
            TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow
            }
        ));
        assert!(matches!(
            classify_voucher_window_error(TallyError::Protocol {
                code: "response_truncated".to_string(),
            }),
            TallyError::Protocol { code } if code == "response_truncated"
        ));
        assert!(matches!(
            classify_voucher_window_error(TallyError::InvalidData {
                code: "voucher_export_invalid".to_string(),
            }),
            TallyError::InvalidData { code } if code == "voucher_export_invalid"
        ));
    }
}
