use crate::tally::canonical_window::build_core_window;
use bridge_tally_core::report_tie_out::{LedgerPeriodBalance, LedgerPeriodBalanceReport};
use bridge_tally_core::{
    CanonicalPackWindow, CapabilityEvidence, CapabilityPackId, CapabilityState, CompanyRef,
    EvidenceConfidence, ExactDecimal, PackBatch, ProbeResult, ReadResponseScope, ReadWindow,
    RequestContext, SourceIdentity, TallyConnector, TallyDate, TallyError,
    CORE_ACCOUNTING_SCHEMA_VERSION,
};
use bridge_tally_protocol::xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName};
use bridge_tally_protocol::{
    native_outstandings::{
        render_native_group_snapshot_request, render_native_ledger_export_request,
        render_native_voucher_export_request, render_native_voucher_type_export_request,
    },
    outstandings_shared::{parse_company_book_extent, require_master_witness, CompanyBookExtent},
    parse_companies_from_collection, parse_ledger_period_balance_report,
    parse_native_group_source_records_with_evidence,
    parse_native_ledger_source_records_with_evidence,
    parse_native_voucher_source_records_with_evidence,
    parse_native_voucher_type_source_records_with_evidence, ParsedExport, ParsedSourceRecord,
    TallyNamedMaster,
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
        // Native Collection exports deliberately carry no company GUID. Bind
        // the group rows out-of-band: each extent response is GUID-verified,
        // each extent is internally paired, and the unchanged opening/closing
        // extent brackets two equal native collection responses. This is not weaker
        // than the retired report field: Tally renders FIELD amounts for
        // display and has been observed dropping their sign, so it cannot be
        // a trustworthy company-authentication channel for a money path.
        let opening_extent = self
            .read_pinned_company_book_extent(&company_name, &expected_guid)
            .await?;
        let native_group_request = render_native_group_snapshot_request(&company_name);
        let first_group_xml = self
            .post_xml_validated(native_group_request.clone(), {
                let validation_guid = expected_guid.clone();
                move |xml| {
                    parse_native_group_source_records_with_evidence(xml, &validation_guid).is_ok()
                }
            })
            .await?;
        let second_group_xml = self
            .post_xml_validated(native_group_request, {
                let validation_guid = expected_guid.clone();
                move |xml| {
                    parse_native_group_source_records_with_evidence(xml, &validation_guid).is_ok()
                }
            })
            .await?;
        if first_group_xml != second_group_xml {
            return Err(invalid_data("native_group_snapshot_drifted"));
        }
        let groups = native_groups_for_core_window(&first_group_xml, &expected_guid)?;
        let closing_extent = self
            .read_pinned_company_book_extent(&company_name, &expected_guid)
            .await?;
        if closing_extent != opening_extent {
            return Err(invalid_data("company_book_changed_during_group_read"));
        }

        // Unlike the native Group collection, every Ledger row carries a
        // master GUID. Pair the collection and bracket it with the same
        // GUID-verified book extent used above, then require at least one row
        // to bind to the selected company. A foreign per-row prefix remains
        // evidence, not a hard failure: imported masters may retain it.
        let ledger_opening_extent = self
            .read_pinned_company_book_extent(&company_name, &expected_guid)
            .await?;
        let native_ledger_request = render_native_ledger_export_request(&company_name);
        let validation_guid = expected_guid.clone();
        let first_ledger_xml = self
            .post_xml_validated(native_ledger_request.clone(), move |xml| {
                parse_native_ledger_source_records_with_evidence(xml, &validation_guid).is_ok()
            })
            .await?;
        let validation_guid = expected_guid.clone();
        let second_ledger_xml = self
            .post_xml_validated(native_ledger_request, move |xml| {
                parse_native_ledger_source_records_with_evidence(xml, &validation_guid).is_ok()
            })
            .await?;
        if first_ledger_xml != second_ledger_xml {
            return Err(invalid_data("native_ledger_snapshot_drifted"));
        }
        let ledgers =
            parse_native_ledger_source_records_with_evidence(&first_ledger_xml, &expected_guid)
                .map_err(|_| protocol_error("ledger_export_invalid"))?;
        let ledger_closing_extent = self
            .read_pinned_company_book_extent(&company_name, &expected_guid)
            .await?;
        if ledger_closing_extent != ledger_opening_extent {
            return Err(invalid_data("company_book_changed_during_ledger_read"));
        }

        let validation_guid = expected_guid.clone();
        let voucher_type_xml = self
            .post_xml_validated(
                render_native_voucher_type_export_request(&company_name),
                move |xml| {
                    parse_native_voucher_type_source_records_with_evidence(xml, &validation_guid)
                        .is_ok()
                },
            )
            .await?;
        let voucher_types = parse_native_voucher_type_source_records_with_evidence(
            &voucher_type_xml,
            &expected_guid,
        )
        .map_err(|_| protocol_error("voucher_type_export_invalid"))?;

        // A native Voucher collection has no envelope company GUID. For a
        // non-empty response its row GUIDs bind the company; for a valid
        // empty window the same unchanged, GUID-verified book extent bracket
        // used for groups authenticates the selected book out of band.
        let voucher_opening_extent = self
            .read_pinned_company_book_extent(&company_name, &expected_guid)
            .await?;
        // Fail closed: `from`/`to` feed a quoted `$$Date:"..."` TDL formula
        // argument, where XML escaping alone cannot contain an embedded
        // quote (Tally decodes `&quot;` back to `"` before evaluating the
        // formula). Requiring a validated `TallyDate` -- exactly 8 ASCII
        // digits -- closes that off at the source instead of sanitising.
        let voucher_window_from = TallyDate::parse(context.window.from_yyyymmdd.clone())?;
        let voucher_window_to = TallyDate::parse(context.window.to_yyyymmdd.clone())?;
        let validation_guid = expected_guid.clone();
        let voucher_xml = self
            .post_xml_validated(
                render_native_voucher_export_request(
                    &company_name,
                    &voucher_window_from,
                    &voucher_window_to,
                ),
                move |xml| {
                    parse_native_voucher_source_records_with_evidence(xml, &validation_guid).is_ok()
                },
            )
            .await
            .map_err(classify_voucher_window_error)?;
        let vouchers =
            parse_native_voucher_source_records_with_evidence(&voucher_xml, &expected_guid)
                .map_err(|_| protocol_error("voucher_export_invalid"))?;
        let voucher_closing_extent = self
            .read_pinned_company_book_extent(&company_name, &expected_guid)
            .await?;
        if voucher_closing_extent != voucher_opening_extent {
            return Err(invalid_data("company_book_changed_during_voucher_read"));
        }

        build_core_window(context, groups, ledgers, voucher_types, vouchers)
    }

    async fn read_pinned_company_book_extent(
        &self,
        company_name: &str,
        expected_guid: &str,
    ) -> Result<CompanyBookExtent, TallyError> {
        let company = ValidatedCompanyName::new(company_name.to_owned())
            .map_err(|_| invalid_data("company_name_invalid"))?;
        let request = ReadOnlyProfile::CompanyBookExtentV1 { company: &company }.render();
        let first_guid = expected_guid.to_owned();
        let first_xml = self
            .post_xml_validated(request.clone(), move |xml| {
                parse_company_book_extent(xml, company_name, &first_guid).is_ok()
            })
            .await?;
        let second_guid = expected_guid.to_owned();
        let second_xml = self
            .post_xml_validated(request, move |xml| {
                parse_company_book_extent(xml, company_name, &second_guid).is_ok()
            })
            .await?;
        let first = parse_company_book_extent(&first_xml, company_name, expected_guid)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;
        let second = parse_company_book_extent(&second_xml, company_name, expected_guid)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;
        if first != second {
            return Err(invalid_data("company_book_extent_drifted"));
        }
        // The parser stays tolerant of an absent ALTMSTID (older captures still parse), but this
        // is the core-window bracket itself: fail closed here so a witness-less pair can never be
        // mistaken for a stable one. See `require_master_witness` for why.
        require_master_witness(&first).map_err(|_| invalid_data("company_altmstid_missing"))?;
        Ok(first)
    }

    async fn snapshot_probe(&self) -> Result<ProbeResult, TallyError> {
        let (_, mut result) = self
            .runtime
            .snapshot_probe_with_observation(self.config.clone(), &self.company.display_name)
            .await
            .map_err(map_transport_error)?;
        let matching_companies = result
            .companies
            .iter()
            .filter(|company| {
                company.guid.as_deref().is_some_and(|guid| {
                    company_guids_equal(guid, &self.company.identity.company_guid)
                })
            })
            .take(2)
            .count();
        if matching_companies != 1 {
            return Err(protocol_error(if matching_companies == 0 {
                "company_identity_not_found"
            } else {
                "company_identity_ambiguous"
            }));
        }
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
}

#[async_trait::async_trait]
impl TallyConnector for RuntimeTallyConnector {
    async fn probe(&self) -> Result<ProbeResult, TallyError> {
        self.snapshot_probe().await
    }

    async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
        self.snapshot_probe().await
    }

    async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
        // A reviewed setup consumes its interactive probe cache before the snapshot starts.
        // Runtime discovery is a fresh, validated company-list read and must not depend on or
        // recreate that single-use UI authority.
        //
        // Uses Tally's documented `Company` collection (`ReadOnlyProfile::CompanyListV2`)
        // rather than the legacy `CompanyListV1` custom TDL report: the collection always
        // answers with the ordinary shaped `HEADER/STATUS=1` envelope, so `parse_companies_from_collection`
        // can require that shape outright instead of depending on a report-rendering path that
        // one Tally instance is known to hang on and another to answer inconsistently.
        let lineage = source_lineage(&self.config)?;
        let companies = parse_companies_from_collection(
            &self
                .post_xml_validated(ReadOnlyProfile::CompanyListV2.render(), |xml| {
                    parse_companies_from_collection(xml).is_ok()
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
    core_snapshot_start_authorized_codes(
        capability_state_code(evidence.state),
        evidence_confidence_code(evidence.confidence),
        evidence.safe_reason_code.as_deref(),
    )
}

/// Storage-level form of [`core_snapshot_start_authorized`]. Persisted restart evidence must use
/// this predicate too, so a resume cannot accidentally drift back to the broader `Supported +
/// Observed` convention used by other capability packs.
pub(crate) fn core_snapshot_start_authorized_codes(
    state: &str,
    confidence: &str,
    safe_reason_code: Option<&str>,
) -> bool {
    state == "unknown"
        && confidence == "observed"
        && safe_reason_code == Some("sealed_profile_executed")
}

fn capability_state_code(state: CapabilityState) -> &'static str {
    match state {
        CapabilityState::Supported => "supported",
        CapabilityState::Unsupported => "unsupported",
        CapabilityState::Unknown => "unknown",
        CapabilityState::NotConfigured => "not_configured",
    }
}

fn evidence_confidence_code(confidence: EvidenceConfidence) -> &'static str {
    match confidence {
        EvidenceConfidence::Documented => "documented",
        EvidenceConfidence::Observed => "observed",
        EvidenceConfidence::Inferred => "inferred",
        EvidenceConfidence::Unknown => "unknown",
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

fn native_groups_for_core_window(
    xml: &str,
    expected_company_guid: &str,
) -> Result<ParsedExport<ParsedSourceRecord<TallyNamedMaster>>, TallyError> {
    parse_native_group_source_records_with_evidence(xml, expected_company_guid)
        .map_err(|_| protocol_error("group_export_invalid"))
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
pub(crate) fn simulator_test_lock() -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;

    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, SequenceSimulator, WireEncoding};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        task::JoinHandle,
    };

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

    fn decode_bomless_utf16le_capture(bytes: &[u8]) -> String {
        bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
            bytes,
            "text/xml; charset=utf-16",
            bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
            bytes.len(),
        )
        .expect("captured BOM-less UTF-16LE response decodes")
        .text
    }

    /// Carries `ALTMSTID` -- unlike a plain company-list row -- because this
    /// is the shape of `CompanyBookExtentV1`'s response, and every core-window
    /// bracket test below routes an extent read through the production
    /// bracket (`read_pinned_company_book_extent`), which now requires that
    /// witness to be present. See `core_window_bracket_fails_closed_when_altmstid_is_absent`
    /// for the case where it deliberately is not.
    fn company_extent(company_name: &str, company_guid: &str) -> String {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="{company_name}"><LASTVOUCHERDATE TYPE="Date">20260701</LASTVOUCHERDATE><BOOKSFROM TYPE="Date">20240101</BOOKSFROM><NAME TYPE="String">{company_name}</NAME><GUID TYPE="String">{company_guid}</GUID><ALTMSTID TYPE="Number">1</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    }

    fn native_groups(company_guid: &str, groups: &[(&str, &str)]) -> String {
        let groups = if groups.is_empty() {
            &[("Primary", "Primary")][..]
        } else {
            groups
        };
        let rows = groups
            .iter()
            .enumerate()
            .map(|(index, (name, parent))| {
                format!(
                    r#"<GROUP NAME="{name}"><GUID TYPE="String">{company_guid}-{index:08x}</GUID><PARENT TYPE="String">{parent}</PARENT><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">{index}</MASTERID></GROUP>"#
                )
            })
            .collect::<String>();
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{rows}</COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    }

    fn native_ledgers(company_guid: &str) -> String {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME="Synthetic Ledger"><GUID TYPE="String">{company_guid}-00000001</GUID><PARENT TYPE="String">Primary</PARENT><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">1</MASTERID><OPENINGBALANCE TYPE="Amount">0.00</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    }

    fn native_voucher_types(company_guid: &str) -> String {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHERTYPE NAME="Synthetic Voucher Type"><GUID TYPE="String">{company_guid}-00000002</GUID><PARENT TYPE="String">Synthetic Voucher Type</PARENT><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">2</MASTERID></VOUCHERTYPE></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    }

    fn native_vouchers(company_guid: &str) -> String {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER REMOTEID="{company_guid}-00000003"><DATE TYPE="Date">20260701</DATE><GUID>{company_guid}-00000003</GUID><VOUCHERTYPENAME>Synthetic Voucher Type</VOUCHERTYPENAME><VOUCHERNUMBER>1</VOUCHERNUMBER><ISCANCELLED TYPE="Logical">No</ISCANCELLED><ISOPTIONAL TYPE="Logical">No</ISOPTIONAL><ALTERID TYPE="Number">1</ALTERID><MASTERID TYPE="Number">3</MASTERID><ALLLEDGERENTRIES.LIST><LEDGERNAME TYPE="String">Synthetic Ledger</LEDGERNAME><ISDEEMEDPOSITIVE TYPE="Logical">Yes</ISDEEMEDPOSITIVE><AMOUNT TYPE="Amount">-1.00</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME TYPE="String">Synthetic Ledger</LEDGERNAME><ISDEEMEDPOSITIVE TYPE="Logical">No</ISDEEMEDPOSITIVE><AMOUNT TYPE="Amount">1.00</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"#
        )
    }

    #[test]
    fn captured_wr2_native_core_window_builds_without_report_field_amounts() {
        use bridge_tally_protocol::{
            native_outstandings::{
                render_native_voucher_export_request, render_native_voucher_type_export_request,
            },
            parse_native_voucher_source_records_with_evidence,
            parse_native_voucher_type_source_records_with_evidence,
        };

        const COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
        const COMPANY: &str = "WR2 Unicode Lab";
        const GROUPS: &[u8] = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_wr2_with_identity.utf16le.xml"
        );
        const LEDGERS: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/ledgers_native_wr2_core_window.xml"
        );
        const VOUCHER_TYPES: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/voucher_types_native_wr2.xml"
        );
        const VOUCHERS: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/vouchers_native_wr2.xml"
        );

        let voucher_type_request = render_native_voucher_type_export_request(COMPANY);
        assert!(voucher_type_request.contains("<TYPE>Collection</TYPE>"));
        assert!(voucher_type_request.contains("<ID>List of VoucherTypes</ID>"));
        assert!(
            voucher_type_request.contains("<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH>")
        );
        assert!(render_native_group_snapshot_request(COMPANY)
            .contains("<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME</FETCH>"));
        assert!(!voucher_type_request.contains("<REPORT>"));
        let probe_from = TallyDate::parse("20260401").unwrap();
        let probe_to = TallyDate::parse("20260930").unwrap();
        assert!(
            render_native_voucher_export_request(COMPANY, &probe_from, &probe_to)
                .contains("ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE")
        );

        let context = RequestContext {
            run_id: "wr2-native-core-window".to_string(),
            company: CompanyRef {
                identity: company_source_identity("synthetic-lineage", COMPANY_GUID),
                display_name: COMPANY.to_string(),
            },
            pack: CapabilityPackId::CoreAccounting,
            schema_version: CORE_ACCOUNTING_SCHEMA_VERSION,
            window: ReadWindow {
                from_yyyymmdd: "20260401".to_string(),
                to_yyyymmdd: "20260930".to_string(),
            },
            query_profile: bridge_tally_core::CanonicalText::parse(CORE_QUERY_PROFILE).unwrap(),
            filters_sha256: bridge_tally_core::CanonicalText::parse("0".repeat(64)).unwrap(),
        };
        let vouchers = parse_native_voucher_source_records_with_evidence(VOUCHERS, COMPANY_GUID)
            .expect("captured vouchers parse");
        assert_eq!(vouchers.records[0].record.ledger_entries.len(), 2);
        assert_eq!(
            vouchers.records[0]
                .record
                .ledger_entries
                .iter()
                .map(|entry| entry.amount.as_str())
                .collect::<Vec<_>>(),
            ["-101.01", "101.01"]
        );
        assert_eq!(
            vouchers.records[0].record.ledger_entries[0].ledger_name,
            "नमस्ते ट्रेडर्स"
        );
        let groups = decode_bomless_utf16le_capture(GROUPS);
        let window = build_core_window(
            &context,
            native_groups_for_core_window(&groups, COMPANY_GUID).expect("captured groups parse"),
            parse_native_ledger_source_records_with_evidence(LEDGERS, COMPANY_GUID)
                .expect("captured ledgers parse"),
            parse_native_voucher_type_source_records_with_evidence(VOUCHER_TYPES, COMPANY_GUID)
                .expect("captured voucher types parse"),
            vouchers,
        )
        .expect("captured native core window builds");
        let PackBatch::CoreAccounting(batch) = window.batch else {
            panic!("wrong pack");
        };
        assert_eq!(
            (
                batch.groups.len(),
                batch.ledgers.len(),
                batch.voucher_types.len(),
                batch.vouchers.len(),
            ),
            (29, 9, 24, 3)
        );
        assert_eq!(
            batch
                .ledgers
                .iter()
                .filter(|ledger| ledger.parent_source_id.is_none())
                .count(),
            1
        );
        assert_eq!(batch.ledger_entries.len(), 6);
        for (voucher, expected_amounts) in batch.vouchers.iter().zip([
            ["-101.01", "101.01"],
            ["-102.02", "102.02"],
            ["-103.03", "103.03"],
        ]) {
            let amounts = batch
                .ledger_entries
                .iter()
                .filter(|entry| entry.voucher_source_id == voucher.source_id)
                .map(|entry| entry.amount.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                amounts,
                expected_amounts.map(|amount| ExactDecimal::parse(amount).expect("test decimal"))
            );
        }
    }

    #[test]
    fn captured_aarav_native_master_parents_resolve_to_the_canonical_tree() {
        use std::collections::{BTreeMap, BTreeSet};

        const COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";
        const GROUPS: &[u8] = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_aarav_with_identity.utf16le.xml"
        );
        const LEDGERS: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/ledgers_native_aarav.xml"
        );

        // There is no captured voucher or voucher-type export for this company. The complete
        // window consequently cannot be constructed without fabricating two inputs, so exercise
        // the captured Group and Ledger bytes through the exact production conversion and parent
        // resolution paths instead.
        let group_xml = decode_bomless_utf16le_capture(GROUPS);
        let groups =
            native_groups_for_core_window(&group_xml, COMPANY_GUID).expect("captured groups parse");
        let ledgers = parse_native_ledger_source_records_with_evidence(LEDGERS, COMPANY_GUID)
            .expect("captured ledgers parse");
        assert_eq!((groups.records.len(), ledgers.records.len()), (28, 88));
        let group_ids_by_name = groups
            .records
            .iter()
            .map(|group| {
                (
                    group.record.name.clone(),
                    group.source_id.clone().expect("native group GUID"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            group_ids_by_name.len(),
            28,
            "captured group names remain unique"
        );
        let group_source_ids = group_ids_by_name
            .values()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let resolved_group_parents = groups
            .records
            .iter()
            .map(|group| {
                crate::tally::canonical_window::resolve_group_parent(
                    group.record.parent.as_deref(),
                    &group_ids_by_name,
                    "group_parent_missing",
                )
                .expect("captured group parent resolves")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            resolved_group_parents
                .iter()
                .filter(|parent| parent.is_none())
                .count(),
            15,
            "every captured reserved-root group remains rooted at None"
        );
        assert!(resolved_group_parents
            .iter()
            .flatten()
            .all(|parent| group_source_ids.contains(parent.as_str())));

        let resolved_ledger_parents = ledgers
            .records
            .iter()
            .map(|ledger| {
                crate::tally::canonical_window::resolve_optional_reference(
                    ledger.record.parent.as_deref(),
                    &group_ids_by_name,
                    "ledger_parent_group_missing",
                )
                .expect("captured ledger parent resolves")
            })
            .collect::<Vec<_>>();
        let (profit_and_loss, profit_and_loss_parent) = ledgers
            .records
            .iter()
            .zip(&resolved_ledger_parents)
            .find(|(ledger, _)| ledger.record.name == "Profit & Loss A/c")
            .expect("captured root-parented ledger is present");
        assert_eq!(profit_and_loss_parent, &None);
        assert_eq!(
            profit_and_loss
                .record
                .opening_balance
                .as_ref()
                .map(|balance| ExactDecimal::parse(balance.clone()).expect("exact opening balance"))
                .as_ref()
                .map(ExactDecimal::as_str),
            Some("18255356.27")
        );
        assert_eq!(
            resolved_ledger_parents
                .iter()
                .flatten()
                .filter(|parent| group_source_ids.contains(parent.as_str()))
                .count(),
            87,
            "every other captured ledger parent resolves to an exported group"
        );
        for (group_name, expected_count) in [("Sundry Debtors", 40), ("Sundry Creditors", 20)] {
            let source_id = group_ids_by_name
                .get(group_name)
                .map(String::as_str)
                .expect("captured parent group is present");
            assert_eq!(
                resolved_ledger_parents
                    .iter()
                    .filter(|parent| parent.as_deref() == Some(source_id))
                    .count(),
                expected_count,
                "captured child ledgers retain their resolved parent"
            );
        }
    }

    async fn spawn_method_routed_server(
        post_responses: Vec<String>,
    ) -> (SocketAddr, JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind routed Tally server");
        let address = listener.local_addr().expect("routed server address");
        let worker = tokio::spawn(async move {
            let mut post_responses = post_responses.into_iter();
            let mut posts_remaining = post_responses.len();
            let mut methods = Vec::new();
            while posts_remaining > 0 {
                let (mut socket, _) = listener.accept().await.expect("accept routed request");
                let mut request = Vec::new();
                let (header_end, content_length) = loop {
                    let mut buffer = [0_u8; 8 * 1024];
                    let read = socket.read(&mut buffer).await.expect("read routed request");
                    assert!(read > 0, "routed request closed before its headers");
                    request.extend_from_slice(&buffer[..read]);
                    assert!(
                        request.len() <= 256 * 1024,
                        "routed request exceeded test bound"
                    );
                    let Some(header_end) = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|position| position + 4)
                    else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end.saturating_add(content_length) {
                        break (header_end, content_length);
                    }
                };
                let request_line = String::from_utf8_lossy(&request[..header_end])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let method = request_line.split_whitespace().next().unwrap_or_default();
                methods.push(method.to_string());
                let response = if method == "GET" {
                    utf8_status_response("<RESPONSE>TallyPrime Server is Running</RESPONSE>")
                } else {
                    assert_eq!(request.len(), header_end + content_length);
                    posts_remaining -= 1;
                    utf16_xml_response(post_responses.next().expect("next routed POST response"))
                };
                socket
                    .write_all(&response)
                    .await
                    .expect("write routed response");
            }
            methods
        });
        (address, worker)
    }

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
                ..bridge_tally_core::CoreAccountingBatch::default()
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
    async fn duplicate_company_snapshot_probe_stops_before_core_exports() {
        let _simulator_guard = simulator_test_lock().lock().await;
        let company_guid = "synthetic-company-guid";
        // `TallyClient::probe` requests the trusted `Company` collection
        // (`CompanyListV2`) first, so the two duplicate-GUID rows this test
        // exercises are expressed in that shape rather than the legacy
        // `CompanyListV1` direct report.
        let duplicate_company_xml = format!(
            r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company A"><GUID TYPE="String">{company_guid}</GUID></COMPANY><COMPANY NAME="Synthetic Company B"><GUID TYPE="String">{company_guid}</GUID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        let (address, server) = spawn_method_routed_server(vec![duplicate_company_xml]).await;
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let company = CompanyRef {
            identity: company_source_identity(
                &format!("tally_xml_http:http://{address}"),
                company_guid,
            ),
            display_name: "Synthetic Company A".to_string(),
        };
        let context = RequestContext {
            run_id: "run-ambiguous-company-probe".to_string(),
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
        let connector =
            RuntimeTallyConnector::new(TallyRuntime::default(), config, company, context).unwrap();

        let error = connector
            .probe()
            .await
            .expect_err("ambiguous company identity must stop the canary");
        assert!(matches!(
            error,
            TallyError::Protocol { code } if code == "company_identity_ambiguous"
        ));
        let methods = server.await.expect("join routed Tally server");
        assert_eq!(methods, ["GET", "POST"]);
    }

    /// `discover_companies` requests the native `Company` collection
    /// (`ReadOnlyProfile::CompanyListV2`) and must fail closed -- rejecting the
    /// whole discovery read -- when a company row in that collection omits its
    /// `GUID`, rather than silently returning an identity-less company.
    #[tokio::test]
    async fn discover_companies_fails_closed_without_a_guid() {
        let _simulator_guard = simulator_test_lock().lock().await;
        let company_guid = "synthetic-company-guid";
        let missing_guid_xml =
            r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
                .to_string();
        let (address, server) = spawn_method_routed_server(vec![missing_guid_xml]).await;
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let company = CompanyRef {
            identity: company_source_identity(
                &format!("tally_xml_http:http://{address}"),
                company_guid,
            ),
            display_name: "Synthetic Company".to_string(),
        };
        let context = RequestContext {
            run_id: "run-discover-companies-missing-guid".to_string(),
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
        let connector =
            RuntimeTallyConnector::new(TallyRuntime::default(), config, company, context).unwrap();

        let error = connector
            .discover_companies()
            .await
            .expect_err("a company row without a GUID must fail closed");
        // The wire-integrity validation closure re-parses with the same
        // strict collection parser, so it rejects the response before the
        // outer `parse_companies_from_collection` call (and its
        // `company_export_invalid` mapping) is ever reached.
        assert!(matches!(
            error,
            TallyError::Protocol { code } if code == "application_response_rejected"
        ));
        let methods = server.await.expect("join routed Tally server");
        assert_eq!(methods, ["POST"]);
    }

    /// The core-window bracket (`read_pinned_company_book_extent`, feeding
    /// `extract_core_window`/`read_pack_window`) must fail closed with a
    /// typed error when both paired reads agree but neither carries
    /// `ALTMSTID` -- the exact case the review flagged: two witness-less
    /// extents compare equal, so an ordinary `first != second` drift check
    /// alone cannot tell a stable book from one where a GROUP/LEDGER master
    /// moved mid-window without a signal to detect it. This uses the real,
    /// unmodified `unit_a_company_extent_live.xml` capture -- from before
    /// `ALTMSTID` was added to the fetch list -- rather than a synthetic
    /// response, so the absence being tested is the one Tally has actually
    /// produced.
    #[tokio::test]
    async fn core_window_bracket_fails_closed_when_altmstid_is_absent() {
        let _simulator_guard = simulator_test_lock().lock().await;
        const COMPANY_EXTENT_WITHOUT_ALTMSTID: &str = include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
        );
        const COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";
        const COMPANY: &str = "Aarav Trading Company Demo";
        assert!(
            !COMPANY_EXTENT_WITHOUT_ALTMSTID.contains("ALTMSTID"),
            "this fixture must predate the ALTMSTID fetch for this test to prove anything"
        );

        let (address, server) = spawn_method_routed_server(vec![
            COMPANY_EXTENT_WITHOUT_ALTMSTID.to_string(),
            COMPANY_EXTENT_WITHOUT_ALTMSTID.to_string(),
        ])
        .await;
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let company = CompanyRef {
            identity: company_source_identity(
                &format!("tally_xml_http:http://{address}"),
                COMPANY_GUID,
            ),
            display_name: COMPANY.to_string(),
        };
        let context = RequestContext {
            run_id: "run-core-window-missing-altmstid".to_string(),
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
        let connector =
            RuntimeTallyConnector::new(TallyRuntime::default(), config, company, context.clone())
                .unwrap();

        let error = connector
            .read_pack_window(&context)
            .await
            .expect_err("a core-window bracket formed without ALTMSTID must fail closed");
        assert!(
            matches!(
                &error,
                TallyError::InvalidData { code } if code == "company_altmstid_missing"
            ),
            "unexpected error: {error:?}"
        );
        let methods = server.await.expect("join routed Tally server");
        assert_eq!(methods, ["POST", "POST"]);
    }

    #[tokio::test]
    async fn direct_company_snapshot_probe_reverifies_scoped_identity_before_core_exports() {
        let _simulator_guard = simulator_test_lock().lock().await;
        assert!(
            parse_native_group_source_records_with_evidence(
                &native_groups("scoped-guid", &[]),
                "scoped-guid"
            )
            .is_ok(),
            "synthetic native groups satisfy the production identity parser"
        );
        let direct = "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>direct-guid-must-not-escape</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>";
        // `fetch_companies` (called from `bootstrap_direct_company`, the third
        // POST below) now requests the native `Company` collection
        // (`ReadOnlyProfile::CompanyListV2`) rather than the legacy
        // `CompanyListV1` direct report, so its response carries that shape.
        // Its GUID must still not escape into the returned identity -- only
        // the fourth, scoped `standard` read may do that.
        let discovered = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><GUID TYPE="String">discovered-guid-must-not-escape</GUID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let standard = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO /></DESC><DATA><COLLECTION MSTDEPTYPE=\"Ledger\" ISMSTDEPTYPE=\"Yes\"><SyntheticLedger NAME=\"synthetic-ledger\" RESERVEDNAME=\"\"><GUID TYPE=\"String\">ledger-guid</GUID><PARENT TYPE=\"String\">Primary</PARENT><BRIDGECOMPANYGUID TYPE=\"String\">scoped-guid</BRIDGECOMPANYGUID><BRIDGECOMPANYNAME TYPE=\"String\">Synthetic Company</BRIDGECOMPANYNAME><LANGUAGENAME.LIST><LANGUAGEID>1033</LANGUAGEID></LANGUAGENAME.LIST></SyntheticLedger></COLLECTION></DATA></BODY></ENVELOPE>";
        let (address, server) = spawn_method_routed_server(vec![
            // `TallyClient::probe` requests the trusted `Company` collection
            // (`CompanyListV2`) first. This responder rejects it (the bare
            // direct report has no `HEADER`/`STATUS` at all), so `probe` falls
            // back to the legacy `CompanyListV1` report — one extra `direct`
            // response ahead of the pre-existing legacy sequence below.
            direct.to_string(),
            direct.to_string(),
            discovered.to_string(),
            standard.to_string(),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            native_groups("scoped-guid", &[]),
            native_groups("scoped-guid", &[]),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            native_ledgers("scoped-guid"),
            native_ledgers("scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            native_voucher_types("scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            native_vouchers("scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
            company_extent("Synthetic Company", "scoped-guid"),
        ])
        .await;
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let company = CompanyRef {
            identity: company_source_identity(
                &format!("tally_xml_http:http://{address}"),
                "scoped-guid",
            ),
            display_name: "Synthetic Company".to_string(),
        };
        let context = RequestContext {
            run_id: "run-direct-company-snapshot-probe".to_string(),
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
        let connector =
            RuntimeTallyConnector::new(TallyRuntime::default(), config, company, context).unwrap();

        let probe = connector
            .probe()
            .await
            .expect("scoped re-verification should admit the snapshot probe");
        let methods = server.await.expect("join routed Tally server");
        let core_evidence = probe
            .profile
            .packs
            .get(&CapabilityPackId::CoreAccounting)
            .unwrap();
        assert!(
            core_snapshot_start_authorized(core_evidence),
            "core evidence was {core_evidence:?}; methods were {methods:?}"
        );
        assert_eq!(methods.len(), 23);
        assert_eq!(methods[0], "GET");
        assert!(methods[1..].iter().all(|method| method == "POST"));
    }

    #[tokio::test]
    async fn snapshot_start_and_end_probes_preserve_setup_review_and_read_transport_freshly() {
        let _simulator_guard = simulator_test_lock().lock().await;
        let company_guid = "synthetic-company-guid";
        // `TallyClient::probe` requests the trusted `Company` collection
        // (`CompanyListV2`) first; this shape is what every `probe`/`probe_fresh`
        // POST below answers with, so the trusted path resolves the company in
        // one request and never falls back to the legacy, explicitly-untrusted
        // direct report.
        let company_collection_xml = format!(
            r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><GUID TYPE="String">{company_guid}</GUID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        // `discover_companies` now posts the native `Company` collection
        // (`ReadOnlyProfile::CompanyListV2`) directly and parses it with
        // `parse_companies_from_collection`, so its one response below is the
        // same shape as `company_collection_xml`.
        let company_xml = company_collection_xml.clone();
        let mut post_responses = vec![company_collection_xml.clone()];
        for _ in 0..2 {
            post_responses.extend([
                company_collection_xml.clone(),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
                native_groups(company_guid, &[]),
                native_groups(company_guid, &[]),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
                native_ledgers(company_guid),
                native_ledgers(company_guid),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
                native_voucher_types(company_guid),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
                native_vouchers(company_guid),
                company_extent("Synthetic Company", company_guid),
                company_extent("Synthetic Company", company_guid),
            ]);
        }
        post_responses.push(company_xml.clone());
        let (address, server) = spawn_method_routed_server(post_responses).await;
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let runtime = TallyRuntime::default();
        let (review_id, observed_at_unix_ms, reviewed) = runtime
            .probe_with_observation(config.clone())
            .await
            .expect("install interactive setup review");
        assert!(reviewed.connection.reachable);
        let reviewed_company_count = reviewed.companies.len();
        let reviewed_product = reviewed.profile.product.clone();

        let company = CompanyRef {
            identity: company_source_identity(
                &format!("tally_xml_http:http://{address}"),
                company_guid,
            ),
            display_name: "Synthetic Company".to_string(),
        };
        let context = RequestContext {
            run_id: "run-uncached-probes".to_string(),
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
        let connector =
            RuntimeTallyConnector::new(runtime.clone(), config.clone(), company, context).unwrap();

        let start = connector.probe().await.expect("snapshot start probe");
        let end = connector.probe_fresh().await.expect("snapshot end probe");
        let start_evidence = start
            .profile
            .packs
            .get(&CapabilityPackId::CoreAccounting)
            .unwrap();
        assert!(
            core_snapshot_start_authorized(start_evidence),
            "start evidence was {start_evidence:?}"
        );
        let end_evidence = end
            .profile
            .packs
            .get(&CapabilityPackId::CoreAccounting)
            .unwrap();
        assert!(
            core_snapshot_start_authorized(end_evidence),
            "end evidence was {end_evidence:?}"
        );

        let mut preserved = runtime
            .reserve_cached_probe_fresh(&config, &review_id, 300_000)
            .expect("review cache remains readable")
            .expect("snapshot probes preserve interactive review");
        assert_eq!(preserved.review_id(), review_id);
        assert_eq!(preserved.observed_at_unix_ms(), observed_at_unix_ms);
        assert_eq!(preserved.result().companies.len(), reviewed_company_count);
        assert_eq!(preserved.result().profile.product, reviewed_product);
        assert!(preserved.release().expect("release preserved review"));

        let mut consumed = runtime
            .reserve_cached_probe_fresh(&config, &review_id, 300_000)
            .expect("review cache remains reservable")
            .expect("preserved review is still present");
        assert!(consumed.consume().expect("consume setup review"));
        assert!(runtime.cached_probe(&config).unwrap().is_none());
        let discovered = connector
            .discover_companies()
            .await
            .expect("snapshot discovery is independent of consumed setup review");
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].identity, connector.company.identity);

        let methods = server.await.expect("join routed Tally server");
        assert!(methods
            .iter()
            .all(|method| method == "GET" || method == "POST"));
        assert_eq!(
            methods
                .iter()
                .filter(|method| method.as_str() == "GET")
                .count(),
            3
        );
        assert_eq!(
            methods
                .iter()
                .filter(|method| method.as_str() == "POST")
                .count(),
            40
        );
    }

    #[tokio::test]
    async fn same_context_snapshot_read_does_not_reuse_pre_run_canary_rows() {
        let _simulator_guard = simulator_test_lock().lock().await;
        let company_guid = "synthetic-company-guid";
        let empty_native_groups = native_groups(company_guid, &[]);
        let second_group = native_groups(company_guid, &[("Post-start Assets", "Primary")]);
        let plans = [
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            empty_native_groups.clone(),
            empty_native_groups,
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            native_ledgers(company_guid),
            native_ledgers(company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            native_voucher_types(company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            native_vouchers(company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            second_group.clone(),
            second_group,
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            native_ledgers(company_guid),
            native_ledgers(company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            native_voucher_types(company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
            native_vouchers(company_guid),
            company_extent("Synthetic Company", company_guid),
            company_extent("Synthetic Company", company_guid),
        ]
        .into_iter()
        .map(Fixture::SyntheticXml)
        .map(|fixture| ScenarioPlan::new(fixture).with_encoding(WireEncoding::Utf16Le))
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
        assert_eq!(pre_run_batch.groups.len(), 1);
        assert_eq!(pre_run_batch.groups[0].name, "Primary");

        let snapshot_window = connector.read_pack_window(&context).await.unwrap();
        let PackBatch::CoreAccounting(snapshot_batch) = snapshot_window.batch else {
            panic!("expected core snapshot batch");
        };
        assert_eq!(snapshot_batch.groups.len(), 1);
        assert_eq!(snapshot_batch.groups[0].name, "Post-start Assets");
        assert_eq!(
            snapshot_batch.groups[0].source_id,
            format!("{company_guid}-00000000")
        );

        let requests = simulator.finish().expect("finish sequence simulator");
        assert_eq!(requests.len(), 36);
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

    #[test]
    fn response_encoding_failure_remains_distinct_from_unreachable_for_connector_callers() {
        assert!(matches!(
            map_transport_error(anyhow::Error::new(TallyTransportError::InvalidEncoding {
                code: "declared_encoding_mismatch",
            })),
            TallyError::Protocol { code } if code == "response_encoding_invalid"
        ));
        assert!(matches!(
            map_transport_error(anyhow::Error::new(TallyTransportError::ConnectionFailed)),
            TallyError::Unreachable
        ));
    }
}
