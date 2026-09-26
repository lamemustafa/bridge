//! Captured voucher boundary and accounting-state regressions.
use super::*;

const CAPTURED_VOUCHER_COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const CAPTURED_BILL_ALLOCATION_COMPANY_GUID: &str = "74d7e825-396a-4667-90b2-83f593f06a36";

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

fn captured_bill_allocation_vouchers() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/vouchers_agst_ref_reopen_live.utf16le.xml"
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
fn captured_bill_allocations_preserve_raw_fields_and_empty_entries() {
    let captured = captured_bill_allocation_vouchers();
    let rows = parse_agent_rows(&captured, CAPTURED_BILL_ALLOCATION_COMPANY_GUID).unwrap();
    assert_eq!(
        rows[0]["amounts"][0]["bill_allocations"],
        json!([{
            "reference": {"kind": "named", "name": "SET-INV-001"},
            "bill_type": "New Ref",
            "amount": "-1137.50"
        }])
    );
    assert_eq!(rows[0]["amounts"][1]["bill_allocations"], json!([]));

    let padded_reference = captured.replacen(
        "<NAME>SET-INV-001</NAME>",
        "<NAME>  SET-INV-001  </NAME>",
        1,
    );
    let padded =
        parse_agent_rows(&padded_reference, CAPTURED_BILL_ALLOCATION_COMPANY_GUID).unwrap();
    assert_eq!(
        padded[0]["amounts"][0]["bill_allocations"][0]["reference"]["name"],
        "  SET-INV-001  "
    );
}

#[test]
fn on_account_bill_allocation_with_empty_name_is_explicitly_unnamed() {
    let captured = captured_bill_allocation_vouchers()
        .replacen("<NAME>SET-INV-001</NAME>", "<NAME></NAME>", 1)
        .replacen(
            "<BILLTYPE>New Ref</BILLTYPE>",
            "<BILLTYPE>On Account</BILLTYPE>",
            1,
        );
    let rows = parse_agent_rows(&captured, CAPTURED_BILL_ALLOCATION_COMPANY_GUID)
        .expect("an unnamed On Account allocation must not abort its voucher read");

    let allocation = &rows[0]["amounts"][0]["bill_allocations"][0];
    assert_eq!(allocation["reference"], json!({"kind": "on_account"}));
    assert!(
        allocation.get("name").is_none(),
        "On Account must not be represented by an empty or placeholder name"
    );
}

#[test]
fn reference_bearing_bill_allocation_with_empty_name_fails_closed() {
    let captured = captured_bill_allocation_vouchers().replacen(
        "<NAME>SET-INV-001</NAME>",
        "<NAME></NAME>",
        1,
    );
    assert_eq!(
        parse_agent_rows(&captured, CAPTURED_BILL_ALLOCATION_COMPANY_GUID),
        Err("bill_allocation_field_missing".into())
    );
}

const WILDCARD_ALLOCATION_COMPANY_GUID: &str = "ae1490be-52c5-4544-9ffc-4b7da85f9797";

fn captured_wildcard_allocation_vouchers() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-billallocations-wildcard.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn captured_entry_wildcard_vouchers() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-entry-wildcard-allocations.utf16le.xml"
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
fn entry_wildcard_response_parses_with_its_twenty_seven_nested_lists() {
    // ALLLEDGERENTRIES.* returns 27 nested *.LIST types per entry, including
    // TAXBILLALLOCATIONS.LIST -- a DIFFERENT list that a loose scope match could
    // confuse with BILLALLOCATIONS.LIST. This is the shape the profile requests,
    // so the parser has to survive all of it and still report allocations exactly.
    let captured = captured_entry_wildcard_vouchers();
    let rows = parse_agent_rows(&captured, WILDCARD_ALLOCATION_COMPANY_GUID)
        .expect("the entry wildcard response must parse");

    let allocations: Vec<&serde_json::Value> = rows
        .iter()
        .flat_map(|row| row["amounts"].as_array().unwrap())
        .flat_map(|amount| amount["bill_allocations"].as_array().unwrap())
        .collect();
    assert!(
        allocations
            .iter()
            .any(|a| a["bill_type"] == "On Account"
                && a["reference"] == json!({"kind": "on_account"})),
        "On Account must arrive typed and explicitly unnamed"
    );
    assert!(
        allocations
            .iter()
            .any(|a| a["bill_type"] == "New Ref" && a["reference"]["kind"] == "named"),
        "a reference-bearing allocation must keep its name"
    );
}

#[test]
fn allocation_wildcard_response_parses_and_types_on_account() {
    // Captured live from TallyPrime 7.1 Silver with
    // ALLLEDGERENTRIES.BILLALLOCATIONS.* -- the shape this profile now requests.
    // Curating NAME/BILLTYPE/AMOUNT instead DROPS BILLTYPE on On Account
    // allocations, so they arrive as amount-only placeholders and are skipped,
    // losing real allocations silently. This fixture is the proof that the
    // wildcard restores the type, and that the extra sibling elements Tally
    // returns inside the allocation do not disturb the parse.
    let captured = captured_wildcard_allocation_vouchers();
    let rows = parse_agent_rows(&captured, WILDCARD_ALLOCATION_COMPANY_GUID)
        .expect("the allocation wildcard response must parse");

    let allocations: Vec<&serde_json::Value> = rows
        .iter()
        .flat_map(|row| row["amounts"].as_array().unwrap())
        .flat_map(|amount| amount["bill_allocations"].as_array().unwrap())
        .collect();

    assert!(
        allocations
            .iter()
            .any(|allocation| allocation["bill_type"] == "On Account"
                && allocation["reference"] == json!({"kind": "on_account"})),
        "On Account must arrive typed and explicitly unnamed, not as a placeholder"
    );
    assert!(
        allocations
            .iter()
            .any(|allocation| allocation["bill_type"] == "New Ref"
                && allocation["reference"]["kind"] == "named"),
        "a reference-bearing allocation must keep its name"
    );
}

#[test]
fn amount_only_bill_allocation_placeholder_is_ignored_not_refused() {
    // Tally emits an amount-only container for a ledger entry with no typed
    // allocation. It is not empty, so it reaches the field checks; requiring
    // BILLTYPE unconditionally aborted the ENTIRE vouchers read over a row that
    // carries no bill identity to record.
    let captured = captured_bill_allocation_vouchers();
    let start = captured.find("<BILLALLOCATIONS.LIST>").unwrap();
    let end = start
        + captured[start..].find("</BILLALLOCATIONS.LIST>").unwrap()
        + "</BILLALLOCATIONS.LIST>".len();
    let mut placeholder = captured.clone();
    placeholder.replace_range(
        start..end,
        "<BILLALLOCATIONS.LIST><AMOUNT>-1137.50</AMOUNT></BILLALLOCATIONS.LIST>",
    );

    let rows = parse_agent_rows(&placeholder, CAPTURED_BILL_ALLOCATION_COMPANY_GUID)
        .expect("an amount-only placeholder must not abort the voucher read");

    assert_eq!(
        rows[0]["amounts"][0]["bill_allocations"],
        json!([]),
        "the placeholder carries no allocation, so none is reported -- and it is \
         skipped rather than invented"
    );
    // The rest of the read must be intact: skipping the row has to fall through
    // to the scope bookkeeping, or every later element is mis-attributed.
    assert_eq!(
        rows.len(),
        parse_agent_rows(&captured, CAPTURED_BILL_ALLOCATION_COMPANY_GUID)
            .unwrap()
            .len()
    );
    assert_eq!(rows[0]["amounts"][1]["bill_allocations"], json!([]));
}

#[test]
fn on_account_carrying_a_name_is_refused_not_silently_unnamed() {
    // On Account cannot carry a bill identity. A NAME alongside it is a
    // contradiction, and dropping it loses a supplier reference that a malformed
    // response -- or a request-shape regression -- is trying to report. The typed
    // outstandings boundary refuses the same state; this one did not.
    let captured = captured_bill_allocation_vouchers().replacen(
        "<BILLTYPE>New Ref</BILLTYPE>",
        "<BILLTYPE>On Account</BILLTYPE>",
        1,
    );
    assert!(captured.contains("<NAME>SET-INV-001</NAME>"));
    assert_eq!(
        parse_agent_rows(&captured, CAPTURED_BILL_ALLOCATION_COMPANY_GUID),
        Err("bill_reference_forbidden".into())
    );
}

#[test]
fn named_bill_allocation_without_a_type_still_fails_closed() {
    // The other half of the admission rule: a name without a type is partially
    // populated, not a placeholder. Guessing the type would invent an allocation
    // the book does not contain.
    let captured = captured_bill_allocation_vouchers();
    let start = captured.find("<BILLALLOCATIONS.LIST>").unwrap();
    let end = start
        + captured[start..].find("</BILLALLOCATIONS.LIST>").unwrap()
        + "</BILLALLOCATIONS.LIST>".len();
    let mut named_untyped = captured.clone();
    named_untyped.replace_range(
        start..end,
        "<BILLALLOCATIONS.LIST><NAME>SET-INV-001</NAME>\
         <AMOUNT>-1137.50</AMOUNT></BILLALLOCATIONS.LIST>",
    );
    assert_eq!(
        parse_agent_rows(&named_untyped, CAPTURED_BILL_ALLOCATION_COMPANY_GUID),
        Err("bill_allocation_field_missing".into())
    );
}

#[test]
fn incomplete_bill_allocations_are_refused_instead_of_becoming_empty() {
    let captured = captured_bill_allocation_vouchers();
    for field in ["NAME", "BILLTYPE", "AMOUNT"] {
        let start = captured.find("<BILLALLOCATIONS.LIST>").unwrap();
        let field_start = start + captured[start..].find(&format!("<{field}")).unwrap();
        let field_end = field_start
            + captured[field_start..]
                .find(&format!("</{field}>"))
                .unwrap()
            + field.len()
            + 3;
        for replacement in [String::new(), format!("<{field}></{field}>")] {
            let mut damaged = captured.clone();
            damaged.replace_range(field_start..field_end, &replacement);
            assert_eq!(
                parse_agent_rows(&damaged, CAPTURED_BILL_ALLOCATION_COMPANY_GUID),
                Err("bill_allocation_field_missing".into()),
                "{field}"
            );
        }
    }
}

#[test]
fn a_sign_the_polarity_flag_contradicts_is_reported_on_the_entry_not_refused() {
    // This read used to be refused. On a real book every disagreement was a
    // rounding ledger whose arithmetic was correct -- summing AMOUNT alone
    // reproduced Tally's own closing balance -- so refusing discarded the whole
    // window over entries that were right. The disagreement is recorded instead.
    let captured = captured_native_vouchers();
    for (original, replacement) in [("-101.01", "101.01"), ("101.01", "-101.01")] {
        let damaged = captured.replacen(
            &format!("<AMOUNT TYPE=\"Amount\">{original}</AMOUNT>"),
            &format!("<AMOUNT TYPE=\"Amount\">{replacement}</AMOUNT>"),
            1,
        );
        assert_ne!(damaged, captured);
        for accounting_state in [false, true] {
            let rows = parse_agent_rows_with_accounting_state(
                &damaged,
                accounting_state,
                CAPTURED_VOUCHER_COMPANY_GUID,
            )
            .expect("a contradicted sign must not refuse the read");
            let flagged = rows
                .iter()
                .flat_map(|row| row["amounts"].as_array().cloned().unwrap_or_default())
                .filter(|entry| entry["polarity_disagrees_with_amount"] == json!(true))
                .count();
            assert_eq!(flagged, 1, "the disagreement must be visible on the entry");
        }
    }

    // An entry whose observations agree carries no marker at all.
    let rows = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows
        .iter()
        .flat_map(|row| row["amounts"].as_array().cloned().unwrap_or_default())
        .all(|entry| entry.get("polarity_disagrees_with_amount").is_none()));

    // Zero has no sign to contradict, so either flag agrees with it.
    for zero in ["0", "0.00", "-0.000"] {
        for polarity in [false, true] {
            assert!(tally_entry_polarity_agrees(
                &bridge_tally_core::ExactDecimal::parse(zero.to_string()).unwrap(),
                polarity
            ));
        }
    }
}

#[test]
fn an_empty_ledger_entry_list_is_an_empty_list_not_a_malformed_entry() {
    // A Stock Journal moves inventory and has no accounting effect, so Tally
    // returns ALLLEDGERENTRIES.LIST with no children. Refusing it lost the whole
    // window. A partially populated entry must still be refused: that is what a
    // truncated response or a request-shape regression looks like.
    let captured = captured_native_vouchers();
    let entry_start = captured.find("<ALLLEDGERENTRIES.LIST>").unwrap();
    let entry_end =
        captured.find("</ALLLEDGERENTRIES.LIST>").unwrap() + "</ALLLEDGERENTRIES.LIST>".len();
    let before = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    let entries_before: usize = before
        .iter()
        .map(|row| row["amounts"].as_array().map_or(0, Vec::len))
        .sum();

    let mut emptied = captured.clone();
    emptied.replace_range(
        entry_start..entry_end,
        "<ALLLEDGERENTRIES.LIST></ALLLEDGERENTRIES.LIST>",
    );
    let after = parse_agent_rows(&emptied, CAPTURED_VOUCHER_COMPANY_GUID)
        .expect("an empty entry list must not refuse the read");
    let entries_after: usize = after
        .iter()
        .map(|row| row["amounts"].as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(
        entries_after,
        entries_before - 1,
        "the empty list contributes no entry, and no other entry is lost"
    );
    assert_eq!(
        after.len(),
        before.len(),
        "the voucher itself is still returned"
    );

    // Whitespace-only is still empty; a single populated field is not.
    let mut spaced = captured.clone();
    spaced.replace_range(
        entry_start..entry_end,
        "<ALLLEDGERENTRIES.LIST>\r\n     </ALLLEDGERENTRIES.LIST>",
    );
    assert!(parse_agent_rows(&spaced, CAPTURED_VOUCHER_COMPANY_GUID).is_ok());

    let mut partial = captured.clone();
    partial.replace_range(
        entry_start..entry_end,
        "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Partial</LEDGERNAME></ALLLEDGERENTRIES.LIST>",
    );
    assert_eq!(
        parse_agent_rows(&partial, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("agent_read_protocol_invalid".into()),
        "a partially populated entry must stay refused"
    );
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
            assert_eq!(
                parse_agent_rows(&damaged, CAPTURED_VOUCHER_COMPANY_GUID),
                Err(code.into())
            );
        }
        for replacement in [format!("<{field}/>"), format!("<{field}></{field}>")] {
            let damaged = captured.replacen(&original, &replacement, 1);
            assert_eq!(
                parse_agent_rows(&damaged, CAPTURED_VOUCHER_COMPANY_GUID),
                Err(code.into())
            );
        }
        let absent = captured.replacen(&original, "", 1);
        let rows = parse_agent_rows(&absent, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
        let output = if field == "ALTERID" {
            "alter_id"
        } else {
            "master_id"
        };
        assert!(rows[0][output].is_null());
    }
    let rows = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
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
    let ordinary =
        parse_agent_rows(optional_capture, "bb8ad19e-6aef-4239-a917-87fec0c6215e").unwrap();
    let mut accounting =
        parse_agent_changed_rows(optional_capture, "bb8ad19e-6aef-4239-a917-87fec0c6215e").unwrap();
    for row in &mut accounting {
        row.as_object_mut().unwrap().remove("remote_id");
    }
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
                parse_agent_rows(&damaged, CAPTURED_VOUCHER_COMPANY_GUID),
                Err("voucher_accounting_state_not_observed".into())
            );
        }
        // Simulate a flag transition in captured bytes; neither path may
        // drop the row or conceal its non-posting state.
        let changed = captured.replacen(&original, &format!("<{field}>Yes</{field}>"), 1);
        let ordinary = parse_agent_rows(&changed, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
        let mut accounting =
            parse_agent_changed_rows(&changed, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
        for row in &mut accounting {
            row.as_object_mut().unwrap().remove("remote_id");
        }
        assert_eq!(ordinary, accounting);
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
                parse_agent_rows_with_accounting_state(
                    &damaged,
                    accounting_state,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
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
    let rows = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["amounts"][0]["is_deemed_positive"], "Yes");
    assert_eq!(rows[0]["amounts"][1]["is_deemed_positive"], "No");
    // Tally logical values may carry surrounding presentation whitespace. The
    // parser admits that observed representation, then emits its canonical
    // protocol value so desktop debit/credit labeling never compares raw text.
    for (raw, padded, entry_index, canonical) in
        [("Yes", " Yes ", 0, "Yes"), ("No", " No ", 1, "No")]
    {
        let whitespace = captured.replacen(
            &format!("\n      <ISDEEMEDPOSITIVE TYPE=\"Logical\">{raw}</ISDEEMEDPOSITIVE>"),
            &format!("\n      <ISDEEMEDPOSITIVE TYPE=\"Logical\">{padded}</ISDEEMEDPOSITIVE>"),
            1,
        );
        assert_ne!(whitespace, captured, "captured {raw} mutation must apply");
        for accounting_state in [false, true] {
            let rows = parse_agent_rows_with_accounting_state(
                &whitespace,
                accounting_state,
                CAPTURED_VOUCHER_COMPANY_GUID,
            )
            .expect("whitespace around an observed logical value remains admissible");
            assert_eq!(
                rows[0]["amounts"][entry_index]["is_deemed_positive"],
                canonical
            );
        }
    }
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
                parse_agent_rows_with_accounting_state(
                    &damaged,
                    accounting_state,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
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
    let rows = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
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
                parse_agent_rows_with_accounting_state(
                    &damaged,
                    accounting_state,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
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
                    parse_agent_rows_with_accounting_state(
                        &damaged,
                        accounting_state,
                        CAPTURED_VOUCHER_COMPANY_GUID
                    ),
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
    assert_eq!(
        parse_agent_rows(&fragmented, CAPTURED_VOUCHER_COMPANY_GUID),
        parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID)
    );
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
            parse_agent_rows_with_accounting_state(
                &repeated,
                accounting_state,
                CAPTURED_VOUCHER_COMPANY_GUID
            ),
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
        assert_eq!(
            parse_agent_rows(&fragmented, CAPTURED_VOUCHER_COMPANY_GUID),
            parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID)
        );
    }
    let malformed = captured.replacen(original, "<AMOUNT>-101<![CDATA[.99]]>.01</AMOUNT>", 1);
    assert_eq!(
        parse_agent_rows(&malformed, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("voucher_amount_invalid".into())
    );
    for value in ["-101<NESTED>99</NESTED>.01", "-101<NESTED/>.01"] {
        let nested = captured.replacen(original, &format!("<AMOUNT>{value}</AMOUNT>"), 1);
        assert_eq!(
            parse_agent_rows(&nested, CAPTURED_VOUCHER_COMPANY_GUID),
            Err("agent_read_protocol_invalid".into())
        );
    }
}

#[test]
fn captured_remote_id_is_validated_once_and_exposed_only_to_changed_rows() {
    let captured = captured_native_vouchers();
    let ordinary = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    let changed = parse_agent_changed_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(ordinary.iter().all(|row| row.get("remote_id").is_none()));
    assert!(changed.iter().all(|row| row["remote_id"]
        .as_str()
        .is_some_and(|value| !value.is_empty())));
    for replacement in [
        "REMOTEID=\"duplicate\" REMOTEID=\"",
        "remoteid=\"duplicate\" REMOTEID=\"",
        "REMOTEID=\"&invalid;\" OTHER=\"",
    ] {
        let invalid = captured.replacen("REMOTEID=\"", replacement, 1);
        assert_ne!(invalid, captured);
        for require_identity in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(
                    &invalid,
                    require_identity,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
                Err("agent_read_protocol_invalid".into())
            );
        }
    }
}

#[test]
fn repeated_captured_voucher_identities_are_refused_before_selection_or_movement() {
    let captured = captured_native_vouchers();
    let original = parse_agent_changed_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    let first_start = captured.find("<VOUCHER ").unwrap();
    let first_end =
        first_start + captured[first_start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
    let first = &captured[first_start..first_end];
    let guid = original[0]["guid"].as_str().unwrap();
    let master = original[0]["master_id"].as_str().unwrap();
    for duplicate in [
        first.to_string(),
        first
            .replace(guid, &guid.to_ascii_uppercase())
            .replace(&format!(">{master}</MASTERID>"), ">999</MASTERID>"),
        first
            .replace(guid, &format!("{CAPTURED_VOUCHER_COMPANY_GUID}-distinct"))
            .replace(&format!(">{master}</MASTERID>"), ">0001</MASTERID>"),
    ] {
        let repeated = captured.replacen("</COLLECTION>", &format!("{duplicate}</COLLECTION>"), 1);
        for require_identity in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(
                    &repeated,
                    require_identity,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
                Err("voucher_source_identity_invalid".into())
            );
        }
    }
    // Distinct accounting contents are not an identity; preserve distinct rows.
    assert_eq!(
        parse_agent_changed_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn unexpected_direct_collection_rows_are_not_empty_voucher_reads() {
    let captured = captured_native_vouchers();
    for row_type in ["LEDGER", "GROUP", "UNEXPECTED"] {
        let mismatched = captured
            .replace("<VOUCHER ", &format!("<{row_type} "))
            .replace("</VOUCHER>", &format!("</{row_type}>"));
        assert_ne!(mismatched, captured);
        for require_identity in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(
                    &mismatched,
                    require_identity,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
                Err("agent_read_protocol_invalid".into())
            );
        }
    }
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    );
    let empty = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(parse_agent_rows(&empty, CAPTURED_VOUCHER_COMPANY_GUID)
        .unwrap()
        .is_empty());
    for row in [
        "<LEDGER/>",
        "<GROUP/>",
        "<UNEXPECTED/>",
        "<LEDGER></LEDGER>",
    ] {
        let mismatched = empty.replace("</COLLECTION>", &format!("{row}</COLLECTION>"));
        assert_ne!(mismatched, empty);
        for require_identity in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(
                    &mismatched,
                    require_identity,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
                Err("agent_read_protocol_invalid".into())
            );
        }
    }
}

#[test]
fn captured_voucher_guids_must_bind_every_row_to_the_selected_company() {
    let captured = captured_native_vouchers();
    let rows = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        parse_agent_rows(
            &captured,
            &CAPTURED_VOUCHER_COMPANY_GUID.to_ascii_uppercase()
        )
        .unwrap(),
        rows
    );
    let guid = rows[0]["guid"].as_str().unwrap();
    let field = format!("<GUID>{guid}</GUID>");
    assert!(captured.contains(&field));
    for replacement in [
        String::new(),
        "<GUID/>".to_string(),
        format!("<GUID>{CAPTURED_VOUCHER_COMPANY_GUID}</GUID>"),
        format!("<GUID>{CAPTURED_VOUCHER_COMPANY_GUID}-</GUID>"),
        format!("<GUID>{CAPTURED_VOUCHER_COMPANY_GUID}suffix</GUID>"),
        "<GUID>71c6de69-1748-461c-ad3f-162cb949df9f-00000001</GUID>".to_string(),
    ] {
        let damaged = captured.replacen(&field, &replacement, 1);
        for changed in [false, true] {
            assert_eq!(
                parse_agent_rows_with_accounting_state(
                    &damaged,
                    changed,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
                Err("voucher_company_identity_invalid".into())
            );
        }
        assert_eq!(
            parse_movement_vouchers(
                &damaged,
                "20260801",
                "20260801",
                CAPTURED_VOUCHER_COMPANY_GUID
            )
            .err()
            .as_deref(),
            Some("voucher_company_identity_invalid")
        );
    }
}

/// The captured three-voucher response with the first voucher's EFFECTIVEDATE
/// element replaced by `element`.
fn with_effective_date(element: &str) -> String {
    captured_native_vouchers().replacen(
        "<EFFECTIVEDATE TYPE=\"Date\">20260801</EFFECTIVEDATE>",
        element,
        1,
    )
}

#[test]
fn effective_date_is_read_for_import_verification_only() {
    // The capture returns EFFECTIVEDATE on its first two vouchers and not on the third.
    let captured = captured_native_vouchers();
    let rows = parse_import_verification_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["effective_date"], "20260801");
    assert_eq!(rows[1]["effective_date"], "20260801");
    // not returned for a voucher is not invented for it
    assert!(rows[2].get("effective_date").is_none());
    // the public voucher tools do not surface it
    for public in [
        parse_agent_changed_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
        parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
    ] {
        assert!(public.iter().all(|row| row.get("effective_date").is_none()));
    }

    // an empty element is not observed, like an absent one
    let empty = with_effective_date("<EFFECTIVEDATE TYPE=\"Date\"></EFFECTIVEDATE>");
    let rows = parse_import_verification_rows(&empty, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows[0].get("effective_date").is_none());

    // surrounding whitespace is not part of the date
    let padded = with_effective_date("<EFFECTIVEDATE TYPE=\"Date\"> 20260802\n</EFFECTIVEDATE>");
    let rows = parse_import_verification_rows(&padded, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["effective_date"], "20260802");

    let invalid = with_effective_date("<EFFECTIVEDATE TYPE=\"Date\">20261345</EFFECTIVEDATE>");
    assert_eq!(
        parse_import_verification_rows(&invalid, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("voucher_effective_date_invalid".to_string())
    );
    // two effective dates on one voucher cannot be compared with one written value
    let repeated = with_effective_date(
        "<EFFECTIVEDATE TYPE=\"Date\">20260801</EFFECTIVEDATE><EFFECTIVEDATE TYPE=\"Date\">20260802</EFFECTIVEDATE>",
    );
    assert_eq!(
        parse_import_verification_rows(&repeated, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("agent_read_protocol_invalid".to_string())
    );
}

/// The captured three-voucher response with the first voucher's ISCANCELLED
/// element immediately followed by one injected `element`. This is fault
/// injection into a captured response, used both for a field the capture
/// never carried at all (ISPOSTDATED; see the absent case below) and for a
/// value not observed live (a populated PARTYGSTIN; protocol reference
/// §8.2c only ever observed that tag empty) — not a new fixture or evidence
/// of a live Tally response shape either way.
fn with_injected_voucher_element(element: &str) -> String {
    captured_native_vouchers().replacen(
        "<ISCANCELLED TYPE=\"Logical\">No</ISCANCELLED>",
        &format!("<ISCANCELLED TYPE=\"Logical\">No</ISCANCELLED>{element}"),
        1,
    )
}

#[test]
fn post_dated_is_optional_unlike_cancelled_and_optional() {
    // The unmodified capture never asserts ISPOSTDATED on any of its three
    // vouchers — this is real captured Tally output, not a synthetic gap.
    // Unlike a missing ISCANCELLED/ISOPTIONAL (which required_tally_bool
    // refuses the whole read over), an absent ISPOSTDATED must not fail the
    // read and must not be invented as `false`: the key is omitted so a
    // caller can tell "Tally did not say" from "Tally said no".
    let captured = captured_native_vouchers();
    for rows in [
        parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
        parse_agent_changed_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
        parse_import_verification_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
    ] {
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter().all(|row| row.get("post_dated").is_none()),
            "an absent ISPOSTDATED must not be invented as false or true"
        );
        // cancelled/optional are unaffected and still always booleans.
        assert!(rows
            .iter()
            .all(|row| row["cancelled"].is_boolean() && row["optional"].is_boolean()));
    }

    // ISPOSTDATED=Yes on voucher 1 only.
    let yes = with_injected_voucher_element("<ISPOSTDATED TYPE=\"Logical\">Yes</ISPOSTDATED>");
    let rows = parse_agent_rows(&yes, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["post_dated"], true);
    assert!(rows[1].get("post_dated").is_none());
    assert!(rows[2].get("post_dated").is_none());

    // ISPOSTDATED=No is observed and distinct from absent.
    let no = with_injected_voucher_element("<ISPOSTDATED TYPE=\"Logical\">No</ISPOSTDATED>");
    let rows = parse_agent_rows(&no, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["post_dated"], false);

    // An empty element is not observed, exactly like an absent one.
    let empty = with_injected_voucher_element("<ISPOSTDATED TYPE=\"Logical\"></ISPOSTDATED>");
    let rows = parse_agent_rows(&empty, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows[0].get("post_dated").is_none());

    // A present but unrecognised value refuses the read rather than guessing.
    let invalid =
        with_injected_voucher_element("<ISPOSTDATED TYPE=\"Logical\">Maybe</ISPOSTDATED>");
    assert_eq!(
        parse_agent_rows(&invalid, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("voucher_post_dated_invalid".to_string())
    );

    // Two ISPOSTDATED elements on one voucher cannot be reconciled into one
    // written value; the wire admission layer refuses the duplicate scalar.
    let repeated = with_injected_voucher_element(
        "<ISPOSTDATED TYPE=\"Logical\">Yes</ISPOSTDATED><ISPOSTDATED TYPE=\"Logical\">No</ISPOSTDATED>",
    );
    assert_eq!(
        parse_agent_rows(&repeated, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("agent_read_protocol_invalid".to_string())
    );
}

/// `reference`, `is_invoice` and `party_gstin` (protocol reference §8.2c),
/// checked against the same captured three-voucher fixture the ISPOSTDATED
/// test above uses. The unmodified capture never asserts REFERENCE or
/// PARTYGSTIN on any of its three vouchers -- real captured Tally output,
/// not a synthetic gap. It already carries `<ISINVOICE>No</ISINVOICE>` on
/// all three, immediately after each voucher's ISCANCELLED element, from
/// whatever broader FETCH originally captured it: this was inert dead data
/// until ISINVOICE was allow-listed, and its baseline below is real
/// evidence too, not injection.
#[test]
fn reference_is_invoice_and_party_gstin_are_optional_and_non_conflated() {
    let captured = captured_native_vouchers();
    for rows in [
        parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
        parse_agent_changed_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
        parse_import_verification_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap(),
    ] {
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|row| row.get("reference").is_none() && row.get("party_gstin").is_none()),
            "reference/party_gstin must not be invented when Tally did not report them"
        );
        // ISINVOICE is genuinely present as "No" on all three real captured
        // vouchers already -- not absent, and not injected.
        assert!(
            rows.iter().all(|row| row["is_invoice"] == false),
            "the captured fixture already asserts ISINVOICE=No on every voucher"
        );
    }

    // REFERENCE populated on voucher 1 only -- protocol reference §8.2c
    // observed exactly this literal value live on 9 of 67 captured vouchers.
    let referenced =
        with_injected_voucher_element("<REFERENCE TYPE=\"String\">SHAPELAB-MANUAL-1</REFERENCE>");
    let rows = parse_agent_rows(&referenced, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["reference"], "SHAPELAB-MANUAL-1");
    assert!(rows[1].get("reference").is_none());
    assert!(rows[2].get("reference").is_none());

    // An empty REFERENCE is not observed, exactly like an absent one -- this
    // is the shape §8.2c actually observed on 58 of 67 vouchers, not a guess.
    let empty_reference = with_injected_voucher_element("<REFERENCE TYPE=\"String\"></REFERENCE>");
    let rows = parse_agent_rows(&empty_reference, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows[0].get("reference").is_none());

    // ISINVOICE already exists once per voucher in this fixture, so these
    // cases mutate the real captured element in place (voucher 1 only, via
    // the first-occurrence replacement) instead of appending a second one,
    // which would just be a duplicate-scalar rejection instead of a value.
    const CAPTURED_INVOICE_NO: &str = "<ISINVOICE>No</ISINVOICE>";

    // ISINVOICE=Yes on voucher 1 only. §8.2c observed both values live (16
    // Yes, 51 No of 67), so this is a real observed shape, not a guess --
    // just not one this particular committed fixture happens to carry.
    let invoice_yes = captured.replacen(CAPTURED_INVOICE_NO, "<ISINVOICE>Yes</ISINVOICE>", 1);
    let rows = parse_agent_rows(&invoice_yes, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["is_invoice"], true);
    assert_eq!(rows[1]["is_invoice"], false);
    assert_eq!(rows[2]["is_invoice"], false);

    // An empty ISINVOICE is not observed, exactly like an absent one. Never
    // observed live (§8.2c's capture asserted Yes/No on every voucher), so
    // this defends the optional-field contract rather than reproducing a
    // seen shape.
    let invoice_empty = captured.replacen(CAPTURED_INVOICE_NO, "<ISINVOICE></ISINVOICE>", 1);
    let rows = parse_agent_rows(&invoice_empty, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows[0].get("is_invoice").is_none());

    // Removing the element outright is the same "not observed" case as
    // empty; ISPOSTDATED's absence handling is exercised via a tag that
    // never appears at all, so this proves ISINVOICE's absence path
    // independently rather than only its empty-element path.
    let invoice_absent = captured.replacen(CAPTURED_INVOICE_NO, "", 1);
    let rows = parse_agent_rows(&invoice_absent, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows[0].get("is_invoice").is_none());
    assert_eq!(rows[1]["is_invoice"], false);

    // A present but unrecognised ISINVOICE value refuses the read rather
    // than guessing, exactly like a malformed ISPOSTDATED.
    let invoice_invalid = captured.replacen(CAPTURED_INVOICE_NO, "<ISINVOICE>Maybe</ISINVOICE>", 1);
    assert_eq!(
        parse_agent_rows(&invoice_invalid, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("voucher_is_invoice_invalid".to_string())
    );

    // Two ISINVOICE elements on one voucher cannot be reconciled into one
    // written value; the wire admission layer refuses the duplicate scalar.
    let invoice_repeated = captured.replacen(
        CAPTURED_INVOICE_NO,
        "<ISINVOICE>Yes</ISINVOICE><ISINVOICE>No</ISINVOICE>",
        1,
    );
    assert_eq!(
        parse_agent_rows(&invoice_repeated, CAPTURED_VOUCHER_COMPANY_GUID),
        Err("agent_read_protocol_invalid".to_string())
    );

    // A populated PARTYGSTIN was never observed live -- §8.2c's capture found
    // the tag present but empty on all 67 vouchers, so this value is
    // synthetic, proving only that the parser round-trips a non-empty string
    // when Tally does report one; it is not evidence Tally has been observed
    // to populate it.
    let gstin =
        with_injected_voucher_element("<PARTYGSTIN TYPE=\"String\">27AAAAA0000A1Z5</PARTYGSTIN>");
    let rows = parse_agent_rows(&gstin, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows[0]["party_gstin"], "27AAAAA0000A1Z5");
    assert!(rows[1].get("party_gstin").is_none());
    assert!(rows[2].get("party_gstin").is_none());

    // An empty PARTYGSTIN is not observed, exactly like an absent one -- this
    // is the shape §8.2c actually observed live on every one of 67 vouchers.
    let empty_gstin = with_injected_voucher_element("<PARTYGSTIN TYPE=\"String\"></PARTYGSTIN>");
    let rows = parse_agent_rows(&empty_gstin, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert!(rows[0].get("party_gstin").is_none());
}

fn captured_utf16le(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

/// `TALLY_PROTOCOL_REFERENCE.md` §1.1(d): the agent parsers read `&#4;` as
/// the marker the protocol crate's native parsers produce, not as U+0004.
const MARKED_ROOT: &str = "\u{fffd}#4; Primary";

#[test]
fn a_captured_group_parent_reads_as_the_marker_in_the_changed_master_feed() {
    // The captured Group collection carries `&#4; Primary` in PARENT. It was
    // fetched without a NAME element, which the changed-master reader
    // requires, so each row's own NAME attribute is copied into one; nothing
    // else in the capture is touched.
    let captured = captured_utf16le(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml"
    ));
    let company_guid = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let named = captured
        .split("<GROUP NAME=\"")
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                return part.to_string();
            }
            let name = part.split('"').next().unwrap();
            let open_end = part.find('>').unwrap() + 1;
            format!(
                "<GROUP NAME=\"{}<NAME>{name}</NAME>{}",
                &part[..open_end],
                &part[open_end..]
            )
        })
        .collect::<String>();
    let rows = parse_agent_changed_masters(&named).expect("captured groups parse");
    let native = bridge_tally_protocol::native_outstandings::parse_native_group_snapshot(
        &captured,
        company_guid,
    )
    .expect("the same capture parses natively");
    let root_rows = rows
        .iter()
        .filter(|row| row["parent"] == MARKED_ROOT)
        .count();
    assert_eq!(
        root_rows,
        captured
            .matches("<PARENT TYPE=\"String\">&#4; Primary</PARENT>")
            .count()
    );
    assert!(root_rows > 0);
    assert!(!rows.iter().any(|row| row["parent"]
        .as_str()
        .is_some_and(|parent| parent.contains('\u{4}'))));
    for row in &rows {
        let group = native
            .iter()
            .find(|group| row["name"] == group.name.as_str())
            .expect("both readers see the same groups");
        assert_eq!(row["parent"].as_str(), group.parent.returned_text());
    }
}

#[test]
fn a_captured_forbidden_reference_in_voucher_text_reads_as_the_marker() {
    // No captured voucher carries `&#4;` in a field this reader keeps; it
    // appears on GST fields the reader skips. The captured atom is moved into
    // the first captured NARRATION, and the rest of the capture must read
    // exactly as it did.
    let captured = captured_entry_wildcard_vouchers();
    let atom = "&#4; Not Applicable";
    assert!(captured.contains(&format!(">{atom}</GSTCLASS>")));
    // The capture's first non-empty narration, whatever it says.
    let open = "<NARRATION TYPE=\"String\">";
    let start = captured
        .find(open)
        .expect("the capture carries a narration")
        + open.len();
    let narration = &captured[start..start + captured[start..].find('<').unwrap()];
    let derived = format!(
        "{}{}{}",
        &captured[..start],
        atom,
        &captured[start + narration.len()..]
    );
    assert_ne!(derived, captured);
    let before = parse_agent_rows(&captured, WILDCARD_ALLOCATION_COMPANY_GUID).unwrap();
    let mut after = parse_agent_rows(&derived, WILDCARD_ALLOCATION_COMPANY_GUID).unwrap();
    let changed = after
        .iter()
        .position(|row| {
            row["narration"] != json!(narration)
                && row["narration"]
                    .as_str()
                    .is_some_and(|n| n.contains("Not Applicable"))
        })
        .expect("the moved atom is read");
    assert_eq!(
        after[changed]["narration"],
        json!("\u{fffd}#4; Not Applicable")
    );
    after[changed]["narration"] = json!(narration);
    assert_eq!(after, before);
    // The import-verification reader is the same reader with one more field.
    let verification =
        parse_import_verification_rows(&derived, WILDCARD_ALLOCATION_COMPANY_GUID).unwrap();
    assert_eq!(
        verification[changed]["narration"],
        json!("\u{fffd}#4; Not Applicable")
    );
}

#[test]
fn a_literal_replacement_character_that_looks_like_a_marker_reads_back_escaped() {
    // The rule keeps its rewrite reversible by escaping a literal U+FFFD that
    // is followed by `#`, digits and `;`. A voucher Bridge posted with such
    // text would read back as other text; `validate_payload` refuses it.
    let captured = captured_entry_wildcard_vouchers();
    // The capture's first non-empty narration, whatever it says.
    let open = "<NARRATION TYPE=\"String\">";
    let start = captured
        .find(open)
        .expect("the capture carries a narration")
        + open.len();
    let narration = &captured[start..start + captured[start..].find('<').unwrap()];
    let derived = format!(
        "{}{}{}",
        &captured[..start],
        "A\u{fffd}#5;",
        &captured[start + narration.len()..]
    );
    let rows = parse_import_verification_rows(&derived, WILDCARD_ALLOCATION_COMPANY_GUID).unwrap();
    assert!(rows
        .iter()
        .any(|row| row["narration"] == json!("A\u{fffd}#65533;#5;")));
}

// -- #674: a foreign-currency composite withholds its voucher ------------------

const FOREX_COMPANY_GUID: &str = "b14e9b2d-8a63-4779-804d-25d59eb787eb";

/// A live `vouchers` read of the synthetic several-currency book: one Sales
/// voucher whose party entry, bill allocation and sales entry each hold a
/// composite (fixtures/agent/vouchers-forex-composite-20260915.PROVENANCE.md).
fn captured_forex_composite_vouchers() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/vouchers-forex-composite-20260915.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn withheld_view(row: &VoucherRow) -> Value {
    match row {
        VoucherRow::Withheld(withheld) => withheld.filter_view(),
        VoucherRow::Read(read) => panic!("expected a withheld voucher, read {read}"),
    }
}

#[test]
fn a_captured_composite_voucher_is_withheld_with_its_identity_and_no_amount() {
    let rows =
        parse_agent_rows_withholding(&captured_forex_composite_vouchers(), FOREX_COMPANY_GUID)
            .unwrap();
    assert_eq!(rows.len(), 1);
    let view = withheld_view(&rows[0]);
    assert_eq!(view[WITHHELD_MARKER], WITHHELD_FOREIGN_CURRENCY);
    assert_eq!(view["date"], "20260915");
    assert_eq!(view["voucher_type"], "Sales");
    assert_eq!(view["voucher_number"], "1");
    assert_eq!(view["alter_id"], 18);
    assert!(view["guid"].as_str().unwrap().starts_with(FOREX_COMPANY_GUID));
    // Its entries keep their ledgers, for the ledger filter, and nothing else.
    let entries = view["amounts"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        assert_eq!(entry.as_object().unwrap().keys().collect::<Vec<_>>(), vec!["ledger"]);
        assert!(entry["ledger"].is_string());
    }
    assert!(!view.to_string().contains(" @ "), "{view}");
}

#[test]
fn every_amount_consuming_parse_still_refuses_the_composite_window() {
    let captured = captured_forex_composite_vouchers();
    // The bill allocation closes before its entry, so it is refused first.
    assert_eq!(
        parse_agent_rows(&captured, FOREX_COMPANY_GUID).unwrap_err(),
        "bill_allocation_amount_invalid"
    );
    assert_eq!(
        parse_agent_changed_rows(&captured, FOREX_COMPANY_GUID).unwrap_err(),
        "bill_allocation_amount_invalid"
    );
    assert_eq!(
        parse_import_verification_rows(&captured, FOREX_COMPANY_GUID).unwrap_err(),
        "bill_allocation_amount_invalid"
    );
}

#[test]
fn only_a_whole_composite_withholds_anything_else_still_refuses() {
    let captured = captured_forex_composite_vouchers();
    let composite = "-$ 100.00 @ I\u{20b9} 86/$  = -I\u{20b9} 8600.00";
    assert!(captured.contains(composite));
    // The party entry's amount comes first in the text, its bill allocation's
    // second. Cut either short and it is no composite, so its window refuses.
    let at = captured.find(composite).unwrap();
    let cut_entry = format!(
        "{}-$ 100.00 @ I\u{20b9} 86/${}",
        &captured[..at],
        &captured[at + composite.len()..]
    );
    assert_eq!(
        parse_agent_rows_withholding(&cut_entry, FOREX_COMPANY_GUID).unwrap_err(),
        "voucher_amount_invalid"
    );
    // Not the next occurrence: a VATEXPAMOUNT between them carries one too.
    let allocations = captured.find("<BILLALLOCATIONS.LIST").unwrap();
    let second = allocations + captured[allocations..].find(composite).unwrap();
    let cut_allocation = format!(
        "{}-$ 100.00 @ I\u{20b9} 86/${}",
        &captured[..second],
        &captured[second + composite.len()..]
    );
    assert_eq!(
        parse_agent_rows_withholding(&cut_allocation, FOREX_COMPANY_GUID).unwrap_err(),
        "bill_allocation_amount_invalid"
    );
    // A plain garbled amount on an ordinary voucher still refuses.
    let garbled = captured_native_vouchers().replacen(
        "<AMOUNT TYPE=\"Amount\">-101.01</AMOUNT>",
        "<AMOUNT TYPE=\"Amount\">Maybe</AMOUNT>",
        1,
    );
    assert_eq!(
        parse_agent_rows_withholding(&garbled, CAPTURED_VOUCHER_COMPANY_GUID).unwrap_err(),
        "voucher_amount_invalid"
    );
    // A structural fault in a withheld voucher still refuses: an entry with
    // no ledger name is refused as in any other voucher.
    let unnamed = captured.replacen(
        "<LEDGERNAME>FX Party 01</LEDGERNAME>",
        "<LEDGERNAME></LEDGERNAME>",
        1,
    );
    assert_ne!(unnamed, captured);
    assert_eq!(
        parse_agent_rows_withholding(&unnamed, FOREX_COMPANY_GUID).unwrap_err(),
        "agent_read_protocol_invalid"
    );
}

#[test]
fn a_rupee_voucher_beside_a_composite_one_is_read_whole() {
    let captured = captured_native_vouchers();
    let mutated = window_with_composite_vouchers(1);
    let rows = parse_agent_rows_withholding(&mutated, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    let ordinary = parse_agent_rows(&captured, CAPTURED_VOUCHER_COMPANY_GUID).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(withheld_view(&rows[0])["voucher_number"], "1");
    for (row, expected) in rows[1..].iter().zip(&ordinary[1..]) {
        match row {
            VoucherRow::Read(read) => assert_eq!(read, expected),
            VoucherRow::Withheld(_) => panic!("a rupee voucher was withheld"),
        }
    }
}

#[test]
fn the_captured_empty_rate_composite_withholds_too() {
    // Synthetic mutation: the sales entry's amount replaced by the empty-rate
    // composite captured in the several-currency Trial Balance.
    let captured = captured_forex_composite_vouchers();
    let sales = "$ 100.00 @ I\u{20b9} 86/$  = I\u{20b9} 8600.00";
    assert!(captured.contains(sales));
    let trial_balance = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/trial_balance_currency_forex_live.utf16le.xml"
    );
    let trial_balance = String::from_utf16(
        &trial_balance
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let empty_rate = trial_balance
        .split('>')
        .filter_map(|tail| tail.split('<').next())
        .find(|text| text.contains(" /$") && text.contains(" @ "))
        .expect("the captured empty-rate composite");
    assert!(bridge_tally_protocol::currency_composite::is_currency_composite(empty_rate));
    let mutated = captured.replacen(sales, empty_rate, 1);
    let rows = parse_agent_rows_withholding(&mutated, FOREX_COMPANY_GUID).unwrap();
    assert_eq!(withheld_view(&rows[0])[WITHHELD_MARKER], WITHHELD_FOREIGN_CURRENCY);
}

#[test]
fn the_captured_request_is_what_vouchers_renders_today() {
    let request = include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/vouchers-forex-composite-20260915.request.xml"
    );
    assert_eq!(
        render_agent_vouchers("BRIDGE CORPUS FOREX", "20260915", "20260915", None).unwrap(),
        request
    );
}
