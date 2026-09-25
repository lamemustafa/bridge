use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, parse_native_ledger_source_records_with_evidence,
    parse_native_party_ledger_master_records_with_evidence, ExpectedTallyTextEncoding,
    NativeLedgerAmountError, ParsedSourceIdentityKind, PartyLedgerMasterFieldObservation,
};

const AARAV: &[u8] = include_bytes!("fixtures/native/ledgers_native_aarav.utf16le.xml");
const WR2: &[u8] = include_bytes!("fixtures/native/ledgers_native_wr2_core_window.utf16le.xml");
const BVL: &[u8] = include_bytes!("fixtures/native/ledgers_native_bvl.utf16le.xml");
const FOREX_LEDGERS: &[u8] = include_bytes!("fixtures/ledgers_currency_forex_live.utf16le.xml");
const MASTER_FIELDS_LAB: &str =
    include_str!("fixtures/native/ledgers_native_master_fields_lab.utf8.xml");
const MASTER_FIELDS_LAB_PARTIAL_ALTER_BEFORE: &str =
    include_str!("fixtures/native/master_fields_lab_partial_alter_before.response.xml");

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

fn captured_empty_tag<'a>(capture: &'a str, name: &str) -> &'a str {
    let tag = format!("<{name}/>");
    let start = capture
        .find(&tag)
        .expect("captured partial-master response carries the self-closing field");
    &capture[start..start + tag.len()]
}

fn native_party_master_collection(fields: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME=\"Bridge self-closing master field\"><GUID>56359347-3976-4d01-b44e-56fa0f6a422c-000000ce</GUID><BRIDGECOMPANYGUID>56359347-3976-4d01-b44e-56fa0f6a422c</BRIDGECOMPANYGUID><MASTERID>206</MASTERID><ALTERID>208</ALTERID><PARENT>Sundry Debtors</PARENT><OPENINGBALANCE>0.00</OPENINGBALANCE>{fields}</LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

#[test]
fn captured_native_ledgers_preserve_real_identity_book_openings_and_invalid_parent_reference() {
    let aarav = decode_utf16le(AARAV);
    let parsed = parse_native_ledger_source_records_with_evidence(
        &aarav,
        "bb8ad19e-6aef-4239-a917-87fec0c6215e",
    )
    .expect("captured native ledger collection parses");

    assert_eq!(parsed.records.len(), 88);
    assert_eq!(parsed.evidence.identified_record_count, 88);
    assert!(parsed.evidence.duplicate_identities.is_empty());
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 88);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 0);
    assert!(parsed.records.iter().all(|record| {
        record.identity_kind == Some(ParsedSourceIdentityKind::Guid)
            && record.identities.master_id.is_some()
            && record.alter_id.is_some()
            && record.raw_source_sha256.len() == 64
    }));
    assert!(parsed.records.iter().any(|record| {
        record.record.name == "Profit & Loss A/c"
            && record.record.parent.returned_text() == Some("\u{fffd}#4; Primary")
    }));

    let non_zero_openings = parsed
        .records
        .iter()
        .filter(|record| {
            record
                .record
                .opening_balance
                .as_deref()
                .is_some_and(|balance| balance != "0.00")
        })
        .map(|record| {
            (
                record.record.name.as_str(),
                record.record.opening_balance.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        non_zero_openings,
        [
            ("Capital Account - Arjun Mehta", Some("-800000.00")),
            ("HDFC Bank Current Account", Some("350000.00")),
            ("Petty Cash", Some("25000.00")),
        ]
    );
}

#[test]
fn captured_wr2_native_ledger_preserves_the_discriminating_signed_decimal() {
    let wr2 = decode_utf16le(WR2);
    let parsed = parse_native_ledger_source_records_with_evidence(
        &wr2,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect("captured native ledger collection parses");
    let row = parsed
        .records
        .iter()
        .find(|record| record.record.name == "Bridge Nested Debtor WR4")
        .expect("captured discriminating ledger is present");

    assert_eq!(
        row.record.parent.returned_text(),
        Some("Bridge Nested Debtors WR4")
    );
    assert_eq!(row.record.opening_balance.as_deref(), Some("-50000.00"));
    assert_eq!(row.identities.master_id.as_deref(), Some("213"));
    assert_eq!(row.alter_id.as_deref(), Some("215"));
    assert_eq!(row.identity_kind, Some(ParsedSourceIdentityKind::Guid));
}

#[test]
fn captured_master_fields_lab_preserves_contra_signed_party_openings() {
    let parsed = parse_native_ledger_source_records_with_evidence(
        MASTER_FIELDS_LAB,
        "56359347-3976-4d01-b44e-56fa0f6a422c",
    )
    .expect("captured master-fields lab collection parses");

    assert_eq!(parsed.records.len(), 17);
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 17);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 0);
    assert!(parsed.evidence.duplicate_identities.is_empty());

    let debtor = parsed
        .records
        .iter()
        .find(|row| row.record.name == "BRIDGE MFLAB DEBTOR CREDIT BALANCE")
        .expect("captured credit-balance debtor is present");
    assert_eq!(debtor.record.parent.returned_text(), Some("Sundry Debtors"));
    assert_eq!(debtor.record.opening_balance.as_deref(), Some("1250.00"));

    let creditor = parsed
        .records
        .iter()
        .find(|row| row.record.name == "BRIDGE MFLAB CREDITOR DEBIT BALANCE")
        .expect("captured debit-balance creditor is present");
    assert_eq!(
        creditor.record.parent.returned_text(),
        Some("Sundry Creditors")
    );
    assert_eq!(creditor.record.opening_balance.as_deref(), Some("-1250.00"));
}

#[test]
fn captured_self_closing_master_fields_are_returned_empty_and_mixed_duplicates_fail_closed() {
    // This is the real full-object response from the partial master Alter. Its
    // object envelope is not the `List of Ledgers` collection profile, so this
    // test retains its literal field bytes inside the parser's documented
    // collection envelope rather than treating the object response as a
    // collection capture.
    let email = captured_empty_tag(MASTER_FIELDS_LAB_PARTIAL_ALTER_BEFORE, "EMAIL");
    let state = captured_empty_tag(MASTER_FIELDS_LAB_PARTIAL_ALTER_BEFORE, "STATENAME");
    let ifsc = captured_empty_tag(MASTER_FIELDS_LAB_PARTIAL_ALTER_BEFORE, "IFSCODE");
    let pin_code = captured_empty_tag(MASTER_FIELDS_LAB_PARTIAL_ALTER_BEFORE, "LEDPINCODE");
    let fields = format!("{email}{state}{ifsc}{pin_code}");

    let parsed = parse_native_party_ledger_master_records_with_evidence(
        &native_party_master_collection(&fields),
        "56359347-3976-4d01-b44e-56fa0f6a422c",
    )
    .expect("captured self-closing master fields parse");
    assert_eq!(parsed.records.len(), 1, "one wrapped collection row parses");
    let row = &parsed.records[0].record.fields;
    assert_eq!(
        row.email,
        PartyLedgerMasterFieldObservation::Returned(String::new())
    );
    assert_eq!(
        row.state,
        PartyLedgerMasterFieldObservation::Returned(String::new())
    );
    assert_eq!(
        row.ifsc_code,
        PartyLedgerMasterFieldObservation::Returned(String::new())
    );
    assert_eq!(
        row.pin_code,
        PartyLedgerMasterFieldObservation::Returned(String::new())
    );

    let duplicate = native_party_master_collection(&format!("<EMAIL>x</EMAIL>{email}"));
    let error = parse_native_party_ledger_master_records_with_evidence(
        &duplicate,
        "56359347-3976-4d01-b44e-56fa0f6a422c",
    )
    .expect_err("mixed populated and self-closing EMAIL must not collapse");
    assert!(
        error
            .to_string()
            .contains("repeated a party/ledger master field"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn party_gstin_distinguishes_an_omitted_field_from_an_explicit_empty_field() {
    let omitted = parse_native_party_ledger_master_records_with_evidence(
        &native_party_master_collection(""),
        "56359347-3976-4d01-b44e-56fa0f6a422c",
    )
    .expect("captured-shape ledger without GSTIN parses");
    assert_eq!(
        omitted.records[0].record.ledger.party_gstin,
        PartyLedgerMasterFieldObservation::NotObserved
    );

    let explicitly_empty = parse_native_party_ledger_master_records_with_evidence(
        &native_party_master_collection("<PARTYGSTIN/>"),
        "56359347-3976-4d01-b44e-56fa0f6a422c",
    )
    .expect("captured-shape ledger with an empty GSTIN parses");
    assert_eq!(
        explicitly_empty.records[0].record.ledger.party_gstin,
        PartyLedgerMasterFieldObservation::Returned(String::new())
    );
}

#[test]
fn native_ledgers_fail_closed_when_opening_balance_or_company_prefix_is_absent() {
    let wr2 = decode_utf16le(WR2);
    let missing_balance = wr2.replace(
        "<OPENINGBALANCE TYPE=\"Amount\">-50000.00</OPENINGBALANCE>",
        "",
    );
    assert!(parse_native_ledger_source_records_with_evidence(
        &missing_balance,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .is_err());
    assert!(parse_native_ledger_source_records_with_evidence(
        &wr2,
        "00000000-0000-0000-0000-000000000000",
    )
    .is_err());
}

/// The one composite `OPENINGBALANCE` in the captured several-currency book,
/// read from its bytes rather than typed here (LEDGER_CURRENCY_CAPTURE_PROVENANCE).
fn captured_composite_opening() -> String {
    let forex = decode_utf16le(FOREX_LEDGERS);
    let openings = forex
        .split("<OPENINGBALANCE")
        .skip(1)
        .filter_map(|tail| {
            let text = &tail[tail.find('>')? + 1..tail.find("</OPENINGBALANCE>")?];
            text.contains(" @ ").then(|| text.to_string())
        })
        .collect::<Vec<_>>();
    assert_eq!(openings.len(), 1, "the capture carries exactly one composite opening");
    openings.into_iter().next().unwrap()
}

/// A foreign-currency opening still refuses the whole read, as any non-decimal
/// does, but now with a typed cause instead of an untyped parse error (#675).
/// The captured composite is placed in the captured basic-read row, since no
/// basic read of the several-currency book has been captured yet.
#[test]
fn a_foreign_currency_opening_refuses_with_its_typed_cause() {
    let wr2 = decode_utf16le(WR2);
    let row = "<OPENINGBALANCE TYPE=\"Amount\">-50000.00</OPENINGBALANCE>";
    assert_eq!(wr2.matches(row).count(), 1);
    let composite = captured_composite_opening();
    let foreign = wr2.replace(
        row,
        &format!("<OPENINGBALANCE TYPE=\"Amount\">{composite}</OPENINGBALANCE>"),
    );
    let error = parse_native_ledger_source_records_with_evidence(
        &foreign,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect_err("a composite opening is never read as a decimal");
    assert_eq!(
        error.downcast_ref::<NativeLedgerAmountError>(),
        Some(&NativeLedgerAmountError::ForeignCurrencyOpening),
        "{error:#}"
    );
    assert_eq!(
        NativeLedgerAmountError::ForeignCurrencyOpening.safe_code(),
        "foreign_currency_ledger_balance"
    );

    // Control: the same composite cut short before its base amount is not a
    // composite. It still refuses, untyped, as before.
    let cut = &composite[..composite.find(" = ").expect("captured composite has a base")];
    let truncated = wr2.replace(
        row,
        &format!("<OPENINGBALANCE TYPE=\"Amount\">{cut}</OPENINGBALANCE>"),
    );
    let error = parse_native_ledger_source_records_with_evidence(
        &truncated,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect_err("a truncated composite is still refused");
    assert!(error.downcast_ref::<NativeLedgerAmountError>().is_none(), "{error:#}");
}

/// `build_core_window` treats an explicitly empty parent as a root marker.
/// A response quietly dropping PARENT must not look identical to that
/// genuinely root-parented ledger, so the field must be observed, not merely
/// defaulted.
#[test]
fn native_ledger_row_omitting_parent_entirely_is_rejected() {
    let wr2 = decode_utf16le(WR2);
    let row_start = wr2
        .find(r#"<PARENT TYPE="String">Bridge Nested Debtors WR4</PARENT>"#)
        .expect("captured row carries the discriminating PARENT element");
    let row_end = row_start + r#"<PARENT TYPE="String">Bridge Nested Debtors WR4</PARENT>"#.len();
    let omitted_parent = format!("{}{}", &wr2[..row_start], &wr2[row_end..]);
    assert!(
        !omitted_parent.contains("Bridge Nested Debtors WR4"),
        "the removal must actually drop the PARENT element for this test to prove anything"
    );

    let error = parse_native_ledger_source_records_with_evidence(
        &omitted_parent,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect_err("a native ledger row that never sent PARENT must be rejected");
    assert!(
        error
            .to_string()
            .contains("native ledger row omitted PARENT"),
        "unexpected error: {error:#}"
    );
}

/// An explicitly EMPTY `PARENT` element is Tally's real shape for a
/// genuinely root-parented ledger (see `captured_aarav_native_master_parents_resolve_to_the_canonical_tree`
/// in `connector.rs`, e.g. "Profit & Loss A/c"). It must keep parsing to
/// `parent: Returned("")` and must not be confused with the omitted-field
/// case above, which is now rejected instead.
#[test]
fn native_ledger_row_with_an_explicitly_empty_parent_is_accepted_and_stays_rooted() {
    let wr2 = decode_utf16le(WR2);
    let empty_parent = wr2.replace(
        r#"<PARENT TYPE="String">Bridge Nested Debtors WR4</PARENT>"#,
        r#"<PARENT TYPE="String"></PARENT>"#,
    );
    assert_ne!(
        empty_parent, wr2,
        "the substitution must actually change the fixture for this test to prove anything"
    );

    let parsed = parse_native_ledger_source_records_with_evidence(
        &empty_parent,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect("a native ledger row with an explicitly empty PARENT must still parse");
    let row = parsed
        .records
        .iter()
        .find(|record| record.record.name == "Bridge Nested Debtor WR4")
        .expect("the edited row is still present");
    assert_eq!(
        row.record.parent,
        PartyLedgerMasterFieldObservation::Returned(String::new()),
        "an explicitly empty PARENT must remain a returned empty observation"
    );
    // The rest of the row must be untouched by the PARENT edit.
    assert_eq!(row.record.opening_balance.as_deref(), Some("-50000.00"));
    assert_eq!(row.identities.master_id.as_deref(), Some("213"));
}

#[test]
fn foreign_ledger_prefix_is_counted_without_rejecting_an_otherwise_bound_collection() {
    let aarav = decode_utf16le(AARAV);
    let mixed_prefix = aarav.replacen(
        "bb8ad19e-6aef-4239-a917-87fec0c6215e-00000107",
        "01234567-89ab-cdef-0123-456789abcdef-00000107",
        1,
    );
    let parsed = parse_native_ledger_source_records_with_evidence(
        &mixed_prefix,
        "bb8ad19e-6aef-4239-a917-87fec0c6215e",
    )
    .expect("one foreign master does not erase the response's company binding");
    assert_eq!(parsed.evidence.company_guid_prefix_match_count, 87);
    assert_eq!(parsed.evidence.company_guid_prefix_mismatch_count, 1);
}

#[test]
fn captured_bvl_native_ledgers_preserve_the_book_openings() {
    let bvl = decode_utf16le(BVL);
    let parsed = parse_native_ledger_source_records_with_evidence(
        &bvl,
        "c6afd306-00e1-4f51-802a-babe44daddd3",
    )
    .expect("captured native ledger collection parses");

    assert_eq!(parsed.records.len(), 13);
    assert_eq!(
        parsed
            .records
            .iter()
            .filter(|record| {
                record
                    .record
                    .opening_balance
                    .as_deref()
                    .is_some_and(|balance| balance != "0.00")
            })
            .count(),
        2
    );
}
