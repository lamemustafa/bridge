//! Portable, deterministic conversion from strict Tally export records to Bridge canonical packs.
//!
//! This crate deliberately has no HTTP, database, OpenSSL, or Tauri dependency so the complete
//! identity and reference-binding boundary remains executable on every supported development host.

use bridge_tally_core::{
    source_count_scope_fingerprint, CanonicalPackWindow, CanonicalText, CoreAccountingBatch,
    ExactDecimal, GroupRecord, LedgerEntryPolarity, LedgerEntryRecord, LedgerRecord,
    ObservedSourceIdentities, PackBatch, RawSourceSha256, RequestContext, SourceAlterId,
    SourceCountScope, SourceCountScopeDescriptor, SourceIdentityKind, SourceRecordEvidence,
    SourceRecordId, SourceReportedCountEvidence, TallyDate, TallyError, VoucherRecord,
    VoucherTypeRecord,
};
use bridge_tally_protocol::{
    ParsedExport, ParsedSourceIdentityKind, ParsedSourceRecord, TallyLedger, TallyNamedMaster,
    TallyVoucher,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Converts the four exact core-accounting exports into one reference-complete canonical window.
/// Any missing/ambiguous identity, mutable-name collision, or unresolved relationship fails closed.
pub fn build_core_window(
    context: &RequestContext,
    groups: ParsedExport<ParsedSourceRecord<TallyNamedMaster>>,
    ledgers: ParsedExport<ParsedSourceRecord<TallyLedger>>,
    voucher_types: ParsedExport<ParsedSourceRecord<TallyNamedMaster>>,
    vouchers: ParsedExport<ParsedSourceRecord<TallyVoucher>>,
) -> Result<CanonicalPackWindow, TallyError> {
    let requested_from = TallyDate::parse(context.window.from_yyyymmdd.clone())
        .map_err(|_| invalid_data("requested_window_invalid"))?;
    let requested_to = TallyDate::parse(context.window.to_yyyymmdd.clone())
        .map_err(|_| invalid_data("requested_window_invalid"))?;
    if requested_from.as_str() > requested_to.as_str() {
        return Err(invalid_data("requested_window_invalid"));
    }
    let group_count = required_source_count(&groups, "group_source_count_missing")?;
    let ledger_count = required_source_count(&ledgers, "ledger_source_count_missing")?;
    let voucher_type_count =
        required_source_count(&voucher_types, "voucher_type_source_count_missing")?;
    let voucher_count = required_source_count(&vouchers, "voucher_source_count_missing")?;
    validate_selected_voucher_window(
        context.window.from_yyyymmdd.as_str(),
        context.window.to_yyyymmdd.as_str(),
        &vouchers,
    )?;
    let mut batch = CoreAccountingBatch::default();
    let mut record_evidence = Vec::new();

    let group_ids_by_name = unique_source_ids_by_name(
        &groups.records,
        |record| &record.name,
        "group_identity_missing",
        "group_name_missing",
        "group_name_duplicate",
    )?;
    for source in groups.records {
        let source_id = required_source_id(&source, "group_identity_missing")?;
        let evidence = source_evidence("group", source_id.clone(), &source)?;
        let name = required_text(&source.record.name, "group_name_missing")?;
        let parent_source_id = resolve_group_parent(
            source.record.parent.as_deref(),
            &group_ids_by_name,
            "group_parent_missing",
        )?;
        batch.groups.push(GroupRecord {
            source_id,
            name,
            parent_source_id,
        });
        record_evidence.push(evidence);
    }

    let ledger_ids_by_name = unique_source_ids_by_name(
        &ledgers.records,
        |record| &record.name,
        "ledger_identity_missing",
        "ledger_name_missing",
        "ledger_name_duplicate",
    )?;
    for source in ledgers.records {
        let source_id = required_source_id(&source, "ledger_identity_missing")?;
        let evidence = source_evidence("ledger", source_id.clone(), &source)?;
        let name = required_text(&source.record.name, "ledger_name_missing")?;
        let parent_source_id = resolve_optional_reference(
            source.record.parent.as_deref(),
            &group_ids_by_name,
            "ledger_parent_group_missing",
        )?;
        let opening_balance = source
            .record
            .opening_balance
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| ExactDecimal::parse(value.to_string()))
            .transpose()?;
        batch.ledgers.push(LedgerRecord {
            source_id,
            name,
            parent_source_id,
            opening_balance,
        });
        record_evidence.push(evidence);
    }

    let voucher_type_ids_by_name = unique_source_ids_by_name(
        &voucher_types.records,
        |record| &record.name,
        "voucher_type_identity_missing",
        "voucher_type_name_missing",
        "voucher_type_name_duplicate",
    )?;
    for source in voucher_types.records {
        let source_id = required_source_id(&source, "voucher_type_identity_missing")?;
        let evidence = source_evidence("voucher_type", source_id.clone(), &source)?;
        let name = required_text(&source.record.name, "voucher_type_name_missing")?;
        batch
            .voucher_types
            .push(VoucherTypeRecord { source_id, name });
        record_evidence.push(evidence);
    }

    let mut ledger_entry_count = 0_u64;
    for source in vouchers.records {
        let voucher_source_id = required_source_id(&source, "voucher_identity_missing")?;
        let voucher_evidence = source_evidence("voucher", voucher_source_id.clone(), &source)?;
        let voucher_type_name = source
            .record
            .voucher_type
            .as_deref()
            .ok_or_else(|| invalid_data("voucher_type_missing"))?;
        let voucher_type_source_id = resolve_required_reference(
            voucher_type_name,
            &voucher_type_ids_by_name,
            "voucher_type_reference_missing",
        )?;
        let date_yyyymmdd = required_text(
            source
                .record
                .date
                .as_deref()
                .ok_or_else(|| invalid_data("voucher_date_missing"))?,
            "voucher_date_missing",
        )?;
        let voucher_date = TallyDate::parse(date_yyyymmdd.clone())
            .map_err(|_| invalid_data("voucher_date_invalid"))?;
        if voucher_date.as_str() < requested_from.as_str()
            || voucher_date.as_str() > requested_to.as_str()
        {
            return Err(invalid_data("voucher_date_outside_requested_window"));
        }
        let voucher_number = source
            .record
            .voucher_number
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| required_text(value, "voucher_number_invalid"))
            .transpose()?;
        let cancelled = source
            .record
            .cancelled
            .ok_or_else(|| invalid_data("voucher_cancelled_missing"))?;
        let optional = source
            .record
            .optional
            .ok_or_else(|| invalid_data("voucher_optional_missing"))?;

        for entry in &source.record.ledger_entries {
            let ledger_source_id = resolve_required_reference(
                &entry.ledger_name,
                &ledger_ids_by_name,
                "voucher_ledger_reference_missing",
            )?;
            let entry_source_id = derived_ledger_entry_id(
                &context.company.identity.company_guid,
                &source,
                entry.entry_index,
                &entry.raw_source_sha256,
            )?;
            batch.ledger_entries.push(LedgerEntryRecord {
                source_id: entry_source_id.clone(),
                voucher_source_id: voucher_source_id.clone(),
                ledger_source_id,
                amount: ExactDecimal::parse(entry.amount.clone())?,
                polarity: if entry.is_deemed_positive {
                    LedgerEntryPolarity::Debit
                } else {
                    LedgerEntryPolarity::Credit
                },
            });
            record_evidence.push(SourceRecordEvidence {
                object_type: CanonicalText::parse("ledger_entry")?,
                source_id: SourceRecordId::parse(entry_source_id)?,
                identity_kind: SourceIdentityKind::Fallback,
                observed_identities: ObservedSourceIdentities::default(),
                // Hash of the exact decoded XML row fragment, not the HTTP transport bytes.
                raw_source_sha256: RawSourceSha256::parse(entry.raw_source_sha256.clone())?,
                alter_id: None,
            });
            ledger_entry_count = ledger_entry_count
                .checked_add(1)
                .ok_or_else(|| invalid_data("ledger_entry_count_overflow"))?;
        }

        batch.vouchers.push(VoucherRecord {
            source_id: voucher_source_id,
            date_yyyymmdd,
            voucher_type_source_id,
            voucher_number,
            cancelled,
            optional,
        });
        record_evidence.push(voucher_evidence);
    }

    let source_counts = vec![
        count_evidence(context, "group", group_count, SourceCountScope::Complete)?,
        count_evidence(context, "ledger", ledger_count, SourceCountScope::Complete)?,
        count_evidence(
            context,
            "voucher_type",
            voucher_type_count,
            SourceCountScope::Complete,
        )?,
        count_evidence(context, "voucher", voucher_count, SourceCountScope::Window)?,
        count_evidence(
            context,
            "ledger_entry",
            ledger_entry_count,
            SourceCountScope::Window,
        )?,
    ];
    let window = CanonicalPackWindow {
        batch: PackBatch::CoreAccounting(batch),
        source_counts: Some(source_counts),
        record_evidence: Some(record_evidence),
    };
    window.validate_source_count_evidence()?;
    window.validate_record_evidence_binding()?;
    Ok(window)
}

/// Validates the exact selected voucher profile without canonicalising or retaining book data.
/// A successful zero-row response proves only execution of the selected profile, not emptiness or
/// source completeness.
pub fn validate_selected_voucher_window(
    from_yyyymmdd: &str,
    to_yyyymmdd: &str,
    vouchers: &ParsedExport<ParsedSourceRecord<TallyVoucher>>,
) -> Result<(), TallyError> {
    let requested_from = TallyDate::parse(from_yyyymmdd.to_string())
        .map_err(|_| invalid_data("requested_window_invalid"))?;
    let requested_to = TallyDate::parse(to_yyyymmdd.to_string())
        .map_err(|_| invalid_data("requested_window_invalid"))?;
    if requested_from.as_str() > requested_to.as_str() {
        return Err(invalid_data("requested_window_invalid"));
    }
    for source in &vouchers.records {
        let source_id = required_source_id(source, "voucher_identity_missing")?;
        source_evidence("voucher", source_id, source)?;
        required_text(
            source
                .record
                .voucher_type
                .as_deref()
                .ok_or_else(|| invalid_data("voucher_type_missing"))?,
            "voucher_type_missing",
        )?;
        let date = required_text(
            source
                .record
                .date
                .as_deref()
                .ok_or_else(|| invalid_data("voucher_date_missing"))?,
            "voucher_date_missing",
        )?;
        let voucher_date =
            TallyDate::parse(date).map_err(|_| invalid_data("voucher_date_invalid"))?;
        if voucher_date.as_str() < requested_from.as_str()
            || voucher_date.as_str() > requested_to.as_str()
        {
            return Err(invalid_data("voucher_date_outside_requested_window"));
        }
        source
            .record
            .cancelled
            .ok_or_else(|| invalid_data("voucher_cancelled_missing"))?;
        source
            .record
            .optional
            .ok_or_else(|| invalid_data("voucher_optional_missing"))?;
        source
            .record
            .voucher_number
            .as_deref()
            .map(|value| required_text(value, "voucher_number_invalid"))
            .transpose()?;
        source
            .record
            .party_ledger_name
            .as_deref()
            .map(|value| required_text(value, "voucher_party_ledger_name_invalid"))
            .transpose()?;
        let declared_entries = source
            .record
            .ledger_entry_count
            .ok_or_else(|| invalid_data("voucher_ledger_entry_count_missing"))?;
        if declared_entries != source.record.ledger_entries.len() as u64 {
            return Err(invalid_data("voucher_ledger_entry_count_mismatch"));
        }
        let mut entry_indices = std::collections::BTreeSet::new();
        for entry in &source.record.ledger_entries {
            if entry.entry_index == 0 || !entry_indices.insert(entry.entry_index) {
                return Err(invalid_data("voucher_ledger_entry_index_invalid"));
            }
            required_text(&entry.ledger_name, "voucher_ledger_name_invalid")?;
            ExactDecimal::parse(entry.amount.clone())?;
            RawSourceSha256::parse(entry.raw_source_sha256.clone())?;
        }
    }
    Ok(())
}

fn required_source_count<T>(
    export: &ParsedExport<T>,
    code: &'static str,
) -> Result<u64, TallyError> {
    export
        .evidence
        .source_record_count
        .ok_or_else(|| protocol_error(code))
}

fn unique_source_ids_by_name<T, F>(
    records: &[ParsedSourceRecord<T>],
    name: F,
    missing_identity_code: &'static str,
    invalid_name_code: &'static str,
    duplicate_name_code: &'static str,
) -> Result<BTreeMap<String, String>, TallyError>
where
    F: Fn(&T) -> &str,
{
    let mut ids = BTreeMap::new();
    for source in records {
        let source_id = required_source_id(source, missing_identity_code)?;
        let canonical_name = required_text(name(&source.record), invalid_name_code)?;
        if ids.insert(canonical_name, source_id).is_some() {
            return Err(invalid_data(duplicate_name_code));
        }
    }
    Ok(ids)
}

fn resolve_optional_reference(
    value: Option<&str>,
    ids_by_name: &BTreeMap<String, String>,
    missing_code: &'static str,
) -> Result<Option<String>, TallyError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| resolve_required_reference(value, ids_by_name, missing_code))
        .transpose()
}

fn resolve_group_parent(
    value: Option<&str>,
    ids_by_name: &BTreeMap<String, String>,
    missing_code: &'static str,
) -> Result<Option<String>, TallyError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    // `Primary` is Tally's reserved top-level classification, not one of the exported Group
    // masters. Preserve the canonical tree root as `None`; every other named parent must resolve.
    if value.trim().eq_ignore_ascii_case("primary") {
        return Ok(None);
    }
    resolve_required_reference(value, ids_by_name, missing_code).map(Some)
}

fn resolve_required_reference(
    value: &str,
    ids_by_name: &BTreeMap<String, String>,
    missing_code: &'static str,
) -> Result<String, TallyError> {
    let name = required_text(value, missing_code)?;
    ids_by_name
        .get(&name)
        .cloned()
        .ok_or_else(|| invalid_data(missing_code))
}

fn count_evidence(
    context: &RequestContext,
    object_type: &str,
    count: u64,
    scope: SourceCountScope,
) -> Result<SourceReportedCountEvidence, TallyError> {
    let object_type = CanonicalText::parse(object_type)?;
    let descriptor = SourceCountScopeDescriptor {
        source_identity: context.company.identity.clone(),
        pack: context.pack,
        pack_schema_version: context.schema_version,
        object_type: object_type.clone(),
        query_profile: context.query_profile.clone(),
        filters_sha256: context.filters_sha256.clone(),
        window: (scope == SourceCountScope::Window).then(|| context.window.clone()),
    };
    Ok(SourceReportedCountEvidence {
        object_type,
        query_profile: context.query_profile.clone(),
        source_scope_fingerprint: source_count_scope_fingerprint(&descriptor, scope)?,
        source_count_scope: scope,
        source_reported_count: count,
    })
}

fn source_evidence<T>(
    object_type: &str,
    source_id: String,
    source: &ParsedSourceRecord<T>,
) -> Result<SourceRecordEvidence, TallyError> {
    let identity_kind = match source.identity_kind {
        Some(ParsedSourceIdentityKind::Guid) => SourceIdentityKind::Guid,
        Some(ParsedSourceIdentityKind::RemoteId) => SourceIdentityKind::RemoteId,
        Some(ParsedSourceIdentityKind::MasterId) => SourceIdentityKind::MasterId,
        None => return Err(invalid_data("source_identity_kind_missing")),
    };
    Ok(SourceRecordEvidence {
        object_type: CanonicalText::parse(object_type)?,
        source_id: SourceRecordId::parse(source_id)?,
        identity_kind,
        observed_identities: ObservedSourceIdentities {
            guid: source
                .identities
                .guid
                .clone()
                .map(SourceRecordId::parse)
                .transpose()?,
            remote_id: source
                .identities
                .remote_id
                .clone()
                .map(SourceRecordId::parse)
                .transpose()?,
            master_id: source
                .identities
                .master_id
                .clone()
                .map(SourceRecordId::parse)
                .transpose()?,
        },
        raw_source_sha256: RawSourceSha256::parse(source.raw_source_sha256.clone())?,
        alter_id: source
            .alter_id
            .clone()
            .map(SourceAlterId::parse)
            .transpose()?,
    })
}

fn required_source_id<T>(
    source: &ParsedSourceRecord<T>,
    code: &'static str,
) -> Result<String, TallyError> {
    source
        .source_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_data(code))
}

fn required_text(value: &str, code: &'static str) -> Result<String, TallyError> {
    CanonicalText::parse(value.to_string())
        .map(|value| value.as_str().to_string())
        .map_err(|_| invalid_data(code))
}

fn derived_ledger_entry_id(
    company_guid: &str,
    voucher: &ParsedSourceRecord<TallyVoucher>,
    entry_index: u64,
    entry_fragment_sha256: &str,
) -> Result<String, TallyError> {
    let identity_kind = voucher
        .identity_kind
        .ok_or_else(|| invalid_data("voucher_identity_kind_missing"))?;
    let source_id = required_source_id(voucher, "voucher_identity_missing")?;
    RawSourceSha256::parse(entry_fragment_sha256.to_string())?;

    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-ledger-entry-derived-id-v1\0");
    hash_field(&mut digest, company_guid.as_bytes());
    hash_field(&mut digest, parsed_identity_kind_code(identity_kind));
    hash_field(&mut digest, source_id.as_bytes());
    hash_field(&mut digest, &entry_index.to_be_bytes());
    hash_field(&mut digest, entry_fragment_sha256.as_bytes());
    Ok(format!(
        "bridge-derived:ledger-entry:v1:{}",
        hex_lower(&digest.finalize())
    ))
}

fn parsed_identity_kind_code(kind: ParsedSourceIdentityKind) -> &'static [u8] {
    match kind {
        ParsedSourceIdentityKind::Guid => b"guid",
        ParsedSourceIdentityKind::RemoteId => b"remote_id",
        ParsedSourceIdentityKind::MasterId => b"master_id",
    }
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
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
