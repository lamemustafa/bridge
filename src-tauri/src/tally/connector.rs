use bridge_tally_canonical::build_core_window;
use bridge_tally_core::report_tie_out::{LedgerPeriodBalance, LedgerPeriodBalanceReport};
use bridge_tally_core::{
    CanonicalPackWindow, CapabilityEvidence, CapabilityPackId, CapabilityState, CompanyRef,
    EvidenceConfidence, ExactDecimal, ProbeResult, ReadResponseScope, ReadWindow, RequestContext,
    SourceIdentity, TallyConnector, TallyError, CORE_ACCOUNTING_SCHEMA_VERSION,
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
            Some(_) => Ok(()),
            None => match self.extract_core_window(&self.canary_context).await {
                Ok(window) => self.store_canary_window(window),
                Err(error) => Err(error),
            },
        };
        let core_evidence = match canary_result {
            Ok(()) => CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("nested_entry_identity_inferred".to_string()),
            },
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
            Ok(_) => CapabilityEvidence {
                state: CapabilityState::Supported,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some("nested_entry_identity_inferred".to_string()),
            },
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
