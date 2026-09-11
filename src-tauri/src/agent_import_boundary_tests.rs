use super::*;

pub(super) fn captured_vouchers() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn import_boundary_rejects_malformed_accounting_scalars_in_captured_vouchers() {
    let captured = captured_vouchers();
    let baseline = parse_import_vouchers(&captured, CAPTURED_GUID).unwrap();
    assert_eq!(baseline.rows.len(), 3);
    let escaped_flags = captured
        .replace(">No</ISCANCELLED>", ">N&#111;</ISCANCELLED>")
        .replace(">No</ISOPTIONAL>", ">N&#111;</ISOPTIONAL>")
        .replace(">Yes</ISDEEMEDPOSITIVE>", "> Y&#101;s </ISDEEMEDPOSITIVE>");
    assert_eq!(
        parse_import_vouchers(&escaped_flags, CAPTURED_GUID).unwrap(),
        baseline
    );
    // Fault injection into existing captured responses, not new Tally fixtures.
    //
    // Each case carries its own needle rather than a `>value</TAG>` pattern built
    // from the field name. The captured bill allocation repeats its ledger entry's
    // amount verbatim -- an allocation covers the entry -- so a needle that ignores
    // the opening tag corrupts BOTH and the bill-allocation parse, which runs
    // first, answers instead. The case then asserts which layer speaks first
    // rather than that a malformed entry amount is refused. The entry element
    // carries `TYPE="Amount"` and the allocation's does not, so they separate
    // cleanly, and both layers are now asserted.
    let entry_amount = baseline.rows[0].entries[0].amount.as_str();
    for (label, needle, replacement, code) in [
        (
            "ISDEEMEDPOSITIVE",
            ">Yes</ISDEEMEDPOSITIVE>".to_string(),
            ">Maybe</ISDEEMEDPOSITIVE>".to_string(),
            "import_verification_export_invalid",
        ),
        (
            "ISCANCELLED",
            ">No</ISCANCELLED>".to_string(),
            ">Maybe</ISCANCELLED>".to_string(),
            "import_verification_export_invalid",
        ),
        (
            "ISOPTIONAL",
            ">No</ISOPTIONAL>".to_string(),
            ">Maybe</ISOPTIONAL>".to_string(),
            "import_verification_export_invalid",
        ),
        (
            "DATE",
            format!(">{}</DATE>", baseline.rows[0].date.as_deref().unwrap()),
            ">20260230</DATE>".to_string(),
            "import_verification_export_invalid",
        ),
        (
            "entry AMOUNT",
            format!("<AMOUNT TYPE=\"Amount\">{entry_amount}</AMOUNT>"),
            "<AMOUNT TYPE=\"Amount\">not-a-number</AMOUNT>".to_string(),
            "import_verification_amount_invalid",
        ),
        (
            "bill allocation AMOUNT",
            format!("<AMOUNT>{entry_amount}</AMOUNT>"),
            "<AMOUNT>not-a-number</AMOUNT>".to_string(),
            "bill_allocation_amount_invalid",
        ),
    ] {
        let invalid = captured.replace(&needle, &replacement);
        assert_ne!(invalid, captured, "fault injection for {label}");
        assert_eq!(
            parse_import_vouchers(&invalid, CAPTURED_GUID),
            Err(code.to_string()),
            "{label}"
        );
    }
}
