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
    for (field, original, invalid, code) in [
        (
            "ISDEEMEDPOSITIVE",
            "Yes",
            "Maybe",
            "import_verification_export_invalid",
        ),
        (
            "ISCANCELLED",
            "No",
            "Maybe",
            "import_verification_export_invalid",
        ),
        (
            "ISOPTIONAL",
            "No",
            "Maybe",
            "import_verification_export_invalid",
        ),
        (
            "DATE",
            baseline.rows[0].date.as_deref().unwrap(),
            "20260230",
            "import_verification_export_invalid",
        ),
        (
            "AMOUNT",
            baseline.rows[0].entries[0].amount.as_str(),
            "not-a-number",
            "import_verification_amount_invalid",
        ),
    ] {
        let invalid = captured.replace(
            &format!(">{original}</{field}>"),
            &format!(">{invalid}</{field}>"),
        );
        assert_ne!(invalid, captured, "fault injection for {field}");
        assert_eq!(
            parse_import_vouchers(&invalid, CAPTURED_GUID),
            Err(code.to_string()),
            "{field}"
        );
    }
}
