use super::*;

fn envelope(allocation: &str, outstanding: &str, counts: (&str, &str)) -> String {
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><BILLSPARTYCONTEXT SCHEMA=\"{BILLS_OBSERVED_RAW_SCHEMA_V1}\" PROFILE=\"{BILLS_OBSERVED_RAW_PROFILE_V1}\" OBJECTTYPE=\"PARTYOUTSTANDING\" COMPANYGUID=\"synthetic-company-guid\" PARTYLEDGER=\"Synthetic Party\" FROMDATE=\"20260101\" TODATE=\"20260731\" ASOFDATE=\"20260731\" DIRECTION=\"RECEIVABLE\" QUERYPROFILE=\"bills-confidence-v1\" BILLWISESTATE=\"ENABLED\" ALLOCATIONCOUNT=\"{}\" OUTSTANDINGCOUNT=\"{}\"/>{allocation}{outstanding}</BODY></ENVELOPE>",
        counts.0, counts.1
    )
}

fn voucher_allocation(kind: &str, name: Option<&str>, amount: &str) -> String {
    let name = name.map_or(String::new(), |value| format!(" REFERENCENAME=\"{value}\""));
    format!(
        "<BILLALLOCATION ORIGIN=\"VOUCHER\" VOUCHERIDENTITY=\"synthetic-voucher-1\" PARTYENTRYORDINAL=\"1\" ROWORDINAL=\"1\" REFERENCEKIND=\"{kind}\"{name} BILLDATE=\"20260701\" DUEDATE=\"20260731\" AMOUNT=\"{amount}\" POLARITY=\"DEBIT\" CURRENCY=\"company-base\"/>"
    )
}

fn outstanding(kind: &str, name: Option<&str>, pending: &str) -> String {
    let name = name.map_or(String::new(), |value| format!(" REFERENCENAME=\"{value}\""));
    format!(
        "<BILLOUTSTANDING ROWORDINAL=\"1\" REFERENCEKIND=\"{kind}\"{name} BILLDATE=\"20260701\" DUEDATE=\"20260731\" OPENINGAMOUNT=\"-1000\" PENDINGAMOUNT=\"{pending}\" POLARITY=\"DEBIT\" CURRENCY=\"company-base\" OVERDUEDAYS=\"0\"/>"
    )
}

#[test]
fn parses_unbound_exact_scope_opening_optional_dates_and_on_account() {
    let xml = envelope(
        "<BILLALLOCATION ORIGIN=\"LEDGEROPENING\" ROWORDINAL=\"1\" REFERENCEKIND=\"On Account\" AMOUNT=\"-50.00\" POLARITY=\"DEBIT\" CURRENCY=\"company-base\"/>",
        &outstanding("On Account", None, "-50"),
        ("1", "1"),
    );
    let parsed = parse_unbound_party_outstanding_observation(
        xml.as_bytes(),
        BillsObservationLimits::default(),
    )
    .unwrap();
    assert_eq!(
        parsed.evidence().binding(),
        BillsObservationBinding::UnboundNoRequestArtifact
    );
    assert_eq!(
        parsed.allocations()[0].origin(),
        ParsedBillOrigin::LedgerOpening
    );
    assert_eq!(
        parsed.allocations()[0].reference_kind(),
        ParsedBillReferenceKind::OnAccount
    );
    assert!(parsed.allocations()[0].reference_name().is_none());
    assert_eq!(parsed.outstanding()[0].pending_amount(), "-50");
}

#[test]
fn preserves_unclassified_reference_without_coercing_it() {
    let xml = envelope(
        &voucher_allocation("Future Ref", Some("SYNTHETIC-1"), "-100"),
        &outstanding("Future Ref", Some("SYNTHETIC-1"), "-100"),
        ("1", "1"),
    );
    let parsed = parse_unbound_party_outstanding_observation(
        xml.as_bytes(),
        BillsObservationLimits::default(),
    )
    .unwrap();
    assert_eq!(
        parsed.allocations()[0].reference_kind(),
        ParsedBillReferenceKind::Unclassified
    );
    assert_eq!(parsed.allocations()[0].raw_reference_kind(), "Future Ref");
}

#[test]
fn exact_reference_types_partial_amounts_and_utf16_are_preserved() {
    for kind in ["Advance", "Agst Ref", "New Ref"] {
        let xml = envelope(
            &voucher_allocation(kind, Some("SYNTHETIC-1"), "-500.000"),
            &outstanding(kind, Some("SYNTHETIC-1"), "-500"),
            ("1", "1"),
        );
        let mut encoded = vec![0xff, 0xfe];
        encoded.extend(
            xml.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        let parsed =
            parse_unbound_party_outstanding_observation(encoded, BillsObservationLimits::default())
                .unwrap();
        assert_eq!(parsed.allocations()[0].amount(), "-500.000");
        assert_eq!(parsed.outstanding()[0].opening_amount(), Some("-1000"));
    }
}

#[test]
fn counts_scope_reference_rules_and_grammar_fail_closed() {
    let valid_allocation = voucher_allocation("New Ref", Some("SYNTHETIC-1"), "-100");
    let valid_outstanding = outstanding("New Ref", Some("SYNTHETIC-1"), "-100");
    for xml in [
        envelope(&valid_allocation, &valid_outstanding, ("2", "1")),
        envelope(
            &voucher_allocation("", Some("SYNTHETIC-1"), "-100"),
            &valid_outstanding,
            ("1", "1"),
        ),
        envelope(
            &voucher_allocation(" New Ref ", Some("SYNTHETIC-1"), "-100"),
            &valid_outstanding,
            ("1", "1"),
        ),
        envelope(
            &voucher_allocation("New Ref", None, "-100"),
            &valid_outstanding,
            ("1", "1"),
        ),
        envelope(
            &valid_allocation,
            "<BILLOUTSTANDING ROWORDINAL=\"1\" REFERENCEKIND=\"New Ref\" REFERENCENAME=\"SYNTHETIC-1\" PENDINGAMOUNT=\"1e2\"/>",
            ("1", "1"),
        ),
        envelope(
            &format!("{valid_allocation}{valid_allocation}"),
            &valid_outstanding,
            ("2", "1"),
        ),
        envelope(
            &valid_allocation,
            &valid_outstanding.replace("/>", " UNKNOWN=\"x\"/>"),
            ("1", "1"),
        ),
    ] {
        assert!(parse_unbound_party_outstanding_observation(
            xml.as_bytes(),
            BillsObservationLimits::default(),
        )
        .is_err());
    }
}

#[test]
fn resource_limits_and_errors_never_echo_sensitive_values() {
    let sentinel = "SENSITIVE-PARTY-AND-BILL";
    let xml = envelope(
        &voucher_allocation("New Ref", Some(sentinel), "-100"),
        &outstanding("New Ref", Some(sentinel), "-100"),
        ("1", "1"),
    );
    let limits = BillsObservationLimits {
        max_field_bytes: 8,
        ..BillsObservationLimits::default()
    };
    let error = parse_unbound_party_outstanding_observation(xml.as_bytes(), limits).unwrap_err();
    assert!(!format!("{error:?} {error}").contains(sentinel));

    let limits = BillsObservationLimits {
        max_records: 1,
        ..BillsObservationLimits::default()
    };
    assert_eq!(
        parse_unbound_party_outstanding_observation(xml.as_bytes(), limits).unwrap_err(),
        BillsObservationError::ResourceLimitExceeded
    );
}
