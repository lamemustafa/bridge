//! Captured voucher boundary and accounting-state regressions.
use super::*;

fn captured_native_vouchers() -> String {
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
fn contradictory_signed_amounts_are_refused_before_accounting_math() {
    let captured = captured_native_vouchers();
    for (original, replacement) in [("-101.01", "101.01"), ("101.01", "-101.01")] {
        let damaged = captured.replacen(
            &format!("<AMOUNT TYPE=\"Amount\">{original}</AMOUNT>"),
            &format!("<AMOUNT TYPE=\"Amount\">{replacement}</AMOUNT>"),
            1,
        );
        assert_ne!(damaged, captured);
        for accounting_state in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(&damaged, accounting_state),
                Err("voucher_entry_polarity_mismatch".into())
            );
        }
    }
    for zero in ["0", "0.00", "-0.000"] {
        for polarity in [false, true] {
            assert_eq!(
                validate_tally_entry_polarity(
                    &bridge_tally_core::ExactDecimal::parse(zero.to_string()).unwrap(),
                    polarity
                ),
                Ok(())
            );
        }
    }
}

#[test]
fn numeric_voucher_ids_distinguish_absence_from_invalid_observations() {
    let captured = captured_native_vouchers();
    for (field, code) in [
        ("ALTERID", "voucher_alter_id_invalid"),
        ("MASTERID", "voucher_master_id_invalid"),
    ] {
        let original = format!("<{field} TYPE=\"Number\"> 1</{field}>");
        assert!(captured.contains(&original));
        for invalid in ["Maybe", "-1", "+1", "1.1", "18446744073709551616", " "] {
            let damaged = captured.replacen(&original, &format!("<{field}>{invalid}</{field}>"), 1);
            assert_eq!(parse_agent_rows(&damaged), Err(code.into()));
        }
        for replacement in [format!("<{field}/>"), format!("<{field}></{field}>")] {
            let damaged = captured.replacen(&original, &replacement, 1);
            assert_eq!(parse_agent_rows(&damaged), Err(code.into()));
        }
        let absent = captured.replacen(&original, "", 1);
        let rows = parse_agent_rows(&absent).unwrap();
        let output = if field == "ALTERID" {
            "alter_id"
        } else {
            "master_id"
        };
        assert!(rows[0][output].is_null());
    }
    let rows = parse_agent_rows(&captured).unwrap();
    assert_eq!(rows[0]["alter_id"], 1);
    assert_eq!(rows[0]["master_id"], " 1");
    assert_eq!(parse_optional_tally_u64(None, "invalid"), Ok(None));
    assert_eq!(
        parse_optional_tally_u64(Some(" 0001 "), "invalid"),
        Ok(Some(1))
    );
    assert_eq!(
        parse_optional_tally_u64(Some("18446744073709551615"), "invalid"),
        Ok(Some(u64::MAX))
    );
}

#[test]
fn ordinary_vouchers_preserve_captured_nonposting_state_and_reject_unknown_flags() {
    let optional_capture = include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/unit_a_optional_voucher_live.xml"
    );
    let ordinary = parse_agent_rows(optional_capture).unwrap();
    let accounting = parse_agent_changed_rows(optional_capture).unwrap();
    assert_eq!(ordinary, accounting);
    assert!(ordinary.iter().any(|row| row["optional"] == true));
    assert!(ordinary
        .iter()
        .all(|row| row["cancelled"].is_boolean() && row["optional"].is_boolean()));

    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    for field in ["ISCANCELLED", "ISOPTIONAL"] {
        let original = format!("<{field} TYPE=\"Logical\">No</{field}>");
        for replacement in [
            String::new(),
            format!("<{field}/>"),
            format!("<{field}>Maybe</{field}>"),
        ] {
            let damaged = captured.replacen(&original, &replacement, 1);
            assert_ne!(damaged, captured);
            assert_eq!(
                parse_agent_rows(&damaged),
                Err("voucher_accounting_state_not_observed".into())
            );
        }
        // Simulate a flag transition in captured bytes; neither path may
        // drop the row or conceal its non-posting state.
        let changed = captured.replacen(&original, &format!("<{field}>Yes</{field}>"), 1);
        let ordinary = parse_agent_rows(&changed).unwrap();
        assert_eq!(ordinary, parse_agent_changed_rows(&changed).unwrap());
        assert_eq!(ordinary.len(), 3);
        let flag = if field == "ISCANCELLED" {
            "cancelled"
        } else {
            "optional"
        };
        assert_eq!(ordinary[0][flag], true);
    }
}

#[test]
fn invalid_calendar_dates_in_captured_vouchers_are_refused_before_filtering() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    for invalid in ["2026080A", "20260230", "20261301"] {
        let damaged = captured.replacen("20260801", invalid, 1);
        assert_ne!(damaged, captured);
        for accounting_state in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(&damaged, accounting_state),
                Err("voucher_date_invalid".to_string())
            );
        }
    }
}

#[test]
fn malformed_polarity_in_captured_voucher_is_refused_at_the_parse_boundary() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    let rows = parse_agent_rows(&captured).unwrap();
    assert_eq!(rows[0]["amounts"][0]["is_deemed_positive"], "Yes");
    assert_eq!(rows[0]["amounts"][1]["is_deemed_positive"], "No");
    for invalid in ["Maybe", "true", "1"] {
        // Mutate only the captured ledger-entry polarity for negative testing.
        let damaged = captured.replacen(
            "\n      <ISDEEMEDPOSITIVE TYPE=\"Logical\">Yes</ISDEEMEDPOSITIVE>",
            &format!("\n      <ISDEEMEDPOSITIVE TYPE=\"Logical\">{invalid}</ISDEEMEDPOSITIVE>"),
            1,
        );
        assert_ne!(damaged, captured);
        for accounting_state in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(&damaged, accounting_state),
                Err("voucher_accounting_state_not_observed".to_string())
            );
        }
    }
}

#[test]
fn malformed_amount_in_captured_voucher_is_refused_at_the_parse_boundary() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    let rows = parse_agent_rows(&captured).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["amounts"][0]["amount"], "-101.01");
    for invalid in ["not-observed", "NaN", "101.01 INR", "1.2.3"] {
        // Negative fault injection into a captured response, not a new
        // fixture or evidence of a live Tally response shape.
        let damaged = captured.replacen(
            "<AMOUNT TYPE=\"Amount\">-101.01</AMOUNT>",
            &format!("<AMOUNT TYPE=\"Amount\">{invalid}</AMOUNT>"),
            1,
        );
        assert_ne!(damaged, captured);
        for accounting_state in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(&damaged, accounting_state),
                Err("voucher_amount_invalid".to_string()),
                "{invalid} must not be released as complete accounting evidence"
            );
        }
    }
}

#[test]
fn repeated_scalar_elements_cannot_be_concatenated_into_valid_observations() {
    let captured = captured_native_vouchers();
    for (field, entry) in [
        ("DATE", false),
        ("VOUCHERTYPENAME", false),
        ("VOUCHERNUMBER", false),
        ("PARTYLEDGERNAME", false),
        ("NARRATION", false),
        ("GUID", false),
        ("ALTERID", false),
        ("MASTERID", false),
        ("ISCANCELLED", false),
        ("ISOPTIONAL", false),
        ("LEDGERNAME", true),
        ("AMOUNT", true),
        ("ISDEEMEDPOSITIVE", true),
    ] {
        let offset = if entry {
            captured.find("<ALLLEDGERENTRIES.LIST>").unwrap()
        } else {
            captured.find("<VOUCHER ").unwrap()
        };
        let start = offset
            + [format!("<{field} "), format!("<{field}>")]
                .iter()
                .filter_map(|tag| captured[offset..].find(tag))
                .min()
                .unwrap();
        let end = start + captured[start..].find(&format!("</{field}>")).unwrap() + field.len() + 3;
        let original = &captured[start..end];
        for replacement in [
            format!("{original}{original}"),
            format!("<{field}/>{original}"),
        ] {
            let mut damaged = captured.clone();
            damaged.replace_range(start..end, &replacement);
            for accounting_state in [false, true] {
                assert_eq!(
                    parse_agent_rows_with_accounting_state(&damaged, accounting_state),
                    Err("agent_read_protocol_invalid".into()),
                    "{field}"
                );
            }
        }
    }
    // Multiple events inside one element are still a single observation.
    let fragmented = captured.replacen(
        "<AMOUNT TYPE=\"Amount\">-101.01</AMOUNT>",
        "<AMOUNT TYPE=\"Amount\">-101&#46;01</AMOUNT>",
        1,
    );
    assert_ne!(fragmented, captured);
    assert_eq!(parse_agent_rows(&fragmented), parse_agent_rows(&captured));
}

#[test]
fn multiple_native_collections_are_refused_instead_of_merged() {
    let captured = captured_native_vouchers();
    let start = captured.find("<COLLECTION").unwrap();
    let end = captured.find("</COLLECTION>").unwrap() + "</COLLECTION>".len();
    let repeated = format!(
        "{}{}{}",
        &captured[..end],
        &captured[start..end],
        &captured[end..]
    );
    for accounting_state in [false, true] {
        assert_eq!(
            parse_agent_rows_with_accounting_state(&repeated, accounting_state),
            Err("agent_read_protocol_invalid".into())
        );
    }
}

#[test]
fn scalar_content_preserves_cdata_and_rejects_nested_markup() {
    let captured = captured_native_vouchers();
    let original = "<AMOUNT TYPE=\"Amount\">-101.01</AMOUNT>";
    for value in ["-101<![CDATA[.]]>01", "<![CDATA[-101.01]]>"] {
        let fragmented = captured.replacen(original, &format!("<AMOUNT>{value}</AMOUNT>"), 1);
        assert_eq!(parse_agent_rows(&fragmented), parse_agent_rows(&captured));
    }
    let malformed = captured.replacen(original, "<AMOUNT>-101<![CDATA[.99]]>.01</AMOUNT>", 1);
    assert_eq!(
        parse_agent_rows(&malformed),
        Err("voucher_amount_invalid".into())
    );
    for value in ["-101<NESTED>99</NESTED>.01", "-101<NESTED/>.01"] {
        let nested = captured.replacen(original, &format!("<AMOUNT>{value}</AMOUNT>"), 1);
        assert_eq!(
            parse_agent_rows(&nested),
            Err("agent_read_protocol_invalid".into())
        );
    }
}
