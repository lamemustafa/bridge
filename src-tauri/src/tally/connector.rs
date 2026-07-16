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
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

use super::capability_packs::CapabilityPackRegistry;
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
    canary_window: Arc<Mutex<Option<CanonicalPackWindow>>>,
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
            canary_window: Arc::new(Mutex::new(None)),
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

    fn cached_canary_window(&self) -> Result<Option<CanonicalPackWindow>, TallyError> {
        self.canary_window
            .lock()
            .map(|window| window.clone())
            .map_err(|_| protocol_error("canary_cache_unavailable"))
    }

    fn store_canary_window(&self, window: CanonicalPackWindow) -> Result<(), TallyError> {
        *self
            .canary_window
            .lock()
            .map_err(|_| protocol_error("canary_cache_unavailable"))? = Some(window);
        Ok(())
    }

    fn take_canary_window(&self) -> Result<Option<CanonicalPackWindow>, TallyError> {
        self.canary_window
            .lock()
            .map(|mut window| window.take())
            .map_err(|_| protocol_error("canary_cache_unavailable"))
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
        let cached_canary = self.cached_canary_window()?;
        let mut result = match &cached_canary {
            Some(_) => match self
                .runtime
                .cached_probe(&self.config)
                .map_err(|_| protocol_error("capability_cache_unavailable"))?
            {
                Some(probe) => probe,
                None => self
                    .runtime
                    .probe(self.config.clone())
                    .await
                    .map_err(map_transport_error)?,
            },
            None => self
                .runtime
                .probe(self.config.clone())
                .await
                .map_err(map_transport_error)?,
        };
        let canary_result = match cached_canary {
            Some(window) => Ok(window),
            None => match self.extract_core_window(&self.canary_context).await {
                Ok(window) => {
                    self.store_canary_window(window.clone())?;
                    Ok(window)
                }
                Err(error) => Err(error),
            },
        };
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
        if context == &self.canary_context {
            if let Some(window) = self.take_canary_window()? {
                return Ok(window);
            }
        }
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
                        parsed.context.company_guid == validation_company_guid
                            && parsed.context.from_yyyymmdd == validation_from
                            && parsed.context.to_yyyymmdd == validation_to
                            && parsed.context.ordinary_books_requested
                    })
                },
            )
            .await?;
        let parsed = parse_ledger_period_balance_report(&xml)
            .map_err(|_| protocol_error("period_report_invalid"))?;
        if parsed.context.company_guid != self.company.identity.company_guid
            || parsed.context.from_yyyymmdd != context.window.from_yyyymmdd
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
    let PackBatch::CoreAccounting(batch) = &window.batch else {
        return observed_core_capability(
            CapabilityState::Unknown,
            "sealed_profile_executed_unexpected_pack",
        );
    };
    let mut observed = std::collections::BTreeSet::new();
    let mut record = |object_type: &str, field: &str| {
        observed.insert((object_type.to_string(), field.to_string()));
    };

    if !batch.groups.is_empty() {
        record("group", "source_id");
        record("group", "name");
    }
    if batch
        .groups
        .iter()
        .any(|group| group.parent_source_id.is_some())
    {
        record("group", "parent_source_id");
    }
    if !batch.ledgers.is_empty() {
        record("ledger", "source_id");
        record("ledger", "name");
    }
    if batch
        .ledgers
        .iter()
        .any(|ledger| ledger.parent_source_id.is_some())
    {
        record("ledger", "parent_source_id");
    }
    if batch
        .ledgers
        .iter()
        .any(|ledger| ledger.opening_balance.is_some())
    {
        record("ledger", "opening_balance");
    }
    if !batch.voucher_types.is_empty() {
        record("voucher_type", "source_id");
        record("voucher_type", "name");
    }
    if !batch.vouchers.is_empty() {
        record("voucher", "source_id");
        record("voucher", "date_yyyymmdd");
        record("voucher", "voucher_type_source_id");
        record("voucher", "cancelled");
        record("voucher", "optional");
    }
    if batch
        .vouchers
        .iter()
        .any(|voucher| voucher.voucher_number.is_some())
    {
        record("voucher", "voucher_number");
    }
    if !batch.ledger_entries.is_empty() {
        if window.record_evidence.as_deref().is_some_and(|evidence| {
            evidence.iter().any(|record| {
                record.object_type.as_str() == "ledger_entry"
                    && record.identity_kind != bridge_tally_core::SourceIdentityKind::Fallback
            })
        }) {
            record("ledger_entry", "source_id");
        }
        record("ledger_entry", "voucher_source_id");
        record("ledger_entry", "ledger_source_id");
        record("ledger_entry", "amount");
        record("ledger_entry", "polarity");
    }

    let descriptor = CapabilityPackRegistry::descriptor(CapabilityPackId::CoreAccounting);
    if descriptor.required_fields.iter().all(|required| {
        observed.contains(&(required.object_type.to_string(), required.field.to_string()))
    }) {
        observed_core_capability(
            CapabilityState::Supported,
            "all_required_pack_fields_observed",
        )
    } else if observed.is_empty() {
        observed_core_capability(
            CapabilityState::Unknown,
            "sealed_profile_executed_no_required_fields_observed",
        )
    } else {
        observed_core_capability(
            CapabilityState::Unknown,
            "sealed_profile_executed_incomplete_field_coverage",
        )
    }
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
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-company-observation-v1\0");
    digest.update(lineage.as_bytes());
    digest.update(b"\0");
    digest.update(company_guid.as_bytes());
    SourceIdentity {
        bridge_source_lineage: lineage.to_string(),
        company_guid: company_guid.to_string(),
        observed_fingerprint: hex_lower(&digest.finalize()),
    }
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

    #[test]
    fn empty_sealed_canary_stays_unknown() {
        let window = CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
            bridge_tally_core::CoreAccountingBatch::default(),
        ));

        let evidence = core_canary_capability(&window);

        assert_eq!(evidence.state, CapabilityState::Unknown);
        assert_eq!(
            evidence.safe_reason_code.as_deref(),
            Some("sealed_profile_executed_no_required_fields_observed")
        );
        assert_eq!(evidence.confidence, EvidenceConfidence::Observed);
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
            Some("sealed_profile_executed_incomplete_field_coverage")
        );
    }

    #[test]
    fn fully_populated_canary_with_derived_entry_identity_stays_unknown() {
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
            Some("sealed_profile_executed_incomplete_field_coverage")
        );
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
