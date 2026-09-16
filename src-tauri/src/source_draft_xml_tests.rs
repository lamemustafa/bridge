use super::*;
const XML: &str = "<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE><VOUCHER REMOTEID=\"id&amp;1\" VCHTYPE=\"Receipt\"><DATE>20260901</DATE><NARRATION>Party &amp; Co</NARRATION><VOUCHERNUMBER>1</VOUCHERNUMBER><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>-1.00</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>";
#[test]
fn captured_tally_collection_export_is_not_a_voucher_import_document() {
    // This real captured response is a different document contract from the
    // user-authored IMPORTDATA candidates supported by local preparation.
    let captured = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/unit_a_vouchers_wildcard_live.xml"
    );
    assert_eq!(
        parse_source_xml(captured, "captured-collection.xml".into()),
        Err(SourceXmlError::UnsupportedShape)
    );
}
#[test]
fn accepts_the_structure_of_a_real_voucher_import_candidate() {
    // Provenance, stated exactly: this fixture is a structural derivative of
    // a real user-authored Tally voucher-import file. Its envelope nesting,
    // element set and order, attribute set, per-voucher field presence,
    // entry cardinality, balanced +/- pair invariant and value FORMATS are
    // transcribed from that document. Every value is synthetic; no original
    // name, date, amount, narration, transaction id, company or party
    // survives. See the fixture README for the derivation and its limits.
    //
    // What it establishes: the supported shape is not an assumption this
    // suite invented, unlike the hand-authored XML constant above.
    // What it does NOT establish: that Tally accepted an import of this
    // document. The source file's own import outcome is unknown, so this is
    // format evidence, never acceptance evidence.
    let derived = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/voucher_import_candidate_structure_derived.xml"
    );
    let parsed = parse_source_xml(derived, "voucher-import-candidate.xml".into())
        .expect("the real import candidate shape must parse");

    assert_eq!(parsed.vouchers.len(), 77);
    assert!(parsed
        .vouchers
        .iter()
        .all(|voucher| voucher.entries.len() == 2));

    // Fields this format carries that local preparation does not interpret
    // must be reported as omissions rather than silently dropped.
    for voucher in &parsed.vouchers {
        assert_eq!(
            voucher.omitted_fields,
            [
                "VOUCHER/@ACTION",
                "VOUCHER/EFFECTIVEDATE",
                "VOUCHER/PARTYLEDGERNAME",
                "VOUCHER/VOUCHERNUMBER",
            ]
        );
    }

    // Entries carry no ISDEEMEDPOSITIVE, so polarity stays unanswered and is
    // never inferred from the amount sign.
    assert!(parsed
        .vouchers
        .iter()
        .flat_map(|voucher| &voucher.entries)
        .all(|entry| entry.polarity.is_none()));

    // Amounts are retained as exact source text, not reparsed into a number.
    let first = &parsed.vouchers[0];
    assert_eq!(first.voucher_type, "Receipt");
    assert_eq!(first.date, "20010101");
    assert_eq!(first.entries[0].amount, "-7.50");
    assert_eq!(first.entries[1].amount, "7.50");

    // The source bytes survive parsing untouched.
    assert_eq!(parsed.utf8.as_bytes(), derived.as_slice());
}
#[test]
fn preserves_raw_source_and_marks_omitted_fields() {
    let parsed = parse_source_xml(XML.as_bytes(), "source.xml".into()).unwrap();
    assert_eq!(parsed.vouchers[0].remote_id, "id&1");
    assert_eq!(parsed.vouchers[0].narration.as_deref(), Some("Party & Co"));
    assert_eq!(parsed.vouchers[0].entries[0].polarity, None);
    assert_eq!(parsed.vouchers[0].omitted_fields, ["VOUCHER/VOUCHERNUMBER"]);

    let with_entry_attribute = XML.replacen(
        "<ALLLEDGERENTRIES.LIST>",
        "<ALLLEDGERENTRIES.LIST OBSERVED=\"metadata\">",
        1,
    );
    let parsed = parse_source_xml(with_entry_attribute.as_bytes(), "source.xml".into()).unwrap();
    assert_eq!(parsed.utf8, with_entry_attribute);
    assert_eq!(
        parsed.vouchers[0].omitted_fields,
        ["ENTRY/@OBSERVED", "VOUCHER/VOUCHERNUMBER"]
    );
}
#[test]
fn rejects_doctype_and_nested_unknown_source_fields() {
    assert_eq!(
        parse_source_xml(b"<!DOCTYPE a><ENVELOPE/>", "x".into()),
        Err(SourceXmlError::DoctypeForbidden)
    );
    assert_eq!(
        parse_source_xml(
            XML.replacen(
                "<VOUCHERNUMBER>1</VOUCHERNUMBER>",
                "<EXTRA><X>1</X></EXTRA>",
                1
            )
            .as_bytes(),
            "x".into()
        ),
        Err(SourceXmlError::UnsupportedShape)
    );
}
#[test]
fn duplicate_scalar_refuses_while_present_empty_remains_distinct_from_absent() {
    let duplicate = XML.replacen(
        "<DATE>20260901</DATE>",
        "<DATE>20260901</DATE><DATE>20260902</DATE>",
        1,
    );
    assert_eq!(
        parse_source_xml(duplicate.as_bytes(), "x.xml".into()),
        Err(SourceXmlError::UnsupportedShape)
    );
    let empty = XML.replacen("<NARRATION>Party &amp; Co</NARRATION>", "<NARRATION/>", 1);
    assert_eq!(
        parse_source_xml(empty.as_bytes(), "x.xml".into())
            .unwrap()
            .vouchers[0]
            .narration
            .as_deref(),
        Some("")
    );
    let absent = XML.replacen("<NARRATION>Party &amp; Co</NARRATION>", "", 1);
    assert_eq!(
        parse_source_xml(absent.as_bytes(), "x.xml".into())
            .unwrap()
            .vouchers[0]
            .narration,
        None
    );
}
#[test]
fn hash_changes_with_source_bytes_and_duplicate_remote_id_refuses() {
    let one = parse_source_xml(XML.as_bytes(), "x.xml".into()).unwrap();
    let two = parse_source_xml(format!("{XML}\n").as_bytes(), "x.xml".into()).unwrap();
    assert_ne!(one.sha256, two.sha256);
    let duplicate = XML.replacen(
        "</TALLYMESSAGE>",
        &format!(
            "</TALLYMESSAGE><TALLYMESSAGE>{}</TALLYMESSAGE>",
            XML.split("<TALLYMESSAGE>")
                .nth(1)
                .unwrap()
                .split("</TALLYMESSAGE>")
                .next()
                .unwrap()
        ),
        1,
    );
    assert_eq!(
        parse_source_xml(duplicate.as_bytes(), "x.xml".into()),
        Err(SourceXmlError::UnsupportedShape)
    );
}
#[test]
fn distinct_source_notice_categories_are_bounded() {
    let at_limit = (0..MAX_SOURCE_NOTICE_KINDS)
        .map(|index| format!("<NOTICE{index}/>"))
        .collect::<String>();
    let at_limit_xml = XML.replacen("<VOUCHER", &format!("{at_limit}<VOUCHER"), 1);
    assert_eq!(
        parse_source_xml(at_limit_xml.as_bytes(), "x.xml".into())
            .unwrap()
            .source_notices
            .len(),
        MAX_SOURCE_NOTICE_KINDS
    );
    let repeated = parse_source_xml(
        at_limit_xml
            .replacen("<VOUCHER", "<NOTICE0/><VOUCHER", 1)
            .as_bytes(),
        "x.xml".into(),
    )
    .unwrap();
    assert_eq!(repeated.source_notices.len(), MAX_SOURCE_NOTICE_KINDS);
    assert_eq!(
        repeated
            .source_notices
            .iter()
            .find(|notice| notice.kind == "Ignored TALLYMESSAGE/NOTICE0")
            .map(|notice| notice.count),
        Some(2)
    );
    let over_limit = (0..=MAX_SOURCE_NOTICE_KINDS)
        .map(|index| format!("<NOTICE{index}/>"))
        .collect::<String>();
    assert_eq!(
        parse_source_xml(
            XML.replacen("<VOUCHER", &format!("{over_limit}<VOUCHER"), 1)
                .as_bytes(),
            "x.xml".into()
        ),
        Err(SourceXmlError::SourceNoticeLimit)
    );
}

#[test]
fn omitted_categories_share_one_row_bound_across_all_routes() {
    let attributes = (0..21).map(|i| format!(" A{i}=\"\"")).collect::<String>();
    let voucher_fields = (0..21).map(|i| format!("<F{i}/>")).collect::<String>();
    let entry_fields = (0..22).map(|i| format!("<E{i}/>")).collect::<String>();
    let at_limit = XML
        .replacen("<VOUCHER ", &format!("<VOUCHER{attributes} "), 1)
        .replacen("<VOUCHERNUMBER>1</VOUCHERNUMBER>", &voucher_fields, 1)
        .replacen(
            "</ALLLEDGERENTRIES.LIST>",
            &format!("{entry_fields}</ALLLEDGERENTRIES.LIST>"),
            1,
        );
    let parsed = parse_source_xml(at_limit.as_bytes(), "x.xml".into()).unwrap();
    assert_eq!(
        parsed.vouchers[0].omitted_fields.len(),
        MAX_OMITTED_FIELDS_PER_VOUCHER
    );
    assert_eq!(parsed.utf8, at_limit);
    let repeated = at_limit
        .replacen("<F0/>", "<F0/><F0/>", 1)
        .replacen("<E0/>", "<E0/><E0/>", 1);
    assert_eq!(
        parse_source_xml(repeated.as_bytes(), "x.xml".into())
            .unwrap()
            .vouchers[0]
            .omitted_fields
            .len(),
        MAX_OMITTED_FIELDS_PER_VOUCHER
    );
    for over_limit in [
        at_limit.replacen("<VOUCHER ", "<VOUCHER EXTRA=\"\" ", 1),
        at_limit.replacen("</VOUCHER>", "<EXTRA/></VOUCHER>", 1),
        at_limit.replacen(
            "</ALLLEDGERENTRIES.LIST>",
            "<EXTRA/></ALLLEDGERENTRIES.LIST>",
            1,
        ),
        at_limit.replacen(
            "<ALLLEDGERENTRIES.LIST>",
            "<ALLLEDGERENTRIES.LIST EXTRA=\"\">",
            1,
        ),
    ] {
        assert_eq!(
            parse_source_xml(over_limit.as_bytes(), "x.xml".into()),
            Err(SourceXmlError::OmittedFieldLimit)
        );
    }
}
