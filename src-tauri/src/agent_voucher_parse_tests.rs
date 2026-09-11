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
    assert_eq!(rows.len(), parse_agent_rows(&captured, CAPTURED_BILL_ALLOCATION_COMPANY_GUID).unwrap().len());
    assert_eq!(rows[0]["amounts"][1]["bill_allocations"], json!([]));
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
                parse_agent_rows_with_accounting_state(
                    &damaged,
                    accounting_state,
                    CAPTURED_VOUCHER_COMPANY_GUID
                ),
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
