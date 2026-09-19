use super::*;

const EXPECTED_COMPANY_GUID: &str = "11111111-1111-1111-1111-111111111111";

fn master_response(response_company_guid: Option<&str>) -> String {
    master_response_with_extra(response_company_guid, "")
}

/// Same required-field skeleton as [`master_response`], with an extra child
/// element (e.g. a `NAMEONPAN`) spliced into the row so the full
/// sanitize-then-parse pipeline -- including
/// `tolerant_xml::sanitize_invalid_numeric_references_with_provenance` -- can
/// be exercised for a specific retained field, not just the unit-level
/// reader functions.
fn master_response_with_extra(response_company_guid: Option<&str>, extra: &str) -> String {
    let response_company_guid = response_company_guid
        .map(|guid| format!("<BRIDGECOMPANYGUID>{guid}</BRIDGECOMPANYGUID>"))
        .unwrap_or_default();
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
            <LEDGER NAME=\"Imported selected ledger\"><GUID>{EXPECTED_COMPANY_GUID}-00000001</GUID>\
            <MASTERID>1</MASTERID><ALTERID>1</ALTERID>{response_company_guid}\
            <PARENT>Sundry Debtors</PARENT><OPENINGBALANCE>-100.00</OPENINGBALANCE>{extra}</LEDGER>\
            </COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

/// Runs [`read_flattened_optional_text`] over `inner`, which must be exactly
/// the content of one `<FIELD>...</FIELD>` element (no surrounding markup).
/// Mirrors `configured_reader`'s `trim_text(true)`, matching how this reader
/// is actually configured for the whole document in production.
fn run_flattened(tag: &str, inner: &str) -> anyhow::Result<Option<String>> {
    let xml = format!("<{tag}>{inner}</{tag}>");
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    match reader.read_event()? {
        Event::Start(start) => read_flattened_optional_text(&mut reader, start.name()),
        other => panic!("expected a Start event, got {other:?}"),
    }
}

/// Runs [`read_scalar_rejecting_nested_markup`] the same way [`run_flattened`]
/// runs the flattening reader.
fn run_scalar(tag: &str, inner: &str) -> anyhow::Result<Option<String>> {
    let xml = format!("<{tag}>{inner}</{tag}>");
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(true);
    match reader.read_event()? {
        Event::Start(start) => read_scalar_rejecting_nested_markup(&mut reader, start.name()),
        other => panic!("expected a Start event, got {other:?}"),
    }
}

#[test]
fn party_ledger_master_requires_a_response_bound_company_guid() {
    let wrong_company = parse_native_party_ledger_master_records_with_evidence(
        &master_response(Some("22222222-2222-2222-2222-222222222222")),
        EXPECTED_COMPANY_GUID,
    );
    assert!(
        wrong_company.is_err(),
        "an imported selected-prefix ledger cannot prove the responding company"
    );

    let missing_company = parse_native_party_ledger_master_records_with_evidence(
        &master_response(None),
        EXPECTED_COMPANY_GUID,
    );
    assert!(
        missing_company.is_err(),
        "the dedicated master response must carry Tally's computed company GUID"
    );

    assert_eq!(
        parse_native_party_ledger_master_records_with_evidence(
            &master_response(Some(EXPECTED_COMPANY_GUID)),
            EXPECTED_COMPANY_GUID,
        )
        .unwrap()
        .records
        .len(),
        1,
        "a matching response-bound company GUID admits the master response"
    );
}

// ---------------------------------------------------------------------------
// GeneralRef handling for `read_flattened_optional_text` and
// `read_scalar_rejecting_nested_markup`.
//
// quick_xml 0.41 delivers an entity or character reference (`&amp;`,
// `&#8377;`, ...) as its own `Event::GeneralRef`, separate from the
// surrounding `Text`/`CData` events. Before this fix, the catch-all `_ => {}`
// arm silently dropped that event, and every other Text/CData fragment was
// pushed as its own entry in `parts`, joined with `\n` -- so a single value
// merely split by a reference came back as multiple newline-joined pieces
// with the referenced character missing entirely. These names are invented
// for the test ("RAM & SONS"), not drawn from any real client's data.
// ---------------------------------------------------------------------------

#[test]
fn flattened_field_resolves_named_entity_references_without_inserting_newlines() {
    let value = run_flattened("NAMEONPAN", "RAM &amp; SONS")
        .expect("named entity reference resolves")
        .expect("field carried text");
    assert_eq!(
        value, "RAM & SONS",
        "a value merely split by a named-entity reference is one line, not two"
    );
}

#[test]
fn flattened_field_resolves_every_predefined_named_entity() {
    let value = run_flattened("NAMEONPAN", "&lt;&gt;&amp;&quot;&apos;")
        .expect("all five predefined entities resolve")
        .expect("field carried text");
    assert_eq!(value, "<>&\"'");
}

#[test]
fn flattened_field_resolves_legal_decimal_and_hex_numeric_references() {
    // U+20B9 (RUPEE SIGN) is legal in both forms; used here only as a
    // convenient legal code point, not to imply anything about currency.
    let decimal = run_flattened("NAMEONPAN", "RAM &#8377; SONS")
        .expect("legal decimal numeric reference resolves")
        .expect("field carried text");
    assert_eq!(decimal, "RAM \u{20B9} SONS");

    let hex = run_flattened("NAMEONPAN", "RAM &#x20B9; SONS")
        .expect("legal hex numeric reference resolves")
        .expect("field carried text");
    assert_eq!(hex, "RAM \u{20B9} SONS");
}

#[test]
fn flattened_field_rejoins_text_split_across_cdata_without_a_newline() {
    let value = run_flattened("BANKDETAILS", "-101<![CDATA[.]]>01")
        .expect("a value split across Text and CDATA still parses")
        .expect("field carried text");
    assert_eq!(
        value, "-101.01",
        "a value split by CDATA is one token, not two newline-joined pieces"
    );
}

#[test]
fn flattened_field_preserves_whitespace_either_side_of_a_reference() {
    // Regression for a subtler failure mode than the dropped character: the
    // shared reader has `trim_text(true)` set for the whole document (see
    // `configured_reader`), and quick_xml applies that trim independently to
    // each `Text` event -- including the fragments immediately before and
    // after a `GeneralRef`. Naively concatenating resolved fragments without
    // accounting for this would silently lose the spaces around `&`.
    let value = run_flattened("NAMEONPAN", "RAM &amp; SONS &#8377; X")
        .expect("mixed named and numeric references resolve")
        .expect("field carried text");
    assert_eq!(value, "RAM & SONS \u{20B9} X");
}

#[test]
fn flattened_field_still_joins_genuinely_separate_child_lines() {
    // `LEDADDRESS.LIST` carries one real line of the address per child
    // `<LEDADDRESS>` element -- the one case where joining with `\n` is
    // correct and must be kept.
    let value = run_flattened(
        "LEDADDRESS.LIST",
        "<LEDADDRESS>221B Baker Street</LEDADDRESS><LEDADDRESS>Some Town</LEDADDRESS>",
    )
    .expect("a multi-line address parses")
    .expect("field carried text");
    assert_eq!(value, "221B Baker Street\nSome Town");
}

#[test]
fn flattened_field_does_not_split_a_referenced_line_from_its_own_text() {
    // A reference inside one `<LEDADDRESS>` line must stay on that line, not
    // become its own entry joined by `\n` to the rest of the same line.
    let value = run_flattened(
        "LEDADDRESS.LIST",
        "<LEDADDRESS>RAM &amp; SONS</LEDADDRESS><LEDADDRESS>Some Town</LEDADDRESS>",
    )
    .expect("a multi-line address with a reference in one line parses")
    .expect("field carried text");
    assert_eq!(value, "RAM & SONS\nSome Town");
}

#[test]
fn scalar_field_resolves_references_the_same_way_as_the_flattening_reader() {
    let value = run_scalar("GSTDUTYHEAD", "CGST &amp; SGST")
        .expect("a reference in a rejecting-nested-markup scalar still resolves")
        .expect("field carried text");
    assert_eq!(value, "CGST & SGST");
}

#[test]
fn scalar_field_still_rejects_genuine_nested_markup() {
    let err = run_scalar("GSTDUTYHEAD", "<VALUE>CGST</VALUE>")
        .expect_err("nested markup under a rejecting scalar must still fail closed");
    assert!(
        err.to_string().contains("nested markup"),
        "unexpected error for nested markup: {err}"
    );
}

#[test]
fn party_ledger_master_pipeline_resolves_a_reference_end_to_end() {
    // Exercises the full pipeline used in production: sanitize (marks any
    // forbidden numeric reference before quick_xml ever sees it), then parse.
    // "RAM & SONS" is an invented name for this test, not a real client's.
    let xml = master_response_with_extra(
        Some(EXPECTED_COMPANY_GUID),
        "<NAMEONPAN>RAM &amp; SONS</NAMEONPAN>",
    );
    let parsed =
        parse_native_party_ledger_master_records_with_evidence(&xml, EXPECTED_COMPANY_GUID)
            .expect("a legal entity reference must not fail the whole response");
    let record = &parsed.records[0].record;
    assert_eq!(
        record.fields.name_on_pan,
        PartyLedgerMasterFieldObservation::Returned("RAM & SONS".to_string()),
        "NAMEONPAN must round-trip the ampersand, not drop it and split the value"
    );
}

#[test]
fn party_ledger_master_pipeline_preserves_a_forbidden_reference_marker_as_literal_text() {
    // `tolerant_xml::mark_forbidden_numeric_references` (TALLY_PROTOCOL_REFERENCE.md
    // section 1.1(d)) rewrites a forbidden numeric reference like `&#4;` into
    // the literal marker text `U+FFFD#4;` before quick_xml ever parses the
    // document, so by the time this field's own reader runs, the marker is
    // ordinary text, not a `GeneralRef` event. This proves the two do not
    // collide: the marker is retained byte-for-byte, not mistaken for a
    // reference and not corrupted by the fix in this file.
    let xml = master_response_with_extra(
        Some(EXPECTED_COMPANY_GUID),
        "<NAMEONPAN>&#4; Primary</NAMEONPAN>",
    );
    let parsed =
        parse_native_party_ledger_master_records_with_evidence(&xml, EXPECTED_COMPANY_GUID)
            .expect("a forbidden numeric reference must be sanitized, not fail the response");
    let record = &parsed.records[0].record;
    assert_eq!(
        record.fields.name_on_pan,
        PartyLedgerMasterFieldObservation::Returned("\u{FFFD}#4; Primary".to_string()),
        "the forbidden-reference marker must survive as literal text"
    );
}
