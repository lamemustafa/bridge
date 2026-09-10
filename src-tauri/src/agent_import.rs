use super::{
    combine_evidence, company_json, normalized_date, parse_company_high_water, party_name,
    render_agent_company_high_water, required_string, sha256_hex, sha256_json, Evidence, Server,
    ToolFailure, ToolOutcome,
};
use crate::tally::agent_read_request::AgentReadRequest;
use crate::tally::standard_ledger_catalog::{
    admit_standard_ledger_catalog_request, parse_standard_ledger_catalog_response,
    render_standard_ledger_catalog_request,
};
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::native_outstandings::{
    parse_native_group_snapshot, render_native_group_snapshot_request,
};
use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Seek, SeekFrom, Write};

#[path = "agent_import_cash_bank.rs"]
mod cash_bank;
use cash_bank::{CashBankState, LegRequirement, ObservedMasters};
#[path = "agent_import_identity.rs"]
mod identity;
use identity::{import_identity, ImportIdentityScheme};
#[path = "agent_import_schema.rs"]
mod schema;
pub(super) use schema::voucher_input_schema;
#[path = "agent_desktop_journal.rs"]
mod desktop_journal;
#[path = "agent_desktop_journal_review.rs"]
pub(crate) mod desktop_journal_review;
#[cfg(test)]
#[path = "agent_desktop_journal_tests.rs"]
mod desktop_journal_tests;
use crate::endpoint_coordination as dispatch_lease;
use crate::local_files::file::lock_error as import_admission_lock_error;
#[path = "agent_import_ledger.rs"]
pub(super) mod ledger;
#[path = "agent_import_persistence.rs"]
mod persistence;
#[path = "agent_import_post.rs"]
mod post;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

struct ImportProfileObservation {
    qualification: Result<(), ImportProfileRefusal>,
    evidence: Evidence,
    observed_profile: Value,
    admission_key: (String, String),
}

#[derive(Clone, Copy)]
enum ImportProfileRefusal {
    Mode,
}

impl ImportProfileRefusal {
    fn build_code(self) -> &'static str {
        match self {
            Self::Mode => "import_mode_unqualified",
        }
    }

    fn verification_code(self) -> &'static str {
        match self {
            Self::Mode => "verification_mode_unqualified",
        }
    }
}

const MAX_VOUCHERS: usize = 1_000;
pub(super) const MAX_MASTER_NAMES: usize = 100;
pub(super) const MAX_MASTER_NAME_CHARS: usize = 1024;
const MAX_TEXT_CHARS: usize = 2_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImportPayload {
    company_guid: String,
    vouchers: Vec<ImportVoucher>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImportVoucher {
    bridge_txn_id: String,
    date: String,
    voucher_type: VoucherType,
    #[serde(default)]
    narration: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    voucher_number: Option<String>,
    entries: Vec<ImportEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum VoucherType {
    Payment,
    Receipt,
    Journal,
    Contra,
}

impl VoucherType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Payment => "Payment",
            Self::Receipt => "Receipt",
            Self::Journal => "Journal",
            Self::Contra => "Contra",
        }
    }
}

/// Which sides of a voucher type are constrained, and how.
///
/// This is the whole of what distinguishes Payment, Receipt and Contra from a
/// Journal on the write path: a Journal names no party and constrains no side.
struct BankVoucherShape {
    legs: &'static [(EntrySide, LegRequirement)],
}

impl BankVoucherShape {
    /// The side rendered as `PARTYLEDGERNAME`, which is the counterparty leg by
    /// definition. A Contra moves money between two of the company's own
    /// accounts, so it has no counterparty leg and names no party.
    fn party_side(&self) -> Option<&EntrySide> {
        self.legs
            .iter()
            .find(|(_, requirement)| *requirement == LegRequirement::Counterparty)
            .map(|(side, _)| side)
    }
}

impl VoucherType {
    /// `None` for a Journal, whose qualified file shape predates and does not
    /// carry these elements.
    fn bank_shape(&self) -> Option<BankVoucherShape> {
        match self {
            // Payment: Dr party, Cr bank. Receipt: Dr bank, Cr party.
            Self::Payment => Some(BankVoucherShape {
                legs: &[
                    (EntrySide::Cr, LegRequirement::Money),
                    (EntrySide::Dr, LegRequirement::Counterparty),
                ],
            }),
            Self::Receipt => Some(BankVoucherShape {
                legs: &[
                    (EntrySide::Dr, LegRequirement::Money),
                    (EntrySide::Cr, LegRequirement::Counterparty),
                ],
            }),
            Self::Contra => Some(BankVoucherShape {
                legs: &[
                    (EntrySide::Dr, LegRequirement::Money),
                    (EntrySide::Cr, LegRequirement::Money),
                ],
            }),
            Self::Journal => None,
        }
    }
}

// Every variant here carries live import/readback evidence for the exact file
// shape this module renders for it: Journal in
// docs/agent/ASSESSMENT-2026-09-06.md, and Payment/Receipt/Contra in
// docs/tally/TALLY_PROTOCOL_REFERENCE.md §9.13. Adding a `VoucherType` variant
// does not qualify it; the build refuses any type absent from this list, so
// evidence has to arrive before the file can.
const LIVE_QUALIFIED_VOUCHER_TYPES: &[VoucherType] = &[
    VoucherType::Journal,
    VoucherType::Payment,
    VoucherType::Receipt,
    VoucherType::Contra,
];

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum EntrySide {
    Dr,
    Cr,
}

impl EntrySide {
    fn tally_positive(&self) -> &'static str {
        match self {
            Self::Dr => "Yes",
            Self::Cr => "No",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImportEntry {
    ledger: String,
    amount: String,
    side: EntrySide,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ImportLedgerLine {
    batch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_scheme: Option<ImportIdentityScheme>,
    company_guid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint_origin: Option<String>,
    #[serde(default)]
    company: Option<ImportCompanyTuple>,
    txn_ids: Vec<String>,
    date_from: String,
    date_to: String,
    sha256: String,
    built_at: String,
    status: String,
    pre_import_mark: PreImportMark,
    vouchers: Vec<ImportVoucher>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ImportCompanyTuple {
    name: String,
    guid: String,
    company_number: String,
    books_from: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PreImportMark {
    kind: String,
    value: Option<u64>,
    #[serde(default)]
    master_value: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct ReadEntry {
    ledger: String,
    amount: String,
    is_deemed_positive: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct ReadVoucher {
    remote_id: Option<String>,
    guid: Option<String>,
    alter_id: Option<u64>,
    date: Option<String>,
    voucher_type: Option<String>,
    narration: Option<String>,
    voucher_number: Option<String>,
    master_id: Option<String>,
    cancelled: Option<bool>,
    optional: Option<bool>,
    #[serde(rename = "amounts")]
    entries: Vec<ReadEntry>,
}

/// A complete collection admitted before either corroboration or attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ImportReadSource {
    rows: Vec<ReadVoucher>,
}

impl ImportReadSource {
    fn admit(mut rows: Vec<ReadVoucher>) -> Result<Self, String> {
        let mut identities = super::VoucherSourceIdentities::default();
        let mut transaction_tags = BTreeSet::new();
        for row in &mut rows {
            if let Some(guid) = row.guid.as_mut() {
                *guid = guid.trim().to_ascii_lowercase();
            }
            let master_id = super::parse_optional_tally_u64(
                row.master_id.as_deref(),
                "import_verification_master_id_invalid",
            )?;
            identities
                .admit(row.guid.as_deref(), master_id)
                .map_err(|_| "import_verification_identity_invalid".to_string())?;
            row.master_id = master_id.map(|id| id.to_string());
            if row.guid.is_none() && row.master_id.is_none() {
                return Err("import_verification_identity_invalid".into());
            }
            let narration = row.narration.as_deref().unwrap_or_default();
            for (index, (start, _)) in narration.match_indices("[BRIDGE:").enumerate() {
                if index > 0 {
                    return Err("import_verification_tag_ambiguous".into());
                }
                let tag = narration[start..]
                    .strip_prefix("[BRIDGE:")
                    .and_then(|tail| tail.split_once(']').map(|(id, _)| id))
                    .filter(|id| valid_txn_id(id))
                    .ok_or_else(|| "import_verification_tag_invalid".to_string())?;
                if !transaction_tags.insert(tag.to_string()) {
                    return Err("import_verification_tag_ambiguous".into());
                }
            }
        }
        Ok(Self { rows })
    }
}

impl Server {
    pub(super) fn voucher_schema(&self) -> Result<ToolOutcome, String> {
        let schema = voucher_input_schema();
        Ok(ToolOutcome {
            payload: json!({"result": {"schema": schema, "rules": [
                "bridge_txn_id is client-supplied, unique within this batch, 1-64 ASCII characters from [A-Za-z0-9_-]",
                "new files accept Journal, Payment, Receipt and Contra, the voucher types with recorded live import/readback evidence",
                "a Journal takes any balanced set of entries and may carry a voucher_number",
                "Payment, Receipt and Contra take exactly two entries over two distinct ledgers, and neither voucher_number nor reference: neither element's fate on these types has been observed, and the bank's own reference belongs in the narration, which survives",
                "a Payment credits, and a Receipt debits, a ledger whose live group ancestry reaches Bank Accounts or Cash-in-Hand; both Contra legs must name one, and a leg that cannot be established is refused",
                "the other leg of a Payment or Receipt must be established as holding no money: a ledger under any money group is refused there, because money on both sides is a Contra whatever the type says, and so is one whose group ancestry cannot be resolved at all",
                "each voucher has at least two entries and exact debit total equals credit total",
                "amounts are positive decimal strings with exactly two fractional digits",
                "dates must be within the selected company's BOOKSFROM through today",
                "ledger names must exactly match the live catalogue; validate_masters before build_import_xml",
                "a batch may contain at most 100 distinct ledger names of at most 1024 characters each"
            ], "limits": {"import_mode_qualification": "New files require freshly observed supported TallyPrime product and licence mode before and after the build reads. Release and licence tier are reported as observed facts. Journal, Payment, Receipt and Contra are the voucher types with recorded import/readback evidence, each only in the exact file shape this schema admits; a multi-entry Payment, Receipt or Contra and every other voucher type are refused. Only an unnumbered single-voucher Journal batch is eligible for post_import; the other types are import-only."}}}),
            evidence: local_evidence("voucher_schema"),
            company_guid: None,
            truncated: false,
        })
    }

    pub(super) async fn validate_masters(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let ledgers = args
            .get("ledgers")
            .and_then(Value::as_array)
            .ok_or_else(|| "ledgers_required".to_string())?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .filter(|names| !names.is_empty())
            .ok_or_else(|| "ledgers_required".to_string())?;
        let (company, identity, identity_evidence) = self.verified_company(guid).await?;
        let (catalogue, evidence) = self
            .read_ledger_catalogue(&identity, &company.name)
            .await
            .map_err(|failure| failure.with_prior_evidence(identity_evidence.clone()))?;
        let report = ledgers
            .into_iter()
            .map(|wanted| master_match(wanted, &catalogue))
            .collect::<Vec<_>>();
        let hash = sha256_json(&catalogue);
        Ok(ToolOutcome {
            payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"masters": report, "catalogue_evidence_sha256": hash}}),
            evidence: combine_evidence(identity_evidence, evidence),
            company_guid: Some(guid.to_string()),
            truncated: false,
        })
    }

    async fn qualified_import_profile(&self) -> Result<ImportProfileObservation, ToolFailure> {
        let observation = self.observe_import_profile().await?;
        if let Err(refusal) = observation.qualification {
            return Err(ToolFailure::from(refusal.build_code().to_string())
                .with_prior_evidence(observation.evidence));
        }
        Ok(observation)
    }

    async fn observe_import_profile(&self) -> Result<ImportProfileObservation, ToolFailure> {
        use bridge_tally_core::{CapabilityFeatureId, CapabilityState, EvidenceConfidence};
        let (probe, wire) = self
            .runtime
            .probe_with_wire_evidence(self.tally_config())
            .await
            .map_err(|error| ToolFailure::from_runtime("import_mode_probe_failed", error))?;
        let evidence = super::evidence_from_runtime_read(wire);
        let product = probe.profile.product.to_ascii_lowercase();
        let mode = probe
            .profile
            .mode
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let qualified = product == "tallyprime"
            && matches!(mode.as_str(), "licensed" | "education" | "educational")
            && probe
                .profile
                .features
                .get(&CapabilityFeatureId::ProductAndMode)
                .is_some_and(|feature| {
                    feature.state == CapabilityState::Supported
                        && feature.confidence == EvidenceConfidence::Observed
                });
        // The literal-date verification filter has its own returned-row and
        // corroboration checks. Product/mode must be observed; a release or
        // tier label is evidence, not a categorical import-file admission gate.
        let qualification = qualified.then_some(()).ok_or(ImportProfileRefusal::Mode);
        Ok(ImportProfileObservation {
            qualification,
            evidence,
            observed_profile: json!({
                "product": probe.profile.product,
                "release": probe.profile.release,
                "license_tier": probe.profile.license_tier,
                "mode": probe.profile.mode,
            }),
            admission_key: (product, mode),
        })
    }

    pub(super) async fn build_import_xml(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let mut payload = parse_payload(args)?;
        validate_payload(&payload)?;
        let (debit, credit) = totals(&payload.vouchers)?;
        refuse_unqualified_types(&payload.vouchers, LIVE_QUALIFIED_VOUCHER_TYPES)?;
        normalize_payload_dates(&mut payload)?;
        let opening_profile = self.qualified_import_profile().await?;
        validate_import_dates_for_profile(&payload, &opening_profile).map_err(|code| {
            ToolFailure::from(code).with_prior_evidence(opening_profile.evidence.clone())
        })?;
        let mode_evidence = opening_profile.evidence.clone();
        let (company, identity, identity_evidence) = self
            .verified_company(&payload.company_guid)
            .await
            .map_err(|failure| failure.with_prior_evidence(mode_evidence.clone()))?;
        let mut accumulated = combine_evidence(mode_evidence, identity_evidence.clone());
        let result: Result<ToolOutcome, ToolFailure> = async {
            let (catalogue, ledger_masters, catalogue_evidence) = self
                .read_import_ledger_catalogue(&identity, &company.name)
                .await
                .map(|(names, catalogue, _, evidence)| (names, catalogue, evidence))?;
            accumulated = combine_evidence(accumulated.clone(), catalogue_evidence.clone());
            let report = masters_for_payload(&payload, &catalogue);
            if report.iter().any(|value| value["match_state"] != "exact") {
                return Ok(ToolOutcome {
                    payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {
                        "state":"refused", "reason":"masters_not_exact", "masters":report,
                        "catalogue_evidence_sha256":sha256_json(&catalogue),
                        "next_step":"Use the exact live spelling from validate_masters, then build a new batch. No file was written."
                    }}),
                    evidence: accumulated.clone(),
                    company_guid: Some(payload.company_guid),
                    truncated: false,
                });
            }
            // Only a payload carrying a cash/bank voucher reads the group
            // collection, so a Journal-only batch keeps the request sequence its
            // own qualification was measured on.
            let mut group_evidence = None;
            if renders_bank_shape(&payload.vouchers) {
                let (groups, evidence) = self.read_group_collection(&identity, &company.name).await?;
                accumulated = combine_evidence(accumulated.clone(), evidence.clone());
                let observed = ObservedMasters::new(ledger_masters.parents(), groups);
                let refusals =
                    cash_bank_refusals(&payload, &observed, self.settings.max_bytes);
                if !refusals.ledgers.is_empty() {
                    return Ok(ToolOutcome {
                        payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {
                            "state":"refused", "reason":"cash_bank_ledger_not_established",
                            "refused_ledgers":refusals.ledgers, "refused_leg_count":refusals.legs,
                            "refused_ledgers_omitted":refusals.omitted,
                            "group_evidence_sha256":evidence.response_sha256,
                            "next_step":"Each refused leg names its ledger and why. A leg marked cash_bank must reach Bank Accounts or Cash-in-Hand — the credit on a Payment, the debit on a Receipt, both legs on a Contra. A leg marked not_cash_bank must be established as holding no money: any money group there means the voucher is really a Contra, and an unresolvable group is refused too because it cannot be established either way. Bridge admits a money group only where a captured ledger sits under it, so an overdraft or cash-credit ledger is refused on either side for now. Correct the payload or the ledger's group in Tally, then build a new batch. No file was written."
                        }}),
                        evidence: accumulated.clone(),
                        company_guid: Some(payload.company_guid),
                        truncated: false,
                    });
                }
                group_evidence = Some(evidence);
            }
            validate_dates(&payload, company.books_from.as_deref())?;
            let _admission_lock = self.lock_import_admission()?;
            // Admit the journal before publication; labels in older batches do not
            // collide with this build's independently generated wire identities.
            self.import_snapshot_while_admitted(None)?;
            let (mark, mark_evidence) = self.pre_import_mark(&company, &identity).await?;
            accumulated = combine_evidence(accumulated.clone(), mark_evidence.clone());
            let (_, repeated_catalogue_evidence) =
                self.read_ledger_catalogue(&identity, &company.name).await?;
            accumulated = combine_evidence(accumulated.clone(), repeated_catalogue_evidence.clone());
            // Compare the complete captured catalogue, including identities and parents.
            // This proves stability across these observations, not an atomic snapshot.
            if catalogue_evidence.response_sha256 != repeated_catalogue_evidence.response_sha256 {
                return Err("import_catalogue_changed".to_string().into());
            }
            // The classification that admitted these legs rests on the group
            // collection as much as on the catalogue, so hold it to the same
            // stability requirement rather than trusting a single observation.
            if let Some(group_evidence) = group_evidence {
                let (_, repeated_group_evidence) =
                    self.read_group_collection(&identity, &company.name).await?;
                accumulated = combine_evidence(accumulated.clone(), repeated_group_evidence.clone());
                if group_evidence.response_sha256 != repeated_group_evidence.response_sha256 {
                    return Err("import_groups_changed".to_string().into());
                }
            }
            let date_from = payload
                .vouchers
                .iter()
                .map(|voucher| voucher.date.clone())
                .min()
                .unwrap_or_default();
            let date_to = payload
                .vouchers
                .iter()
                .map(|voucher| voucher.date.clone())
                .max()
                .unwrap_or_default();
            // Exercise the exact future readback projection before publishing a file.
            // This observes today's source, not a bound on later Tally mutations.
            let (preflight_xml, preflight_evidence) = self
                .post_read(
                    &identity,
                    render_import_verification_read(&company.name, &date_from, &date_to),
                )
                .await?;
            accumulated = combine_evidence(accumulated.clone(), preflight_evidence.clone());
            let preflight = parse_import_vouchers(&preflight_xml, identity.company_guid())?;
            verification_window_identities(&preflight, &date_from, &date_to)?;
            let verification_preflight = json!({
                "state":"current_window_readable", "from":date_from, "to":date_to,
                "source_rows":preflight.rows.len(),
                "paired_source_bytes":preflight_evidence.bytes,
                "response_sha256":preflight_evidence.response_sha256
            });
            let closing_profile = self.qualified_import_profile().await?;
            accumulated = combine_evidence(accumulated.clone(), closing_profile.evidence);
            if closing_profile.admission_key != opening_profile.admission_key {
                return Err("import_mode_changed_during_build".to_string().into());
            }
            let batch_id = format!("bridge-{}", Uuid::new_v4());
            let xml = render_import_xml(&company.name, &payload.vouchers, &batch_id);
            let sha256 = sha256_hex(xml.as_bytes());
            let line = ImportLedgerLine {
                batch_id: batch_id.clone(),
                identity_scheme: Some(ImportIdentityScheme::BatchV1),
                company_guid: canonical_batch_guid(&payload.company_guid),
                endpoint_origin: Some(super::canonical_loopback_origin(&self.settings.endpoint).map_err(|_| "host_setting_invalid".to_string())?),
                company: Some(import_company_tuple(&company)?),
                txn_ids: payload
                    .vouchers
                    .iter()
                    .map(|voucher| voucher.bridge_txn_id.clone())
                    .collect(),
                date_from,
                date_to,
                sha256: sha256.clone(),
                built_at: now(),
                status: "built".to_string(),
                pre_import_mark: mark,
                vouchers: payload.vouchers,
            };
            let imports = self.imports_dir()?;
            let path = imports.join(format!("{batch_id}.xml"));
            if let Some(error) = persistence::persist_build(&imports, &line, xml.as_bytes(), || {
                self.append_import_ledger_while_admitted(&line)
            })? {
                return Ok(ToolOutcome {
                    payload: json!({"result":{"batch_id":batch_id,"error":{"code":error,
                        "message":"Local batch publication is incomplete; reconcile its recovery journal before continuing."}}}),
                    evidence: Evidence {
                        state: "partial",
                        reason_code: Some(error),
                        ..accumulated.clone()
                    },
                    company_guid: Some(line.company_guid.clone()),
                    truncated: false,
                });
            }
            // Reuse the posting admission path to describe only a route that
            // this exact saved batch can take. A manual-only batch is still a
            // successful build.
            let native_post_eligible = self.settings.writes_enabled
                && post::admit_saved_journal(&line, &self.settings.endpoint).is_ok();
            let (warnings, next_step) = build_import_guidance(
                self.settings.writes_enabled,
                native_post_eligible,
                renders_bank_shape(&line.vouchers),
                line.vouchers.iter().any(|voucher| {
                    voucher
                        .voucher_type
                        .bank_shape()
                        .is_some_and(|shape| shape.party_side().is_some())
                }),
            );
            Ok(ToolOutcome {
                payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {
                    "batch_id": batch_id, "path": path, "sha256": sha256,
                    "voucher_count": line.vouchers.len(), "total_debit": debit.as_str(), "total_credit": credit.as_str(),
                    "live_evidence": live_evidence(&line.vouchers),
                    "verification_preflight": verification_preflight,
                    "identity_scheme": line.identity_scheme,
                    "observed_profile": opening_profile.observed_profile,
                    "warnings": warnings,
                    "next_step": next_step
                }}),
                evidence: accumulated.clone(),
                company_guid: Some(line.company_guid.clone()),
                truncated: false,
            })
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(accumulated))
    }

    pub(super) async fn verify_import(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        self.verify_import_with_dispatch(args, false).await
    }

    pub(in crate::agent) async fn verify_import_after_current_dispatch(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        self.verify_import_with_dispatch(args, true).await
    }

    async fn verify_import_with_dispatch(
        &self,
        args: &Value,
        current_dispatch: bool,
    ) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let batch_id = required_string(args, "batch_id")?;
        let ledger::BatchSnapshot {
            batch: line,
            generation,
            response: dispatch_response,
            dispatched,
            ..
        } = self
            .latest_import_snapshot(batch_id)?
            .ok_or_else(|| "import_batch_not_found".to_string())?;
        if !batch_guid_matches(&line.company_guid, guid) {
            return Err("import_batch_company_mismatch".to_string().into());
        }
        validate_dispatched_import_endpoint(&line, dispatched, &self.settings.endpoint)?;
        let opening_mode = self.observe_import_profile().await?;
        let (company, identity, identity_evidence) = self
            .verified_company(guid)
            .await
            .map_err(|failure| failure.with_prior_evidence(opening_mode.evidence.clone()))?;
        let mut accumulated =
            combine_evidence(opening_mode.evidence.clone(), identity_evidence.clone());
        let result: Result<ToolOutcome, ToolFailure> = async {
            if line.company.as_ref() != Some(&import_company_tuple(&company)?) {
                return Err("company_identity_mismatch".to_string().into());
            }
            let request =
                render_import_verification_read(&company.name, &line.date_from, &line.date_to);
            let (xml, evidence) = self.post_read(&identity, request.clone()).await?;
            accumulated = combine_evidence(accumulated.clone(), evidence.clone());
            let observed = parse_import_vouchers(&xml, identity.company_guid())?;
            let (corroboration_xml, corroboration_evidence) =
                self.post_read(&identity, request).await?;
            accumulated = combine_evidence(accumulated.clone(), corroboration_evidence.clone());
            let corroboration = parse_import_vouchers(&corroboration_xml, identity.company_guid())?;
            corroborate_verification_window(&observed, &corroboration, &line.date_from, &line.date_to)?;
            let result = verify_batch(&line, &observed)?;
            let mut closing_mode_evidence = None;
            if result["counts"]["not_found"].as_u64().unwrap_or(0) > 0 {
                // Positive rows are direct observations. Absence additionally requires
                // the product, release and licence tier qualified by the live slice.
                if let Err(refusal) = opening_mode.qualification {
                    return Err(refusal.verification_code().to_string().into());
                }
                let closing_mode = self.observe_import_profile().await?;
                accumulated = combine_evidence(accumulated.clone(), closing_mode.evidence.clone());
                if let Err(refusal) = closing_mode.qualification {
                    return Err(refusal.verification_code().to_string().into());
                }
                if closing_mode.admission_key != opening_mode.admission_key {
                    return Err("verification_mode_changed_during_read".to_string().into());
                }
                closing_mode_evidence = Some(closing_mode.evidence);
            }
            let proof = json!({
                "company": company_json(&company, std::slice::from_ref(&company)),
                "batch_id": line.batch_id, "batch_sha256": line.sha256,
                "built_at": line.built_at, "verified_at": now(),
                "dispatch_response": dispatch_response,
                "pre_import_mark": line.pre_import_mark, "alter_id_delta": alter_id_delta(&line.pre_import_mark, &observed.rows),
                "counts": result["counts"], "vouchers": result["vouchers"], "duplicates": result["duplicates"],
                "unrelated_duplicates_in_window": result["unrelated_duplicates_in_window"],
                "evidence": {"mode_opening": opening_mode.evidence, "mode_closing": closing_mode_evidence, "company": identity_evidence, "voucher_read": evidence, "voucher_read_corroboration": corroboration_evidence, "voucher_read_sha256": sha256_hex(xml.as_bytes())}
            });
            let mut payload = json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": proof});
            if dispatched {
                if current_dispatch {
                    post::finalize_current_dispatch(&mut payload, dispatch_response.as_ref());
                } else {
                    post::finalize_previous_attempt_reconciliation(
                        &mut payload,
                        dispatch_response.as_ref(),
                    );
                }
            }
            let status = if dispatched
                && payload["result"]["dispatch"]["state"] == "reconciliation_required"
            {
                "verification_incomplete"
            } else {
                verification_status(&result, line.vouchers.len())
            };
            let mut update = line.clone();
            update.status = status.to_string();
            self.persist_import_verification(&payload["result"], &update, generation)?;
            Ok(ToolOutcome {
                payload,
                evidence: accumulated.clone(),
                company_guid: Some(guid.to_string()),
                truncated: false,
            })
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(accumulated))
    }

    fn persist_import_verification(
        &self,
        proof: &Value,
        update: &ImportLedgerLine,
        expected_generation: ledger::VerificationGeneration,
    ) -> Result<(), String> {
        let _admission_lock = self.lock_import_admission()?;
        let current = self.import_snapshot_while_admitted(Some(&update.batch_id))?;
        if current.map(|snapshot| snapshot.generation) != Some(expected_generation) {
            return Err("import_verification_conflict_retry".into());
        }
        let imports = self.imports_dir()?;
        let local_proof = super::redact_value(proof.clone(), super::Redaction::None);
        let json = serde_json::to_vec_pretty(&local_proof)
            .map_err(|_| "proof_serialization_failed".to_string())?;
        let markdown = render_proof_markdown(&local_proof);
        persistence::publish_proofs(
            &imports,
            update,
            &json,
            markdown.as_bytes(),
            || self.append_import_record_while_admitted(&ledger::StatusRecord::from(update)),
            |_| Ok(()),
        )
    }

    pub(super) async fn read_ledger_catalogue(
        &self,
        identity: &super::VerifiedCompanyIdentity,
        company_name: &str,
    ) -> Result<(Vec<String>, Evidence), ToolFailure> {
        let (names, _, _, evidence) = self
            .read_import_ledger_catalogue(identity, company_name)
            .await?;
        Ok((names, evidence))
    }

    async fn read_import_ledger_catalogue(
        &self,
        identity: &super::VerifiedCompanyIdentity,
        company_name: &str,
    ) -> Result<
        (
            Vec<String>,
            bridge_tally_protocol::StandardLedgerCatalog,
            AgentReadRequest,
            Evidence,
        ),
        ToolFailure,
    > {
        let request_xml = render_standard_ledger_catalog_request(company_name)
            .map_err(|_| "company_name_invalid".to_string())?;
        let request = admit_standard_ledger_catalog_request(request_xml.clone())
            .map_err(|_| "ledger_export_invalid".to_string())?;
        let (xml, evidence) = self.post_read(identity, request_xml).await?;
        let catalogue =
            parse_standard_ledger_catalog_response(&xml, company_name, identity.company_guid())
                .map_err(|_| {
                    ToolFailure::from("ledger_export_invalid".to_string())
                        .with_prior_evidence(evidence.clone())
                })?;
        Ok((
            catalogue.names().map(str::to_string).collect(),
            catalogue,
            request,
            evidence,
        ))
    }

    /// The company's group tree, read only when a payload needs one leg
    /// classified as cash or bank. A ledger row carries a single `PARENT` hop
    /// and no `PARENTSTRUCTURE`, so the group identities live here.
    async fn read_group_collection(
        &self,
        identity: &super::VerifiedCompanyIdentity,
        company_name: &str,
    ) -> Result<(Vec<bridge_tally_protocol::TallyNamedMaster>, Evidence), ToolFailure> {
        let request_xml = render_native_group_snapshot_request(company_name);
        let (xml, evidence) = self.post_read(identity, request_xml).await?;
        let groups = parse_native_group_snapshot(&xml, identity.company_guid()).map_err(|_| {
            ToolFailure::from("group_export_invalid".to_string())
                .with_prior_evidence(evidence.clone())
        })?;
        Ok((groups, evidence))
    }

    async fn pre_import_mark(
        &self,
        company: &bridge_tally_protocol::TallyCompany,
        identity: &super::VerifiedCompanyIdentity,
    ) -> Result<(PreImportMark, Evidence), ToolFailure> {
        let guid = company
            .guid
            .as_deref()
            .ok_or_else(|| "pre_import_mark_unobserved".to_string())?;
        let (xml, evidence) = self
            .post_read(identity, render_agent_company_high_water(&company.name))
            .await?;
        let high_water = parse_company_high_water(&xml, guid).map_err(|_| {
            ToolFailure::from("pre_import_mark_unobserved".to_string())
                .with_prior_evidence(evidence.clone())
        })?;
        let mark = company_high_water_mark(&high_water)
            .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
        Ok((mark, evidence))
    }

    fn imports_dir(&self) -> Result<PathBuf, String> {
        let path = self.settings.data_dir.join("imports");
        super::ensure_private_directory(&path).map_err(|error| match error {
            super::DirectoryAdmissionError::Unavailable => "imports_dir_unavailable".to_string(),
            #[cfg(unix)]
            super::DirectoryAdmissionError::Permissions => {
                "import_file_permissions_failed".to_string()
            }
        })?;
        Ok(path)
    }

    pub(super) fn lock_import_admission(&self) -> Result<std::fs::File, String> {
        let path = self.settings.data_dir.join("agent-import-admission.lock");
        let file = super::local_file::open_local_file(&path, true)
            .map_err(|_| "import_admission_lock_unavailable".to_string())?;
        // Never park the async request loop behind another process's network
        // work. Contention is an in-band refusal, not a deferred posting request.
        file.try_lock().map_err(import_admission_lock_error)?;
        persistence::require_settled(&self.settings.data_dir.join("imports"))?;
        Ok(file)
    }

    fn lock_import_admission_shared(&self) -> Result<std::fs::File, String> {
        let path = self.settings.data_dir.join("agent-import-admission.lock");
        let file = super::local_file::open_local_file(&path, true)
            .map_err(|_| "import_admission_lock_unavailable".to_string())?;
        file.try_lock_shared()
            .map_err(import_admission_lock_error)?;
        persistence::require_settled(&self.settings.data_dir.join("imports"))?;
        Ok(file)
    }

    #[cfg(test)]
    fn import_ledger(&self) -> Result<Vec<ImportLedgerLine>, String> {
        let _admission_lock = self.lock_import_admission_shared()?;
        let history = match self.import_journal_while_admitted()? {
            Some(reader) => ledger::read_history(reader)?,
            None => Vec::new(),
        };
        Ok(history.into_iter().map(|snapshot| snapshot.batch).collect())
    }

    fn import_journal_while_admitted(
        &self,
    ) -> Result<Option<std::io::BufReader<std::fs::File>>, String> {
        let path = self.settings.data_dir.join("agent-import-ledger.jsonl");
        let file = match super::local_file::open_local_file(&path, false) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("import_ledger_unavailable".to_string()),
        };
        Ok(Some(std::io::BufReader::new(file)))
    }

    fn import_snapshot_while_admitted(
        &self,
        batch_id: Option<&str>,
    ) -> Result<Option<ledger::BatchSnapshot>, String> {
        match self.import_journal_while_admitted()? {
            Some(reader) => ledger::read_snapshot(reader, batch_id),
            None => Ok(None),
        }
    }

    fn latest_import_snapshot(
        &self,
        batch_id: &str,
    ) -> Result<Option<ledger::BatchSnapshot>, String> {
        let _admission_lock = self.lock_import_admission_shared()?;
        self.import_snapshot_while_admitted(Some(batch_id))
    }

    #[cfg(test)]
    pub(super) fn append_import_ledger(&self, line: &ImportLedgerLine) -> Result<(), String> {
        let _admission_lock = self.lock_import_admission()?;
        self.append_import_ledger_while_admitted(line)
    }

    fn append_import_ledger_while_admitted(&self, line: &ImportLedgerLine) -> Result<(), String> {
        self.append_import_record_while_admitted(line)
    }

    pub(super) fn append_import_record_while_admitted(
        &self,
        line: &impl Serialize,
    ) -> Result<(), String> {
        let path = self.settings.data_dir.join("agent-import-ledger.jsonl");
        let encoded = serde_json::to_string(line)
            .map_err(|_| "import_ledger_serialization_failed".to_string())?;
        let encoded = format!("{encoded}\n");
        if encoded.len() > ledger::MAX_RECORD_BYTES {
            return Err("import_ledger_record_too_large".into());
        }
        append_private_import_ledger(&path, encoded.as_bytes(), set_private_file)
    }
}

/// A native-dispatched batch is tied to the Tally endpoint used for its saved
/// admission. Older manual imports retain their original verification path.
fn validate_dispatched_import_endpoint(
    line: &ImportLedgerLine,
    dispatched: bool,
    endpoint: &super::TallyEndpointConfig,
) -> Result<(), String> {
    if !dispatched {
        return Ok(());
    }
    let origin = super::canonical_loopback_origin(endpoint)
        .map_err(|_| "host_setting_invalid".to_string())?;
    if line.endpoint_origin.as_deref() != Some(origin.as_str()) {
        return Err("import_post_endpoint_mismatch".into());
    }
    Ok(())
}

fn append_private_import_ledger(
    path: &Path,
    bytes: &[u8],
    prepare_permissions: impl FnOnce(&std::fs::File) -> Result<(), String>,
) -> Result<(), String> {
    // Admission serializes writers; read/write access also permits Windows rollback.
    let mut file = super::local_file::open_local_file(path, true)
        .map_err(|_| "import_ledger_unavailable".to_string())?;
    prepare_permissions(&file)?;
    file.seek(SeekFrom::End(0))
        .map_err(|_| "import_ledger_unavailable".to_string())?;
    append_import_ledger_bytes(&mut file, bytes)
}

trait ImportLedgerWriter {
    fn length(&mut self) -> std::io::Result<u64>;
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn sync(&mut self) -> std::io::Result<()>;
    fn truncate(&mut self, length: u64) -> std::io::Result<()>;
}

impl ImportLedgerWriter for std::fs::File {
    fn length(&mut self) -> std::io::Result<u64> {
        self.metadata().map(|metadata| metadata.len())
    }

    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_all(bytes)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.sync_data()
    }

    fn truncate(&mut self, length: u64) -> std::io::Result<()> {
        self.set_len(length)
    }
}

fn append_import_ledger_bytes(
    writer: &mut impl ImportLedgerWriter,
    bytes: &[u8],
) -> Result<(), String> {
    let original_length = writer
        .length()
        .map_err(|_| "import_ledger_unavailable".to_string())?;
    if writer.append(bytes).and_then(|_| writer.sync()).is_err() {
        writer
            .truncate(original_length)
            .and_then(|_| writer.sync())
            .map_err(|_| "import_ledger_rollback_failed".to_string())?;
        return Err("import_ledger_unavailable".to_string());
    }
    Ok(())
}

fn canonical_batch_guid(guid: &str) -> String {
    guid.to_ascii_lowercase()
}

fn batch_guid_matches(stored: &str, supplied: &str) -> bool {
    stored.eq_ignore_ascii_case(supplied)
}

fn parse_payload(args: &Value) -> Result<ImportPayload, String> {
    serde_json::from_value(args.clone()).map_err(|_| "voucher_schema_invalid".to_string())
}

fn import_company_tuple(
    company: &bridge_tally_protocol::TallyCompany,
) -> Result<ImportCompanyTuple, String> {
    Ok(ImportCompanyTuple {
        name: nonempty_company_field(&company.name)?,
        guid: nonempty_company_field(
            company
                .guid
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?,
        company_number: nonempty_company_field(
            company
                .company_number
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?,
        books_from: normalized_date(
            company
                .books_from
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?,
    })
}

fn nonempty_company_field(value: &str) -> Result<String, String> {
    (!value.trim().is_empty())
        .then(|| value.to_string())
        .ok_or_else(|| "company_identity_incomplete".to_string())
}

fn build_import_guidance(
    writes_enabled: bool,
    native_post_eligible: bool,
    bank_types: bool,
    names_a_counterparty: bool,
) -> (Value, &'static str) {
    let preflight_warning =
        "The preflight observes the current verification window. The import or subsequent changes can make later readback exceed the source limits.";
    // §9.13: every party amount in the observed import landed On Account, and
    // that is explicitly not established as correct for a book that reconciles
    // bills. Bridge cannot yet tell the two kinds of book apart — the ledger
    // catalogue it reads carries no bill-wise flag — so the limit is stated
    // rather than silently accepted on the operator's behalf.
    // §9.8 qualified exact-file repeat on the Journal path only, and §9.13
    // imported each bank file exactly once. A caller recovering an uncertain
    // outcome must not reach for the same remedy on both.
    let repeat_warning = bank_types.then_some(
        "Do not re-import this file if the outcome is uncertain. Exact-file repeat is qualified for Journal only; for Payment, Receipt and Contra a second import may create a second set of vouchers. Call verify_import, which reads the window back without writing.",
    );
    let allocation_warning = names_a_counterparty.then_some(
        "This batch names a counterparty on a Payment or Receipt and carries no bill allocation, so each amount lands On Account. If that ledger is configured for bill-wise accounting, the entry will need allocating in Tally afterwards; Bridge does not read that configuration and cannot warn per ledger.",
    );
    let warnings = |first: &str| {
        json!(std::iter::once(first)
            .chain(std::iter::once(preflight_warning))
            .chain(repeat_warning)
            .chain(allocation_warning)
            .collect::<Vec<_>>())
    };
    if writes_enabled && native_post_eligible {
        (
            warnings(
                "No import XML was sent to Tally. To post this saved batch, call post_import; it requires a separate native approval. If you import the file manually, call verify_import afterward and do not call post_import for that batch.",
            ),
            "Call post_import with this company_guid and batch_id; the local user must review and approve it before one posting attempt.",
        )
    } else if writes_enabled {
        (
            warnings(
                "No import XML was sent to Tally. This saved batch is not eligible for native posting because native posting requires one unnumbered Journal with a reviewable preview. Import the written file manually, then use verify_import; do not call post_import for this batch.",
            ),
            "Import this file in Tally (Gateway of Tally → Import → Vouchers) with the company open, then call verify_import",
        )
    } else {
        (
            warnings(
                "No import XML was sent to Tally. Import the written file manually, then use verify_import.",
            ),
            "Import this file in Tally (Gateway of Tally → Import → Vouchers) with the company open, then call verify_import",
        )
    }
}

/// The live observation each voucher type in this batch actually rests on.
///
/// A Journal file rests on the synthetic-lab import/readback recorded in the
/// 2026-09-06 assessment — a report that in the same breath records Payment,
/// Receipt and Contra being refused. Those three rest on the licensed 7.1
/// import recorded as reference §9.13 instead. Citing either for the other
/// would look auditable and be wrong. The two never share a file — mixing the
/// rendered shapes is refused — but the map stays general so a batch of several
/// bank types reports the one source they share, once.
fn live_evidence(vouchers: &[ImportVoucher]) -> Vec<Value> {
    let mut sources = BTreeMap::<(&str, &str), BTreeSet<&str>>::new();
    for voucher in vouchers {
        let source = match voucher.voucher_type.bank_shape() {
            None => (
                "synthetic_lab_readback",
                "docs/agent/ASSESSMENT-2026-09-06.md",
            ),
            Some(_) => (
                "licensed_bank_voucher_import",
                "docs/tally/TALLY_PROTOCOL_REFERENCE.md",
            ),
        };
        sources
            .entry(source)
            .or_default()
            .insert(voucher.voucher_type.as_str());
    }
    sources
        .into_iter()
        .map(|((observation, report), voucher_types)| {
            json!({"observation":observation, "report":report,
                "voucher_types":voucher_types.into_iter().collect::<Vec<_>>()})
        })
        .collect()
}

/// The last gate before any read: a voucher type absent from the qualified
/// list never reaches a live request, let alone a written file. `qualified` is
/// a parameter so the guard itself stays exercised even while every declared
/// `VoucherType` happens to be qualified.
fn refuse_unqualified_types(
    vouchers: &[ImportVoucher],
    qualified: &[VoucherType],
) -> Result<(), String> {
    vouchers
        .iter()
        .all(|voucher| qualified.contains(&voucher.voucher_type))
        .then_some(())
        .ok_or_else(|| "import_voucher_type_unqualified".to_string())
}

/// Whether this batch renders the bank shape at all.
///
/// After `validate_payload` a batch is homogeneous by shape, so this is both
/// "any" and "all" — the group read, the guidance and the evidence record can
/// each ask it once and get a whole-batch answer.
fn renders_bank_shape(vouchers: &[ImportVoucher]) -> bool {
    vouchers
        .iter()
        .any(|voucher| voucher.voucher_type.bank_shape().is_some())
}

/// A file may carry more than one voucher type — the measured statement files
/// mixed Payment and Receipt freely, 61+54 in one and 20+8 in another, both
/// imported clean. What it may not do is mix two rendered *shapes*.
///
/// The three bank types share one shape: `EFFECTIVEDATE` beside `DATE`, a party
/// on the counterparty side, never a number or reference. A Journal's shape
/// carries none of that and comes from a separate qualification lineage
/// (§9.8 against §9.13). No file mixing the two has been imported — the
/// reallocation Journals went in on their own — so the union is refused rather
/// than assumed from holding both citations at once.
fn refuse_mixed_shapes(vouchers: &[ImportVoucher]) -> Result<(), String> {
    let bank = vouchers
        .iter()
        .filter(|voucher| voucher.voucher_type.bank_shape().is_some())
        .count();
    (bank == 0 || bank == vouchers.len())
        .then_some(())
        .ok_or_else(|| "voucher_type_shapes_mixed".to_string())
}

fn validate_payload(payload: &ImportPayload) -> Result<(), String> {
    refuse_mixed_shapes(&payload.vouchers)?;
    if payload.company_guid.trim().is_empty()
        || payload.vouchers.is_empty()
        || payload.vouchers.len() > MAX_VOUCHERS
    {
        return Err("voucher_count_invalid".to_string());
    }
    let mut txn_ids = BTreeSet::new();
    let mut ledger_names = BTreeSet::new();
    for voucher in &payload.vouchers {
        if !valid_txn_id(&voucher.bridge_txn_id) || !txn_ids.insert(&voucher.bridge_txn_id) {
            return Err("bridge_txn_id_invalid_or_duplicate".to_string());
        }
        normalized_date(&voucher.date)?;
        if voucher.entries.len() < 2 {
            return Err("voucher_entries_too_few".to_string());
        }
        for text in [voucher.narration.as_deref(), voucher.reference.as_deref()]
            .into_iter()
            .flatten()
        {
            if contains_reserved_marker(text) {
                return Err("narration_reserved_marker".to_string());
            }
        }
        for text in [
            voucher.narration.as_deref(),
            voucher.reference.as_deref(),
            voucher.voucher_number.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            // JSON Schema minLength/maxLength count Unicode code points, not UTF-8 bytes.
            if text.is_empty()
                || text.chars().count() > MAX_TEXT_CHARS
                || text.chars().any(char::is_control)
            {
                return Err("voucher_text_invalid".to_string());
            }
        }
        if voucher
            .voucher_number
            .as_deref()
            .is_some_and(|number| number.chars().count() > 32 || number.contains('$'))
        {
            return Err("voucher_number_invalid".to_string());
        }
        for entry in &voucher.entries {
            if entry.ledger.trim().is_empty()
                || entry.ledger.chars().count() > MAX_MASTER_NAME_CHARS
                || entry.ledger.chars().any(char::is_control)
                || !valid_2dp_amount(&entry.amount)
            {
                return Err("voucher_entry_invalid".to_string());
            }
            ledger_names.insert(entry.ledger.as_str());
            if ledger_names.len() > MAX_MASTER_NAMES {
                return Err("voucher_unique_ledger_limit_exceeded".to_string());
            }
        }
        let (debit, credit) = totals(std::slice::from_ref(voucher))?;
        if !debit.numeric_eq(&credit) {
            return Err("voucher_not_balanced".to_string());
        }
        if voucher.voucher_type.bank_shape().is_some() {
            validate_bank_voucher_shape(voucher)?;
        }
    }
    Ok(())
}

/// Payment, Receipt and Contra are admitted only in the two-entry shape that
/// was imported and read back live: one debit, one credit, two distinct
/// ledgers, and neither a supplied voucher number nor a reference.
///
/// A multi-leg Payment is a perfectly ordinary Tally voucher and is
/// deliberately not admitted. No such file has been imported and read back
/// here, and the readback pairing that verify_import relies on has never been
/// exercised on one; a Journal remains available for a batch that needs it.
fn validate_bank_voucher_shape(voucher: &ImportVoucher) -> Result<(), String> {
    let [first, second] = voucher.entries.as_slice() else {
        return Err("voucher_entry_pair_required".to_string());
    };
    if first.side == second.side {
        return Err("voucher_entry_pair_required".to_string());
    }
    if first.ledger == second.ledger {
        return Err("voucher_entry_ledger_repeated".to_string());
    }
    // A supplied VOUCHERNUMBER's fate is decided by the *voucher type's*
    // numbering method (§9.8), which is per-type configuration this build has
    // never read: under Automatic, Tally discards the number without reporting
    // it; under Manual it keeps it. The observed book numbered these types
    // automatically, and that is one book — so the number is refused because
    // its fate is unobserved, not because every company is automatic. Reading
    // the method would need its own qualified voucher-type read contract,
    // which §9.8 says cannot be inferred; native posting refuses a supplied
    // number for exactly this reason. The bank's own reference belongs in the
    // narration, which survives either way.
    if voucher.voucher_number.is_some() {
        return Err("voucher_number_unqualified_for_type".to_string());
    }
    // §9.13's measured shape carries no REFERENCE element. Tally may well
    // accept one here, but no file carrying it has been imported and read
    // back on these types, and verify_import compares accounting entries
    // rather than this annotation, so nothing downstream would notice if it
    // were dropped or rewritten. Refuse it rather than write an unmeasured
    // variant while claiming the qualified shape.
    if voucher.reference.is_some() {
        return Err("voucher_reference_unqualified_for_type".to_string());
    }
    Ok(())
}

fn entry_for_side<'a>(voucher: &'a ImportVoucher, side: &EntrySide) -> Option<&'a ImportEntry> {
    voucher.entries.iter().find(|entry| &entry.side == side)
}

/// Every constrained (voucher, side, ledger) in the payload, with what that
/// leg must be. Empty for a Journal-only batch, which is what keeps the group
/// read off that path entirely.
fn constrained_legs(
    payload: &ImportPayload,
) -> Vec<(&ImportVoucher, &EntrySide, &str, LegRequirement)> {
    payload
        .vouchers
        .iter()
        .flat_map(|voucher| {
            voucher
                .voucher_type
                .bank_shape()
                .into_iter()
                .flat_map(move |shape| {
                    shape.legs.iter().filter_map(move |(side, requirement)| {
                        entry_for_side(voucher, side)
                            .map(|entry| (voucher, side, entry.ledger.as_str(), *requirement))
                    })
                })
        })
        .collect()
}

/// One row per constrained leg, in payload order, whether or not it passed.
/// A refusal names every failing leg at once: a caller fixing them one build
/// at a time pays a full live read cycle for each.
/// What a batch's constrained legs refuse, and why.
///
/// Empty `ledgers` means every constrained leg was admitted.
struct CashBankRefusals {
    /// One row per distinct failing (ledger, requirement), not per leg. A
    /// ledger in the wrong group fails identically in every voucher that names
    /// it, and 400 copies of one problem is not 400 problems.
    ///
    /// This is what bounds the result. `validate_payload` already caps a batch
    /// at `MAX_MASTER_NAMES` distinct ledger names, and a ledger can be
    /// constrained at most once per side, so these rows cannot exceed 200
    /// however many vouchers the batch carries. Emitting one row per leg had no
    /// such bound: a 1,000-voucher batch produced up to 2,000 rows, and once
    /// that passed the response cap the whole actionable refusal collapsed into
    /// a generic size error.
    ledgers: Vec<Value>,
    /// Failing legs before deduplication, so a caller can tell one misfiled
    /// ledger from one that poisons the entire batch.
    legs: usize,
    /// Distinct failures the byte budget left out. Deduplication bounds the
    /// row *count*; it does not bound their size, and a ledger name may be
    /// 1,024 characters.
    omitted: usize,
}

/// How many serialized bytes the refusal diagnostics may occupy, given the
/// caller's configured response cap.
///
/// Row count alone is not a bound on size: a batch may carry
/// `MAX_MASTER_NAMES` names of `MAX_MASTER_NAME_CHARS` each, which passes even
/// the default cap on names alone before any detail text. The share is a
/// quarter because the diagnostics are one field among several in the refusal
/// and the transport repeats structured content as text, so the frame carries
/// roughly twice what is measured here. `max_bytes` is configurable down to
/// 256, where a quarter leaves room for nothing — which is why one row always
/// goes out regardless, and the rest are counted rather than dropped silently.
fn refusal_diagnostic_budget(max_bytes: usize) -> usize {
    (max_bytes / 4).min(32 * 1024)
}

fn cash_bank_refusals(
    payload: &ImportPayload,
    observed: &ObservedMasters,
    max_bytes: usize,
) -> CashBankRefusals {
    let mut classified = BTreeMap::<&str, CashBankState>::new();
    let mut refused = BTreeMap::<(&str, &'static str), Value>::new();
    let mut legs = 0_usize;
    for (voucher, side, ledger, requirement) in constrained_legs(payload) {
        let state = classified
            .entry(ledger)
            .or_insert_with(|| observed.classify(ledger))
            .clone();
        let admitted = requirement.admits(&state);
        if admitted {
            continue;
        }
        legs += 1;
        let requires = match requirement {
            LegRequirement::Money => "cash_bank",
            LegRequirement::Counterparty => "not_cash_bank",
        };
        refused.entry((ledger, requires)).or_insert_with(|| {
            json!({
                "ledger": party_name(ledger),
                "requires": requires,
                "side": side,
                "state": state.state(),
                "refused_because": requirement.refusal(&state, voucher.voucher_type.as_str()),
                // One voucher a caller can open to see the problem, rather
                // than every voucher that repeats it.
                "first_bridge_txn_id": voucher.bridge_txn_id,
            })
        });
    }
    let mut budget = refusal_diagnostic_budget(max_bytes);
    let distinct = refused.len();
    let ledgers = refused
        .into_values()
        .enumerate()
        .take_while(|(index, row)| {
            let cost = serde_json::to_string(row).map_or(usize::MAX, |text| text.len());
            // The first row always goes out, however long its ledger name:
            // one actionable failure beats a bare count.
            let affordable = *index == 0 || cost <= budget;
            budget = budget.saturating_sub(cost);
            affordable
        })
        .map(|(_, row)| row)
        .collect::<Vec<_>>();
    CashBankRefusals {
        omitted: distinct.saturating_sub(ledgers.len()),
        ledgers,
        legs,
    }
}

fn contains_reserved_marker(value: &str) -> bool {
    quick_xml::escape::unescape(value)
        .map(|decoded| decoded.to_ascii_uppercase().contains("[BRIDGE:"))
        .unwrap_or_else(|_| value.to_ascii_uppercase().contains("[BRIDGE:"))
}

/// Dates cross the tool boundary in the human-friendly form but are persisted
/// in the exact Tally form used in the generated XML and verification window.
fn normalize_payload_dates(payload: &mut ImportPayload) -> Result<(), String> {
    for voucher in &mut payload.vouchers {
        voucher.date = normalized_date(&voucher.date)?;
    }
    Ok(())
}

fn validate_dates(payload: &ImportPayload, books_from: Option<&str>) -> Result<(), String> {
    let from =
        normalized_date(books_from.ok_or_else(|| "company_identity_incomplete".to_string())?)?;
    let today = super::tally_host_today();
    for voucher in &payload.vouchers {
        let date = normalized_date(&voucher.date)?;
        if date < from || date > today {
            return Err("voucher_date_outside_company_extent".to_string());
        }
    }
    Ok(())
}

fn validate_import_dates_for_profile(
    payload: &ImportPayload,
    profile: &ImportProfileObservation,
) -> Result<(), String> {
    let boundary_profile = if matches!(
        profile.admission_key.1.as_str(),
        "education" | "educational"
    ) {
        DateBoundaryProfile::EducationRestricted
    } else {
        DateBoundaryProfile::ModeAgnostic
    };
    for voucher in &payload.vouchers {
        let date = bridge_tally_core::TallyDate::parse(voucher.date.clone())
            .map_err(|_| "voucher_date_invalid".to_string())?;
        if !boundary_profile.accepts_boundary(&date) {
            return Err("education_voucher_date_unsupported".to_string());
        }
    }
    Ok(())
}

fn valid_txn_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
fn valid_2dp_amount(value: &str) -> bool {
    let Some((whole, fractional)) = value.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fractional.len() == 2
        && fractional.bytes().all(|byte| byte.is_ascii_digit())
        && ExactDecimal::parse(value.to_string())
            .is_ok_and(|amount| !amount.is_zero() && !amount.is_negative())
}

fn totals(vouchers: &[ImportVoucher]) -> Result<(ExactDecimal, ExactDecimal), String> {
    let mut debit = ExactDecimal::zero();
    let mut credit = ExactDecimal::zero();
    for entry in vouchers.iter().flat_map(|voucher| &voucher.entries) {
        let amount = ExactDecimal::parse(entry.amount.clone())
            .map_err(|_| "voucher_amount_invalid".to_string())?;
        match entry.side {
            EntrySide::Dr => {
                debit = debit
                    .checked_add(&amount)
                    .map_err(|_| "voucher_amount_overflow".to_string())?
            }
            EntrySide::Cr => {
                credit = credit
                    .checked_add(&amount)
                    .map_err(|_| "voucher_amount_overflow".to_string())?
            }
        }
    }
    Ok((debit, credit))
}

fn masters_for_payload(payload: &ImportPayload, catalogue: &[String]) -> Vec<Value> {
    requested_ledger_names(payload)
        .into_iter()
        .map(|name| master_match(&name, catalogue))
        .collect()
}

fn requested_ledger_names(payload: &ImportPayload) -> Vec<String> {
    payload
        .vouchers
        .iter()
        .flat_map(|voucher| &voucher.entries)
        .map(|entry| entry.ledger.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn master_match(wanted: &str, catalogue: &[String]) -> Value {
    if catalogue.iter().any(|name| name == wanted) {
        return json!({"requested": party_name(wanted), "match_state":"exact", "exact_live_spelling":party_name(wanted)});
    }
    let key = master_key(wanted);
    let candidates = catalogue
        .iter()
        .filter(|name| {
            let candidate = master_key(name);
            candidate == key || candidate.starts_with(&key) || key.starts_with(&candidate)
        })
        .collect::<BTreeSet<_>>();
    let candidate_count = candidates.len();
    if candidate_count == 0 {
        json!({"requested":party_name(wanted),"match_state":"missing"})
    } else {
        let mut bytes = 0_usize;
        let candidates = candidates
            .into_iter()
            .take(25)
            .take_while(|name| {
                bytes = bytes.saturating_add(name.len());
                bytes <= 8192
            })
            .map(|name| party_name(name.clone()))
            .collect::<Vec<_>>();
        json!({"requested":party_name(wanted),"match_state":"near_miss",
            "exact_live_spelling":candidates.first(),"candidate_count":candidate_count,
            "candidates_truncated":candidates.len() < candidate_count,"candidates":candidates})
    }
}

fn master_key(value: &str) -> String {
    value
        .nfc()
        .flat_map(|character| match character {
            '–' | '—' | '−' | '‐' | '‑' => "-".chars().collect::<Vec<_>>(),
            '‘' | '’' | '‚' | '‛' => "'".chars().collect(),
            '“' | '”' | '„' | '‟' => "\"".chars().collect(),
            other => other.to_lowercase().collect(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_import_xml(company: &str, vouchers: &[ImportVoucher], batch_id: &str) -> String {
    let messages = vouchers
        .iter()
        .map(|voucher| {
            let identity = import_identity(batch_id, &voucher.bridge_txn_id);
            render_voucher_xml(voucher, identity, identity)
        })
        .collect::<String>();
    render_import_envelope(company, &messages)
}

fn render_native_journal_xml(company: &str, voucher: &ImportVoucher, batch_id: &str) -> String {
    // A public file may already have been imported and edited. Never reuse its
    // client REMOTEID for a native Create, which Tally can treat as an upsert.
    // The stable narration tag remains the batch attribution used by readback.
    let messages = render_voucher_xml(
        voucher,
        Uuid::new_v4(),
        import_identity(batch_id, &voucher.bridge_txn_id),
    );
    render_import_envelope(company, &messages)
}

fn render_import_envelope(company: &str, messages: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ENVELOPE><HEADER><TALLYREQUEST>Import Data</TALLYREQUEST></HEADER><BODY><IMPORTDATA><REQUESTDESC><REPORTNAME>Vouchers</REPORTNAME><STATICVARIABLES><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES></REQUESTDESC><REQUESTDATA>{messages}</REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>", xml_escape(company))
}

fn render_voucher_xml(voucher: &ImportVoucher, remote_id: Uuid, attribution_id: Uuid) -> String {
    let narration = format!(
        "<NARRATION>{}</NARRATION>",
        xml_escape(
            format!(
                "{} [BRIDGE:{}]",
                voucher.narration.as_deref().unwrap_or("").trim(),
                attribution_id
            )
            .trim(),
        )
    );
    // REFERENCE is retained because it is part of the agent input contract. Its effect is not used as posting evidence; verify_import compares the accounting entries, not this annotation.
    let reference = voucher
        .reference
        .as_deref()
        .map(|value| format!("<REFERENCE>{}</REFERENCE>", xml_escape(value)))
        .unwrap_or_default();
    let voucher_number = voucher
        .voucher_number
        .as_deref()
        .map(|value| format!("<VOUCHERNUMBER>{}</VOUCHERNUMBER>", xml_escape(value)))
        .unwrap_or_default();
    let shape = voucher.voucher_type.bank_shape();
    // §9.13's measured files put the debit first in every voucher, and a
    // caller's ordering is not a fact about the batch. Canonicalise rather than
    // refuse: it costs the caller nothing and removes the variance entirely.
    // A Journal keeps the caller's order, since its own measured file did.
    let mut ordered = voucher.entries.iter().collect::<Vec<_>>();
    if shape.is_some() {
        ordered.sort_by_key(|entry| match entry.side {
            EntrySide::Dr => 0,
            EntrySide::Cr => 1,
        });
    }
    let entries = ordered.iter().map(|entry| {
        let amount = match entry.side { EntrySide::Dr => format!("-{}", entry.amount), EntrySide::Cr => entry.amount.clone() };
        format!("<ALLLEDGERENTRIES.LIST><LEDGERNAME>{}</LEDGERNAME><ISDEEMEDPOSITIVE>{}</ISDEEMEDPOSITIVE><AMOUNT>{}</AMOUNT></ALLLEDGERENTRIES.LIST>", xml_escape(&entry.ledger), entry.side.tally_positive(), amount)
    }).collect::<String>();
    let date = normalized_date(&voucher.date).unwrap_or_default();
    // §9.13: the imported Payment/Receipt/Contra files carried EFFECTIVEDATE
    // beside DATE, and named the party on the side opposite the money. The
    // Journal shape qualified in §9.8 carries neither element, and is left
    // byte-identical to the file that measurement actually ran on.
    let effective_date = shape
        .as_ref()
        .map(|_| format!("<EFFECTIVEDATE>{date}</EFFECTIVEDATE>"))
        .unwrap_or_default();
    let party = shape
        .as_ref()
        .and_then(BankVoucherShape::party_side)
        .and_then(|side| entry_for_side(voucher, side))
        .map(|entry| {
            format!(
                "<PARTYLEDGERNAME>{}</PARTYLEDGERNAME>",
                xml_escape(&entry.ledger)
            )
        })
        .unwrap_or_default();
    // The qualified human-import slice uses Create + stable client REMOTEID;
    // native posting uses a separate private REMOTEID and no supplied number.
    // See docs/tally/TALLY_PROTOCOL_REFERENCE.md §9.8 for scope and limits.
    format!("<TALLYMESSAGE xmlns:UDF=\"TallyUDF\"><VOUCHER REMOTEID=\"{}\" VCHTYPE=\"{}\" ACTION=\"Create\" OBJVIEW=\"Accounting Voucher View\"><DATE>{date}</DATE>{effective_date}<VOUCHERTYPENAME>{}</VOUCHERTYPENAME>{party}{voucher_number}{narration}{reference}{entries}</VOUCHER></TALLYMESSAGE>", remote_id, voucher.voucher_type.as_str(), voucher.voucher_type.as_str())
}

fn render_import_verification_read(company: &str, from: &str, to: &str) -> String {
    format!("<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Import Verification</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeImportWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"</SYSTEM><COLLECTION NAME=\"Bridge Agent Import Verification\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,REMOTEID,GUID,MASTERID,ALTERID,NARRATION,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.LEDGERNAME,ALLLEDGERENTRIES.AMOUNT,ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH><FILTERS>BridgeImportWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>", xml_escape(company))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_import_vouchers(xml: &str, company_guid: &str) -> Result<ImportReadSource, String> {
    let parsed = super::parse_agent_changed_rows(xml, company_guid).map_err(|code| {
        match code.as_str() {
            // Preserve the import error contract while sharing scalar admission.
            "agent_read_protocol_invalid" if super::validate_agent_envelope(xml).is_err() => {
                "import_verification_protocol_invalid"
            }
            "agent_read_protocol_invalid"
            | "change_row_core_field_invalid"
            | "voucher_date_invalid"
            | "voucher_accounting_state_not_observed" => "import_verification_export_invalid",
            "change_row_identity_invalid"
            | "voucher_source_identity_invalid"
            | "voucher_company_identity_invalid" => "import_verification_identity_invalid",
            "voucher_amount_invalid" => "import_verification_amount_invalid",
            "voucher_master_id_invalid" => "import_verification_master_id_invalid",
            _ => return code,
        }
        .to_string()
    })?;
    let mut rows: Vec<ReadVoucher> = parsed
        .into_iter()
        .map(|row| {
            serde_json::from_value(row)
                .map_err(|_| "import_verification_export_invalid".to_string())
        })
        .collect::<Result<_, _>>()?;
    // The shared parser has validated these lexemes; comparisons use canonical
    // Yes/No while preserving original amount strings for proof output.
    for entry in rows.iter_mut().flat_map(|row| &mut row.entries) {
        entry.is_deemed_positive = entry.is_deemed_positive.trim().to_string();
    }
    ImportReadSource::admit(rows)
}

// File preflight and later verification require the same window and row identities.
fn verification_window_identities(
    observed: &ImportReadSource,
    from: &str,
    to: &str,
) -> Result<BTreeSet<(String, u64)>, String> {
    if observed.rows.iter().any(|voucher| {
        voucher
            .date
            .as_deref()
            .is_none_or(|date| date < from || date > to)
    }) {
        return Err("window_not_honoured".to_string());
    }
    observed
        .rows
        .iter()
        .map(|voucher| {
            Ok((
                voucher
                    .guid
                    .clone()
                    .ok_or_else(|| "verification_incomplete:window_not_corroborated".to_string())?,
                voucher
                    .alter_id
                    .ok_or_else(|| "verification_incomplete:window_not_corroborated".to_string())?,
            ))
        })
        .collect()
}

fn corroborate_verification_window(
    observed: &ImportReadSource,
    corroboration: &ImportReadSource,
    from: &str,
    to: &str,
) -> Result<(), String> {
    // This collection request has no row limit. The MCP output-page setting
    // cannot establish source truncation; corroborate the observed identity set
    // independently of that presentation cap.
    if verification_window_identities(observed, from, to)?
        != verification_window_identities(corroboration, from, to)?
    {
        return Err("verification_incomplete:window_not_corroborated".to_string());
    }
    Ok(())
}

fn canonical_verification_amount(value: &str) -> Result<String, String> {
    ExactDecimal::parse(value.to_string())
        .and_then(|amount| amount.checked_add(&ExactDecimal::zero()))
        .map(|amount| amount.as_str().to_string())
        .map_err(|_| "import_verification_amount_invalid".to_string())
}

type VerificationFingerprint = (Option<String>, Option<String>, Vec<String>);

#[derive(Default)]
struct VerificationCandidates {
    remaining: BTreeSet<usize>,
    after_mark: BTreeSet<usize>,
    consumed: bool,
}

impl VerificationCandidates {
    fn insert(&mut self, index: usize, after_mark: bool) {
        self.remaining.insert(index);
        if after_mark {
            self.after_mark.insert(index);
        }
    }
    fn consume(&mut self, index: usize) {
        self.consumed |= self.remaining.remove(&index);
        self.after_mark.remove(&index);
    }
}

fn observed_fingerprint(voucher: &ReadVoucher) -> VerificationFingerprint {
    (
        voucher.date.clone(),
        voucher.voucher_type.clone(),
        actual_entry_fingerprint(voucher),
    )
}

fn verify_batch(line: &ImportLedgerLine, observed: &ImportReadSource) -> Result<Value, String> {
    // Normalize only the comparison copies. Persisted batches and generated XML
    // retain their original amount lexemes and remain backward compatible.
    let mut comparison_line = line.clone();
    for entry in comparison_line
        .vouchers
        .iter_mut()
        .flat_map(|v| &mut v.entries)
    {
        entry.amount = canonical_verification_amount(&entry.amount)?;
    }
    let mut comparison_observed = observed.rows.clone();
    for entry in comparison_observed.iter_mut().flat_map(|v| &mut v.entries) {
        entry.amount = canonical_verification_amount(&entry.amount)?;
    }
    let line = &comparison_line;
    let observed = comparison_observed.as_slice();
    let observed_identities = observed
        .iter()
        .map(observed_voucher_identity)
        .collect::<Result<Vec<_>, _>>()?;
    let mut fully_verified_identities = BTreeSet::new();
    let mut rows = Vec::new();
    let expected_fingerprints = line
        .vouchers
        .iter()
        .map(|voucher| {
            (
                normalized_date(&voucher.date).ok(),
                Some(voucher.voucher_type.as_str().to_string()),
                expected_entry_fingerprint(voucher),
            )
        })
        .collect::<Vec<VerificationFingerprint>>();
    let observed_fingerprints = observed
        .iter()
        .map(observed_fingerprint)
        .collect::<Vec<_>>();
    let mut expected_fingerprint_counts = BTreeMap::new();
    for fingerprint in &expected_fingerprints {
        *expected_fingerprint_counts
            .entry(fingerprint)
            .or_insert(0_usize) += 1;
    }
    let expected_markers = line
        .vouchers
        .iter()
        .map(|voucher| line.attribution_tag(voucher))
        .collect::<Vec<_>>();
    let expected_tags = expected_markers
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    // Source admission permits at most one well-formed reserved marker. Parse it
    // once, then reserve expected tags before any fingerprint fallback is used.
    let observed_tags = observed
        .iter()
        .map(|voucher| {
            voucher
                .narration
                .as_deref()?
                .split_once("[BRIDGE:")?
                .1
                .split_once(']')
                .map(|(tag, _)| tag)
        })
        .collect::<Vec<_>>();
    let mut tagged = BTreeMap::<&str, VerificationCandidates>::new();
    let mut fallback = BTreeMap::<&VerificationFingerprint, VerificationCandidates>::new();
    for (index, voucher) in observed.iter().enumerate() {
        let after_mark = line
            .pre_import_mark
            .value
            .is_some_and(|mark| voucher.alter_id.is_some_and(|id| id > mark));
        if let Some(tag) = observed_tags[index].filter(|tag| expected_tags.contains(tag)) {
            tagged.entry(tag).or_default().insert(index, after_mark);
        } else {
            fallback
                .entry(&observed_fingerprints[index])
                .or_default()
                .insert(index, after_mark);
        }
    }
    let mut ambiguous_within_batch = Vec::new();
    let mut counts = BTreeMap::from([
        ("posted_verified", 0_u64),
        ("matching_content_observed", 0),
        ("posted_not_effective", 0),
        ("posted_divergent", 0),
        ("not_found", 0),
        ("not_attributable", 0),
        ("duplicate_fingerprint", 0),
    ]);
    for ((expected, expected_key), marker_identity) in line
        .vouchers
        .iter()
        .zip(&expected_fingerprints)
        .zip(&expected_markers)
    {
        let fingerprint_ambiguous_within_batch = expected_fingerprint_counts[expected_key] > 1;
        let tagged_group = tagged.get(marker_identity.as_str());
        let fingerprint_fallback = tagged_group.is_none();
        let group = tagged_group.or_else(|| fallback.get(expected_key));
        let marker = if fingerprint_fallback {
            "accounting_fingerprint"
        } else {
            "narration_tag"
        };
        let not_attributable = line.pre_import_mark.value.is_some()
            && group.is_some_and(|group| !fingerprint_fallback || !group.remaining.is_empty());
        let match_count = group.map_or(0, |group| group.after_mark.len());
        let matched_index = group.and_then(|group| group.after_mark.first().copied());
        let already_consumed = tagged_group.is_some_and(|group| group.consumed);
        let mut value = if not_attributable && match_count == 0 {
            counts
                .entry("not_attributable")
                .and_modify(|count| *count += 1);
            let reason = if already_consumed {
                "observed_voucher_already_attributed"
            } else if marker == "narration_tag" {
                "tag_precedes_pre_import_voucher_mark"
            } else {
                "fingerprint_precedes_pre_import_voucher_mark"
            };
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"not_attributable","marker":marker,"reason":reason})
        } else if match_count == 0 {
            counts.entry("not_found").and_modify(|count| *count += 1);
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"not_found"})
        } else if match_count > 1 {
            counts
                .entry("duplicate_fingerprint")
                .and_modify(|count| *count += 1);
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"duplicate_fingerprint","marker":marker,"matches":match_count})
        } else {
            let matched_index = matched_index.expect("one indexed candidate");
            if let Some(tag) = observed_tags[matched_index] {
                if let Some(group) = tagged.get_mut(tag) {
                    group.consume(matched_index);
                }
            }
            if let Some(group) = fallback.get_mut(&observed_fingerprints[matched_index]) {
                group.consume(matched_index);
            }
            let matched = &observed[matched_index];
            let diffs = voucher_diffs(
                expected,
                matched,
                expected_key.2 == observed_fingerprints[matched_index].2,
            );
            if fingerprint_fallback {
                counts
                    .entry("matching_content_observed")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"matching_content_observed","marker":marker,"attribution":"not_established","accounting_effective":voucher_is_accounting_effective(matched)?,"diffs":diffs,"voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id,"alter_id":matched.alter_id})
            } else if diffs.is_empty() && voucher_is_accounting_effective(matched)? {
                fully_verified_identities.insert(observed_identities[matched_index].clone());
                counts
                    .entry("posted_verified")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_verified","marker":marker,"voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id,"alter_id":matched.alter_id})
            } else if diffs.is_empty() {
                counts
                    .entry("posted_not_effective")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_not_effective","marker":marker,"reason":"voucher_cancelled_or_optional","voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id,"alter_id":matched.alter_id})
            } else {
                counts
                    .entry("posted_divergent")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_divergent","marker":marker,"diffs":diffs,"voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id})
            }
        };
        if fingerprint_fallback && fingerprint_ambiguous_within_batch {
            value["ambiguous_within_batch"] = Value::Bool(true);
            ambiguous_within_batch.push(expected.bridge_txn_id.clone());
        }
        rows.push(value);
    }
    let (batch_duplicates, unrelated_duplicates_in_window) = batch_duplicate_sets(
        observed,
        &observed_identities,
        &observed_fingerprints,
        &expected_fingerprint_counts,
        &observed_tags,
        &expected_tags,
        &fully_verified_identities,
    );
    Ok(
        json!({"counts":counts,"vouchers":rows,"duplicates":batch_duplicates,"unrelated_duplicates_in_window":unrelated_duplicates_in_window,"ambiguous_within_batch":ambiguous_within_batch}),
    )
}

fn voucher_is_accounting_effective(voucher: &ReadVoucher) -> Result<bool, String> {
    match (voucher.cancelled, voucher.optional) {
        (Some(false), Some(false)) => Ok(true),
        (Some(true), _) | (_, Some(true)) => Ok(false),
        _ => Err("voucher_accounting_state_not_observed".to_string()),
    }
}

fn verification_status(result: &Value, expected_voucher_count: usize) -> &'static str {
    if result["counts"]["posted_verified"].as_u64() == Some(expected_voucher_count as u64)
        && result["duplicates"].as_array().is_some_and(Vec::is_empty)
    {
        "posted_verified"
    } else {
        "verification_incomplete"
    }
}

fn batch_duplicate_sets(
    observed: &[ReadVoucher],
    identities: &[String],
    fingerprints: &[VerificationFingerprint],
    expected_fingerprints: &BTreeMap<&VerificationFingerprint, usize>,
    tags: &[Option<&str>],
    expected_tags: &BTreeSet<&str>,
    fully_verified_identities: &BTreeSet<String>,
) -> (Vec<Value>, Vec<Value>) {
    // Serialize the structured vector before hashing: ledger names may contain
    // the delimiters used inside an entry, so joining entries is ambiguous.
    let duplicate_keys = fingerprints.iter().map(sha256_json).collect::<Vec<_>>();
    let all_duplicates = duplicates(observed, identities, &duplicate_keys)
        .into_iter()
        .filter(|duplicate| {
            duplicate["kind"] != "accounting_fingerprint"
                || !duplicate["voucher_ids"].as_array().is_some_and(|ids| {
                    ids.iter().all(|id| {
                        id.as_str()
                            .is_some_and(|id| fully_verified_identities.contains(id))
                    })
                })
        });
    let batch_indexes = (0..observed.len())
        .filter(|&index| {
            tags[index].is_some_and(|tag| expected_tags.contains(tag))
                || expected_fingerprints.contains_key(&fingerprints[index])
        })
        .collect::<Vec<_>>();
    let batch_remote_ids = batch_indexes
        .iter()
        .filter_map(|&index| observed[index].remote_id.as_deref())
        .collect::<BTreeSet<_>>();
    let batch_fingerprints = batch_indexes
        .iter()
        .map(|&index| duplicate_keys[index].as_str())
        .collect::<BTreeSet<_>>();
    let (batch, unrelated): (Vec<_>, Vec<_>) =
        all_duplicates.partition(|duplicate| match duplicate["kind"].as_str() {
            Some("remote_id") => duplicate["remote_id"]
                .as_str()
                .is_some_and(|id| batch_remote_ids.contains(id)),
            Some("accounting_fingerprint") => duplicate["fingerprint"]
                .as_str()
                .is_some_and(|key| batch_fingerprints.contains(key)),
            _ => false,
        });
    let safe_duplicate = |mut duplicate: Value| {
        if let Some(fields) = duplicate.as_object_mut() {
            if let Some(Value::String(fingerprint)) = fields.remove("fingerprint") {
                fields.insert("fingerprint_sha256".into(), json!(fingerprint));
            }
        }
        duplicate
    };
    (
        batch.into_iter().map(safe_duplicate).collect(),
        unrelated.into_iter().map(safe_duplicate).collect(),
    )
}

fn voucher_diffs(
    expected: &ImportVoucher,
    actual: &ReadVoucher,
    entries_match: bool,
) -> Vec<Value> {
    let mut diffs = Vec::new();
    if actual.date.as_deref() != normalized_date(&expected.date).ok().as_deref() {
        diffs.push(json!("date"));
    }
    if actual.voucher_type.as_deref() != Some(expected.voucher_type.as_str()) {
        diffs.push(json!("voucher_type"));
    }
    if expected.voucher_number.is_some()
        && actual.voucher_number.as_deref() != expected.voucher_number.as_deref()
    {
        diffs.push(json!("voucher_number"));
    }
    if !entries_match {
        let expected_entries = expected
            .entries
            .iter()
            .map(|entry| {
                json!({"ledger":party_name(&entry.ledger), "amount":entry.amount, "side":entry.side,
                "is_deemed_positive":entry.side.tally_positive()})
            })
            .collect::<Vec<_>>();
        let actual_entries = actual
            .entries
            .iter()
            .map(|entry| {
                json!({"ledger":party_name(&entry.ledger), "amount":entry.amount,
                "is_deemed_positive":entry.is_deemed_positive})
            })
            .collect::<Vec<_>>();
        diffs.push(json!({"entries":{"expected":expected_entries,"observed":actual_entries}}));
    }
    diffs
}

fn expected_entry_fingerprint(voucher: &ImportVoucher) -> Vec<String> {
    let mut result = voucher
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{}",
                entry.ledger,
                match entry.side {
                    EntrySide::Dr => format!("-{}", entry.amount),
                    EntrySide::Cr => entry.amount.clone(),
                },
                entry.side.tally_positive()
            )
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}
fn actual_entry_fingerprint(voucher: &ReadVoucher) -> Vec<String> {
    let mut result = voucher
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{}",
                entry.ledger, entry.amount, entry.is_deemed_positive
            )
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}

fn observed_voucher_identity(voucher: &ReadVoucher) -> Result<String, String> {
    voucher
        .guid
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map(|id| format!("guid:{}", id.to_ascii_lowercase()))
        .or_else(|| {
            voucher
                .master_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .map(|id| format!("master_id:{id}"))
        })
        .ok_or_else(|| "import_verification_identity_invalid".to_string())
}

fn duplicates(
    observed: &[ReadVoucher],
    identities: &[String],
    cached_fingerprints: &[String],
) -> Vec<Value> {
    let mut remote = BTreeMap::<String, BTreeSet<String>>::new();
    let mut fingerprints = BTreeMap::<String, BTreeMap<String, Option<String>>>::new();
    for ((voucher, identity), fingerprint) in
        observed.iter().zip(identities).zip(cached_fingerprints)
    {
        if let Some(id) = voucher
            .remote_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
        {
            remote
                .entry(id.clone())
                .or_default()
                .insert(identity.clone());
        }
        fingerprints
            .entry(fingerprint.clone())
            .or_default()
            .insert(identity.clone(), voucher.remote_id.clone());
    }
    let mut result = remote.into_iter().filter(|(_, identities)| identities.len() > 1)
        .map(|(remote_id, identities)| json!({"kind":"remote_id","remote_id":remote_id,"count":identities.len()}))
        .collect::<Vec<_>>();
    result.extend(fingerprints.into_iter().filter(|(_, identities)| identities.len() > 1)
        .map(|(fingerprint, identities)| {
            let remote_ids = identities.values().flatten().collect::<BTreeSet<_>>();
            json!({"kind":"accounting_fingerprint","fingerprint":fingerprint,"voucher_ids":identities.keys().collect::<Vec<_>>(),"remote_ids":remote_ids})
        }));
    result
}

fn company_high_water_mark(high_water: &Value) -> Result<PreImportMark, String> {
    let voucher = high_water["altvchid"]
        .as_u64()
        .ok_or_else(|| "pre_import_mark_unobserved".to_string())?;
    let master = high_water["altmstid"]
        .as_u64()
        .ok_or_else(|| "pre_import_mark_unobserved".to_string())?;
    Ok(PreImportMark {
        kind: "company_high_water".to_string(),
        value: Some(voucher),
        master_value: Some(master),
    })
}

fn alter_id_delta(mark: &PreImportMark, observed: &[ReadVoucher]) -> Value {
    let latest = observed.iter().filter_map(|voucher| voucher.alter_id).max();
    match (mark.value, latest) {
        (Some(before), Some(after)) if after >= before => {
            json!({"before":before,"after_seen":after,"delta":after-before})
        }
        _ => json!({"before":mark.value,"after_seen":latest,"delta":"not_observed"}),
    }
}

fn render_proof_markdown(proof: &Value) -> String {
    let mut output = format!(
        "# Voucher import verification — {}\n\n",
        proof["batch_id"].as_str().unwrap_or("unknown")
    );
    let dispatch_state = proof["dispatch"]["state"].as_str();
    if !proof["error"].is_null()
        || (proof.get("dispatch").is_some()
            && !matches!(
                dispatch_state,
                Some("posted_verified" | "previous_attempt_reconciled")
            ))
    {
        output.push_str("**Reconciliation required — this report does not confirm posting.**\n\nA matching voucher readback alone is insufficient. Reconcile the original saved batch; do not rebuild or resend it.\n\n");
    }
    if let Some(state) = dispatch_state {
        output.push_str(&format!(
            "- Dispatch verdict: `{state}`\n- Response state: `{}`\n",
            proof["dispatch"]["response_state"]
                .as_str()
                .unwrap_or("unknown")
        ));
    }
    if let Some(code) = proof["error"]["code"].as_str() {
        output.push_str(&format!("- Error: `{code}`\n"));
    }
    output.push_str(&format!("\n- Company: `{}`\n- Batch SHA-256: `{}`\n- Readback checked: `{}`\n- Readback counts: matching {}, divergent {}, not found {}\n- AlterID delta: `{}`\n- Unrelated duplicates in window: {}\n\n| Transaction | Readback status |\n| --- | --- |\n", proof["company"]["name"].as_str().unwrap_or("unknown"), proof["batch_sha256"].as_str().unwrap_or("unknown"), proof["verified_at"].as_str().unwrap_or("unknown"), proof["counts"]["posted_verified"], proof["counts"]["posted_divergent"], proof["counts"]["not_found"], proof["alter_id_delta"], proof["unrelated_duplicates_in_window"].as_array().map_or(0, Vec::len)));
    for row in proof["vouchers"].as_array().into_iter().flatten() {
        output.push_str(&format!(
            "| {} | {} |\n",
            row["bridge_txn_id"].as_str().unwrap_or("unknown"),
            row["status"].as_str().unwrap_or("unknown")
        ));
    }
    output.push_str(&format!(
        "\nEvidence hashes: company `{}`, voucher read `{}`.\n",
        proof["evidence"]["company"]["response_sha256"]
            .as_str()
            .unwrap_or("unknown"),
        proof["evidence"]["voucher_read_sha256"]
            .as_str()
            .unwrap_or("unknown")
    ));
    output
}
fn local_evidence(label: &str) -> Evidence {
    Evidence {
        request_sha256: sha256_hex(label.as_bytes()),
        response_sha256: sha256_hex(label.as_bytes()),
        bytes: 0,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    }
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = super::local_file::open_local_file(path, true)
        .map_err(|_| "import_file_write_failed".to_string())?;
    set_private_file(&file)?;
    file.set_len(0)
        .and_then(|_| file.write_all(bytes))
        .and_then(|_| file.sync_data())
        .map_err(|_| "import_file_write_failed".to_string())
}
fn set_private_file(file: &std::fs::File) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "import_file_permissions_failed".to_string())?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}
#[cfg(test)]
#[path = "agent_import_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_import_file_tests.rs"]
mod file_tests;
