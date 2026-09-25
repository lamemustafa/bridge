use bridge_tally_protocol::{
    parse_native_voucher_source_records_with_evidence,
    parse_native_voucher_type_source_records_with_evidence, NativeCollectionError,
    ParsedSourceIdentityKind,
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

/// The captured voucher-type list, changed in one place, refused for one
/// typed reason (bridge#676). Each case asserts the variant, never text.
fn voucher_type_refusal(xml: &str, company_guid: &str) -> NativeCollectionError {
    parse_native_voucher_type_source_records_with_evidence(xml, company_guid)
        .expect_err("the changed voucher-type list is refused")
}

const FIRST_VOUCHER_TYPE_GUID: &str =
    "<GUID TYPE=\"String\">61c6de69-1748-461c-ad3f-162cb949df9f-0000004e</GUID>";

#[test]
fn a_voucher_type_list_whose_root_is_not_envelope_is_malformed() {
    let renamed = VOUCHER_TYPES
        .replacen("<ENVELOPE>", "<RESPONSE>", 1)
        .replacen("</ENVELOPE>", "</RESPONSE>", 1);
    assert_eq!(
        voucher_type_refusal(&renamed, COMPANY_GUID),
        NativeCollectionError::MalformedResponse
    );
}

#[test]
fn a_voucher_type_list_cut_off_before_its_root_closes_is_malformed() {
    let cut = &VOUCHER_TYPES[..VOUCHER_TYPES.find("</COLLECTION>").unwrap()];
    assert_eq!(
        voucher_type_refusal(cut, COMPANY_GUID),
        NativeCollectionError::MalformedResponse
    );
}

#[test]
fn an_xml_error_inside_a_voucher_type_row_is_malformed_not_the_row() {
    // The row parser reports through anyhow; the XML error inside it is
    // recovered by type, so a broken response is not blamed on one master.
    let broken = VOUCHER_TYPES.replacen(
        FIRST_VOUCHER_TYPE_GUID,
        "<GUID TYPE=\"String\">61c6de69-1748-461c-ad3f-162cb949df9f-0000004e</MASTERID>",
        1,
    );
    assert_eq!(
        voucher_type_refusal(&broken, COMPANY_GUID),
        NativeCollectionError::MalformedResponse
    );
}

#[test]
fn a_voucher_type_list_whose_status_is_not_one_did_not_succeed() {
    let failed = VOUCHER_TYPES.replacen("<STATUS>1</STATUS>", "<STATUS>0</STATUS>", 1);
    assert_eq!(
        voucher_type_refusal(&failed, COMPANY_GUID),
        NativeCollectionError::NotSuccess
    );
}

#[test]
fn a_voucher_type_row_without_its_guid_is_an_unusable_row() {
    let unidentified = VOUCHER_TYPES.replacen(FIRST_VOUCHER_TYPE_GUID, "", 1);
    assert_ne!(unidentified, VOUCHER_TYPES, "the GUID was removed");
    assert_eq!(
        voucher_type_refusal(&unidentified, COMPANY_GUID),
        NativeCollectionError::RowUnusable
    );
}

#[test]
fn a_voucher_type_list_with_no_row_of_the_company_is_an_identity_mismatch() {
    assert_eq!(
        voucher_type_refusal(VOUCHER_TYPES, "00000000-0000-0000-0000-000000000000"),
        NativeCollectionError::CompanyIdentityMismatch
    );
}

#[test]
fn native_collection_causes_are_distinct_and_name_no_row() {
    let codes = [
        NativeCollectionError::MalformedResponse,
        NativeCollectionError::NotSuccess,
        NativeCollectionError::RowUnusable,
        NativeCollectionError::CompanyIdentityMismatch,
        NativeCollectionError::BoundsViolation,
    ]
    .map(NativeCollectionError::safe_code);
    let distinct = codes.iter().collect::<std::collections::HashSet<_>>();
    assert_eq!(distinct.len(), codes.len(), "{codes:?}");
    assert!(codes
        .iter()
        .all(|code| code.starts_with("native_collection_")
            && code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')));
}
