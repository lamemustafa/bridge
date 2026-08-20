use bridge_tally_protocol::{
    parse_native_voucher_source_records_with_evidence,
    parse_native_voucher_type_source_records_with_evidence, ParsedSourceIdentityKind,
};

const COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const VOUCHER_TYPES: &str = include_str!("fixtures/native/voucher_types_native_wr2.xml");
const VOUCHERS: &str = include_str!("fixtures/native/vouchers_native_wr2.xml");

#[test]
fn captured_native_vouchers_preserve_identity_exact_amounts_and_direct_entry_scope() {
    let parsed = parse_native_voucher_source_records_with_evidence(VOUCHERS, COMPANY_GUID)
        .expect("captured native voucher collection parses");

    assert_eq!(parsed.records.len(), 3);
    assert_eq!(parsed.evidence.identified_record_count, 3);
    assert!(parsed.evidence.duplicate_identities.is_empty());
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 3);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 0);
    assert!(parsed.records.iter().all(|record| {
        record.identity_kind == Some(ParsedSourceIdentityKind::Guid)
            && record.identities.guid.is_some()
            && record.identities.remote_id.is_some()
            && record.identities.master_id.is_some()
            && record.alter_id.is_some()
            && record.raw_source_sha256.len() == 64
    }));

    let first = &parsed.records[0].record;
    assert_eq!(
        first.ledger_entries.len(),
        2,
        "bill allocations are not entries"
    );
    assert_eq!(
        first
            .ledger_entries
            .iter()
            .map(|entry| entry.amount.as_str())
            .collect::<Vec<_>>(),
        ["-101.01", "101.01"]
    );
    assert_eq!(first.ledger_entries[0].ledger_name, "नमस्ते ट्रेडर्स");
    assert!(first.ledger_entries[0].is_deemed_positive);
    assert!(!first.ledger_entries[1].is_deemed_positive);

    let amounts = parsed
        .records
        .iter()
        .flat_map(|record| record.record.ledger_entries.iter())
        .map(|entry| entry.amount.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        amounts,
        ["-101.01", "101.01", "-102.02", "102.02", "-103.03", "103.03"]
    );
}

#[test]
fn captured_native_voucher_types_preserve_real_identity_and_company_binding() {
    let parsed =
        parse_native_voucher_type_source_records_with_evidence(VOUCHER_TYPES, COMPANY_GUID)
            .expect("captured native voucher type collection parses");

    assert_eq!(parsed.records.len(), 24);
    assert_eq!(parsed.evidence.identified_record_count, 24);
    assert!(parsed.evidence.duplicate_identities.is_empty());
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 24);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 0);
    assert!(parsed.records.iter().all(|record| {
        record.identity_kind == Some(ParsedSourceIdentityKind::Guid)
            && record.identities.master_id.is_some()
            && record.alter_id.is_some()
            && record.raw_source_sha256.len() == 64
    }));
    assert!(parsed
        .records
        .iter()
        .any(|record| record.record.name == "Sales"));
}

#[test]
fn native_voucher_collections_fail_closed_without_a_company_bound_row() {
    assert!(parse_native_voucher_source_records_with_evidence(
        VOUCHERS,
        "00000000-0000-0000-0000-000000000000",
    )
    .is_err());
}

#[test]
fn foreign_voucher_identity_prefix_is_counted_per_row_without_erasing_binding() {
    let mixed_prefix = VOUCHERS.replacen(
        "REMOTEID=\"61c6de69-1748-461c-ad3f-162cb949df9f-00000001\"",
        "REMOTEID=\"01234567-89ab-cdef-0123-456789abcdef-00000001\"",
        1,
    );
    let parsed = parse_native_voucher_source_records_with_evidence(&mixed_prefix, COMPANY_GUID)
        .expect("one foreign voucher identity does not erase collection binding");
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 2);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 1);
}
