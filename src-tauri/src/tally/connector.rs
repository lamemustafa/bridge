use crate::tally::canonical_window::build_core_window;
use bridge_tally_core::report_tie_out::{LedgerPeriodBalance, LedgerPeriodBalanceReport};
use bridge_tally_core::{
    CanonicalPackWindow, CapabilityEvidence, CapabilityPackId, CapabilityProfile, CapabilityState,
    CompanyRef, EvidenceConfidence, ExactDecimal, PackBatch, ProbeResult, ReadResponseScope,
    ReadWindow, RequestContext, SourceIdentity, TallyConnector, TallyDate, TallyError,
    CORE_ACCOUNTING_SCHEMA_VERSION,
};
use bridge_tally_protocol::xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName};
use bridge_tally_protocol::{
    native_outstandings::{
        render_native_group_snapshot_request, render_native_ledger_export_request,
        render_native_voucher_export_request, render_native_voucher_type_export_request,
        NativeLedgerExportPeriod,
    },
    outstandings_shared::{
        parse_company_book_extent_v2, require_master_witness, CompanyBookExtent,
        DateBoundaryProfile,
    },
    parse_companies_from_collection, parse_ledger_period_balance_report,
    parse_native_group_source_records_with_evidence,
    parse_native_ledger_source_records_with_evidence,
    parse_native_voucher_source_records_with_evidence,
    parse_native_voucher_type_source_records_with_evidence, ParsedExport, ParsedSourceRecord,
    TallyNamedMaster,
};
use bridge_tally_transport::TallyTransportError;
use sha2::{Digest, Sha256};
use std::sync::{Arc, RwLock};
use tokio_util::sync::CancellationToken;

use super::connection::DirectCompanyBootstrapError;
use super::runtime::{TallyRuntimeControlError, TallyRuntimeReadError};
use super::{
    tdl_engine, TallyCompany, TallyConfig, TallyRuntime, VerifiedCompanyIdentity,
    VerifiedCompanyIdentityError,
};

const CORE_QUERY_PROFILE: &str = "core_accounting_v3";

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
    // Snapshot lifecycle evidence is intentionally separate from the runtime's
    // single-use interactive-review cache. It is replaced only by the latest
    // fresh snapshot probe and is consumed solely by this connector's reads.
    snapshot_boundary_profile: Arc<RwLock<Option<DateBoundaryProfile>>>,
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
            snapshot_boundary_profile: Arc::new(RwLock::new(None)),
        })
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub(crate) fn endpoint(&self) -> &TallyConfig {
        &self.config
    }

    /// Records the exact profile a snapshot lifecycle probe just observed. This
    /// is connector-local run evidence, not the runtime's interactive-review
    /// cache, and supplies every core extraction until the next fresh probe.
    pub(crate) fn observe_snapshot_profile(
        &self,
        profile: &CapabilityProfile,
    ) -> Result<DateBoundaryProfile, TallyError> {
        let boundary_profile = self
            .runtime
            .master_ledger_export_boundary_profile_from_profile(Some(profile));
        *self
            .snapshot_boundary_profile
            .write()
            .map_err(|_| invalid_data("snapshot_boundary_profile_unavailable"))? =
            Some(boundary_profile);
        Ok(boundary_profile)
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
        boundary_profile: DateBoundaryProfile,
        identity: &VerifiedCompanyIdentity,
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

        if company_source_identity(
            &self.company.identity.bridge_source_lineage,
            identity.company_guid(),
            identity.company_number.as_str(),
            identity.display_name(),
            identity.books_from_yyyymmdd.as_str(),
        ) != self.company.identity
        {
            return Err(invalid_data("company_identity_mismatch"));
        }
        let company_name = identity.display_name().to_owned();
        let expected_guid = identity.company_guid().to_owned();
        // Native Collection exports deliberately carry no company GUID. Bind
        // the group rows out-of-band: each extent response is GUID-verified,
        // each extent is internally paired, and the unchanged opening/closing
        // extent brackets two equal native collection responses. This is not weaker
        // than the retired report field: Tally renders FIELD amounts for
        // display and has been observed dropping their sign, so it cannot be
        // a trustworthy company-authentication channel for a money path.
        let opening_extent = self.read_pinned_company_book_extent(identity).await?;
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
        let closing_extent = self.read_pinned_company_book_extent(identity).await?;
        if closing_extent != opening_extent {
            return Err(invalid_data("company_book_changed_during_group_read"));
        }

        // Unlike the native Group collection, every Ledger row carries a
        // master GUID. Pair the collection and bracket it with the same
        // GUID-verified book extent used above, then require at least one row
        // to bind to the selected company. A foreign per-row prefix remains
        // evidence, not a hard failure: imported masters may retain it.
        let ledger_opening_extent = self.read_pinned_company_book_extent(identity).await?;
        let ledger_period = NativeLedgerExportPeriod::new(
            boundary_profile,
            ledger_opening_extent.books_from().clone(),
            ledger_opening_extent.last_voucher_date().clone(),
        )
        .map_err(|_| invalid_data("master_ledger_export_period_not_supported"))?;
        let native_ledger_request =
            render_native_ledger_export_request(&company_name, &ledger_period);
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
        let ledger_closing_extent = self.read_pinned_company_book_extent(identity).await?;
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
        let voucher_opening_extent = self.read_pinned_company_book_extent(identity).await?;
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
        let voucher_closing_extent = self.read_pinned_company_book_extent(identity).await?;
        if voucher_closing_extent != voucher_opening_extent {
            return Err(invalid_data("company_book_changed_during_voucher_read"));
        }

        build_core_window(context, groups, ledgers, voucher_types, vouchers)
    }

    async fn read_pinned_company_book_extent(
        &self,
        identity: &VerifiedCompanyIdentity,
    ) -> Result<CompanyBookExtent, TallyError> {
        let company = ValidatedCompanyName::new(identity.display_name().to_owned())
            .map_err(|_| invalid_data("company_name_invalid"))?;
        let expectation = identity
            .company_book_extent_expectation()
            .map_err(|_| invalid_data("company_identity_invalid"))?;
        let request = ReadOnlyProfile::CompanyBookExtentV2 { company: &company }.render();
        let first_xml = self
            .post_xml_validated(request.clone(), |xml| {
                parse_company_book_extent_v2(xml, &expectation).is_ok()
            })
            .await?;
        let second_xml = self
            .post_xml_validated(request, |xml| {
                parse_company_book_extent_v2(xml, &expectation).is_ok()
            })
            .await?;
        let first = parse_company_book_extent_v2(&first_xml, &expectation)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;
        let second = parse_company_book_extent_v2(&second_xml, &expectation)
            .map_err(|_| invalid_data("company_identity_mismatch"))?;
        if first != second {
            return Err(invalid_data("company_book_extent_drifted"));
        }
        // Identity selection does not replace the master-change witness.
        require_master_witness(&first).map_err(|_| invalid_data("company_altmstid_missing"))?;
        Ok(first)
    }

    async fn snapshot_probe(&self) -> Result<ProbeResult, TallyError> {
        let (_, mut result) = self
            .runtime
            .snapshot_probe_with_observation(self.config.clone(), &self.company.display_name)
            .await
            .map_err(map_transport_error)?;
        let identity = self.verify_snapshot_identity_from_companies(&result.companies)?;
        let boundary_profile = self.observe_snapshot_profile(&result.profile)?;
        let source_read = self
            .extract_core_window(&self.canary_context, boundary_profile, &identity)
            .await;
        // A failed source read is still a source-read attempt. Always make the
        // closing full-tuple observation, while preserving the source error as
        // the decisive capability evidence when both operations fail.
        let closing_identity = self.verify_closing_snapshot_identity(&identity).await;
        let core_evidence = match (source_read, closing_identity) {
            (Ok(window), Ok(())) => core_canary_capability(&window),
            (Ok(_), Err(error)) => CapabilityEvidence {
                state: CapabilityState::Unknown,
                confidence: EvidenceConfidence::Observed,
                safe_reason_code: Some(capability_failure_code(&error)),
            },
            (Err(error), _) => CapabilityEvidence {
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

    fn verify_snapshot_identity_from_companies(
        &self,
        companies: &[TallyCompany],
    ) -> Result<VerifiedCompanyIdentity, TallyError> {
        let matching = companies
            .iter()
            .filter(|company| {
                let (Some(guid), Some(company_number), Some(books_from)) = (
                    company.guid.as_deref(),
                    company.company_number.as_deref(),
                    company.books_from.as_deref(),
                ) else {
                    return false;
                };
                company_source_identity(
                    &self.company.identity.bridge_source_lineage,
                    guid,
                    company_number,
                    &company.name,
                    books_from,
                ) == self.company.identity
            })
            .collect::<Vec<_>>();
        let Some(company) = matching.first() else {
            return Err(protocol_error("company_identity_not_found"));
        };
        let (Some(guid), Some(company_number), Some(books_from)) = (
            company.guid.as_deref(),
            company.company_number.as_deref(),
            company.books_from.as_deref(),
        ) else {
            return Err(protocol_error("company_identity_not_found"));
        };
        VerifiedCompanyIdentity::from_observed_companies(
            company.name.clone(),
            guid.to_string(),
            company_number.to_string(),
            books_from.to_string(),
            companies,
        )
        .map_err(|error| match error {
            VerifiedCompanyIdentityError::InvalidCompanyNumber => {
                protocol_error("company_number_invalid")
            }
            VerifiedCompanyIdentityError::InvalidBooksFrom => {
                protocol_error("company_books_from_invalid")
            }
            VerifiedCompanyIdentityError::Missing => protocol_error("company_identity_not_found"),
            VerifiedCompanyIdentityError::DuplicateTuple => {
                protocol_error("company_identity_ambiguous")
            }
            VerifiedCompanyIdentityError::DisplayScopeAmbiguous => {
                protocol_error("company_identity_display_scope_ambiguous")
            }
        })
    }

    async fn verify_snapshot_identity(&self) -> Result<VerifiedCompanyIdentity, TallyError> {
        let companies = self
            .runtime
            .fetch_companies(self.config.clone())
            .await
            .map_err(map_transport_error)?;
        self.verify_snapshot_identity_from_companies(&companies)
    }

    async fn verify_closing_snapshot_identity(
        &self,
        opening: &VerifiedCompanyIdentity,
    ) -> Result<(), TallyError> {
        let closing = self.verify_snapshot_identity().await?;
        if closing != *opening {
            return Err(protocol_error("company_identity_changed_during_read"));
        }
        Ok(())
    }

    /// Every source read gets a closing identity check. A closing mismatch
    /// invalidates a successful source response, but it must not replace an
    /// earlier source failure with a later, unrelated company-list failure.
    async fn finish_snapshot_source_read<T>(
        &self,
        source_read: Result<T, TallyError>,
        opening: &VerifiedCompanyIdentity,
    ) -> Result<T, TallyError> {
        let closing_identity = self.verify_closing_snapshot_identity(opening).await;
        match (source_read, closing_identity) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(error)) => Err(error),
            (Err(error), _) => Err(error),
        }
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
        companies
            .into_iter()
            .map(|company| {
                let guid = company
                    .guid
                    .filter(|guid| !guid.trim().is_empty())
                    .ok_or_else(|| protocol_error("company_identity_incomplete"))?;
                let company_number = company
                    .company_number
                    .filter(|number| !number.trim().is_empty())
                    .ok_or_else(|| protocol_error("company_identity_incomplete"))?;
                let books_from = company
                    .books_from
                    .filter(|date| !date.trim().is_empty())
                    .ok_or_else(|| protocol_error("company_identity_incomplete"))?;
                Ok(CompanyRef {
                    identity: company_source_identity(
                        &lineage,
                        &guid,
                        &company_number,
                        &company.name,
                        &books_from,
                    ),
                    display_name: company.name,
                })
            })
            .collect()
    }

    async fn read_pack_window(
        &self,
        context: &RequestContext,
    ) -> Result<CanonicalPackWindow, TallyError> {
        // Capability probes happen before a durable run receives its started_at timestamp.
        // Always perform a new source read here, including for the same canary context, so
        // pre-run observations can never enter the snapshot as if they were run data.
        let boundary_profile = self
            .snapshot_boundary_profile
            .read()
            .map_err(|_| invalid_data("snapshot_boundary_profile_unavailable"))?
            .ok_or_else(|| invalid_data("snapshot_boundary_profile_unavailable"))?;
        let identity = self.verify_snapshot_identity().await?;
        let window = self
            .extract_core_window(context, boundary_profile, &identity)
            .await;
        self.finish_snapshot_source_read(window, &identity).await
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
        let identity = self.verify_snapshot_identity().await?;
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
            .await;
        let xml = self.finish_snapshot_source_read(xml, &identity).await?;
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
        let report = LedgerPeriodBalanceReport {
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
        };
        Ok(report)
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

pub fn company_source_identity(
    lineage: &str,
    company_guid: &str,
    company_number: &str,
    display_name: &str,
    books_from_yyyymmdd: &str,
) -> SourceIdentity {
    let canonical_guid = company_guid.to_ascii_lowercase();
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-company-observation-v2\0");
    digest.update(lineage.as_bytes());
    digest.update(b"\0");
    digest.update(canonical_guid.as_bytes());
    digest.update(b"\0");
    digest.update(company_number.as_bytes());
    digest.update(b"\0");
    digest.update(display_name.as_bytes());
    digest.update(b"\0");
    digest.update(books_from_yyyymmdd.as_bytes());
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
    if error
        .downcast_ref::<DirectCompanyBootstrapError>()
        .is_some()
    {
        return protocol_error("company_identity_not_found");
    }
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
#[path = "connector_tests.rs"]
mod tests;
