use super::*;

const COMPANY_GUID: &str = "11111111-1111-1111-1111-111111111111";
const FOREIGN_GUID: &str = "22222222-2222-2222-2222-222222222222";

const LIVE_SHAPE: &str = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO><GROUP>0</GROUP></CMPINFO></DESC><DATA><COLLECTION><GROUP NAME="North Region"><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>Current Assets</PARENT></GROUP><GROUP NAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>&#4; Primary</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#;

#[test]
fn reads_group_rows_only_from_the_native_collection() {
    let groups = parse_native_group_snapshot(LIVE_SHAPE, COMPANY_GUID)
        .expect("native group snapshot parses");
    assert_eq!(groups.len(), 2, "CMPINFO group counter is not a row");
    assert_eq!(groups[0].name, "North Region");
    assert_eq!(groups[0].parent.returned_text(), Some("Current Assets"));
    assert_eq!(
        groups[1].parent.returned_text(),
        Some("\u{fffd}#4; Primary")
    );

    let evidence = parse_native_group_snapshot_with_evidence(LIVE_SHAPE, COMPANY_GUID)
        .expect("native group snapshot evidence parses");
    assert_eq!(evidence.len(), 2);
    assert_eq!(evidence[0].record, groups[0]);
    assert_eq!(evidence[0].raw_source_sha256.len(), 64);
    assert_ne!(evidence[0].raw_source_sha256, evidence[1].raw_source_sha256);
}

#[test]
fn missing_group_parent_fails_closed() {
    let xml = LIVE_SHAPE.replace("<PARENT>Current Assets</PARENT>", "");
    assert_eq!(
        parse_native_group_snapshot(&xml, COMPANY_GUID),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_parent_missing"
        ))
    );
}

#[test]
fn group_evidence_hashes_the_unsanitised_wire_fragment() {
    let decimal = parse_native_group_snapshot_with_evidence(LIVE_SHAPE, COMPANY_GUID)
        .expect("decimal illegal reference remains parseable");
    let hexadecimal_xml = LIVE_SHAPE.replace("&#4;", "&#x4;");
    let hexadecimal = parse_native_group_snapshot_with_evidence(&hexadecimal_xml, COMPANY_GUID)
        .expect("hexadecimal illegal reference remains parseable");

    let decimal_fragment = b"<GROUP NAME=\"Sundry Debtors\"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>&#4; Primary</PARENT></GROUP>";
    let hexadecimal_fragment = b"<GROUP NAME=\"Sundry Debtors\"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>&#x4; Primary</PARENT></GROUP>";
    assert_eq!(decimal[1].raw_source_sha256, sha256_hex(decimal_fragment));
    assert_eq!(
        hexadecimal[1].raw_source_sha256,
        sha256_hex(hexadecimal_fragment)
    );
    assert_ne!(
        decimal[1].raw_source_sha256,
        hexadecimal[1].raw_source_sha256
    );
}

/// A Group object's own GUID can belong to an imported master. The
/// response-bound computed company GUID, not that object identity,
/// determines whether the response is admissible.
#[test]
fn group_response_retains_imported_foreign_object_guids() {
    let xml = LIVE_SHAPE
        .replace(
            "11111111-1111-1111-1111-111111111111-00000001",
            "22222222-2222-2222-2222-222222222222-00000001",
        )
        .replace(
            "11111111-1111-1111-1111-111111111111-00000002",
            "22222222-2222-2222-2222-222222222222-00000002",
        );
    assert!(xml.contains(FOREIGN_GUID), "sanity: replacement took hold");
    assert_eq!(
        parse_native_group_snapshot(&xml, COMPANY_GUID)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn group_response_requires_its_own_company_identity() {
    let response = |company_guid: Option<&str>| {
        let company_guid = company_guid
            .map(|guid| format!("<BRIDGECOMPANYGUID>{guid}</BRIDGECOMPANYGUID>"))
            .unwrap_or_default();
        format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\\
                <GROUP NAME=\"Imported selected group\"><GUID>{COMPANY_GUID}-00000001</GUID>{company_guid}\\
                <PARENT>Primary</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"
        )
    };

    assert_eq!(
        parse_native_group_snapshot(&response(Some(FOREIGN_GUID)), COMPANY_GUID),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_mismatch"
        )),
        "a selected-prefix imported group cannot prove which company answered"
    );
    assert_eq!(
        parse_native_group_snapshot(&response(None), COMPANY_GUID),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_missing"
        )),
        "a response without Tally's computed company GUID is withheld"
    );
    assert_eq!(
        parse_native_group_snapshot(&response(Some(COMPANY_GUID)), COMPANY_GUID)
            .unwrap()
            .len(),
        1,
        "the response-bound company GUID, not a row GUID prefix, admits the snapshot"
    );

    let second_row_company_mismatch = LIVE_SHAPE.replace(
        "<GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID>",
        "<GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>22222222-2222-2222-2222-222222222222</BRIDGECOMPANYGUID>",
    );
    assert_eq!(
        parse_native_group_snapshot(&second_row_company_mismatch, COMPANY_GUID),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_mismatch"
        )),
        "a matching first row cannot admit a later row with a foreign responder"
    );

    let second_row_company_missing = LIVE_SHAPE.replace(
        "<GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID>",
        "<GUID>11111111-1111-1111-1111-111111111111-00000002</GUID>",
    );
    assert_eq!(
        parse_native_group_snapshot(&second_row_company_missing, COMPANY_GUID),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_missing"
        )),
        "a matching first row cannot admit a later row without responder identity"
    );
}

/// A response with the matching computed company GUID continues to parse
/// names and parents exactly as before the fix.
#[test]
fn group_response_with_the_correct_prefix_is_accepted_and_still_parses() {
    let groups = parse_native_group_snapshot(LIVE_SHAPE, COMPANY_GUID)
        .expect("a response bound to the selected company is accepted");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name, "North Region");
    assert_eq!(groups[0].parent.returned_text(), Some("Current Assets"));
    assert_eq!(groups[1].name, "Sundry Debtors");
}

/// A book can legitimately hold masters imported with their original
/// GUIDs. A response with one foreign object GUID stays admissible because
/// every row still proves its responder through BRIDGECOMPANYGUID.
#[test]
fn mixed_prefix_response_is_accepted_with_the_foreign_row_retained_and_counted() {
    let xml = LIVE_SHAPE.replace(
        "11111111-1111-1111-1111-111111111111-00000002",
        "22222222-2222-2222-2222-222222222222-00000002",
    );
    let groups = parse_native_group_snapshot(&xml, COMPANY_GUID)
        .expect("the response-bound company GUID admits the whole snapshot");
    assert_eq!(
        groups.len(),
        2,
        "the foreign-prefix row is retained, not rejected"
    );
    assert_eq!(groups[1].name, "Sundry Debtors");
}

/// A row object's GUID does not establish the responder identity. It can
/// be omitted without weakening the row's independent computed binding.
#[test]
fn row_omitting_guid_entirely_is_not_scored_but_does_not_block_acceptance() {
    let xml = LIVE_SHAPE.replace(
        "<GUID>11111111-1111-1111-1111-111111111111-00000001</GUID>",
        "",
    );
    let groups = parse_native_group_snapshot(&xml, COMPANY_GUID)
        .expect("the row's computed company GUID binds the response");
    assert_eq!(groups.len(), 2);
}

/// If every object GUID is omitted, the per-row response identity remains
/// sufficient. This specifically prevents a future refactor from drifting
/// back to a row-GUID-prefix admission rule.
#[test]
fn a_response_where_every_row_omits_object_guid_is_still_bound() {
    let xml = LIVE_SHAPE
        .replace(
            "<GUID>11111111-1111-1111-1111-111111111111-00000001</GUID>",
            "",
        )
        .replace(
            "<GUID>11111111-1111-1111-1111-111111111111-00000002</GUID>",
            "",
        );
    assert_eq!(
        parse_native_group_snapshot(&xml, COMPANY_GUID)
            .unwrap()
            .len(),
        2
    );
}

/// `group_snapshot_aarav.xml` is a real TallyPrime response captured
/// before this request computed `BRIDGECOMPANYGUID`. It must now be
/// withheld: object GUIDs, whether present or absent, cannot replace the
/// response-bound value.
#[test]
fn a_real_pre_widening_capture_with_no_guid_anywhere_is_rejected() {
    let xml = include_str!("../../tests/fixtures/native/group_snapshot_aarav.xml");
    assert_eq!(
        parse_native_group_snapshot(xml, "bb8ad19e-6aef-4239-a917-87fec0c6215e"),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_missing"
        ))
    );
}

/// `RESERVEDNAME` parsing must keep three states distinct: a real
/// (non-empty) predefined identity, Tally's own explicit empty-string
/// "this is user-created" signal, and the attribute being absent
/// entirely (an older capture, or a build that omits it). Folding the
/// empty-string case into "absent" would let a custom group merely named
/// like a predefined one pass as the identity fallback -- see
/// `crate::group_ancestry::GroupIndex::reserved_ancestor` for how the
/// distinction is used.
#[test]
fn reserved_name_attribute_parsing_distinguishes_present_empty_and_absent() {
    let xml = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP NAME="WR5 Renamed Suspense" RESERVEDNAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>Primary</PARENT></GROUP><GROUP NAME="Sundry Debtors" RESERVEDNAME=""><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>Primary</PARENT></GROUP><GROUP NAME="Old Capture Group"><GUID>11111111-1111-1111-1111-111111111111-00000003</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>Primary</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#;
    let groups = parse_native_group_snapshot(xml, COMPANY_GUID).expect("parses");
    assert_eq!(
        groups[0].reserved_name.as_deref(),
        Some("Sundry Debtors"),
        "a renamed predefined group keeps its immutable RESERVEDNAME identity"
    );
    assert_eq!(
        groups[1].reserved_name.as_deref(),
        Some(""),
        "an empty RESERVEDNAME is Tally's own signal, not a missing value"
    );
    assert_eq!(
        groups[2].reserved_name, None,
        "a row that never carried the attribute at all must stay None, not empty-string"
    );
}
