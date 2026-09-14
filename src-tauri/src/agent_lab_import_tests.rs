//! Unit tests for the lab writer (Phase 3.4/3.5). Synthetic fixtures only, no
//! client data and no live Tally connection -- matching `agent_lab.rs`'s own
//! test discipline. Golden XML strings below are asserted verbatim; any
//! change to the rendered shape must be a deliberate, reviewed edit here.
use super::*;

const REMOTE_ID: Uuid = Uuid::from_u128(1);
const ATTRIBUTION_ID: Uuid = Uuid::from_u128(2);

// ---------------------------------------------------------------------------
// canonical_master_key -- §9.4d, licensed TallyPrime 7.1
// ---------------------------------------------------------------------------

#[test]
fn canonical_master_key_folds_the_composed_9_4d_rules() {
    let base = canonical_master_key("MB-PROBE-LEDGER-A");
    assert_eq!(
        canonical_master_key("MB PROBE LEDGER A"),
        base,
        "hyphen == space"
    );
    assert_eq!(
        canonical_master_key("mb probe ledger a"),
        base,
        "ASCII case folds"
    );
    assert_eq!(
        canonical_master_key("  mb probe  ledger a "),
        base,
        "surrounding + collapsed runs"
    );
    assert_eq!(
        canonical_master_key("mb/probe/ledger/a"),
        base,
        "slash == space"
    );
}

#[test]
fn canonical_master_key_does_not_fold_unmeasured_separators() {
    // §9.4d: an en dash and an underscore are ordinary characters to Tally,
    // not separators -- a class-based fold would wrongly merge these.
    assert_ne!(canonical_master_key("A_B"), canonical_master_key("A B"));
    assert_ne!(
        canonical_master_key("A\u{2013}B"),
        canonical_master_key("A B")
    );
}

#[test]
fn canonical_master_key_does_not_normalise_unicode_forms() {
    // §9.4d: "canonical equivalence is still refused" -- NFC vs NFD must stay
    // distinguishable through this fold, unlike every other transformation.
    let nfc = "\u{00e9}"; // é, single codepoint
    let nfd = "e\u{0301}"; // e + combining acute accent
    assert_ne!(canonical_master_key(nfc), canonical_master_key(nfd));
}

// ---------------------------------------------------------------------------
// Master XML -- golden fixtures
// ---------------------------------------------------------------------------

#[test]
fn unit_create_xml_golden() {
    let u = BookUnit {
        name: "Kgs".into(),
        is_simple_unit: Some("Yes".into()),
        decimal_places: Some("3".into()),
    };
    assert_eq!(
        render_unit_xml(&u),
        "<TALLYMESSAGE><UNIT NAME=\"Kgs\" ACTION=\"Create\"><NAME>Kgs</NAME>\
<ISSIMPLEUNIT>Yes</ISSIMPLEUNIT><DECIMALPLACES>3</DECIMALPLACES></UNIT></TALLYMESSAGE>"
    );
}

#[test]
fn godown_create_xml_golden() {
    let g = BookNamedParent {
        name: "Main Godown".into(),
        parent: Some("Primary".into()),
    };
    assert_eq!(
        render_parented_xml("GODOWN", &g),
        "<TALLYMESSAGE><GODOWN NAME=\"Main Godown\" ACTION=\"Create\"><NAME>Main Godown</NAME>\
<PARENT>Primary</PARENT></GODOWN></TALLYMESSAGE>"
    );
}

// Golden fixtures below are derived byte-for-byte from the proven-good,
// live-created shape captured in
// brain/50-projects/viniyug-fieldwork-2026-09-11/artifacts/Babul-Final-Import/
// Babul-Rounded-2026-09-11/fresh-company-only/babul-masters-complete.xml
// (this exact company's masters were created with it on TallyPrime 7.1):
// `<NAME>` mirrors the attribute, `ISBILLWISEON` is always explicit,
// `OPENINGBALANCE` appears only when the source ledger's balance is
// non-zero, and no `TALLYMESSAGE` declares `xmlns:UDF`.

#[test]
fn ledger_create_xml_golden_babul_bank_ledger_with_negative_opening() {
    // Babul's "HDFC Bank 1649": non-zero (negative) opening balance, no GST
    // fields, ISBILLWISEON explicit No.
    let l = BookLedger {
        name: "HDFC Bank 1649".into(),
        parent: Some("Bank Accounts".into()),
        opening_balance: Some("-5013.35".into()),
        is_billwise_on: Some(false),
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    assert_eq!(
        render_ledger_xml(&l),
        "<TALLYMESSAGE><LEDGER NAME=\"HDFC Bank 1649\" ACTION=\"Create\">\
<NAME>HDFC Bank 1649</NAME><PARENT>Bank Accounts</PARENT><ISBILLWISEON>No</ISBILLWISEON>\
<OPENINGBALANCE>-5013.35</OPENINGBALANCE></LEDGER></TALLYMESSAGE>"
    );
}

#[test]
fn ledger_create_xml_golden_babul_zero_balance_ledger_omits_opening_balance() {
    // Babul's "Sales": zero opening balance -- the proven capture carries no
    // `OPENINGBALANCE` element at all for this ledger.
    let l = BookLedger {
        name: "Sales".into(),
        parent: Some("Sales Accounts".into()),
        opening_balance: Some("0.00".into()),
        is_billwise_on: Some(false),
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    assert_eq!(
        render_ledger_xml(&l),
        "<TALLYMESSAGE><LEDGER NAME=\"Sales\" ACTION=\"Create\">\
<NAME>Sales</NAME><PARENT>Sales Accounts</PARENT><ISBILLWISEON>No</ISBILLWISEON></LEDGER></TALLYMESSAGE>"
    );
}

#[test]
fn ledger_create_xml_golden_babul_billwise_party_with_gstin() {
    // Babul's "Sri Ram Cables Private Limited": ISBILLWISEON=Yes, zero
    // opening balance (so still no OPENINGBALANCE), plus a GSTIN this
    // module's own book model carries that the Babul capture itself did not.
    let l = BookLedger {
        name: "Sri Ram Cables Private Limited".into(),
        parent: Some("Sundry Debtors".into()),
        opening_balance: Some("0.00".into()),
        is_billwise_on: Some(true),
        party_gstin: Some("27ZZZZZ0000Z1Z5".into()),
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    assert_eq!(
        render_ledger_xml(&l),
        "<TALLYMESSAGE><LEDGER NAME=\"Sri Ram Cables Private Limited\" ACTION=\"Create\">\
<NAME>Sri Ram Cables Private Limited</NAME><PARENT>Sundry Debtors</PARENT>\
<ISBILLWISEON>Yes</ISBILLWISEON><PARTYGSTIN>27ZZZZZ0000Z1Z5</PARTYGSTIN></LEDGER></TALLYMESSAGE>"
    );
}

#[test]
fn ledger_create_xml_billwise_defaults_to_no_when_unspecified() {
    let l = BookLedger {
        name: "Wages and Salary".into(),
        parent: Some("Direct Expenses".into()),
        opening_balance: None,
        is_billwise_on: None,
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    let xml = render_ledger_xml(&l);
    // ISBILLWISEON is explicit even though the book model left it unset.
    assert!(xml.contains("<ISBILLWISEON>No</ISBILLWISEON>"));
    assert!(!xml.contains("OPENINGBALANCE"));
}

#[test]
fn ledger_create_xml_uses_the_irregular_9_4d_duty_head_vocabulary_verbatim() {
    // §8.3: the state head is "State Tax", never "SGST" -- passed through
    // exactly as the source carried it, never synthesised or normalised.
    let l = BookLedger {
        name: "Output State Tax 9%".into(),
        parent: Some("Duties & Taxes".into()),
        opening_balance: None,
        is_billwise_on: None,
        party_gstin: None,
        tax_type: Some("GST".into()),
        gst_duty_head: Some("State Tax".into()),
        opening_bill_allocations: vec![],
    };
    let xml = render_ledger_xml(&l);
    assert!(xml.contains("<TAXTYPE>GST</TAXTYPE>"));
    assert!(xml.contains("<GSTDUTYHEAD>State Tax</GSTDUTYHEAD>"));
    assert!(!xml.contains("SGST"));
    // §9.1b: `&` in a group name must be escaped or the whole request is malformed.
    assert!(xml.contains("Duties &amp; Taxes"));
}

#[test]
fn ledger_create_xml_never_emits_taxtype_others() {
    // The 2026-09-14 rehearsal bug: every ledger, including a bank account
    // and a wages ledger, carried `<TAXTYPE>Others</TAXTYPE>` -- Tally's own
    // inert default, not a real classification, and not appropriate outside
    // Duties & Taxes. Tally answered CREATED=0 EXCEPTIONS=17.
    let l = BookLedger {
        name: "HDFC Bank 1649".into(),
        parent: Some("Bank Accounts".into()),
        opening_balance: Some("-5013.35".into()),
        is_billwise_on: Some(false),
        party_gstin: None,
        tax_type: Some("Others".into()),
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    assert!(!render_ledger_xml(&l).contains("TAXTYPE"));
}

#[test]
fn ledger_create_xml_never_emits_taxtype_outside_duties_and_taxes() {
    // A real, non-"Others" tax_type value must still be withheld if the
    // ledger is not parented under Duties & Taxes.
    let l = BookLedger {
        name: "GST Paid".into(),
        parent: Some("Loans & Advances (Asset)".into()),
        opening_balance: None,
        is_billwise_on: None,
        party_gstin: None,
        tax_type: Some("GST".into()),
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    assert!(!render_ledger_xml(&l).contains("TAXTYPE"));
}

#[test]
fn stock_item_create_xml_golden() {
    let s = BookStockItem {
        name: "Sodium Bicarbonate".into(),
        parent: Some("Chemicals".into()),
        base_unit: Some("Kgs".into()),
        opening_qty: Some("100".into()),
        opening_rate: Some("50.00".into()),
        opening_value: Some("5000.00".into()),
        gst_applicable: Some("Applicable".into()),
        hsn_code: Some("28362000".into()),
    };
    let xml = render_stock_item_xml(&s);
    assert_eq!(
        xml,
        "<TALLYMESSAGE><STOCKITEM NAME=\"Sodium Bicarbonate\" ACTION=\"Create\">\
<NAME>Sodium Bicarbonate</NAME><PARENT>Chemicals</PARENT><BASEUNITS>Kgs</BASEUNITS>\
<OPENINGBALANCE>100</OPENINGBALANCE>\
<OPENINGRATE>50.00</OPENINGRATE><OPENINGVALUE>5000.00</OPENINGVALUE>\
<GSTAPPLICABLE>Applicable</GSTAPPLICABLE><HSNCODE>28362000</HSNCODE></STOCKITEM></TALLYMESSAGE>"
    );
}

#[test]
fn stock_item_create_xml_omits_opening_balance_when_zero() {
    let s = BookStockItem {
        name: "Sample Item".into(),
        parent: Some("Primary".into()),
        base_unit: Some("Kgs".into()),
        opening_qty: Some("0".into()),
        opening_rate: Some("0".into()),
        opening_value: Some("0.00".into()),
        gst_applicable: None,
        hsn_code: None,
    };
    let xml = render_stock_item_xml(&s);
    assert!(!xml.contains("OPENINGBALANCE"));
    assert!(!xml.contains("OPENINGRATE"));
    assert!(!xml.contains("OPENINGVALUE"));
}

// ---------------------------------------------------------------------------
// Voucher XML -- golden fixtures, mirroring §9.13/§9.12a
// ---------------------------------------------------------------------------

fn payment_voucher() -> BookVoucher {
    BookVoucher {
        source_guid: "src-1".into(),
        voucher_type: "Payment".into(),
        date: "2026-04-05".into(),
        voucher_number: Some("59".into()),
        narration: Some("UPI payment".into()),
        party: Some("HDFC Bank 1649".into()),
        is_invoice_mode: false,
        ledger_lines: vec![
            BookLedgerLine {
                ledger: "HDFC Bank 1649".into(),
                side: "Cr".into(),
                amount: "30000.00".into(),
                bill_allocations: vec![],
            },
            BookLedgerLine {
                ledger: "Labour Charges".into(),
                side: "Dr".into(),
                amount: "30000.00".into(),
                bill_allocations: vec![],
            },
        ],
        inventory_lines: vec![],
    }
}

#[test]
fn payment_voucher_xml_is_dr_first_with_effective_date_and_counterparty_party() {
    let voucher = payment_voucher();
    let xml = render_accounting_voucher_xml(&voucher, REMOTE_ID, ATTRIBUTION_ID).unwrap();
    // §9.13: Dr leg first regardless of input order; EFFECTIVEDATE present;
    // PARTYLEDGERNAME is the counterparty (Dr side for a Payment), not the bank.
    let dr_pos = xml.find("Labour Charges").unwrap();
    let cr_pos = xml.find("HDFC Bank 1649</LEDGERNAME>").unwrap();
    assert!(dr_pos < cr_pos, "Dr leg must render before Cr leg");
    assert!(xml.contains("<EFFECTIVEDATE>20260405</EFFECTIVEDATE>"));
    assert!(xml.contains("<PARTYLEDGERNAME>Labour Charges</PARTYLEDGERNAME>"));
    assert!(
        xml.contains("<AMOUNT>-30000.00</AMOUNT>"),
        "Dr amount must be negative on the wire"
    );
    assert!(xml.contains(&format!("[BRIDGE-LAB:{ATTRIBUTION_ID}]")));
    assert!(xml.contains("OBJVIEW=\"Accounting Voucher View\""));
    assert!(xml.contains("VCHTYPE=\"Payment\""));
}

#[test]
fn contra_voucher_has_no_party_ledger_name() {
    let mut voucher = payment_voucher();
    voucher.voucher_type = "Contra".into();
    voucher.ledger_lines = vec![
        BookLedgerLine {
            ledger: "Cash".into(),
            side: "Dr".into(),
            amount: "100000.00".into(),
            bill_allocations: vec![],
        },
        BookLedgerLine {
            ledger: "HDFC Bank 1649".into(),
            side: "Cr".into(),
            amount: "100000.00".into(),
            bill_allocations: vec![],
        },
    ];
    let xml = render_accounting_voucher_xml(&voucher, REMOTE_ID, ATTRIBUTION_ID).unwrap();
    assert!(
        !xml.contains("PARTYLEDGERNAME"),
        "Contra names no counterparty (§9.13)"
    );
    assert!(xml.contains("<EFFECTIVEDATE>20260405</EFFECTIVEDATE>"));
}

#[test]
fn journal_voucher_keeps_book_order_and_has_no_effective_date_or_party() {
    let mut voucher = payment_voucher();
    voucher.voucher_type = "Journal".into();
    // Deliberately Cr-first in the book -- a Journal must NOT be reordered.
    let xml = render_accounting_voucher_xml(&voucher, REMOTE_ID, ATTRIBUTION_ID).unwrap();
    let first = xml.find("LEDGERNAME").unwrap();
    assert!(
        xml[first..].starts_with("LEDGERNAME>HDFC Bank 1649"),
        "Journal preserves input order"
    );
    assert!(!xml.contains("EFFECTIVEDATE"));
    assert!(!xml.contains("PARTYLEDGERNAME"));
}

#[test]
fn accounting_mode_sales_carries_bill_allocations_with_matching_sign() {
    let voucher = BookVoucher {
        source_guid: "src-2".into(),
        voucher_type: "Sales".into(),
        date: "2025-05-09".into(),
        voucher_number: Some("1".into()),
        narration: Some("Invoice 21".into()),
        party: Some("Sri Ram Cables Private Limited".into()),
        is_invoice_mode: false,
        ledger_lines: vec![
            BookLedgerLine {
                ledger: "Sri Ram Cables Private Limited".into(),
                side: "Dr".into(),
                amount: "673364.64".into(),
                bill_allocations: vec![BookBillAllocation {
                    name: None,
                    bill_type: "On Account".into(),
                    amount: "673364.64".into(),
                }],
            },
            BookLedgerLine {
                ledger: "Sales".into(),
                side: "Cr".into(),
                amount: "570648.00".into(),
                bill_allocations: vec![],
            },
        ],
        inventory_lines: vec![],
    };
    let xml = render_accounting_voucher_xml(&voucher, REMOTE_ID, ATTRIBUTION_ID).unwrap();
    assert!(
        !xml.contains("PARTYLEDGERNAME"),
        "accounting-mode Sales names no PARTYLEDGERNAME (observed wire shape)"
    );
    assert!(xml.contains("<BILLALLOCATIONS.LIST><NAME></NAME><BILLTYPE>On Account</BILLTYPE><AMOUNT>-673364.64</AMOUNT></BILLALLOCATIONS.LIST>"));
    assert!(xml.contains("ALLLEDGERENTRIES.LIST"));
}

#[test]
fn invoice_voucher_uses_ledgerentries_not_allledgerentries() {
    // §9.12's TRAP: ALLLEDGERENTRIES.LIST is silently discarded on an invoice
    // voucher. LEDGERENTRIES.LIST is the required element.
    let voucher = BookVoucher {
        source_guid: "src-3".into(),
        voucher_type: "Sales".into(),
        date: "2026-04-05".into(),
        voucher_number: Some("1".into()),
        narration: Some("Invoice".into()),
        party: Some("Akash Acid & Chemicals".into()),
        is_invoice_mode: true,
        ledger_lines: vec![
            BookLedgerLine {
                ledger: "Akash Acid & Chemicals".into(),
                side: "Dr".into(),
                amount: "118.00".into(),
                bill_allocations: vec![BookBillAllocation {
                    name: Some("Inv-1".into()),
                    bill_type: "New Ref".into(),
                    amount: "118.00".into(),
                }],
            },
            BookLedgerLine {
                ledger: "Output CGST 9%".into(),
                side: "Cr".into(),
                amount: "9.00".into(),
                bill_allocations: vec![],
            },
            BookLedgerLine {
                ledger: "Output SGST 9%".into(),
                side: "Cr".into(),
                amount: "9.00".into(),
                bill_allocations: vec![],
            },
        ],
        inventory_lines: vec![BookInventoryLine {
            stock_item: Some("Widget".into()),
            rate: Some("100.00/Nos".into()),
            qty: Some("1 Nos".into()),
            billed_qty: Some("1 Nos".into()),
            amount: Some("100.00".into()),
            godown: Some("Main Godown".into()),
            accounting_allocations: vec![BookAccountingAllocation {
                ledger: "Sales".into(),
                amount: "100.00".into(),
            }],
            batch_allocations: vec![],
        }],
    };
    let xml = render_invoice_voucher_xml(&voucher, REMOTE_ID, ATTRIBUTION_ID).unwrap();
    assert!(xml.contains("<LEDGERENTRIES.LIST>"));
    assert!(!xml.contains("ALLLEDGERENTRIES.LIST"));
    assert!(xml.contains("ISINVOICE>Yes"));
    assert!(xml.contains("OBJVIEW=\"Invoice Voucher View\""));
    assert!(xml.contains("<PARTYLEDGERNAME>Akash Acid &amp; Chemicals</PARTYLEDGERNAME>"));
    assert!(xml.contains("<ALLINVENTORYENTRIES.LIST>"));
    assert!(xml.contains("<BILLTYPE>New Ref</BILLTYPE>"));
    assert!(xml.contains("<ACCOUNTINGALLOCATIONS.LIST>"));
    assert!(xml.contains("<GODOWNNAME>Main Godown</GODOWNNAME>"));
}

#[test]
fn invoice_voucher_without_party_is_refused_before_any_xml_is_sent() {
    let mut voucher = payment_voucher();
    voucher.is_invoice_mode = true;
    voucher.party = None;
    voucher.inventory_lines = vec![BookInventoryLine {
        stock_item: Some("Widget".into()),
        rate: None,
        qty: None,
        billed_qty: None,
        amount: Some("1.00".into()),
        godown: None,
        accounting_allocations: vec![],
        batch_allocations: vec![],
    }];
    assert_eq!(
        render_voucher_message(&voucher, REMOTE_ID, ATTRIBUTION_ID),
        Err("lab_invoice_party_required".to_string())
    );
}

#[test]
fn service_inventory_line_omits_quantity_fields() {
    let line = BookInventoryLine {
        stock_item: Some("Consulting".into()),
        rate: None,
        qty: None,
        billed_qty: None,
        amount: Some("500.00".into()),
        godown: None,
        accounting_allocations: vec![],
        batch_allocations: vec![],
    };
    let xml = render_inventory_entry_xml(&line).unwrap();
    assert!(!xml.contains("ACTUALQTY"));
    assert!(!xml.contains("BILLEDQTY"));
    assert!(!xml.contains("<RATE>"));
    assert!(xml.contains("<AMOUNT>500.00</AMOUNT>"));
}

// ---------------------------------------------------------------------------
// Master read-back diff / existing-master collision
// ---------------------------------------------------------------------------

fn row(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn find_readback_row_matches_the_9_4d_fold() {
    let rows = vec![row(&[("NAME", "MB-PROBE-LEDGER-A")])];
    assert!(find_readback_row(&rows, "mb probe ledger a").is_some());
    assert!(find_readback_row(&rows, "an entirely different name").is_none());
}

#[test]
fn diff_ledger_flags_a_wrong_opening_balance() {
    let l = BookLedger {
        name: "Cash".into(),
        parent: Some("Cash-in-Hand".into()),
        opening_balance: Some("100.00".into()),
        is_billwise_on: None,
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    let good = row(&[("PARENT", "Cash-in-Hand"), ("OPENINGBALANCE", "100.00")]);
    assert!(diff_ledger(&l, &good).is_empty());
    let bad = row(&[("PARENT", "Cash-in-Hand"), ("OPENINGBALANCE", "50.00")]);
    let mismatches = diff_ledger(&l, &bad);
    assert_eq!(mismatches.len(), 1);
    assert!(mismatches[0].contains("opening_balance"));
}

#[test]
fn diff_ledger_treats_equal_decimals_as_equal_regardless_of_formatting() {
    let l = BookLedger {
        name: "Cash".into(),
        parent: None,
        opening_balance: Some("0.00".into()),
        is_billwise_on: None,
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    let observed = row(&[("OPENINGBALANCE", "0")]);
    assert!(diff_ledger(&l, &observed).is_empty());
}

#[test]
fn diff_parented_uses_the_9_4d_fold_not_exact_equality() {
    let item = BookNamedParent {
        name: "Main Godown".into(),
        parent: Some("Sub-Location".into()),
    };
    let observed = row(&[("PARENT", "Sub Location")]);
    assert!(diff_parented("godown", &item, &observed).is_empty());
}

#[test]
fn diff_stock_item_flags_a_wrong_parent_but_not_an_unspecified_one() {
    let s = BookStockItem {
        name: "Widget".into(),
        parent: Some("Chemicals".into()),
        base_unit: None,
        opening_qty: None,
        opening_rate: None,
        opening_value: None,
        gst_applicable: None,
        hsn_code: None,
    };
    let mismatched = diff_stock_item(&s, &row(&[("PARENT", "Consumables")]));
    assert_eq!(mismatched.len(), 1);
    let s_no_parent_check = BookStockItem { parent: None, ..s };
    assert!(diff_stock_item(&s_no_parent_check, &row(&[("PARENT", "Anything")])).is_empty());
}

// ---------------------------------------------------------------------------
// Resume: narration marker + type/date/amount fingerprint
// ---------------------------------------------------------------------------

fn observed(
    voucher_type: &str,
    date: &str,
    number: Option<&str>,
    narration: Option<&str>,
    entries: &[(&str, &str, &str)],
) -> ObservedVoucher {
    ObservedVoucher {
        date: date.to_string(),
        voucher_number: number.map(str::to_string),
        voucher_type: Some(voucher_type.to_string()),
        narration: narration.map(str::to_string),
        is_cancelled: false,
        ledger_entries: entries
            .iter()
            .map(|(l, d, a)| (l.to_string(), d.to_string(), a.to_string()))
            .collect(),
    }
}

#[test]
fn voucher_already_verified_matches_by_narration_marker_and_amounts() {
    let expected = payment_voucher_with_marker(&ATTRIBUTION_ID.to_string());
    let matching = observed(
        "Payment",
        "20260405",
        Some("59"),
        Some(&format!("UPI payment [BRIDGE-LAB:{ATTRIBUTION_ID}]")),
        &[
            ("HDFC Bank 1649", "No", "30000.00"),
            ("Labour Charges", "Yes", "-30000.00"),
        ],
    );
    assert!(voucher_already_verified(&expected, &[matching]));
}

fn payment_voucher_with_marker(_marker: &str) -> BookVoucher {
    // The book model never carries the marker itself (it is stamped at
    // render time); resume matches on voucher_number when present, as here.
    payment_voucher()
}

#[test]
fn voucher_already_verified_rejects_a_content_only_match_without_number_or_marker() {
    let mut expected = payment_voucher();
    expected.voucher_number = None; // forces reliance on the marker alone
    let same_shape_different_voucher = observed(
        "Payment",
        "20260405",
        None,
        Some("An unrelated payment, same date and amount"),
        &[
            ("HDFC Bank 1649", "No", "30000.00"),
            ("Labour Charges", "Yes", "-30000.00"),
        ],
    );
    // §9.3: content (date/ledger/amount) alone is not an attribution key.
    assert!(!voucher_already_verified(
        &expected,
        &[same_shape_different_voucher]
    ));
}

#[test]
fn voucher_already_verified_ignores_a_cancelled_voucher() {
    let expected = payment_voucher();
    let mut cancelled = observed(
        "Payment",
        "20260405",
        Some("59"),
        Some("UPI payment"),
        &[
            ("HDFC Bank 1649", "No", "30000.00"),
            ("Labour Charges", "Yes", "-30000.00"),
        ],
    );
    cancelled.is_cancelled = true;
    assert!(!voucher_already_verified(&expected, &[cancelled]));
}

#[test]
fn narration_marker_extracts_the_bracketed_uuid() {
    assert_eq!(
        narration_marker(Some(
            "Some text [BRIDGE-LAB:00000000-0000-4000-8000-000000000002]"
        )),
        Some("00000000-0000-4000-8000-000000000002".to_string())
    );
    assert_eq!(narration_marker(Some("no marker here")), None);
    assert_eq!(narration_marker(None), None);
}

// ---------------------------------------------------------------------------
// Voucher read-back parsing (synthetic Tally export)
// ---------------------------------------------------------------------------

#[test]
fn parses_ledger_entries_and_bill_allocations_per_voucher() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<VOUCHER><DATE>20260405</DATE><VOUCHERNUMBER>59</VOUCHERNUMBER><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME>\
<PARTYLEDGERNAME>Labour Charges</PARTYLEDGERNAME><GUID>g-1</GUID><ISCANCELLED>No</ISCANCELLED>\
<ALLLEDGERENTRIES.LIST><LEDGERNAME>HDFC Bank 1649</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>30000.00</AMOUNT></ALLLEDGERENTRIES.LIST>\
<ALLLEDGERENTRIES.LIST><LEDGERNAME>Labour Charges</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-30000.00</AMOUNT></ALLLEDGERENTRIES.LIST>\
</VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let rows = parse_voucher_readback_nested(xml).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].voucher_number.as_deref(), Some("59"));
    assert_eq!(rows[0].ledger_entries.len(), 2);
    assert_eq!(
        rows[0].ledger_entries[1],
        (
            "Labour Charges".to_string(),
            "Yes".to_string(),
            "-30000.00".to_string()
        )
    );
}

#[test]
fn a_non_voucher_child_of_collection_is_refused() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<CMPINFO><VOUCHER>1</VOUCHER></CMPINFO></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(
        parse_voucher_readback_nested(xml),
        Err("agent_read_protocol_invalid".to_string())
    );
}

// ---------------------------------------------------------------------------
// parse_book_value: inline vs book_path
// ---------------------------------------------------------------------------

#[test]
fn parse_book_value_reads_inline_json() {
    let args = json!({"masters": {"units": [{"name": "Nos"}]}});
    let masters: BookMasters = parse_book_value(&args, "masters", "masters").unwrap();
    assert_eq!(masters.units.len(), 1);
}

#[test]
fn parse_book_value_reads_a_book_path_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.json");
    fs::write(
        &path,
        json!({"masters": {"units": [{"name": "Kgs"}]}, "vouchers": []}).to_string(),
    )
    .unwrap();
    let args = json!({"book_path": path.to_string_lossy()});
    let masters: BookMasters = parse_book_value(&args, "masters", "masters").unwrap();
    assert_eq!(masters.units[0].name, "Kgs");
}

#[test]
fn parse_book_value_requires_one_of_inline_or_book_path() {
    let args = json!({});
    let result: Result<BookMasters, ToolFailure> = parse_book_value(&args, "masters", "masters");
    assert_eq!(result.unwrap_err().code, "masters_or_book_path_required");
}

// ---------------------------------------------------------------------------
// Identity-guard helper and master batching
// ---------------------------------------------------------------------------

#[test]
fn identity_matches_requested_guid_is_case_insensitive() {
    let identity = VerifiedCompanyIdentity::test_fixture(
        "BRIDGE REHEARSAL",
        "89B0CC46-E3B8-4809-8FC7-E29EB2AE547D",
    );
    assert!(identity_matches_requested_guid(
        &identity,
        "89b0cc46-e3b8-4809-8fc7-e29eb2ae547d"
    ));
    assert!(!identity_matches_requested_guid(
        &identity,
        "2864b4ac-e5a3-4efc-9d2b-7593928d8f8b"
    ));
}

#[test]
fn chunked_masters_splits_only_the_requested_kind() {
    let masters = BookMasters {
        ledgers: (0..5)
            .map(|i| BookLedger {
                name: format!("L{i}"),
                parent: None,
                opening_balance: None,
                is_billwise_on: None,
                party_gstin: None,
                tax_type: None,
                gst_duty_head: None,
                opening_bill_allocations: vec![],
            })
            .collect(),
        units: vec![BookUnit {
            name: "Nos".into(),
            is_simple_unit: None,
            decimal_places: None,
        }],
        ..Default::default()
    };
    let chunk = chunked_masters(&masters, MasterKind::Ledger, 2, 2);
    assert_eq!(
        chunk
            .ledgers
            .iter()
            .map(|l| l.name.clone())
            .collect::<Vec<_>>(),
        vec!["L2", "L3"]
    );
    assert!(
        chunk.units.is_empty(),
        "chunking one kind must not carry over another"
    );
}

#[test]
fn master_import_order_matches_the_plan() {
    assert_eq!(
        MasterKind::IMPORT_ORDER.map(MasterKind::tally_type),
        [
            "Unit",
            "Godown",
            "StockGroup",
            "Group",
            "Ledger",
            "StockItem"
        ]
    );
}

#[test]
fn amounts_equal_ignores_decimal_formatting_but_not_value() {
    assert!(amounts_equal("0.00", "0"));
    assert!(amounts_equal("100.00", "100"));
    assert!(!amounts_equal("100.00", "100.01"));
}

// ---------------------------------------------------------------------------
// Tally default masters: default skip, default opening alter, true collision
// still refused, reserved parent normalisation.
// ---------------------------------------------------------------------------

fn default_cash_ledger(opening: &str) -> BookLedger {
    BookLedger {
        name: "Cash".into(),
        parent: Some("Cash-in-Hand".into()),
        opening_balance: Some(opening.into()),
        is_billwise_on: None,
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    }
}

fn default_pl_ledger() -> BookLedger {
    BookLedger {
        name: "Profit & Loss A/c".into(),
        parent: None,
        opening_balance: Some("0.00".into()),
        is_billwise_on: None,
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    }
}

#[test]
fn is_default_ledger_recognises_cash_under_cash_in_hand() {
    assert!(is_default_ledger("Cash", "Cash-in-Hand"));
    // §9.4d fold applies to the parent comparison too.
    assert!(is_default_ledger("Cash", "cash in hand"));
}

#[test]
fn is_default_ledger_recognises_profit_and_loss_under_the_reserved_primary() {
    // Sanitized form `tolerant_xml` actually produces for the raw U+0004
    // metadata prefix (see `bridge_tally_protocol::TALLY_SANITIZED_ROOT_MARKER`).
    assert!(is_default_ledger(
        "Profit & Loss A/c",
        "\u{fffd}#4; Primary"
    ));
    assert!(is_default_ledger("Profit & Loss A/c", "Primary"));
}

#[test]
fn is_default_ledger_rejects_a_same_name_ledger_under_a_different_parent() {
    // A "Cash" ledger moved (or created by a user) under some other group is
    // not Tally's own default -- it must remain a true collision, not be
    // silently treated as the default and skipped.
    assert!(!is_default_ledger("Cash", "Bank Accounts"));
    assert!(!is_default_ledger(
        "Profit & Loss A/c",
        "Current Liabilities"
    ));
}

#[test]
fn is_default_ledger_does_not_recognise_an_unrelated_name() {
    assert!(!is_default_ledger(
        "Sri Ram Cables Private Limited",
        "Primary"
    ));
}

#[test]
fn a_requested_group_named_as_the_reserved_root_marker_is_recognised_as_such() {
    // The exact defect this pre-flight caught in the rehearsal book: a
    // requested Group literally named with Tally's sanitized U+0004 marker
    // (the self-referential root) must be recognised so the caller can
    // refuse it, rather than silently Creating a garbled-name group -- it
    // never collision-matches Tally's own plainly-named "Primary" row.
    assert!(is_tally_reserved_root("\u{fffd}#4; Primary"));
    assert!(is_tally_reserved_root("Primary"));
    assert!(!is_tally_reserved_root("Sundry Debtors"));
}

#[test]
fn is_default_group_reads_reserved_name_not_the_group_name() {
    assert!(is_default_group(&row(&[
        ("NAME", "Sundry Debtors"),
        ("RESERVEDNAME", "Sundry Debtors")
    ])));
    // A user-created group with the same displayed name as a reserved one
    // but an empty RESERVEDNAME is not a default.
    assert!(!is_default_group(&row(&[
        ("NAME", "Sundry Debtors"),
        ("RESERVEDNAME", "")
    ])));
    assert!(!is_default_group(&row(&[("NAME", "Custom Group")])));
}

#[test]
fn ledger_alter_fields_is_empty_when_the_default_already_matches_the_book() {
    // "default skip": no diff, no Alter is offered.
    let l = default_cash_ledger("0.00");
    let observed = row(&[("PARENT", "Cash-in-Hand"), ("OPENINGBALANCE", "0.00")]);
    assert!(ledger_alter_fields(&l, &observed).is_empty());
}

#[test]
fn ledger_alter_fields_offers_only_the_changed_opening_balance() {
    // "default opening alter": book differs from target -> a partial Alter
    // carrying only OPENINGBALANCE, never a Create (which would overwrite).
    let l = default_cash_ledger("5000.00");
    let observed = row(&[("PARENT", "Cash-in-Hand"), ("OPENINGBALANCE", "0.00")]);
    let fields = ledger_alter_fields(&l, &observed);
    assert_eq!(fields, vec![("OPENINGBALANCE", "5000.00".to_string())]);
}

#[test]
fn ledger_alter_fields_never_offers_a_gst_field_alter_9_4d() {
    // §8.3: GST fields are settable at Create but silently dropped at Alter
    // -- never offered here even when they differ from the target.
    let mut l = default_cash_ledger("0.00");
    l.tax_type = Some("GST".into());
    l.gst_duty_head = Some("State Tax".into());
    let observed = row(&[
        ("PARENT", "Cash-in-Hand"),
        ("OPENINGBALANCE", "0.00"),
        ("TAXTYPE", "Others"),
        ("GSTDUTYHEAD", "CGST"),
    ]);
    assert!(ledger_alter_fields(&l, &observed).is_empty());
}

#[test]
fn render_ledger_alter_xml_carries_only_the_given_fields() {
    let xml = render_ledger_alter_xml("Cash", &[("OPENINGBALANCE", "5000.00".to_string())]);
    assert_eq!(
        xml,
        "<TALLYMESSAGE><LEDGER NAME=\"Cash\" ACTION=\"Alter\">\
<OPENINGBALANCE>5000.00</OPENINGBALANCE></LEDGER></TALLYMESSAGE>"
    );
    // Never a Create, and never a field beyond what was asked for.
    assert!(!xml.contains("ACTION=\"Create\""));
    assert!(!xml.contains("PARENT"));
}

#[test]
fn default_ledger_and_default_group_precheck_classification_end_to_end() {
    // A compact end-to-end check of the precheck classification a real
    // `lab_import_masters` call performs: for each requested master, decide
    // default-skip vs. true-collision the same way the tool body does.
    let requested_ledgers = [default_cash_ledger("5000.00"), default_pl_ledger()];
    let existing_ledger_rows = [
        row(&[
            ("NAME", "Cash"),
            ("PARENT", "Cash-in-Hand"),
            ("OPENINGBALANCE", "0.00"),
        ]),
        row(&[
            ("NAME", "Profit & Loss A/c"),
            ("PARENT", "\u{fffd}#4; Primary"),
            ("OPENINGBALANCE", "0.00"),
        ]),
    ];
    let mut collisions = Vec::new();
    let mut alters = Vec::new();
    for ledger in &requested_ledgers {
        let existing = find_readback_row(&existing_ledger_rows, &ledger.name).unwrap();
        let observed_parent = existing.get("PARENT").map(String::as_str).unwrap_or("");
        if is_default_ledger(&ledger.name, observed_parent) {
            alters.push((ledger.name.clone(), ledger_alter_fields(ledger, existing)));
        } else {
            collisions.push(ledger.name.clone());
        }
    }
    assert!(collisions.is_empty(), "both are real Tally defaults");
    assert_eq!(alters[0].0, "Cash");
    assert_eq!(alters[0].1, vec![("OPENINGBALANCE", "5000.00".to_string())]);
    assert_eq!(alters[1].0, "Profit & Loss A/c");
    assert!(
        alters[1].1.is_empty(),
        "Profit & Loss A/c already matches -> default skip, no Alter"
    );

    // "true collision still refused": a non-default same-name ledger.
    let existing_debtor_rows = vec![row(&[
        ("NAME", "Sri Ram Cables Private Limited"),
        ("PARENT", "Sundry Debtors"),
    ])];
    let requested_debtor = BookLedger {
        name: "Sri Ram Cables Private Limited".into(),
        parent: Some("Sundry Debtors".into()),
        opening_balance: None,
        is_billwise_on: None,
        party_gstin: None,
        tax_type: None,
        gst_duty_head: None,
        opening_bill_allocations: vec![],
    };
    let existing = find_readback_row(&existing_debtor_rows, &requested_debtor.name).unwrap();
    let observed_parent = existing.get("PARENT").map(String::as_str).unwrap_or("");
    assert!(
        !is_default_ledger(&requested_debtor.name, observed_parent),
        "an ordinary pre-existing ledger is never treated as a default"
    );
    let _ = requested_debtor.parent; // constructed only to exercise the classification above
}

// ---------------------------------------------------------------------------
// Explicit Tally-rejection reporting (2026-09-14 rehearsal: 17 ledgers sent,
// Tally answered CREATED=0 ERRORS=0 EXCEPTIONS=17, no mutation).
// ---------------------------------------------------------------------------

/// The exact response Tally returned for the failing rehearsal (captured in
/// the request-evidence directory as this batch's `.response.xml`).
const REHEARSAL_REJECTION_RESPONSE: &str = "<RESPONSE>\
 <CREATED>0</CREATED>\
 <ALTERED>0</ALTERED>\
 <DELETED>0</DELETED>\
 <LASTVCHID>0</LASTVCHID>\
 <LASTMID>0</LASTMID>\
 <COMBINED>0</COMBINED>\
 <IGNORED>0</IGNORED>\
 <ERRORS>0</ERRORS>\
 <CANCELLED>0</CANCELLED>\
 <EXCEPTIONS>17</EXCEPTIONS>\
</RESPONSE>";

const CLEAN_RESPONSE: &str = "<RESPONSE>\
 <CREATED>17</CREATED>\
 <ALTERED>0</ALTERED>\
 <DELETED>0</DELETED>\
 <LASTVCHID>0</LASTVCHID>\
 <LASTMID>0</LASTMID>\
 <COMBINED>0</COMBINED>\
 <IGNORED>0</IGNORED>\
 <ERRORS>0</ERRORS>\
 <CANCELLED>0</CANCELLED>\
 <EXCEPTIONS>0</EXCEPTIONS>\
</RESPONSE>";

#[test]
fn tally_rejected_true_for_the_exact_rehearsal_response() {
    let outcome = bridge_tally_protocol::parse_import_outcome(REHEARSAL_REJECTION_RESPONSE)
        .expect("valid RESPONSE shape");
    assert!(tally_rejected(outcome.counters()));
    assert_eq!(outcome.counters().created, 0);
    assert_eq!(outcome.counters().exceptions, 17);
}

#[test]
fn tally_rejected_false_for_a_clean_response() {
    let outcome =
        bridge_tally_protocol::parse_import_outcome(CLEAN_RESPONSE).expect("valid RESPONSE shape");
    assert!(!tally_rejected(outcome.counters()));
}

#[test]
fn tally_rejected_true_when_errors_reported_even_with_zero_exceptions() {
    let response = "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED>\
<LASTVCHID>0</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED>\
<ERRORS>2</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>";
    let outcome =
        bridge_tally_protocol::parse_import_outcome(response).expect("valid RESPONSE shape");
    assert!(tally_rejected(outcome.counters()));
}

#[test]
fn tally_rejection_message_reports_counters_and_no_line_errors_when_absent() {
    let outcome = bridge_tally_protocol::parse_import_outcome(REHEARSAL_REJECTION_RESPONSE)
        .expect("valid RESPONSE shape");
    let line_errors = extract_line_error_texts(REHEARSAL_REJECTION_RESPONSE);
    assert!(
        line_errors.is_empty(),
        "the captured rehearsal response carried no LINEERROR text"
    );
    let message = tally_rejection_message("Ledger", outcome.counters(), &line_errors);
    assert_eq!(
        message,
        "Ledger rejected by Tally: CREATED=0 ALTERED=0 ERRORS=0 EXCEPTIONS=17"
    );
    assert!(!message.contains("LINEERROR"));
}

#[test]
fn extract_line_error_texts_reads_every_lineerror_element() {
    let response = "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED>\
<LASTVCHID>0</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED>\
<ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>2</EXCEPTIONS>\
<LINEERROR>Could not set OPENINGBALANCE : Duplicate name</LINEERROR>\
<LINEERROR>Vch/Ledger deletion/alteration is not permitted</LINEERROR></RESPONSE>";
    let errors = extract_line_error_texts(response);
    assert_eq!(
        errors,
        vec![
            "Could not set OPENINGBALANCE : Duplicate name".to_string(),
            "Vch/Ledger deletion/alteration is not permitted".to_string(),
        ]
    );
    let outcome =
        bridge_tally_protocol::parse_import_outcome(response).expect("valid RESPONSE shape");
    let message = tally_rejection_message("Ledger", outcome.counters(), &errors);
    assert!(message.contains(
        "LINEERROR: Could not set OPENINGBALANCE : Duplicate name; \
Vch/Ledger deletion/alteration is not permitted"
    ));
}

#[test]
fn tally_import_counters_json_surfaces_every_counter() {
    let outcome = bridge_tally_protocol::parse_import_outcome(REHEARSAL_REJECTION_RESPONSE)
        .expect("valid RESPONSE shape");
    let json = tally_import_counters_json(outcome.counters());
    assert_eq!(json["created"], 0);
    assert_eq!(json["errors"], 0);
    assert_eq!(json["exceptions"], 17);
}
