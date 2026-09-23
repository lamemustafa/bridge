use super::*;

#[test]
fn opposing_unallocated_direction_is_preserved_and_only_gross_exposure_is_ranked() {
    let bills = vec![OpenBillRow {
        party: "Synthetic Party".into(),
        reference: "INV-1".into(),
        bill_date: "20260901".into(),
        due_date: "20260901".into(),
        amount: bridge_tally_core::ExactDecimal::parse("100").unwrap(),
        age_days: Some(5),
        kind: ExposureDirection::Receivable,
    }];
    let unallocated = vec![UnallocatedParty {
        party: "Synthetic Party".into(),
        amount: bridge_tally_core::ExactDecimal::parse("30").unwrap(),
        direction: ExposureDirection::Payable,
    }];
    let ranked = ranked_parties_from_exposure(&bills, &unallocated, 1).unwrap();
    let party = &ranked[0];
    assert_eq!(party["gross_exposure"], "130");
    assert_eq!(party["gross_billed"], "100");
    assert_eq!(party["billed_receivable"], "100");
    assert_eq!(party["billed_payable"], "0");
    assert_eq!(party["unallocated_receivable"], "0");
    assert_eq!(party["unallocated_payable"], "30");
    assert_eq!(party["gross_unallocated"], "30");
    for absent in ["outstanding_total", "net_due", "billed", "unallocated"] {
        assert!(party.get(absent).is_none(), "ambiguous total {absent}");
    }
    let residuals = unallocated_totals_from_parties(&unallocated).unwrap();
    assert_eq!(
        residuals,
        json!({"receivable":"0", "payable":"30", "gross_unallocated":"30"})
    );
    let billed = outstanding_totals_from_open_bills(&bills).unwrap();
    assert_eq!(billed["scope"], "open_bills_only");
    assert_eq!(billed["receivable"], "100");
    assert_eq!(billed["payable"], "0");
}

/// bridge#551: a currency refusal names its ledger on the MCP result, masked
/// like any party name; a reason without a ledger carries none.
#[test]
fn a_withheld_result_names_its_ledger_under_redaction() {
    let mut reason =
        crate::tally::OutstandingsPartialReason::code("ledger_currency_base_unmatched");
    reason.foreign_currency_ledger_name = Some("Synthetic FX Debtor".to_string());
    let plain = partial_payload(&reason, Redaction::None);
    assert_eq!(plain["partial_reason"], "ledger_currency_base_unmatched");
    assert_eq!(plain["ledger"], "Synthetic FX Debtor");
    let masked = partial_payload(&reason, Redaction::MaskParties);
    assert_eq!(masked["ledger"], json!(mask("Synthetic FX Debtor")));
    assert_ne!(masked["ledger"], "Synthetic FX Debtor");
    let unnamed = partial_payload(
        &crate::tally::OutstandingsPartialReason::code("native_bills_report_drifted"),
        Redaction::MaskParties,
    );
    assert_eq!(
        unnamed,
        json!({"state":"partial","partial_reason":"native_bills_report_drifted"})
    );
}

/// bridge#551: the foreign-balance refusal, which predates the currency
/// classification, names its ledger on the MCP result through the same
/// party-name redaction.
#[test]
fn the_foreign_balance_refusal_names_its_ledger_under_redaction() {
    let reason = crate::tally::OutstandingsPartialReason::foreign_currency_ledger_balance(
        "Synthetic FX Debtor".to_string(),
    );
    let plain = partial_payload(&reason, Redaction::None);
    assert_eq!(
        plain,
        json!({
            "state": "partial",
            "partial_reason": "company_foreign_currency_ledger_balance",
            "ledger": "Synthetic FX Debtor",
        })
    );
    let masked = partial_payload(&reason, Redaction::MaskParties);
    assert_eq!(masked["ledger"], json!(mask("Synthetic FX Debtor")));
    assert_ne!(masked["ledger"], "Synthetic FX Debtor");
}
