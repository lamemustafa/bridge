//! Deterministic conversion from strict Tally export records to Bridge canonical packs.
//!
//! Folded from the former standalone `bridge-tally-canonical` crate: it had exactly one consumer
//! inside this crate (`tally::connector` and `tally::connection`), so the crate boundary earned
//! nothing except hiding this module's true dead code from the `dead_code` lint. This module
//! deliberately still has no HTTP, database, OpenSSL, or Tauri dependency, so the complete
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

fn is_tally_reserved_root(value: &str) -> bool {
    const SANITIZED_ROOT_MARKER: &str = "\u{fffd}#4;";

    let value = value.trim();
    // Captures from two companies observed this marker only on Tally's reserved root (30 group
    // parents and one ledger parent). Match the marker rather than its English display name.
    // Keep the plain spelling because the legacy export path still emits it.
    value.starts_with(SANITIZED_ROOT_MARKER) || value.eq_ignore_ascii_case("primary")
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
        parse_group_source_records_with_evidence, parse_ledger_source_records_with_evidence,
        parse_voucher_source_records_with_evidence,
        parse_voucher_type_source_records_with_evidence, ParsedExport, ParsedSourceRecord,
        TallyLedger, TallyNamedMaster, TallyVoucher, BRIDGE_GROUP_EXPORT_SCHEMA,
        BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_VOUCHER_EXPORT_SCHEMA,
        BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
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
    fn tally_reserved_root_accepts_marker_and_legacy_plain_forms_for_both_parent_paths() {
        let group_ids_by_name = BTreeMap::from([("Assets".to_string(), "group-guid".to_string())]);

        for value in ["\u{fffd}#4; Primary", "Primary"] {
            assert_eq!(
                resolve_group_parent(Some(value), &group_ids_by_name, "group_parent_missing")
                    .unwrap(),
                None
            );
            assert_eq!(
                resolve_optional_reference(
                    Some(value),
                    &group_ids_by_name,
                    "ledger_parent_group_missing"
                )
                .unwrap(),
                None
            );
        }
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

        let mut invalid_name = vouchers.clone();
        invalid_name.records[0].record.ledger_entries[0].ledger_name = " x ".to_string();
        assert!(validate_selected_voucher_window("20260701", "20260731", &invalid_name).is_err());

        let mut invalid_alter_id = vouchers;
        invalid_alter_id.records[0].alter_id = Some("contains whitespace".to_string());
        assert!(
            validate_selected_voucher_window("20260701", "20260731", &invalid_alter_id).is_err()
        );
    }
}
