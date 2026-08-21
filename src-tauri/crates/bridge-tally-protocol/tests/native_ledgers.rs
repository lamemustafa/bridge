use bridge_tally_protocol::{
    parse_native_ledger_source_records_with_evidence, ParsedSourceIdentityKind,
};

const AARAV: &str = include_str!("fixtures/native/ledgers_native_aarav.xml");
const WR2: &str = include_str!("fixtures/native/ledgers_native_wr2_core_window.xml");

#[test]
fn captured_native_ledgers_preserve_real_identity_signed_balances_and_invalid_parent_reference() {
    let parsed = parse_native_ledger_source_records_with_evidence(
        AARAV,
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
            && record.record.parent.as_deref() == Some("\u{fffd}#4; Primary")
    }));

    let negatives = parsed
        .records
        .iter()
        .filter(|record| {
            record
                .record
                .opening_balance
                .as_deref()
                .is_some_and(|balance| balance.starts_with('-'))
        })
        .count();
    assert_eq!(negatives, 30);
}

#[test]
fn captured_wr2_native_ledger_preserves_the_discriminating_signed_decimal() {
    let parsed = parse_native_ledger_source_records_with_evidence(
        WR2,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect("captured native ledger collection parses");
    let row = parsed
        .records
        .iter()
        .find(|record| record.record.name == "Bridge Nested Debtor WR4")
        .expect("captured discriminating ledger is present");

    assert_eq!(
        row.record.parent.as_deref(),
        Some("Bridge Nested Debtors WR4")
    );
    assert_eq!(row.record.opening_balance.as_deref(), Some("-50000.00"));
    assert_eq!(row.identities.master_id.as_deref(), Some("213"));
    assert_eq!(row.alter_id.as_deref(), Some("215"));
    assert_eq!(row.identity_kind, Some(ParsedSourceIdentityKind::Guid));
}

#[test]
fn native_ledgers_fail_closed_when_opening_balance_or_company_prefix_is_absent() {
    let missing_balance = WR2.replace(
        "<OPENINGBALANCE TYPE=\"Amount\">-50000.00</OPENINGBALANCE>",
        "",
    );
    assert!(parse_native_ledger_source_records_with_evidence(
        &missing_balance,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .is_err());
    assert!(parse_native_ledger_source_records_with_evidence(
        WR2,
        "00000000-0000-0000-0000-000000000000",
    )
    .is_err());
}

/// `build_core_window` reads `TallyLedger::parent == None` as "this ledger
/// sits at the tree root". Before the fix, a row that simply omitted PARENT
/// got that same `None` -- a response quietly dropping the field looked
/// identical to a genuinely root-parented ledger, silently corrupting the
/// hierarchy instead of failing. The field must now be observed, not merely
/// defaulted.
#[test]
fn native_ledger_row_omitting_parent_entirely_is_rejected() {
    let row_start = WR2
        .find(r#"<PARENT TYPE="String">Bridge Nested Debtors WR4</PARENT>"#)
        .expect("captured row carries the discriminating PARENT element");
    let row_end = row_start + r#"<PARENT TYPE="String">Bridge Nested Debtors WR4</PARENT>"#.len();
    let omitted_parent = format!("{}{}", &WR2[..row_start], &WR2[row_end..]);
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
/// `parent: None` and must not be confused with the omitted-field case
/// above, which is now rejected instead.
#[test]
fn native_ledger_row_with_an_explicitly_empty_parent_is_accepted_and_stays_rooted() {
    let empty_parent = WR2.replace(
        r#"<PARENT TYPE="String">Bridge Nested Debtors WR4</PARENT>"#,
        r#"<PARENT TYPE="String"></PARENT>"#,
    );
    assert_ne!(
        empty_parent, WR2,
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
        row.record.parent, None,
        "an explicitly empty PARENT must resolve to the same root-marking None as before"
    );
    // The rest of the row must be untouched by the PARENT edit.
    assert_eq!(row.record.opening_balance.as_deref(), Some("-50000.00"));
    assert_eq!(row.identities.master_id.as_deref(), Some("213"));
}

#[test]
fn foreign_ledger_prefix_is_counted_without_rejecting_an_otherwise_bound_collection() {
    let mixed_prefix = AARAV.replacen(
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
