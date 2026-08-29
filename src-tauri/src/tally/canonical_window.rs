//! Deterministic conversion from strict Tally export records to Bridge canonical packs.
//!
//! Folded from the former standalone `bridge-tally-canonical` crate: it had exactly one consumer
//! inside this crate (`tally::connector` and `tally::connection`), so the crate boundary earned
//! nothing except hiding this module's true dead code from the `dead_code` lint. This module
//! deliberately still has no HTTP, database, OpenSSL, or Tauri dependency, so the complete
//! identity and reference-binding boundary remains executable on every supported development host.

use bridge_tally_core::{
    source_count_scope_fingerprint, CanonicalPackWindow, CanonicalText, CoreAccountingBatch,
    ExactDecimal, ForeignMasterTextDiagnostic, ForeignText, GroupRecord, LedgerEntryPolarity,
    LedgerEntryRecord, LedgerRecord, ObservedSourceIdentities, PackBatch, RawSourceSha256,
    RequestContext, SourceAlterId, SourceCountScope, SourceCountScopeDescriptor,
    SourceIdentityKind, SourceRecordEvidence, SourceRecordId, SourceReportedCountEvidence,
    TallyDate, TallyError, VoucherRecord, VoucherTypeRecord,
};
use bridge_tally_protocol::{
    is_tally_reserved_root, ParsedExport, ParsedSourceIdentityKind, ParsedSourceRecord,
    TallyLedger, TallyNamedMaster, TallyVoucher,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Converts the four exact core-accounting exports into one reference-complete canonical window.
/// Any missing/ambiguous identity, mutable-name collision, or unresolved relationship fails closed.
pub(super) fn build_core_window(
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
    let _group_count = required_observed_or_source_count(&groups, "group_record_count_missing")?;
    let _ledger_count = required_observed_or_source_count(&ledgers, "ledger_record_count_missing")?;
    let _voucher_type_count =
        required_observed_or_source_count(&voucher_types, "voucher_type_record_count_missing")?;
    let _voucher_count =
        required_observed_or_source_count(&vouchers, "voucher_record_count_missing")?;
    let source_record_counts = [
        groups.evidence.source_record_count,
        ledgers.evidence.source_record_count,
        voucher_types.evidence.source_record_count,
        vouchers.evidence.source_record_count,
    ];
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
        let diagnostic_source_id = source_id.clone();
        let evidence = source_evidence("group", source_id.clone(), &source)?;
        let name = required_foreign_text(&source.record.name, "group_name_missing")?;
        let parent_source_id = resolve_group_parent(
            source.record.parent.nonempty_returned_text(),
            &group_ids_by_name,
            "group_parent_missing",
        )?;
        batch.groups.push(GroupRecord {
            source_id,
            name: name.clone().into_string(),
            parent_source_id,
        });
        record_foreign_master_text_diagnostic(&mut batch, "group", &diagnostic_source_id, &name);
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
        let diagnostic_source_id = source_id.clone();
        let evidence = source_evidence("ledger", source_id.clone(), &source)?;
        let name = required_foreign_text(&source.record.name, "ledger_name_missing")?;
        let parent_source_id = resolve_optional_reference(
            source.record.parent.nonempty_returned_text(),
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
            name: name.clone().into_string(),
            parent_source_id,
            opening_balance,
        });
        record_foreign_master_text_diagnostic(&mut batch, "ledger", &diagnostic_source_id, &name);
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
        let diagnostic_source_id = source_id.clone();
        let evidence = source_evidence("voucher_type", source_id.clone(), &source)?;
        let name = required_foreign_text(&source.record.name, "voucher_type_name_missing")?;
        batch.voucher_types.push(VoucherTypeRecord {
            source_id,
            name: name.clone().into_string(),
        });
        record_foreign_master_text_diagnostic(
            &mut batch,
            "voucher_type",
            &diagnostic_source_id,
            &name,
        );
        record_evidence.push(evidence);
    }

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
        let date_yyyymmdd = source
            .record
            .date
            .as_deref()
            .ok_or_else(|| invalid_data("voucher_date_missing"))?
            .to_string();
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
            .map(|value| {
                required_foreign_text(value, "voucher_number_invalid").map(ForeignText::into_string)
            })
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

    // Native collections do not carry an independent count from Tally.  Their
    // parsed row count validates the parser only; it must not be mislabeled as
    // source-reported completeness evidence. A qualification consequently
    // remains incomplete until an independent witness is deliberately added.
    let source_counts = source_record_counts
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map(|counts| {
            Ok(vec![
                count_evidence(context, "group", counts[0], SourceCountScope::Complete)?,
                count_evidence(context, "ledger", counts[1], SourceCountScope::Complete)?,
                count_evidence(
                    context,
                    "voucher_type",
                    counts[2],
                    SourceCountScope::Complete,
                )?,
                count_evidence(context, "voucher", counts[3], SourceCountScope::Window)?,
            ])
        })
        .transpose()?;
    let window = CanonicalPackWindow {
        batch: PackBatch::CoreAccounting(batch),
        source_counts,
        record_evidence: Some(record_evidence),
    };
    window.validate_source_count_evidence()?;
    window.validate_record_evidence_binding()?;
    Ok(window)
}

/// Validates the exact selected voucher profile without canonicalising or retaining book data.
/// A successful zero-row response proves only execution of the selected profile, not emptiness or
/// source completeness.
pub(super) fn validate_selected_voucher_window(
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
        required_foreign_text(
            source
                .record
                .voucher_type
                .as_deref()
                .ok_or_else(|| invalid_data("voucher_type_missing"))?,
            "voucher_type_missing",
        )?;
        let date = source
            .record
            .date
            .as_deref()
            .ok_or_else(|| invalid_data("voucher_date_missing"))?
            .to_string();
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
            .map(|value| required_foreign_text(value, "voucher_number_invalid"))
            .transpose()?;
        source
            .record
            .party_ledger_name
            .as_deref()
            .map(|value| required_foreign_text(value, "voucher_party_ledger_name_invalid"))
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
            required_foreign_text(&entry.ledger_name, "voucher_ledger_name_invalid")?;
            ExactDecimal::parse(entry.amount.clone())?;
            RawSourceSha256::parse(entry.raw_source_sha256.clone())?;
        }
    }
    Ok(())
}

fn required_observed_or_source_count<T>(
    export: &ParsedExport<T>,
    code: &'static str,
) -> Result<u64, TallyError> {
    export
        .evidence
        .observed_record_count
        .or(export.evidence.source_record_count)
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
        let foreign_name = required_foreign_text(name(&source.record), invalid_name_code)?;
        if ids.insert(foreign_name.into_string(), source_id).is_some() {
            return Err(invalid_data(duplicate_name_code));
        }
    }
    Ok(ids)
}

pub(super) fn resolve_optional_reference(
    value: Option<&str>,
    ids_by_name: &BTreeMap<String, String>,
    missing_code: &'static str,
) -> Result<Option<String>, TallyError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    if is_tally_reserved_root(value) {
        return Ok(None);
    }
    resolve_required_reference(value, ids_by_name, missing_code).map(Some)
}

pub(super) fn resolve_group_parent(
    value: Option<&str>,
    ids_by_name: &BTreeMap<String, String>,
    missing_code: &'static str,
) -> Result<Option<String>, TallyError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    if is_tally_reserved_root(value) {
        return Ok(None);
    }
    resolve_required_reference(value, ids_by_name, missing_code).map(Some)
}

fn resolve_required_reference(
    value: &str,
    ids_by_name: &BTreeMap<String, String>,
    missing_code: &'static str,
) -> Result<String, TallyError> {
    let name = required_foreign_text(value, missing_code)?.into_string();
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
        Some(ParsedSourceIdentityKind::Fallback) => SourceIdentityKind::Fallback,
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

fn required_foreign_text(value: &str, code: &'static str) -> Result<ForeignText, TallyError> {
    if value.is_empty() {
        return Err(invalid_data(code));
    }
    Ok(ForeignText::from_tally(value.to_string()))
}

fn record_foreign_master_text_diagnostic(
    batch: &mut CoreAccountingBatch,
    object_type: &str,
    source_id: &str,
    name: &ForeignText,
) {
    let Some(diagnostic) = name.document_rendering_diagnostic() else {
        return;
    };
    batch
        .foreign_master_text_diagnostics
        .push(ForeignMasterTextDiagnostic {
            object_type: object_type.to_string(),
            source_id: source_id.to_string(),
            stored_name: name.as_str().to_string(),
            likely_intended_spelling: diagnostic.likely_intended_spelling,
        });
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
        ParsedSourceIdentityKind::Fallback => b"fallback",
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

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_tally_core::{
        CanonicalPackWindow, CanonicalText, CapabilityPackId, CompanyRef, LedgerEntryPolarity,
        ObservedSourceIdentities, PackBatch, PackSchemaVersion, ReadWindow, RequestContext,
        SourceIdentity, SourceIdentityKind, TallyError,
    };
    use bridge_tally_protocol::{
        decode_tally_xml_response_bytes_limited, parse_group_source_records_with_evidence,
        parse_ledger_source_records_with_evidence, parse_native_group_source_records_with_evidence,
        parse_native_ledger_source_records_with_evidence,
        parse_native_voucher_source_records_with_evidence,
        parse_native_voucher_type_source_records_with_evidence,
        parse_voucher_source_records_with_evidence,
        parse_voucher_type_source_records_with_evidence, ExpectedTallyTextEncoding, ParsedExport,
        ParsedSourceRecord, TallyLedger, TallyNamedMaster, TallyVoucher,
        BRIDGE_GROUP_EXPORT_SCHEMA, BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_VOUCHER_EXPORT_SCHEMA,
        BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA, TALLY_SANITIZED_ROOT_MARKER,
    };

    fn context() -> RequestContext {
        RequestContext {
            run_id: "synthetic-run".to_string(),
            company: CompanyRef {
                identity: SourceIdentity {
                    bridge_source_lineage: "synthetic-lineage".to_string(),
                    company_guid: "synthetic-company-guid".to_string(),
                    observed_fingerprint: "synthetic-observation".to_string(),
                },
                display_name: "BRIDGE SYNTHETIC BOOK".to_string(),
            },
            pack: CapabilityPackId::CoreAccounting,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            window: ReadWindow {
                from_yyyymmdd: "20260701".to_string(),
                to_yyyymmdd: "20260731".to_string(),
            },
            query_profile: CanonicalText::parse("core_accounting_v1").unwrap(),
            filters_sha256: CanonicalText::parse("0".repeat(64)).unwrap(),
        }
    }

    fn groups() -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
        parse_group_source_records_with_evidence(&format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="GROUP" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="1"/><GROUP NAME="Assets" GUID="group-guid" MASTERID="1" ALTERID="5"><PARENT>Primary</PARENT></GROUP></BODY></ENVELOPE>"#,
            BRIDGE_GROUP_EXPORT_SCHEMA
        ))
        .unwrap()
    }

    fn ledgers_and_vouchers(
        cash_name: &str,
        entry_ledger_name: &str,
    ) -> (
        ParsedExport<ParsedSourceRecord<TallyLedger>>,
        ParsedExport<ParsedSourceRecord<TallyVoucher>>,
    ) {
        let ledgers = parse_ledger_source_records_with_evidence(&format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="2"/><LEDGER NAME="{}" GUID="ledger-cash" REMOTEID="cash-remote" MASTERID="2" ALTERID="6"><PARENT>Assets</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER><LEDGER NAME="Sales" GUID="ledger-sales" MASTERID="3" ALTERID="7"><PARENT>Assets</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></BODY></ENVELOPE>"#,
            BRIDGE_LEDGER_EXPORT_SCHEMA, cash_name
        ))
        .unwrap();
        let vouchers = parse_voucher_source_records_with_evidence(&format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHER" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="1"/><VOUCHER GUID="voucher-guid" REMOTEID="voucher-remote" MASTERID="9" ALTERID="10"><DATE>20260714</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><VOUCHERNUMBER>SYN-1</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><LEDGERENTRYCOUNT>2</LEDGERENTRYCOUNT><LEDGERENTRIES><LEDGERENTRY><ENTRYINDEX>1</ENTRYINDEX><LEDGERNAME>{}</LEDGERNAME><AMOUNT>-100.00</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE></LEDGERENTRY><LEDGERENTRY><ENTRYINDEX>2</ENTRYINDEX><LEDGERNAME>Sales</LEDGERNAME><AMOUNT>100.00</AMOUNT><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE></LEDGERENTRY></LEDGERENTRIES></VOUCHER></BODY></ENVELOPE>"#,
            BRIDGE_VOUCHER_EXPORT_SCHEMA, entry_ledger_name
        ))
        .unwrap();
        (ledgers, vouchers)
    }

    fn voucher_types() -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
        parse_voucher_type_source_records_with_evidence(&format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHERTYPE" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="1"/><VOUCHERTYPE NAME="Receipt" GUID="voucher-type-guid" MASTERID="8" ALTERID="9"><PARENT>Receipt</PARENT></VOUCHERTYPE></BODY></ENVELOPE>"#,
            BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA
        ))
        .unwrap()
    }

    fn valid_window() -> CanonicalPackWindow {
        let (ledgers, vouchers) = ledgers_and_vouchers("Cash", "Cash");
        build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers).unwrap()
    }

    #[test]
    fn marker_carrying_parent_policy_fails_closed_for_unobserved_non_root_references() {
        let group_ids_by_name = BTreeMap::from([("Assets".to_string(), "group-guid".to_string())]);

        for value in [
            format!("{TALLY_SANITIZED_ROOT_MARKER} Primary"),
            "Primary".to_string(),
        ] {
            assert_eq!(
                resolve_group_parent(Some(&value), &group_ids_by_name, "group_parent_missing")
                    .unwrap(),
                None
            );
            assert_eq!(
                resolve_optional_reference(
                    Some(&value),
                    &group_ids_by_name,
                    "ledger_parent_group_missing"
                )
                .unwrap(),
                None
            );
        }

        // This pins Bridge's policy for an input Tally has not been observed to emit; it is not
        // evidence about Tally behaviour. Measured: the marker occurs on 30 group parents and
        // one ledger parent across two companies, always followed by `Primary`; Bridge's
        // `TALLY_PROTOCOL_REFERENCE.md:66-76` records it on `OBJECTUPDATEACTION` as
        // `&#4; Resave`. The policy fails closed: the old starts-with rule silently turned an
        // unrecognised marker-prefixed value into a `None` parent, whereas this rule surfaces it
        // as a missing reference.
        for value in [
            format!("{TALLY_SANITIZED_ROOT_MARKER} Resave"),
            format!("{TALLY_SANITIZED_ROOT_MARKER} Anything"),
            format!("{TALLY_SANITIZED_ROOT_MARKER}{TALLY_SANITIZED_ROOT_MARKER} Primary"),
        ] {
            assert!(matches!(
                resolve_group_parent(Some(&value), &group_ids_by_name, "group_parent_missing"),
                Err(TallyError::InvalidData { code }) if code == "group_parent_missing"
            ));
            assert!(matches!(
                resolve_optional_reference(
                    Some(&value),
                    &group_ids_by_name,
                    "ledger_parent_group_missing"
                ),
                Err(TallyError::InvalidData { code }) if code == "ledger_parent_group_missing"
            ));
        }
    }

    #[test]
    fn marker_prefixed_real_master_name_resolves_by_its_raw_name() {
        let marked_name = format!("{TALLY_SANITIZED_ROOT_MARKER} Resave");
        let groups = padded_native_two_group_export(
            &marked_name,
            "Primary",
            "Synthetic Child Group",
            &marked_name,
        );
        let ledgers = padded_native_ledger_export("Synthetic Ledger", "Synthetic Child Group");
        let voucher_types = padded_native_voucher_type_export("Synthetic Receipt");
        let vouchers = padded_native_voucher_export("Synthetic Receipt", "Synthetic Ledger");

        let window = build_core_window(&context(), groups, ledgers, voucher_types, vouchers)
            .expect("a real marker-prefixed master name must resolve by its exact raw text");
        let PackBatch::CoreAccounting(batch) = window.batch else {
            panic!("wrong pack");
        };
        let child = batch
            .groups
            .iter()
            .find(|group| group.name == "Synthetic Child Group")
            .expect("child group is present");

        assert_eq!(
            child.parent_source_id.as_deref(),
            Some(format!("{PADDED_COMPANY_GUID}-00000001").as_str())
        );
    }

    #[test]
    fn canonicalizes_all_core_records_with_exact_reference_and_provenance_binding() {
        let window = valid_window();
        window.validate_record_evidence_binding().unwrap();
        let PackBatch::CoreAccounting(batch) = &window.batch else {
            panic!("wrong pack")
        };
        assert_eq!(
            (
                batch.groups.len(),
                batch.ledgers.len(),
                batch.voucher_types.len()
            ),
            (1, 2, 1)
        );
        assert_eq!((batch.vouchers.len(), batch.ledger_entries.len()), (1, 2));
        assert_eq!(
            batch.ledgers[0].parent_source_id.as_deref(),
            Some("group-guid")
        );
        assert_eq!(batch.groups[0].parent_source_id, None);
        assert_eq!(
            batch.vouchers[0].voucher_type_source_id,
            "voucher-type-guid"
        );
        assert_eq!(batch.ledger_entries[0].ledger_source_id, "ledger-cash");
        assert_eq!(batch.ledger_entries[0].voucher_source_id, "voucher-guid");
        assert_eq!(batch.ledger_entries[0].polarity, LedgerEntryPolarity::Debit);
        assert_eq!(
            batch.ledger_entries[1].polarity,
            LedgerEntryPolarity::Credit
        );
        assert!(batch.ledger_entries[0]
            .source_id
            .starts_with("bridge-derived:ledger-entry:v1:"));
        assert_eq!(window.source_counts.as_ref().unwrap().len(), 4);
        assert!(window
            .source_counts
            .as_ref()
            .unwrap()
            .iter()
            .all(|evidence| evidence.object_type.as_str() != "ledger_entry"));
        assert_eq!(window.record_evidence.as_ref().unwrap().len(), 7);

        let voucher_evidence = window
            .record_evidence
            .as_ref()
            .unwrap()
            .iter()
            .find(|evidence| evidence.object_type.as_str() == "voucher")
            .unwrap();
        assert_eq!(voucher_evidence.identity_kind, SourceIdentityKind::Guid);
        assert_eq!(
            voucher_evidence
                .observed_identities
                .remote_id
                .as_ref()
                .unwrap()
                .as_str(),
            "voucher-remote"
        );
        assert_eq!(
            voucher_evidence
                .observed_identities
                .master_id
                .as_ref()
                .unwrap()
                .as_str(),
            "9"
        );
    }

    #[test]
    fn nested_entry_totals_remain_local_and_are_never_claimed_as_source_reported() {
        let window = valid_window();
        let PackBatch::CoreAccounting(batch) = &window.batch else {
            panic!("wrong pack")
        };

        assert_eq!(batch.ledger_entries.len(), 2);
        assert_eq!(
            window
                .record_evidence
                .as_ref()
                .unwrap()
                .iter()
                .filter(|evidence| evidence.object_type.as_str() == "ledger_entry")
                .count(),
            2
        );
        assert!(window
            .source_counts
            .as_ref()
            .unwrap()
            .iter()
            .all(|evidence| evidence.object_type.as_str() != "ledger_entry"));
    }

    #[test]
    fn derived_entry_ids_are_deterministic_but_never_claim_native_identity() {
        fn entry_ids(window: &CanonicalPackWindow) -> Vec<String> {
            let PackBatch::CoreAccounting(batch) = &window.batch else {
                panic!("wrong pack")
            };
            batch
                .ledger_entries
                .iter()
                .map(|entry| entry.source_id.clone())
                .collect()
        }
        let first = valid_window();
        let second = valid_window();
        assert_eq!(entry_ids(&first), entry_ids(&second));
        let entry_evidence = first
            .record_evidence
            .as_ref()
            .unwrap()
            .iter()
            .filter(|evidence| evidence.object_type.as_str() == "ledger_entry")
            .collect::<Vec<_>>();
        assert_eq!(entry_evidence.len(), 2);
        assert!(entry_evidence.iter().all(|evidence| {
            evidence.identity_kind == SourceIdentityKind::Fallback
                && evidence.observed_identities == ObservedSourceIdentities::default()
        }));
    }

    #[test]
    fn unresolved_mutable_name_reference_fails_closed() {
        let (ledgers, vouchers) = ledgers_and_vouchers("Cash", "Missing Ledger");
        let error = build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers)
            .unwrap_err();
        assert!(matches!(
            error,
            TallyError::InvalidData { code }
                if code == "voucher_ledger_reference_missing"
        ));
    }

    #[test]
    fn duplicate_mutable_names_fail_closed_even_when_native_ids_differ() {
        let (ledgers, vouchers) = ledgers_and_vouchers("Sales", "Sales");
        let error = build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers)
            .unwrap_err();
        assert!(matches!(
            error,
            TallyError::InvalidData { code } if code == "ledger_name_duplicate"
        ));
    }

    #[test]
    fn invalid_or_out_of_window_voucher_dates_fail_before_canonical_state_exists() {
        for (date, expected_code) in [
            ("20260230", "voucher_date_invalid"),
            ("20260630", "voucher_date_outside_requested_window"),
            ("20260801", "voucher_date_outside_requested_window"),
        ] {
            let (ledgers, mut vouchers) = ledgers_and_vouchers("Cash", "Cash");
            vouchers.records[0].record.date = Some(date.to_string());
            let error = build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers)
                .unwrap_err();
            assert!(matches!(
                error,
                TallyError::InvalidData { code } if code == expected_code
            ));
        }
    }

    #[test]
    fn captured_svtodate_bound_drop_fails_closed_before_canonicalisation() {
        let bytes = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/response-illegal-svtodate-bound-dropped-wr2.utf16le.xml"
        );
        let xml = decode_tally_xml_response_bytes_limited(
            bytes,
            "text/xml; charset=utf-16",
            ExpectedTallyTextEncoding::Utf16Le,
            bytes.len(),
        )
        .expect("captured UTF-16LE response decodes")
        .text;
        let vouchers = parse_native_voucher_source_records_with_evidence(
            &xml,
            "61c6de69-1748-461c-ad3f-162cb949df9f",
        )
        .expect("captured response has structurally valid native vouchers");

        let error = validate_selected_voucher_window("20260401", "20260430", &vouchers)
            .expect_err("out-of-window response rows must fail closed");
        assert!(matches!(
            error,
            TallyError::InvalidData { code } if code == "voucher_date_outside_requested_window"
        ));
    }

    #[test]
    fn foreign_master_name_with_c1_control_is_retained_and_diagnosed_verbatim() {
        let mojibake = "ZZ Curly âQuotedâ Ledger";
        let (mut ledgers, mut vouchers) = ledgers_and_vouchers(mojibake, mojibake);
        ledgers.records[0].record.name = mojibake.to_string();
        vouchers.records[0].record.ledger_entries[0].ledger_name = mojibake.to_string();

        let window = build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers)
            .expect("foreign text must not reject an otherwise valid company window");
        let PackBatch::CoreAccounting(batch) = window.batch else {
            panic!("wrong pack");
        };
        assert_eq!(batch.ledgers[0].name, mojibake);
        assert_eq!(batch.foreign_master_text_diagnostics.len(), 1);
        assert_eq!(
            batch.foreign_master_text_diagnostics[0]
                .likely_intended_spelling
                .as_deref(),
            Some("ZZ Curly “Quoted” Ledger")
        );
    }

    // Regression coverage for the master-name/reference trim asymmetry: master NAME
    // attributes are stored verbatim (`attr_value` in bridge-tally-protocol/src/lib.rs
    // does not trim), so the canonical lookup maps below are keyed on the untrimmed
    // name. A LEDGERNAME/PARENT/VOUCHERTYPENAME reference that trimmed its own text
    // before the fix could never match a padded master name and would fail the whole
    // company read with `..._reference_missing`. No real capture exhibits this padding
    // (see the safety-check note in the PR description), so these three tests use
    // synthetic XML shaped exactly like the real native captures in
    // `bridge-tally-protocol/tests/fixtures/native/*` (ENVELOPE/HEADER/STATUS,
    // BODY/DATA/COLLECTION, GUID/MASTERID/ALTERID identity, company-guid-prefixed
    // GUIDs) rather than the ad hoc `LEDGERENTRIES`/`LEDGERENTRY` shape the other
    // helpers in this module use for the pre-native legacy parsers.
    const PADDED_COMPANY_GUID: &str = "synthetic-company-guid";

    fn padded_native_group_export(
        name: &str,
        parent_ref: &str,
    ) -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
        let xml = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP NAME="{name}"><GUID>{PADDED_COMPANY_GUID}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID><PARENT>{parent_ref}</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        parse_native_group_source_records_with_evidence(&xml, PADDED_COMPANY_GUID)
            .expect("synthetic native group row parses")
    }

    fn padded_native_two_group_export(
        parent_name: &str,
        parent_parent_ref: &str,
        child_name: &str,
        child_parent_ref: &str,
    ) -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
        let xml = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP NAME="{parent_name}"><GUID>{PADDED_COMPANY_GUID}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID><PARENT>{parent_parent_ref}</PARENT></GROUP><GROUP NAME="{child_name}"><GUID>{PADDED_COMPANY_GUID}-00000002</GUID><MASTERID>2</MASTERID><ALTERID>2</ALTERID><PARENT>{child_parent_ref}</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        parse_native_group_source_records_with_evidence(&xml, PADDED_COMPANY_GUID)
            .expect("synthetic native two-group collection parses")
    }

    fn padded_native_ledger_export(
        name: &str,
        parent_ref: &str,
    ) -> ParsedExport<ParsedSourceRecord<TallyLedger>> {
        let xml = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME="{name}"><GUID>{PADDED_COMPANY_GUID}-00000002</GUID><MASTERID>2</MASTERID><ALTERID>2</ALTERID><PARENT>{parent_ref}</PARENT><OPENINGBALANCE>0.00</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        parse_native_ledger_source_records_with_evidence(&xml, PADDED_COMPANY_GUID)
            .expect("synthetic native ledger row parses")
    }

    fn padded_native_voucher_type_export(
        name: &str,
    ) -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
        let xml = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHERTYPE NAME="{name}"><GUID>{PADDED_COMPANY_GUID}-00000003</GUID><MASTERID>3</MASTERID><ALTERID>3</ALTERID><PARENT>Primary</PARENT></VOUCHERTYPE></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        parse_native_voucher_type_source_records_with_evidence(&xml, PADDED_COMPANY_GUID)
            .expect("synthetic native voucher type row parses")
    }

    fn padded_native_voucher_export(
        voucher_type_ref: &str,
        entry_ledger_name_ref: &str,
    ) -> ParsedExport<ParsedSourceRecord<TallyVoucher>> {
        let xml = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER REMOTEID="{PADDED_COMPANY_GUID}-00000004"><DATE>20260714</DATE><GUID>{PADDED_COMPANY_GUID}-00000004</GUID><MASTERID>4</MASTERID><ALTERID>4</ALTERID><VOUCHERTYPENAME>{voucher_type_ref}</VOUCHERTYPENAME><VOUCHERNUMBER>SYN-1</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>{entry_ledger_name_ref}</LEDGERNAME><AMOUNT>-100.00</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"#
        );
        parse_native_voucher_source_records_with_evidence(&xml, PADDED_COMPANY_GUID)
            .expect("synthetic native voucher row parses")
    }

    #[test]
    fn padded_ledger_name_and_its_ledgername_reference_resolve_together() {
        let padded_ledger_name = " Padded Cash Ledger ";
        let groups = padded_native_group_export("Assets", "Primary");
        let ledgers = padded_native_ledger_export(padded_ledger_name, "Assets");
        let voucher_types = padded_native_voucher_type_export("Receipt");
        let vouchers = padded_native_voucher_export("Receipt", padded_ledger_name);

        let window = build_core_window(&context(), groups, ledgers, voucher_types, vouchers)
            .expect(
            "a ledger name and its LEDGERNAME reference carry identical padding and must resolve",
        );
        let PackBatch::CoreAccounting(batch) = window.batch else {
            panic!("wrong pack");
        };
        // The resolved name must retain its exact original bytes, not a trimmed copy.
        assert_eq!(batch.ledgers[0].name, padded_ledger_name);
        assert_eq!(batch.ledger_entries.len(), 1);
        assert_eq!(
            batch.ledger_entries[0].ledger_source_id,
            format!("{PADDED_COMPANY_GUID}-00000002")
        );
    }

    #[test]
    fn padded_group_name_and_a_sibling_groups_parent_reference_resolve_together() {
        let padded_group_name = " Padded Parent Group ";
        let groups = padded_native_two_group_export(
            padded_group_name,
            "Primary",
            "Child Group",
            padded_group_name,
        );
        let ledgers = padded_native_ledger_export("Cash", "Child Group");
        let voucher_types = padded_native_voucher_type_export("Receipt");
        let vouchers = padded_native_voucher_export("Receipt", "Cash");

        let window = build_core_window(&context(), groups, ledgers, voucher_types, vouchers).expect(
            "a group name and a sibling group's PARENT reference carry identical padding and must resolve",
        );
        let PackBatch::CoreAccounting(batch) = window.batch else {
            panic!("wrong pack");
        };
        let parent_group = batch
            .groups
            .iter()
            .find(|group| group.source_id == format!("{PADDED_COMPANY_GUID}-00000001"))
            .expect("padded parent group is present");
        // The resolved name must retain its exact original bytes, not a trimmed copy.
        assert_eq!(parent_group.name, padded_group_name);
        let child_group = batch
            .groups
            .iter()
            .find(|group| group.source_id == format!("{PADDED_COMPANY_GUID}-00000002"))
            .expect("child group is present");
        assert_eq!(
            child_group.parent_source_id.as_deref(),
            Some(format!("{PADDED_COMPANY_GUID}-00000001").as_str())
        );
    }

    #[test]
    fn padded_voucher_type_name_and_its_vouchertypename_reference_resolve_together() {
        let padded_voucher_type_name = " Padded Receipt Type ";
        let groups = padded_native_group_export("Assets", "Primary");
        let ledgers = padded_native_ledger_export("Cash", "Assets");
        let voucher_types = padded_native_voucher_type_export(padded_voucher_type_name);
        let vouchers = padded_native_voucher_export(padded_voucher_type_name, "Cash");

        let window = build_core_window(&context(), groups, ledgers, voucher_types, vouchers).expect(
            "a voucher type name and its VOUCHERTYPENAME reference carry identical padding and must resolve",
        );
        let PackBatch::CoreAccounting(batch) = window.batch else {
            panic!("wrong pack");
        };
        // The resolved name must retain its exact original bytes, not a trimmed copy.
        assert_eq!(batch.voucher_types[0].name, padded_voucher_type_name);
        assert_eq!(
            batch.vouchers[0].voucher_type_source_id,
            format!("{PADDED_COMPANY_GUID}-00000003")
        );
    }

    #[test]
    fn invalid_requested_window_fails_before_source_rows_are_canonicalized() {
        for (from, to) in [("20260230", "20260731"), ("20260801", "20260731")] {
            let mut request = context();
            request.window.from_yyyymmdd = from.to_string();
            request.window.to_yyyymmdd = to.to_string();
            let (ledgers, vouchers) = ledgers_and_vouchers("Cash", "Cash");
            let error = build_core_window(&request, groups(), ledgers, voucher_types(), vouchers)
                .unwrap_err();
            assert!(matches!(
                error,
                TallyError::InvalidData { code } if code == "requested_window_invalid"
            ));
        }
    }

    #[test]
    fn selected_voucher_qualification_rejects_noncanonical_records_and_entries() {
        let (_, vouchers) = ledgers_and_vouchers("Cash", "Cash");
        validate_selected_voucher_window("20260701", "20260731", &vouchers).unwrap();

        let mut invalid_amount = vouchers.clone();
        invalid_amount.records[0].record.ledger_entries[0].amount = "not-an-amount".to_string();
        assert!(validate_selected_voucher_window("20260701", "20260731", &invalid_amount).is_err());

        let mut foreign_name = vouchers.clone();
        foreign_name.records[0].record.ledger_entries[0].ledger_name = " x ".to_string();
        assert!(
            validate_selected_voucher_window("20260701", "20260731", &foreign_name).is_ok(),
            "Tally-originated master text is not a Bridge canonical token"
        );

        let mut invalid_alter_id = vouchers;
        invalid_alter_id.records[0].alter_id = Some("contains whitespace".to_string());
        assert!(
            validate_selected_voucher_window("20260701", "20260731", &invalid_alter_id).is_err()
        );
    }
}
