//! Captured voucher boundary and accounting-state regressions.
use super::*;

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
