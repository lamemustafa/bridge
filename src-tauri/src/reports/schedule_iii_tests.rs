use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::{PartyLedgerMasterFieldObservation, PartyLedgerMasterFields};

use super::*;
use crate::tally::OutstandingsCurrencyAssertion;

/// Tally's reserved root as every Bridge reader returns it
/// (`TALLY_PROTOCOL_REFERENCE.md` §1.1(d)); a bare `Primary` would name a group.
const RESERVED_ROOT: &str = "\u{fffd}#4; Primary";

fn row(name: &str, parent: &str, balance: &str) -> PartyLedgerMasterRow {
    PartyLedgerMasterRow {
        name: name.to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned(parent.to_string()),
        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
        fields: PartyLedgerMasterFields::default(),
        guid: format!("guid-{name}"),
        master_id: format!("id-{name}"),
        alter_id: "1".to_string(),
        opening_balance: ExactDecimal::zero(),
        closing_balance: Some(ExactDecimal::parse(balance).unwrap()),
    }
}

#[test]
fn maps_only_immutable_group_evidence_and_lists_everything_else() {
    let source = PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260331").unwrap(),
        rows: vec![
            row("Customer", "Regional customers", "-100"),
            row("Unknown", "Custom", "100"),
        ],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![
            TallyNamedMaster {
                name: "Regional customers".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(
                    "Renamed debtor root".to_string(),
                ),
                reserved_name: Some("".to_string()),
            },
            TallyNamedMaster {
                name: "Renamed debtor root".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
                reserved_name: Some("Sundry Debtors".to_string()),
            },
            TallyNamedMaster {
                name: "Custom".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
                reserved_name: Some("".to_string()),
            },
        ],
    };
    let view = build_schedule_iii_view(&source).unwrap();
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.lines[0].row_indices, vec![0]);
    assert_eq!(view.exclusions.len(), 1);
    assert!(view.exclusions[0].reason.contains("mapping decision"));
    assert!(view.difference.is_zero());
}

#[test]
fn contra_signed_sundry_debtor_is_excluded_not_netted_against_its_group_subtotal() {
    let source = PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![
            row("Customer advance", "Sundry Debtors", "100"),
            row("Receivable", "Sundry Debtors", "-300"),
        ],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![TallyNamedMaster {
            name: "Sundry Debtors".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
            reserved_name: Some("Sundry Debtors".to_string()),
        }],
    };

    let view = build_schedule_iii_view(&source).unwrap();
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.lines[0].row_indices, vec![1]);
    assert_eq!(view.lines[0].total.as_str(), "-300");
    assert_eq!(view.exclusions.len(), 1);
    assert!(view.exclusions[0]
        .reason
        .contains("credit-balance Sundry Debtors"));
}

#[test]
fn contra_signed_sundry_creditor_is_excluded_not_netted_against_its_group_subtotal() {
    let source = PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![
            row("Supplier advance", "Sundry Creditors", "-200"),
            row("Payable", "Sundry Creditors", "300"),
        ],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![TallyNamedMaster {
            name: "Sundry Creditors".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
            reserved_name: Some("Sundry Creditors".to_string()),
        }],
    };

    let view = build_schedule_iii_view(&source).unwrap();
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.lines[0].row_indices, vec![1]);
    assert_eq!(view.lines[0].total.as_str(), "300");
    assert_eq!(view.exclusions.len(), 1);
    assert!(view.exclusions[0]
        .reason
        .contains("debit-balance Sundry Creditors"));
}

#[test]
fn cash_in_hand_and_bank_accounts_keep_separate_group_subtotals_and_totals() {
    let source = PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![
            row("Bank balance", "Bank Accounts", "-200"),
            row("Petty cash", "Cash-in-Hand", "-300"),
        ],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![
            TallyNamedMaster {
                name: "Bank Accounts".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
                reserved_name: Some("Bank Accounts".to_string()),
            },
            TallyNamedMaster {
                name: "Cash-in-Hand".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
                reserved_name: Some("Cash-in-Hand".to_string()),
            },
        ],
    };

    let view = build_schedule_iii_view(&source).unwrap();
    assert_eq!(view.lines.len(), 2);
    assert!(view.lines.iter().any(|line| {
        line.label == "Bank Accounts group subtotal"
            && line.total.as_str() == "-200"
            && line.row_indices == vec![0]
    }));
    assert!(view.lines.iter().any(|line| {
        line.label == "Cash-in-Hand group subtotal"
            && line.total.as_str() == "-300"
            && line.row_indices == vec![1]
    }));
    assert_eq!(view.debit_total.as_str(), "500");
    assert!(view.credit_total.is_zero());
    assert_eq!(view.difference.as_str(), "-500");
}

#[test]
fn contra_signed_bank_account_is_excluded_not_netted_against_its_group_subtotal() {
    let source = PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![
            row("Overdraft", "Bank Accounts", "200"),
            row("Petty cash", "Cash-in-Hand", "-300"),
        ],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![
            TallyNamedMaster {
                name: "Bank Accounts".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
                reserved_name: Some("Bank Accounts".to_string()),
            },
            TallyNamedMaster {
                name: "Cash-in-Hand".to_string(),
                parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
                reserved_name: Some("Cash-in-Hand".to_string()),
            },
        ],
    };

    let view = build_schedule_iii_view(&source).unwrap();
    assert_eq!(view.lines.len(), 1);
    assert_eq!(view.lines[0].row_indices, vec![1]);
    assert_eq!(view.lines[0].total.as_str(), "-300");
    assert_eq!(view.exclusions.len(), 1);
    assert!(view.exclusions[0]
        .reason
        .contains("credit-balance Bank Accounts"));
}

#[test]
fn empty_closing_balance_is_excluded_not_manufactured_as_zero() {
    let mut missing = row("Unestablished", "Sundry Debtors", "-1");
    missing.closing_balance = None;
    let source = PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![missing],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![],
    };

    let view = build_schedule_iii_view(&source).unwrap();
    assert!(view.lines.is_empty());
    assert!(view.debit_total.is_zero());
    assert!(view.credit_total.is_zero());
    assert_eq!(view.exclusions.len(), 1);
    assert!(view.exclusions[0].reason.contains("not established"));
}
