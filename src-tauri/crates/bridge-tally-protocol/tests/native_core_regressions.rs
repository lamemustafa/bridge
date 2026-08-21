use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, parse_native_group_source_records_with_evidence,
    parse_native_voucher_source_records_with_evidence, ExpectedTallyTextEncoding,
    ParsedSourceIdentityKind,
};
use sha2::{Digest, Sha256};

const COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const EMPTY_VOUCHER_WINDOW: &[u8] =
    include_bytes!("fixtures/native/empty_voucher_window_wr2.utf16le.xml");
const GROUPS_WITH_IDENTITY: &[u8] =
    include_bytes!("fixtures/native/group_snapshot_wr2_with_identity.utf16le.xml");

fn decode_utf16le(bytes: &[u8]) -> String {
    decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .expect("captured BOM-less UTF-16LE response decodes")
    .text
}

#[test]
fn captured_empty_voucher_collection_is_a_valid_empty_export() {
    let parsed = parse_native_voucher_source_records_with_evidence(
        &decode_utf16le(EMPTY_VOUCHER_WINDOW),
        COMPANY_GUID,
    )
    .expect("a successful, empty native collection is valid before extent binding");

    assert!(parsed.records.is_empty());
    assert_eq!(parsed.evidence.source_record_count, None);
    assert_eq!(parsed.evidence.observed_record_count, Some(0));
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 0);
}

#[test]
fn native_raw_hash_attests_the_unsanitised_wire_fragment() {
    let parse = |reference: &str| {
        let xml = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP NAME=\"G\"><GUID>{COMPANY_GUID}-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID><PARENT>Primary</PARENT><UNKNOWN>A{reference}B</UNKNOWN></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"
        );
        let raw_group =
            &xml[xml.find("<GROUP").unwrap()..xml.find("</GROUP>").unwrap() + "</GROUP>".len()];
        let parsed = parse_native_group_source_records_with_evidence(&xml, COMPANY_GUID)
            .expect("the narrow XML repair is parse-only");
        let expected: String = Sha256::digest(raw_group.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        (parsed.records[0].raw_source_sha256.clone(), expected)
    };

    let decimal = parse("&#4;");
    let hexadecimal = parse("&#x4;");
    assert_eq!(decimal.0, decimal.1);
    assert_eq!(hexadecimal.0, hexadecimal.1);
    assert_ne!(decimal.0, hexadecimal.0);
}

#[test]
fn captured_native_groups_use_observed_guid_identity() {
    let parsed = parse_native_group_source_records_with_evidence(
        &decode_utf16le(GROUPS_WITH_IDENTITY),
        COMPANY_GUID,
    )
    .expect("captured groups with durable identity parse");

    assert_eq!(parsed.records.len(), 29);
    assert!(parsed.records.iter().all(|record| {
        record.identity_kind == Some(ParsedSourceIdentityKind::Guid)
            && record.source_id == record.identities.guid
            && record.identities.master_id.is_some()
            && record.alter_id.is_some()
    }));
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 29);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 0);
}

#[test]
fn zero_entry_voucher_is_preserved_as_a_supported_shape() {
    let xml = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER REMOTEID="61c6de69-1748-461c-ad3f-162cb949df9f-00000001"><DATE>20260801</DATE><GUID>61c6de69-1748-461c-ad3f-162cb949df9f-00000001</GUID><MASTERID>1</MASTERID><ALTERID>1</ALTERID><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><VOUCHERNUMBER>ZERO</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"#;
    let parsed = parse_native_voucher_source_records_with_evidence(xml, COMPANY_GUID)
        .expect("zero-entry voucher is valid when all state-required fields exist");

    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].record.ledger_entry_count, Some(0));
    assert!(parsed.records[0].record.ledger_entries.is_empty());
}
