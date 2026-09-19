//! The Bridge-schema named-master parser reads a forbidden numeric reference
//! by the same rule as the native collection parsers
//! (`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §1.1(d)).
//!
//! The captured group collection carries `&#4; Primary` in `PARENT`. Its rows
//! carry fields the Bridge-schema envelope refuses, so the test lifts the wire
//! text of one captured `PARENT` into that envelope rather than re-reading the
//! whole row.

use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, is_tally_reserved_root,
    parse_group_source_records_with_evidence, parse_native_group_source_records_with_evidence,
    parse_standard_ledger_catalog_with_identities, parse_voucher_type_source_records_with_evidence,
    ExpectedTallyTextEncoding, BRIDGE_GROUP_EXPORT_SCHEMA, BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
    TALLY_SANITIZED_ROOT_MARKER,
};
use sha2::{Digest, Sha256};

const COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const GROUPS_WITH_IDENTITY: &[u8] =
    include_bytes!("fixtures/native/group_snapshot_wr2_with_identity.utf16le.xml");

const LEDGER_CATALOGUE: &[u8] =
    include_bytes!("fixtures/agent/native-ledger-catalogue.utf16le.xml");

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

fn captured_groups() -> String {
    decode_utf16le(GROUPS_WITH_IDENTITY)
}

/// The name of one captured group whose `PARENT` is the reserved root, with
/// that `PARENT`'s text exactly as it was on the wire.
fn captured_root_parented_group(xml: &str) -> (String, String) {
    let parent_start = xml
        .find("&#4; Primary</PARENT>")
        .expect("the capture carries a root-parented group");
    let row_start = xml[..parent_start]
        .rfind("<GROUP NAME=\"")
        .expect("the PARENT belongs to a GROUP row")
        + "<GROUP NAME=\"".len();
    let name = xml[row_start..]
        .split('"')
        .next()
        .expect("the row carries a NAME attribute")
        .to_string();
    let value_start = xml[..parent_start].rfind('>').unwrap() + 1;
    let wire_parent = xml[value_start..parent_start + "&#4; Primary".len()].to_string();
    (name, wire_parent)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn bridge_schema_named_masters_read_a_captured_reference_as_the_native_parser_does() {
    let xml = captured_groups();
    let (name, wire_parent) = captured_root_parented_group(&xml);
    assert_eq!(
        wire_parent, "&#4; Primary",
        "the lifted atom is the wire text"
    );

    let native = parse_native_group_source_records_with_evidence(&xml, COMPANY_GUID)
        .expect("captured groups parse natively");
    let native_parent = native
        .records
        .iter()
        .find(|record| record.record.name == name)
        .and_then(|record| record.record.parent.returned_text())
        .expect("the native parser returns the captured PARENT")
        .to_string();
    assert_eq!(
        native_parent,
        format!("{TALLY_SANITIZED_ROOT_MARKER} Primary")
    );

    for (schema, object_type, element) in [
        (BRIDGE_GROUP_EXPORT_SCHEMA, "GROUP", "GROUP"),
        (
            BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
            "VOUCHERTYPE",
            "VOUCHERTYPE",
        ),
    ] {
        let row = format!(
            r#"<{element} NAME="{name}" GUID="{COMPANY_GUID}-00000001" MASTERID="1" ALTERID="1"><PARENT>{wire_parent}</PARENT></{element}>"#
        );
        let bridge_xml = format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{schema}" OBJECTTYPE="{object_type}" NAME="BRIDGE SYNTHETIC BOOK" GUID="{COMPANY_GUID}" RECORDCOUNT="1"/>{row}</BODY></ENVELOPE>"#
        );
        let parsed = if element == "GROUP" {
            parse_group_source_records_with_evidence(&bridge_xml)
        } else {
            parse_voucher_type_source_records_with_evidence(&bridge_xml)
        }
        .expect("a Bridge-schema export carrying Tally's reference parses");
        let record = &parsed.records[0];
        assert_eq!(
            record.record.parent.returned_text(),
            Some(native_parent.as_str()),
            "{object_type}: both parsers must spell the captured reference one way"
        );
        assert!(is_tally_reserved_root(&native_parent));
        // The row hash still attests the bytes on the wire, `&#4;` included,
        // not the marked text the reader consumed.
        assert_eq!(record.raw_source_sha256, sha256_hex(row.as_bytes()));
    }
}

#[test]
fn the_standard_ledger_catalogue_reads_a_captured_root_parent_as_the_marker() {
    // The captured `List of Ledgers` carries `&#4; Primary` as the parent of
    // `Profit & Loss A/c`. Read as a raw U+0004, the catalogue discarded it as
    // an unsafe display character and reported the parent as unobserved.
    let xml = decode_utf16le(LEDGER_CATALOGUE);
    assert_eq!(
        xml.matches("<PARENT TYPE=\"String\">&#4; Primary</PARENT>")
            .count(),
        1
    );
    let catalogue =
        parse_standard_ledger_catalog_with_identities(&xml, "WR2 Unicode Lab", COMPANY_GUID)
            .expect("the captured catalogue parses");
    let parent = catalogue
        .parents()
        .find(|(name, _)| *name == "Profit & Loss A/c")
        .map(|(_, parent)| parent)
        .expect("the captured catalogue lists the root-parented ledger");
    assert_eq!(
        parent,
        Some(format!("{TALLY_SANITIZED_ROOT_MARKER} Primary").as_str())
    );
    assert!(is_tally_reserved_root(parent.unwrap()));
}
