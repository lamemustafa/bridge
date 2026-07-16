//! Transport qualification contracts for read-only semantic shadowing.
//!
//! This module deliberately does not select JSONEX, write mirror data, advance
//! checkpoints, or alter accounting proof. A matching observation remains in
//! `Shadowing`; promotion requires a separate policy backed by repeated live
//! evidence and measured operational benefit.

use crate::{
    CanonicalPackWindow, CanonicalText, CapabilityPackId, CoreAccountingBatch, ExactDecimal,
    LedgerEntryPolarity, PackBatch, PackSchemaVersion, ReadWindow, SourceCountScope,
    SourceCountScopeDescriptor, SourceIdentity, SourceIdentityKind, SourceRecordId, TallyDate,
    TallyError, TransportId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const TRANSPORT_QUALIFICATION_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransportQualificationScope {
    pub source_identity: SourceIdentity,
    pub product: CanonicalText,
    pub release: CanonicalText,
    pub mode: CanonicalText,
    pub pack: CapabilityPackId,
    pub pack_schema_version: PackSchemaVersion,
    pub window: ReadWindow,
    pub query_profile: CanonicalText,
    pub filters_sha256: CanonicalText,
    pub reference_transport: TransportId,
    pub candidate_transport: TransportId,
    /// Versioned request/response schema profile used for the candidate read.
    pub candidate_request_profile: CanonicalText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStabilityEvidence {
    ReferenceBracketMatched,
    ReferenceBracketMismatch,
    EvidenceUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportParityVerdict {
    Matched,
    Mismatched,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateTransportRecommendation {
    ContinueShadowing,
    RecommendQuarantine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportParityReasonCode {
    SemanticParityObserved,
    CanonicalSemanticsMismatch,
    SourceCountEvidenceMismatch,
    RecordEvidenceCoverageMismatch,
    ReferenceBracketMismatch,
    SourceCountEvidenceUnavailable,
    RecordEvidenceUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransportReadMetrics {
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
    pub response_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransportParityObservation {
    pub contract_version: u16,
    /// Hash of the exact company/product/release/mode/pack/window/query scope.
    /// The raw company identity is intentionally not repeated in this receipt.
    pub scope_sha256: String,
    pub reference_transport: TransportId,
    pub candidate_transport: TransportId,
    pub reference_semantic_sha256: String,
    pub candidate_semantic_sha256: String,
    pub source_stability: SourceStabilityEvidence,
    pub verdict: TransportParityVerdict,
    /// A recommendation for a separate durable policy reducer. This value is
    /// neither persisted nor enforced by the comparator.
    pub candidate_recommendation: CandidateTransportRecommendation,
    pub reason_codes: Vec<TransportParityReasonCode>,
    pub reference_before_metrics: TransportReadMetrics,
    pub candidate_metrics: TransportReadMetrics,
    pub reference_after_metrics: TransportReadMetrics,
}

/// Compares an XML/JSONEX/XML bracket without making raw payloads or
/// transport-derived entry IDs part of semantic equality. The scope remains a
/// caller-supplied assertion until a runtime reader binds it to a live request.
pub fn qualify_core_transport_shadow(
    scope: &TransportQualificationScope,
    reference_before: &CanonicalPackWindow,
    candidate: &CanonicalPackWindow,
    reference_after: &CanonicalPackWindow,
    reference_before_metrics: TransportReadMetrics,
    candidate_metrics: TransportReadMetrics,
    reference_after_metrics: TransportReadMetrics,
) -> Result<TransportParityObservation, TallyError> {
    validate_scope(scope)?;
    validate_metrics(reference_before_metrics)?;
    validate_metrics(candidate_metrics)?;
    validate_metrics(reference_after_metrics)?;
    if reference_before_metrics.completed_at_unix_ms > candidate_metrics.started_at_unix_ms
        || candidate_metrics.completed_at_unix_ms > reference_after_metrics.started_at_unix_ms
    {
        return Err(invalid_data(
            "transport_qualification_bracket_order_invalid",
        ));
    }
    reference_before.validate_record_evidence_binding()?;
    candidate.validate_record_evidence_binding()?;
    reference_after.validate_record_evidence_binding()?;
    reference_before.validate_source_count_evidence()?;
    candidate.validate_source_count_evidence()?;
    reference_after.validate_source_count_evidence()?;
    validate_count_scopes(scope, reference_before)?;
    validate_count_scopes(scope, candidate)?;
    validate_count_scopes(scope, reference_after)?;

    let reference_batch = core_batch(reference_before)?;
    let candidate_batch = core_batch(candidate)?;
    let reference_after_batch = core_batch(reference_after)?;
    validate_core_reference_integrity(reference_batch)?;
    validate_core_reference_integrity(candidate_batch)?;
    validate_core_reference_integrity(reference_after_batch)?;
    let reference_projection = project_core(reference_batch);
    let candidate_projection = project_core(candidate_batch);
    let reference_after_projection = project_core(reference_after_batch);
    let reference_semantic_sha256 = sha256_json_contract(
        "bridge_tally_core_semantic_projection_v1",
        &reference_projection,
    )?;
    let candidate_semantic_sha256 = sha256_json_contract(
        "bridge_tally_core_semantic_projection_v1",
        &candidate_projection,
    )?;

    let reference_counts = project_source_counts(reference_before);
    let candidate_counts = project_source_counts(candidate);
    let reference_after_counts = project_source_counts(reference_after);
    let reference_evidence = project_record_evidence(reference_before);
    let candidate_evidence = project_record_evidence(candidate);
    let reference_after_evidence = project_record_evidence(reference_after);

    let mut mismatches = Vec::new();
    if reference_projection != candidate_projection {
        mismatches.push(TransportParityReasonCode::CanonicalSemanticsMismatch);
    }
    if reference_counts != candidate_counts {
        mismatches.push(TransportParityReasonCode::SourceCountEvidenceMismatch);
    }
    if reference_evidence != candidate_evidence {
        mismatches.push(TransportParityReasonCode::RecordEvidenceCoverageMismatch);
    }

    let reference_bracket_mismatch = reference_projection != reference_after_projection
        || reference_counts != reference_after_counts
        || reference_evidence != reference_after_evidence;
    let mut unavailable = Vec::new();
    if !has_complete_core_count_evidence(reference_before, reference_batch)
        || !has_complete_core_count_evidence(candidate, candidate_batch)
        || !has_complete_core_count_evidence(reference_after, reference_after_batch)
    {
        unavailable.push(TransportParityReasonCode::SourceCountEvidenceUnavailable);
    }
    if reference_evidence.is_none()
        || candidate_evidence.is_none()
        || reference_after_evidence.is_none()
    {
        unavailable.push(TransportParityReasonCode::RecordEvidenceUnavailable);
    }

    let (source_stability, verdict, candidate_recommendation, reason_codes) =
        if !unavailable.is_empty() {
            unavailable.extend(mismatches);
            (
                SourceStabilityEvidence::EvidenceUnavailable,
                TransportParityVerdict::Inconclusive,
                CandidateTransportRecommendation::ContinueShadowing,
                unavailable,
            )
        } else if reference_bracket_mismatch {
            let mut reasons = vec![TransportParityReasonCode::ReferenceBracketMismatch];
            reasons.extend(mismatches);
            (
                SourceStabilityEvidence::ReferenceBracketMismatch,
                TransportParityVerdict::Inconclusive,
                CandidateTransportRecommendation::ContinueShadowing,
                reasons,
            )
        } else if mismatches.is_empty() {
            (
                SourceStabilityEvidence::ReferenceBracketMatched,
                TransportParityVerdict::Matched,
                CandidateTransportRecommendation::ContinueShadowing,
                vec![TransportParityReasonCode::SemanticParityObserved],
            )
        } else {
            (
                SourceStabilityEvidence::ReferenceBracketMatched,
                TransportParityVerdict::Mismatched,
                CandidateTransportRecommendation::RecommendQuarantine,
                mismatches,
            )
        };

    Ok(TransportParityObservation {
        contract_version: TRANSPORT_QUALIFICATION_CONTRACT_VERSION,
        scope_sha256: sha256_json_contract("bridge_tally_transport_scope_v1", scope)?,
        reference_transport: scope.reference_transport,
        candidate_transport: scope.candidate_transport,
        reference_semantic_sha256,
        candidate_semantic_sha256,
        source_stability,
        verdict,
        candidate_recommendation,
        reason_codes,
        reference_before_metrics,
        candidate_metrics,
        reference_after_metrics,
    })
}

fn validate_scope(scope: &TransportQualificationScope) -> Result<(), TallyError> {
    if scope.pack != CapabilityPackId::CoreAccounting {
        return Err(invalid_data("transport_qualification_pack_not_core"));
    }
    if scope.reference_transport != TransportId::XmlHttp
        || scope.candidate_transport != TransportId::JsonEx
    {
        return Err(invalid_data("transport_qualification_pair_invalid"));
    }
    CanonicalText::parse(scope.source_identity.bridge_source_lineage.clone())?;
    SourceRecordId::parse(scope.source_identity.company_guid.clone())?;
    if !is_lower_sha256(&scope.source_identity.observed_fingerprint) {
        return Err(invalid_data(
            "transport_qualification_source_fingerprint_invalid",
        ));
    }
    if scope.pack_schema_version != crate::CORE_ACCOUNTING_SCHEMA_VERSION {
        return Err(invalid_data(
            "transport_qualification_schema_version_invalid",
        ));
    }
    TallyDate::parse(scope.window.from_yyyymmdd.clone())?;
    TallyDate::parse(scope.window.to_yyyymmdd.clone())?;
    if scope.window.from_yyyymmdd > scope.window.to_yyyymmdd {
        return Err(invalid_data("transport_qualification_window_invalid"));
    }
    if !is_lower_sha256(scope.filters_sha256.as_str()) {
        return Err(invalid_data("transport_qualification_filter_hash_invalid"));
    }
    Ok(())
}

fn validate_count_scopes(
    scope: &TransportQualificationScope,
    window: &CanonicalPackWindow,
) -> Result<(), TallyError> {
    let Some(counts) = &window.source_counts else {
        return Ok(());
    };
    for count in counts {
        let descriptor = SourceCountScopeDescriptor {
            source_identity: scope.source_identity.clone(),
            pack: scope.pack,
            pack_schema_version: scope.pack_schema_version,
            object_type: count.object_type.clone(),
            query_profile: scope.query_profile.clone(),
            filters_sha256: scope.filters_sha256.clone(),
            window: match count.source_count_scope {
                SourceCountScope::Complete => None,
                SourceCountScope::Window => Some(scope.window.clone()),
            },
        };
        if !count.matches_scope_descriptor(&descriptor)? {
            return Err(invalid_data(
                "transport_qualification_source_count_scope_mismatch",
            ));
        }
    }
    Ok(())
}

fn validate_core_reference_integrity(batch: &CoreAccountingBatch) -> Result<(), TallyError> {
    fn unique<'a>(values: impl Iterator<Item = &'a str>) -> bool {
        let mut observed = BTreeSet::new();
        values.into_iter().all(|value| observed.insert(value))
    }

    if !unique(batch.groups.iter().map(|record| record.source_id.as_str()))
        || !unique(batch.ledgers.iter().map(|record| record.source_id.as_str()))
        || !unique(
            batch
                .voucher_types
                .iter()
                .map(|record| record.source_id.as_str()),
        )
        || !unique(
            batch
                .vouchers
                .iter()
                .map(|record| record.source_id.as_str()),
        )
        || !unique(
            batch
                .ledger_entries
                .iter()
                .map(|record| record.source_id.as_str()),
        )
    {
        return Err(invalid_data(
            "transport_qualification_duplicate_canonical_id",
        ));
    }

    let group_ids = batch
        .groups
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    for group in &batch.groups {
        SourceRecordId::parse(group.source_id.clone())?;
        CanonicalText::parse(group.name.clone())?;
        if group
            .parent_source_id
            .as_deref()
            .is_some_and(|parent| !group_ids.contains(parent))
        {
            return Err(invalid_data("transport_qualification_group_parent_missing"));
        }
    }
    for ledger in &batch.ledgers {
        SourceRecordId::parse(ledger.source_id.clone())?;
        CanonicalText::parse(ledger.name.clone())?;
        if ledger
            .parent_source_id
            .as_deref()
            .is_some_and(|parent| !group_ids.contains(parent))
        {
            return Err(invalid_data(
                "transport_qualification_ledger_parent_missing",
            ));
        }
    }
    for voucher_type in &batch.voucher_types {
        SourceRecordId::parse(voucher_type.source_id.clone())?;
        CanonicalText::parse(voucher_type.name.clone())?;
    }
    for voucher in &batch.vouchers {
        SourceRecordId::parse(voucher.source_id.clone())?;
        TallyDate::parse(voucher.date_yyyymmdd.clone())?;
    }
    for entry in &batch.ledger_entries {
        SourceRecordId::parse(entry.source_id.clone())?;
    }
    if crate::reconciliation::assess_core_accounting(batch)
        .checks
        .reference_integrity
        != crate::reconciliation::CheckState::Passed
    {
        return Err(invalid_data(
            "transport_qualification_reference_integrity_failed",
        ));
    }
    Ok(())
}

fn validate_metrics(metrics: TransportReadMetrics) -> Result<(), TallyError> {
    if metrics.started_at_unix_ms < 0
        || metrics.completed_at_unix_ms < metrics.started_at_unix_ms
        || metrics.response_bytes == 0
    {
        return Err(invalid_data("transport_qualification_metrics_invalid"));
    }
    Ok(())
}

fn has_complete_core_count_evidence(
    window: &CanonicalPackWindow,
    batch: &CoreAccountingBatch,
) -> bool {
    let Some(counts) = &window.source_counts else {
        return false;
    };
    let expected = [
        ("group", SourceCountScope::Complete, batch.groups.len()),
        ("ledger", SourceCountScope::Complete, batch.ledgers.len()),
        (
            "voucher_type",
            SourceCountScope::Complete,
            batch.voucher_types.len(),
        ),
        ("voucher", SourceCountScope::Window, batch.vouchers.len()),
        (
            "ledger_entry",
            SourceCountScope::Window,
            batch.ledger_entries.len(),
        ),
    ];
    counts.len() == expected.len()
        && expected.iter().all(|(object_type, scope, count)| {
            counts.iter().any(|evidence| {
                evidence.object_type.as_str() == *object_type
                    && evidence.source_count_scope == *scope
                    && evidence.source_reported_count == *count as u64
            })
        })
}

fn core_batch(window: &CanonicalPackWindow) -> Result<&CoreAccountingBatch, TallyError> {
    match &window.batch {
        PackBatch::CoreAccounting(batch) => Ok(batch),
        _ => Err(invalid_data("transport_qualification_window_pack_mismatch")),
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct CoreProjection {
    groups: Vec<GroupProjection>,
    ledgers: Vec<LedgerProjection>,
    voucher_types: Vec<VoucherTypeProjection>,
    vouchers: Vec<VoucherProjection>,
    ledger_entries: Vec<LedgerEntryProjection>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct GroupProjection {
    source_id: String,
    name: String,
    parent_source_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct LedgerProjection {
    source_id: String,
    name: String,
    parent_source_id: Option<String>,
    opening_balance: Option<String>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct VoucherTypeProjection {
    source_id: String,
    name: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct VoucherProjection {
    source_id: String,
    date_yyyymmdd: String,
    voucher_type_source_id: String,
    voucher_number: Option<String>,
    cancelled: bool,
    optional: bool,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct LedgerEntryProjection {
    voucher_source_id: String,
    ledger_source_id: String,
    amount: String,
    polarity: &'static str,
}

fn project_core(batch: &CoreAccountingBatch) -> CoreProjection {
    let mut groups = batch
        .groups
        .iter()
        .map(|record| GroupProjection {
            source_id: record.source_id.clone(),
            name: record.name.clone(),
            parent_source_id: record.parent_source_id.clone(),
        })
        .collect::<Vec<_>>();
    groups.sort();

    let mut ledgers = batch
        .ledgers
        .iter()
        .map(|record| LedgerProjection {
            source_id: record.source_id.clone(),
            name: record.name.clone(),
            parent_source_id: record.parent_source_id.clone(),
            opening_balance: record.opening_balance.as_ref().map(normalize_decimal),
        })
        .collect::<Vec<_>>();
    ledgers.sort();

    let mut voucher_types = batch
        .voucher_types
        .iter()
        .map(|record| VoucherTypeProjection {
            source_id: record.source_id.clone(),
            name: record.name.clone(),
        })
        .collect::<Vec<_>>();
    voucher_types.sort();

    let mut vouchers = batch
        .vouchers
        .iter()
        .map(|record| VoucherProjection {
            source_id: record.source_id.clone(),
            date_yyyymmdd: record.date_yyyymmdd.clone(),
            voucher_type_source_id: record.voucher_type_source_id.clone(),
            voucher_number: record.voucher_number.clone(),
            cancelled: record.cancelled,
            optional: record.optional,
        })
        .collect::<Vec<_>>();
    vouchers.sort();

    let mut ledger_entries = batch
        .ledger_entries
        .iter()
        .map(|record| LedgerEntryProjection {
            // The production entry source ID includes raw-fragment provenance
            // and is therefore intentionally transport-specific.
            voucher_source_id: record.voucher_source_id.clone(),
            ledger_source_id: record.ledger_source_id.clone(),
            amount: normalize_decimal(&record.amount),
            polarity: match record.polarity {
                LedgerEntryPolarity::Debit => "debit",
                LedgerEntryPolarity::Credit => "credit",
            },
        })
        .collect::<Vec<_>>();
    ledger_entries.sort();

    CoreProjection {
        groups,
        ledgers,
        voucher_types,
        vouchers,
        ledger_entries,
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct SourceCountProjection {
    object_type: String,
    query_profile: String,
    source_scope_fingerprint: String,
    source_count_scope: &'static str,
    source_reported_count: u64,
}

fn project_source_counts(window: &CanonicalPackWindow) -> Option<Vec<SourceCountProjection>> {
    window.source_counts.as_ref().map(|counts| {
        let mut projection = counts
            .iter()
            .map(|count| SourceCountProjection {
                object_type: count.object_type.as_str().to_string(),
                query_profile: count.query_profile.as_str().to_string(),
                source_scope_fingerprint: count.source_scope_fingerprint.as_str().to_string(),
                source_count_scope: match count.source_count_scope {
                    SourceCountScope::Complete => "complete",
                    SourceCountScope::Window => "window",
                },
                source_reported_count: count.source_reported_count,
            })
            .collect::<Vec<_>>();
        projection.sort();
        projection
    })
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct RecordEvidenceProjection {
    object_type: String,
    identity_kind: &'static str,
    observed_guid: Option<String>,
    observed_remote_id: Option<String>,
    observed_master_id: Option<String>,
    alter_id: Option<String>,
}

fn project_record_evidence(window: &CanonicalPackWindow) -> Option<Vec<RecordEvidenceProjection>> {
    window.record_evidence.as_ref().map(|records| {
        let mut projection = records
            .iter()
            .map(|record| RecordEvidenceProjection {
                object_type: record.object_type.as_str().to_string(),
                identity_kind: match record.identity_kind {
                    SourceIdentityKind::Guid => "guid",
                    SourceIdentityKind::RemoteId => "remote_id",
                    SourceIdentityKind::MasterId => "master_id",
                    SourceIdentityKind::Fallback => "fallback",
                },
                observed_guid: record
                    .observed_identities
                    .guid
                    .as_ref()
                    .map(|value| value.as_str().to_string()),
                observed_remote_id: record
                    .observed_identities
                    .remote_id
                    .as_ref()
                    .map(|value| value.as_str().to_string()),
                observed_master_id: record
                    .observed_identities
                    .master_id
                    .as_ref()
                    .map(|value| value.as_str().to_string()),
                alter_id: record
                    .alter_id
                    .as_ref()
                    .map(|value| value.as_str().to_string()),
            })
            .collect::<Vec<_>>();
        projection.sort();
        projection
    })
}

fn normalize_decimal(value: &ExactDecimal) -> String {
    let raw = value.as_str();
    let (negative, unsigned) = raw
        .strip_prefix('-')
        .map_or((false, raw), |body| (true, body));
    let (whole, fractional) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(whole, fraction)| {
            (whole, Some(fraction))
        });
    let normalized_whole = whole.trim_start_matches('0');
    let normalized_whole = if normalized_whole.is_empty() {
        "0"
    } else {
        normalized_whole
    };
    let normalized_fractional = fractional.map(|part| part.trim_end_matches('0'));
    let zero = normalized_whole == "0" && normalized_fractional.is_none_or(str::is_empty);
    let sign = if negative && !zero { "-" } else { "" };
    match normalized_fractional.filter(|part| !part.is_empty()) {
        Some(part) => format!("{sign}{normalized_whole}.{part}"),
        None => format!("{sign}{normalized_whole}"),
    }
}

#[derive(Serialize)]
struct QualificationHashPreimage<'a, T: Serialize + ?Sized> {
    contract: &'static str,
    value: &'a T,
}

fn sha256_json_contract(
    contract: &'static str,
    value: &(impl Serialize + ?Sized),
) -> Result<String, TallyError> {
    let bytes = serde_json::to_vec(&QualificationHashPreimage { contract, value })
        .map_err(|_| invalid_data("transport_qualification_serialization_failed"))?;
    Ok(hex_lower(&Sha256::digest(bytes)))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn invalid_data(code: &str) -> TallyError {
    TallyError::InvalidData {
        code: code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        source_count_scope_fingerprint, LedgerEntryRecord, LedgerRecord, ObservedSourceIdentities,
        RawSourceSha256, SourceRecordEvidence, SourceReportedCountEvidence, VoucherRecord,
        VoucherTypeRecord,
    };

    fn scope() -> TransportQualificationScope {
        TransportQualificationScope {
            source_identity: SourceIdentity {
                bridge_source_lineage: "tally_local:test".to_string(),
                company_guid: "synthetic-private-company-guid".to_string(),
                observed_fingerprint: "c".repeat(64),
            },
            product: CanonicalText::parse("TallyPrime").unwrap(),
            release: CanonicalText::parse("7.0").unwrap(),
            mode: CanonicalText::parse("Educational").unwrap(),
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: crate::CORE_ACCOUNTING_SCHEMA_VERSION,
            window: ReadWindow {
                from_yyyymmdd: "20260401".to_string(),
                to_yyyymmdd: "20260430".to_string(),
            },
            query_profile: CanonicalText::parse("bridge_core_v3").unwrap(),
            filters_sha256: CanonicalText::parse("a".repeat(64)).unwrap(),
            reference_transport: TransportId::XmlHttp,
            candidate_transport: TransportId::JsonEx,
            candidate_request_profile: CanonicalText::parse("jsonex_core_v1").unwrap(),
        }
    }

    fn metrics(started_at_unix_ms: i64, completed_at_unix_ms: i64) -> TransportReadMetrics {
        TransportReadMetrics {
            started_at_unix_ms,
            completed_at_unix_ms,
            response_bytes: 128,
        }
    }

    fn evidence(object_type: &str, source_id: &str, raw_hash_byte: char) -> SourceRecordEvidence {
        let source_id = SourceRecordId::parse(source_id).unwrap();
        let fallback = object_type == "ledger_entry";
        SourceRecordEvidence {
            object_type: CanonicalText::parse(object_type).unwrap(),
            source_id: source_id.clone(),
            identity_kind: if fallback {
                SourceIdentityKind::Fallback
            } else {
                SourceIdentityKind::Guid
            },
            observed_identities: if fallback {
                ObservedSourceIdentities::default()
            } else {
                ObservedSourceIdentities {
                    guid: Some(source_id.clone()),
                    ..Default::default()
                }
            },
            raw_source_sha256: RawSourceSha256::parse(raw_hash_byte.to_string().repeat(64))
                .unwrap(),
            alter_id: None,
        }
    }

    fn entry_window(
        scope: &TransportQualificationScope,
        entry_source_id: &str,
        amount: &str,
        raw_hash_byte: char,
    ) -> CanonicalPackWindow {
        let count = |object_type: &str, source_count_scope: SourceCountScope, value: u64| {
            let descriptor = SourceCountScopeDescriptor {
                source_identity: scope.source_identity.clone(),
                pack: scope.pack,
                pack_schema_version: scope.pack_schema_version,
                object_type: CanonicalText::parse(object_type).unwrap(),
                query_profile: scope.query_profile.clone(),
                filters_sha256: scope.filters_sha256.clone(),
                window: (source_count_scope == SourceCountScope::Window)
                    .then(|| scope.window.clone()),
            };
            SourceReportedCountEvidence {
                object_type: descriptor.object_type.clone(),
                query_profile: descriptor.query_profile.clone(),
                source_scope_fingerprint: source_count_scope_fingerprint(
                    &descriptor,
                    source_count_scope,
                )
                .unwrap(),
                source_count_scope,
                source_reported_count: value,
            }
        };
        CanonicalPackWindow {
            batch: PackBatch::CoreAccounting(CoreAccountingBatch {
                ledgers: vec![LedgerRecord {
                    source_id: "ledger:1".to_string(),
                    name: "Synthetic Ledger".to_string(),
                    parent_source_id: None,
                    opening_balance: Some(ExactDecimal::parse("0.00").unwrap()),
                }],
                voucher_types: vec![VoucherTypeRecord {
                    source_id: "voucher-type:1".to_string(),
                    name: "Synthetic Voucher Type".to_string(),
                }],
                vouchers: vec![VoucherRecord {
                    source_id: "voucher:1".to_string(),
                    date_yyyymmdd: "20260415".to_string(),
                    voucher_type_source_id: "voucher-type:1".to_string(),
                    voucher_number: Some("SYN-1".to_string()),
                    cancelled: false,
                    optional: false,
                }],
                ledger_entries: vec![LedgerEntryRecord {
                    source_id: entry_source_id.to_string(),
                    voucher_source_id: "voucher:1".to_string(),
                    ledger_source_id: "ledger:1".to_string(),
                    amount: ExactDecimal::parse(amount).unwrap(),
                    polarity: LedgerEntryPolarity::Debit,
                }],
                ..Default::default()
            }),
            source_counts: Some(vec![
                count("group", SourceCountScope::Complete, 0),
                count("ledger", SourceCountScope::Complete, 1),
                count("voucher_type", SourceCountScope::Complete, 1),
                count("voucher", SourceCountScope::Window, 1),
                count("ledger_entry", SourceCountScope::Window, 1),
            ]),
            record_evidence: Some(vec![
                evidence("ledger", "ledger:1", raw_hash_byte),
                evidence("voucher_type", "voucher-type:1", raw_hash_byte),
                evidence("voucher", "voucher:1", raw_hash_byte),
                evidence("ledger_entry", entry_source_id, raw_hash_byte),
            ]),
        }
    }

    #[test]
    fn semantic_match_excludes_transport_raw_hash_and_entry_id_and_normalizes_scale() {
        let scope = scope();
        let reference = entry_window(&scope, "xml-entry-hash", "-001.00", 'a');
        let candidate = entry_window(&scope, "json-entry-hash", "-1.0", 'b');
        let observation = qualify_core_transport_shadow(
            &scope,
            &reference,
            &candidate,
            &reference,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .unwrap();

        assert_eq!(observation.verdict, TransportParityVerdict::Matched);
        assert_eq!(
            observation.candidate_recommendation,
            CandidateTransportRecommendation::ContinueShadowing
        );
        assert_eq!(
            observation.reason_codes,
            vec![TransportParityReasonCode::SemanticParityObserved]
        );
        assert_eq!(
            observation.reference_semantic_sha256,
            observation.candidate_semantic_sha256
        );
    }

    #[test]
    fn bracketed_semantic_mismatch_recommends_scope_quarantine() {
        let scope = scope();
        let reference = entry_window(&scope, "xml-entry", "-1.00", 'a');
        let candidate = entry_window(&scope, "json-entry", "-2.00", 'b');
        let observation = qualify_core_transport_shadow(
            &scope,
            &reference,
            &candidate,
            &reference,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .unwrap();

        assert_eq!(observation.verdict, TransportParityVerdict::Mismatched);
        assert_eq!(
            observation.candidate_recommendation,
            CandidateTransportRecommendation::RecommendQuarantine
        );
        assert!(observation
            .reason_codes
            .contains(&TransportParityReasonCode::CanonicalSemanticsMismatch));
    }

    #[test]
    fn drift_or_missing_evidence_never_becomes_a_transport_mismatch() {
        let scope = scope();
        let before = entry_window(&scope, "xml-entry", "-1.00", 'a');
        let candidate = entry_window(&scope, "json-entry", "-2.00", 'b');
        let after = entry_window(&scope, "xml-entry-after", "-3.00", 'c');
        let drift = qualify_core_transport_shadow(
            &scope,
            &before,
            &candidate,
            &after,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .unwrap();
        assert_eq!(drift.verdict, TransportParityVerdict::Inconclusive);
        assert_eq!(
            drift.source_stability,
            SourceStabilityEvidence::ReferenceBracketMismatch
        );
        assert!(drift
            .reason_codes
            .contains(&TransportParityReasonCode::CanonicalSemanticsMismatch));

        let mut no_counts = before.clone();
        no_counts.source_counts = None;
        let missing = qualify_core_transport_shadow(
            &scope,
            &no_counts,
            &no_counts,
            &no_counts,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .unwrap();
        assert_eq!(missing.verdict, TransportParityVerdict::Inconclusive);
        assert!(missing
            .reason_codes
            .contains(&TransportParityReasonCode::SourceCountEvidenceUnavailable));

        let mut partial_counts = before.clone();
        partial_counts.source_counts.as_mut().unwrap().pop();
        let partial = qualify_core_transport_shadow(
            &scope,
            &partial_counts,
            &partial_counts,
            &partial_counts,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .unwrap();
        assert_eq!(partial.verdict, TransportParityVerdict::Inconclusive);
        assert_eq!(
            partial.source_stability,
            SourceStabilityEvidence::EvidenceUnavailable
        );
    }

    #[test]
    fn observation_receipt_does_not_serialize_company_or_record_values() {
        let scope = scope();
        let reference = entry_window(&scope, "xml-private-entry", "-1.00", 'a');
        let candidate = entry_window(&scope, "json-private-entry", "-1.0", 'b');
        let observation = qualify_core_transport_shadow(
            &scope,
            &reference,
            &candidate,
            &reference,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .unwrap();
        let json = serde_json::to_string(&observation).unwrap();
        for private in [
            "synthetic-private-company-guid",
            "xml-private-entry",
            "json-private-entry",
        ] {
            assert!(!json.contains(private));
        }
    }

    #[test]
    fn invalid_pair_and_metrics_fail_closed() {
        let valid_scope = scope();
        let reference = entry_window(&valid_scope, "xml-entry", "-1.00", 'a');
        let candidate = entry_window(&valid_scope, "json-entry", "-1.00", 'b');
        let mut invalid_scope = valid_scope.clone();
        invalid_scope.candidate_transport = TransportId::Odbc;
        assert!(matches!(
            qualify_core_transport_shadow(
                &invalid_scope,
                &reference,
                &candidate,
                &reference,
                metrics(10, 20),
                metrics(21, 25),
                metrics(26, 35),
            ),
            Err(TallyError::InvalidData { code }) if code == "transport_qualification_pair_invalid"
        ));
        assert!(matches!(
            qualify_core_transport_shadow(
                &valid_scope,
                &reference,
                &candidate,
                &reference,
                TransportReadMetrics {
                    started_at_unix_ms: 20,
                    completed_at_unix_ms: 10,
                    response_bytes: 128,
                },
                metrics(21, 25),
                metrics(26, 35),
            ),
            Err(TallyError::InvalidData { code }) if code == "transport_qualification_metrics_invalid"
        ));

        assert!(matches!(
            qualify_core_transport_shadow(
                &valid_scope,
                &reference,
                &candidate,
                &reference,
                metrics(10, 30),
                metrics(20, 25),
                metrics(31, 40),
            ),
            Err(TallyError::InvalidData { code }) if code == "transport_qualification_bracket_order_invalid"
        ));

        let mut wrong_schema = valid_scope.clone();
        wrong_schema.pack_schema_version.minor += 1;
        assert!(matches!(
            qualify_core_transport_shadow(
                &wrong_schema,
                &reference,
                &candidate,
                &reference,
                metrics(10, 20),
                metrics(21, 25),
                metrics(26, 35),
            ),
            Err(TallyError::InvalidData { code }) if code == "transport_qualification_schema_version_invalid"
        ));

        let mut invalid_scope = valid_scope;
        invalid_scope.window.to_yyyymmdd = "20260230".to_string();
        assert!(qualify_core_transport_shadow(
            &invalid_scope,
            &reference,
            &candidate,
            &reference,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        )
        .is_err());
    }
}
