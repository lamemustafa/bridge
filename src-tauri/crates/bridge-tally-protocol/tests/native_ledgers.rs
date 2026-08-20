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
