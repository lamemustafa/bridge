use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::{PartyLedgerMasterFieldObservation, PartyLedgerMasterFields};

use super::*;
use crate::reports::party_ledger_master::build_party_ledger_master_workbook;
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
    let view = build_schedule_iii_view(&workbook(source), &[]).unwrap();
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

    let view = build_schedule_iii_view(&workbook(source), &[]).unwrap();
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

    let view = build_schedule_iii_view(&workbook(source), &[]).unwrap();
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

    let view = build_schedule_iii_view(&workbook(source), &[]).unwrap();
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

    let view = build_schedule_iii_view(&workbook(source), &[]).unwrap();
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

    let view = build_schedule_iii_view(&workbook(source), &[]).unwrap();
    assert!(view.lines.is_empty());
    assert!(view.debit_total.is_zero());
    assert!(view.credit_total.is_zero());
    assert_eq!(view.exclusions.len(), 1);
    assert!(view.exclusions[0].reason.contains("not established"));
}

// --- CA grouping decisions (#737) ---

const FY_2026_27: u16 = 2026;

fn reserved_group(name: &str, reserved: &str) -> TallyNamedMaster {
    TallyNamedMaster {
        name: name.to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
        reserved_name: Some(reserved.to_string()),
    }
}

fn workbook(source: PartyLedgerMasterSource) -> PartyLedgerMasterWorkbook {
    build_party_ledger_master_workbook(source).expect("a valid synthetic source")
}

/// A synthetic book with its balances as of 31 July 2026 (FY 2026-27).
fn book(rows: Vec<PartyLedgerMasterRow>) -> PartyLedgerMasterWorkbook {
    workbook(PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows,
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![
            reserved_group("Sundry Debtors", "Sundry Debtors"),
            reserved_group("Sundry Creditors", "Sundry Creditors"),
            reserved_group("Loans (Liability)", "Loans (Liability)"),
            reserved_group("Current Liabilities", "Current Liabilities"),
            user_group("Director loans", RESERVED_ROOT),
            user_group("Director current accounts", "Director loans"),
            user_group("Staff advances", "Sundry Debtors"),
            user_group("Customers", "Sundry Debtors"),
            user_group("Unsecured loans", RESERVED_ROOT),
            TallyNamedMaster {
                name: "Parentless".to_string(),
                parent: PartyLedgerMasterFieldObservation::NotObserved,
                reserved_name: Some(String::new()),
            },
        ],
    })
}

fn user_group(name: &str, parent: &str) -> TallyNamedMaster {
    TallyNamedMaster {
        name: name.to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned(parent.to_string()),
        reserved_name: Some(String::new()),
    }
}

/// A decision made against `outcome`, with the group chain this test book
/// gives a ledger with that outcome.
fn decision(id: u64, name: &str, outcome: DerivedOutcome, head: ScheduleIIIHead) -> Decision {
    let ancestry = ancestry_in_book(&outcome);
    decision_against(id, name, Derivation { outcome, ancestry }, head)
}

fn decision_against(
    id: u64,
    name: &str,
    made_against: Derivation,
    head: ScheduleIIIHead,
) -> Decision {
    Decision {
        id: DecisionId(id),
        ledger: LedgerGuid::new(&format!("guid-{name}")).unwrap(),
        ledger_name_when_made: name.to_string(),
        made_against,
        head,
        year: FinancialYear::beginning_in(FY_2026_27),
    }
}

fn ancestry_in_book(outcome: &DerivedOutcome) -> Vec<String> {
    let chain: &[&str] = match outcome {
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors)
        | DerivedOutcome::Undetermined(Undetermined::OppositeSide(
            GroupSubtotalKind::SundryDebtors,
        )) => &["Sundry Debtors"],
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryCreditors)
        | DerivedOutcome::Undetermined(Undetermined::OppositeSide(
            GroupSubtotalKind::SundryCreditors,
        )) => &["Sundry Creditors"],
        DerivedOutcome::Undetermined(Undetermined::UnmappedReservedGroup(name)) => {
            match name.as_str() {
                "loans (liability)" => &["Loans (Liability)"],
                "current liabilities" => &["Current Liabilities"],
                other => panic!("no group in the test book for {other}"),
            }
        }
        DerivedOutcome::Undetermined(Undetermined::UserPrimaryGroup(name)) => {
            return vec![name.clone()]
        }
        _ => &[],
    };
    chain.iter().map(|name| name.to_string()).collect()
}

fn outcomes(book: &PartyLedgerMasterWorkbook) -> Vec<DerivedOutcome> {
    derivations(book)
        .into_iter()
        .map(|derivation| derivation.outcome)
        .collect()
}

fn unmapped(reserved: &str) -> DerivedOutcome {
    DerivedOutcome::Undetermined(Undetermined::UnmappedReservedGroup(reserved.to_string()))
}

fn applied(view: &ScheduleIIIView, id: u64) -> bool {
    view.decisions.iter().any(|status| {
        matches!(status, DecisionStatus::Applied { id: applied, .. } if *applied == DecisionId(id))
    })
}

fn not_applied(view: &ScheduleIIIView, id: u64) -> Option<&NotApplied> {
    view.decisions.iter().find_map(|status| match status {
        DecisionStatus::NotApplied {
            id: status_id,
            reason,
        } if *status_id == DecisionId(id) => Some(reason),
        _ => None,
    })
}

fn line(view: &ScheduleIIIView, basis: LineBasis) -> Option<&ScheduleIIILine> {
    view.lines.iter().find(|line| line.basis == basis)
}

/// The totals a decision must never move.
fn totals(view: &ScheduleIIIView) -> (String, String, String) {
    (
        view.debit_total.as_str().to_string(),
        view.credit_total.as_str().to_string(),
        view.difference.as_str().to_string(),
    )
}

#[test]
fn without_decisions_the_view_is_final_and_every_line_is_group_evidence() {
    let source = book(vec![
        row("Customer", "Sundry Debtors", "-100"),
        row("Term loan", "Loans (Liability)", "500"),
    ]);
    let view = build_schedule_iii_view(&source, &[]).unwrap();
    assert_eq!(view.finality, Finality::Final);
    assert!(view.decisions.is_empty());
    assert_eq!(view.lines.len(), 1);
    assert_eq!(
        view.lines[0].basis,
        LineBasis::GroupSubtotal(GroupSubtotalKind::SundryDebtors)
    );
    assert_eq!(view.exclusions.len(), 1);
}

#[test]
fn a_decision_places_an_undetermined_ledger_under_its_head_without_moving_any_total() {
    let source = book(vec![
        row("Customer", "Sundry Debtors", "-100"),
        row("Term loan", "Loans (Liability)", "500"),
    ]);
    let before = build_schedule_iii_view(&source, &[]).unwrap();
    let decisions = [decision(
        1,
        "Term loan",
        unmapped("loans (liability)"),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert!(applied(&view, 1));
    assert_eq!(view.finality, Finality::Final);
    assert!(view.exclusions.is_empty());
    let head = line(
        &view,
        LineBasis::Decided(ScheduleIIIHead::OtherCurrentLiabilities),
    )
    .unwrap();
    assert_eq!(head.row_indices, vec![1]);
    assert_eq!(head.total.as_str(), "500");
    assert_eq!(head.section, "Current liabilities");
    assert_eq!(head.label, "Other current liabilities");
    assert_eq!(totals(&view), totals(&before));
}

#[test]
fn a_decision_moves_one_ledger_out_of_its_group_subtotal_and_leaves_the_rest() {
    let source = book(vec![
        row("Customer", "Sundry Debtors", "-100"),
        row("Staff advance", "Sundry Debtors", "-40"),
    ]);
    let before = build_schedule_iii_view(&source, &[]).unwrap();
    let decisions = [decision(
        7,
        "Staff advance",
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
        ScheduleIIIHead::ShortTermLoansAndAdvances,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert!(applied(&view, 7));
    let debtors = line(
        &view,
        LineBasis::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
    )
    .unwrap();
    assert_eq!(debtors.row_indices, vec![0]);
    assert_eq!(debtors.total.as_str(), "-100");
    let advances = line(
        &view,
        LineBasis::Decided(ScheduleIIIHead::ShortTermLoansAndAdvances),
    )
    .unwrap();
    assert_eq!(advances.row_indices, vec![1]);
    assert_eq!(advances.total.as_str(), "-40");
    assert_eq!(totals(&view), totals(&before));
}

#[test]
fn a_credit_balance_debtor_can_be_decided_onto_a_liability_head() {
    let source = book(vec![row("Customer advance", "Sundry Debtors", "250")]);
    let decisions = [decision(
        2,
        "Customer advance",
        DerivedOutcome::Undetermined(Undetermined::OppositeSide(GroupSubtotalKind::SundryDebtors)),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert!(applied(&view, 2));
    assert!(view.exclusions.is_empty());
    assert_eq!(
        line(
            &view,
            LineBasis::Decided(ScheduleIIIHead::OtherCurrentLiabilities)
        )
        .unwrap()
        .row_indices,
        vec![0]
    );
}

#[test]
fn a_decision_for_a_ledger_no_longer_in_the_read_is_reported_and_the_view_is_not_final() {
    let source = book(vec![row("Customer", "Sundry Debtors", "-100")]);
    let decisions = [decision(
        3,
        "Closed ledger",
        unmapped("loans (liability)"),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(not_applied(&view, 3), Some(&NotApplied::LedgerMissing));
    assert_eq!(view.finality, Finality::NotFinal);
    assert!(line(
        &view,
        LineBasis::Decided(ScheduleIIIHead::OtherCurrentLiabilities)
    )
    .is_none());
}

#[test]
fn a_ledger_moved_in_tally_after_the_decision_keeps_its_group_evidence_and_the_view_is_not_final() {
    let source = book(vec![row("Party", "Sundry Creditors", "80")]);
    let decisions = [decision(
        4,
        "Party",
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
        ScheduleIIIHead::TradeReceivables,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(
        not_applied(&view, 4),
        Some(&NotApplied::GroupChanged {
            now: DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryCreditors),
        })
    );
    assert_eq!(view.finality, Finality::NotFinal);
    assert_eq!(
        line(
            &view,
            LineBasis::GroupSubtotal(GroupSubtotalKind::SundryCreditors)
        )
        .unwrap()
        .row_indices,
        vec![0]
    );
    assert!(line(&view, LineBasis::Decided(ScheduleIIIHead::TradeReceivables)).is_none());
}

#[test]
fn a_move_between_two_unmapped_predefined_groups_also_makes_the_decision_stale() {
    let source = book(vec![row("Deposit", "Current Liabilities", "90")]);
    let decisions = [decision(
        5,
        "Deposit",
        unmapped("loans (liability)"),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(
        not_applied(&view, 5),
        Some(&NotApplied::GroupChanged {
            now: unmapped("current liabilities"),
        })
    );
    assert_eq!(view.exclusions.len(), 1);
    assert_eq!(view.finality, Finality::NotFinal);
}

#[test]
fn a_head_on_the_other_side_from_the_balance_is_not_applied() {
    let source = book(vec![row("Term loan", "Loans (Liability)", "-60")]);
    let decisions = [decision(
        6,
        "Term loan",
        unmapped("loans (liability)"),
        ScheduleIIIHead::TradePayables,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(not_applied(&view, 6), Some(&NotApplied::OppositeSide));
    assert_eq!(view.finality, Finality::NotFinal);
    assert_eq!(view.exclusions.len(), 1);
}

#[test]
fn a_zero_balance_sits_on_either_side() {
    let source = book(vec![
        row("Settled supplier", "Sundry Creditors", "0"),
        row("Settled customer", "Sundry Debtors", "0"),
    ]);
    let decisions = [decision(
        15,
        "Settled customer",
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert!(applied(&view, 15));
    assert!(view.exclusions.is_empty());
    assert!(line(
        &view,
        LineBasis::GroupSubtotal(GroupSubtotalKind::SundryCreditors)
    )
    .is_some());
    assert!(line(
        &view,
        LineBasis::GroupSubtotal(GroupSubtotalKind::SundryDebtors)
    )
    .is_none());
}

#[test]
fn a_zero_balance_sits_on_a_debit_side_too() {
    let source = book(vec![row("Settled customer", "Sundry Debtors", "0")]);
    let view = build_schedule_iii_view(&source, &[]).unwrap();
    assert!(view.exclusions.is_empty());
    assert_eq!(
        line(
            &view,
            LineBasis::GroupSubtotal(GroupSubtotalKind::SundryDebtors)
        )
        .unwrap()
        .row_indices,
        vec![0]
    );

    let decisions = [decision(
        25,
        "Settled customer",
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
        ScheduleIIIHead::TradeReceivables,
    )];
    let view = build_schedule_iii_view(&source, &decisions).unwrap();
    assert!(applied(&view, 25));
}

#[test]
fn an_advance_settled_to_zero_by_year_end_keeps_its_decision() {
    let source = book(vec![row("Customer advance", "Sundry Debtors", "0")]);
    let decisions = [decision(
        16,
        "Customer advance",
        DerivedOutcome::Undetermined(Undetermined::OppositeSide(GroupSubtotalKind::SundryDebtors)),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert!(applied(&view, 16));
    assert_eq!(view.finality, Finality::Final);
}

#[test]
fn a_move_between_subgroups_of_one_group_applies_and_reports_the_ancestry_change() {
    let before = book(vec![row("Advance", "Staff advances", "-40")]);
    let made_against = derivations(&before).remove(0);
    assert_eq!(
        made_against.ancestry,
        vec!["Staff advances".to_string(), "Sundry Debtors".to_string()]
    );
    let decisions = [decision_against(
        22,
        "Advance",
        made_against,
        ScheduleIIIHead::ShortTermLoansAndAdvances,
    )];

    let unchanged = build_schedule_iii_view(&before, &decisions).unwrap();
    assert!(matches!(
        unchanged.decisions[0],
        DecisionStatus::Applied {
            ancestry_changed: None,
            ..
        }
    ));

    let moved = book(vec![row("Advance", "Customers", "-40")]);
    let view = build_schedule_iii_view(&moved, &decisions).unwrap();

    assert_eq!(
        view.decisions,
        vec![DecisionStatus::Applied {
            id: DecisionId(22),
            row_index: 0,
            renamed_from: None,
            ancestry_changed: Some(AncestryChange {
                was: vec!["Staff advances".to_string(), "Sundry Debtors".to_string()],
                now: vec!["Customers".to_string(), "Sundry Debtors".to_string()],
            }),
        }]
    );
    assert_eq!(view.finality, Finality::Final);
}

#[test]
fn groups_above_the_nearest_predefined_group_are_not_recorded_so_a_gap_there_is_not_a_move() {
    // Sundry Debtors sits under Current Assets, as in a real book. A later
    // read that did not capture Current Assets must not look like a move.
    let with_parent = |captured: bool| {
        let mut source = book(vec![row("Advance", "Staff advances", "-40")])
            .source()
            .clone();
        for group in &mut source.groups {
            if group.name == "Sundry Debtors" {
                group.parent =
                    PartyLedgerMasterFieldObservation::Returned("Current Assets".to_string());
            }
        }
        if captured {
            source
                .groups
                .push(reserved_group("Current Assets", "Current Assets"));
        }
        workbook(source)
    };
    let before = with_parent(true);
    let decisions = [decision_against(
        24,
        "Advance",
        derivations(&before).remove(0),
        ScheduleIIIHead::ShortTermLoansAndAdvances,
    )];
    let after = with_parent(false);

    let view = build_schedule_iii_view(&after, &decisions).unwrap();

    assert_eq!(
        derivations(&before)[0].ancestry,
        vec!["Staff advances".to_string(), "Sundry Debtors".to_string()]
    );
    assert!(matches!(
        view.decisions[0],
        DecisionStatus::Applied {
            ancestry_changed: None,
            ..
        }
    ));
}

#[test]
fn a_balance_that_changed_side_within_its_group_is_judged_against_the_head_not_as_a_move() {
    let source = book(vec![row("Customer", "Sundry Debtors", "30")]);
    let decisions = [decision(
        17,
        "Customer",
        DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
        ScheduleIIIHead::TradeReceivables,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(not_applied(&view, 17), Some(&NotApplied::OppositeSide));
    assert_eq!(view.finality, Finality::NotFinal);
}

#[test]
fn a_user_created_primary_group_is_a_standing_a_decision_can_rest_on() {
    let source = book(vec![row("Director", "Director loans", "700")]);
    let made_against =
        DerivedOutcome::Undetermined(Undetermined::UserPrimaryGroup("Director loans".to_string()));
    assert_eq!(outcomes(&source), vec![made_against.clone()]);
    let decisions = [decision(
        18,
        "Director",
        made_against,
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();
    assert!(applied(&view, 18));

    let moved = book(vec![row("Director", "Unsecured loans", "700")]);
    let view = build_schedule_iii_view(&moved, &decisions).unwrap();
    assert_eq!(
        not_applied(&view, 18),
        Some(&NotApplied::GroupChanged {
            now: DerivedOutcome::Undetermined(Undetermined::UserPrimaryGroup(
                "Unsecured loans".to_string()
            )),
        })
    );
}

#[test]
fn a_ledger_directly_under_the_account_root_can_take_a_decision() {
    let source = book(vec![row("Profit & Loss A/c", RESERVED_ROOT, "900")]);
    let decisions = [decision(
        19,
        "Profit & Loss A/c",
        DerivedOutcome::Undetermined(Undetermined::AccountRoot),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert!(applied(&view, 19));
}

#[test]
fn a_user_created_primary_group_is_the_top_of_a_longer_user_chain() {
    let source = book(vec![row("Director", "Director current accounts", "700")]);
    assert_eq!(
        derivations(&source),
        vec![Derivation {
            outcome: DerivedOutcome::Undetermined(Undetermined::UserPrimaryGroup(
                "Director loans".to_string()
            )),
            ancestry: vec![
                "Director current accounts".to_string(),
                "Director loans".to_string()
            ],
        }]
    );
}

#[test]
fn a_ledger_moved_from_the_account_root_into_a_group_makes_its_decision_stale() {
    let decisions = [decision(
        23,
        "Suspense",
        DerivedOutcome::Undetermined(Undetermined::AccountRoot),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];
    let moved = book(vec![row("Suspense", "Director loans", "10")]);

    let view = build_schedule_iii_view(&moved, &decisions).unwrap();

    assert_eq!(
        not_applied(&view, 23),
        Some(&NotApplied::GroupChanged {
            now: DerivedOutcome::Undetermined(Undetermined::UserPrimaryGroup(
                "Director loans".to_string()
            )),
        })
    );
}

#[test]
fn a_group_whose_parent_was_not_returned_is_a_gap_not_a_primary_group() {
    let source = book(vec![row("Stranded", "Parentless", "40")]);
    assert_eq!(
        outcomes(&source),
        vec![DerivedOutcome::Undetermined(Undetermined::ReadIncomplete(
            ReadGap::ParentNotObserved
        ))]
    );
    let decisions = [decision(
        20,
        "Stranded",
        DerivedOutcome::Undetermined(Undetermined::UserPrimaryGroup("Parentless".to_string())),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(not_applied(&view, 20), Some(&NotApplied::ReadIncomplete));
    assert!(view.exclusions[0].reason.contains("no parent in the read"));
}

#[test]
fn a_ledger_with_no_parent_cannot_take_a_decision() {
    let source = book(vec![row("Unparented", "", "40")]);
    let decisions = [decision(
        21,
        "Unparented",
        DerivedOutcome::Undetermined(Undetermined::NoParent),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(not_applied(&view, 21), Some(&NotApplied::ReadIncomplete));
    assert_eq!(view.finality, Finality::NotFinal);
}

#[test]
fn a_renamed_ledger_is_bound_by_its_case_folded_guid_and_reports_its_old_name() {
    let mut renamed = row("Term loan from director", "Loans (Liability)", "500");
    renamed.guid = "GUID-TERM LOAN".to_string();
    let source = book(vec![renamed]);
    let decisions = [decision(
        8,
        "Term loan",
        unmapped("loans (liability)"),
        ScheduleIIIHead::OtherCurrentLiabilities,
    )];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(
        view.decisions,
        vec![DecisionStatus::Applied {
            id: DecisionId(8),
            row_index: 0,
            renamed_from: Some("Term loan".to_string()),
            ancestry_changed: None,
        }]
    );
    assert_eq!(view.finality, Finality::Final);
}

#[test]
fn a_decision_for_another_year_is_offered_not_applied_and_leaves_the_view_final() {
    let source = book(vec![row("Term loan", "Loans (Liability)", "500")]);
    let mut last_year = decision(
        9,
        "Term loan",
        unmapped("loans (liability)"),
        ScheduleIIIHead::OtherCurrentLiabilities,
    );
    last_year.year = FinancialYear::beginning_in(FY_2026_27 - 1);

    let view = build_schedule_iii_view(&source, &[last_year]).unwrap();

    assert_eq!(not_applied(&view, 9), Some(&NotApplied::OtherYear));
    assert_eq!(view.finality, Finality::Final);
    assert_eq!(view.exclusions.len(), 1);
}

#[test]
fn every_provisional_head_states_its_section_caption_and_side() {
    for (head, section, caption, side) in [
        (
            ScheduleIIIHead::TradeReceivables,
            "Current assets",
            "Trade receivables",
            Side::Debit,
        ),
        (
            ScheduleIIIHead::CashAndCashEquivalents,
            "Current assets",
            "Cash and cash equivalents",
            Side::Debit,
        ),
        (
            ScheduleIIIHead::ShortTermLoansAndAdvances,
            "Current assets",
            "Short-term loans and advances",
            Side::Debit,
        ),
        (
            ScheduleIIIHead::TradePayables,
            "Current liabilities",
            "Trade payables",
            Side::Credit,
        ),
        (
            ScheduleIIIHead::OtherCurrentLiabilities,
            "Current liabilities",
            "Other current liabilities",
            Side::Credit,
        ),
    ] {
        assert_eq!(
            (head.section(), head.caption(), head.side()),
            (section, caption, side)
        );
    }
}

#[test]
fn financial_year_runs_from_the_first_of_april_to_the_thirty_first_of_march() {
    let year = FinancialYear::beginning_in(2026);
    for (date, inside) in [
        ("20260331", false),
        ("20260401", true),
        ("20261231", true),
        ("20270331", true),
        ("20270401", false),
    ] {
        assert_eq!(
            year.contains(&TallyDate::parse(date).unwrap()),
            inside,
            "{date}"
        );
    }
}

#[test]
fn two_decisions_for_one_ledger_in_one_year_withhold_the_view() {
    let source = book(vec![row("Term loan", "Loans (Liability)", "500")]);
    let decisions = [
        decision(
            10,
            "Term loan",
            unmapped("loans (liability)"),
            ScheduleIIIHead::OtherCurrentLiabilities,
        ),
        decision(
            11,
            "Term loan",
            unmapped("loans (liability)"),
            ScheduleIIIHead::TradePayables,
        ),
    ];

    assert!(matches!(
        build_schedule_iii_view(&source, &decisions),
        Err(ScheduleIIIError::DecisionRepeated)
    ));
}

#[test]
fn a_decision_cannot_rest_on_an_unestablished_balance_or_an_incomplete_group_read() {
    let mut unestablished = row("Unestablished", "Loans (Liability)", "1");
    unestablished.closing_balance = None;
    let orphan = row("Orphan", "Group not in the read", "5");
    let source = book(vec![unestablished, orphan]);
    let decisions = [
        decision(
            12,
            "Unestablished",
            DerivedOutcome::Undetermined(Undetermined::BalanceNotEstablished),
            ScheduleIIIHead::OtherCurrentLiabilities,
        ),
        decision(
            13,
            "Orphan",
            DerivedOutcome::Undetermined(Undetermined::ReadIncomplete(ReadGap::GroupAbsent)),
            ScheduleIIIHead::OtherCurrentLiabilities,
        ),
    ];

    let view = build_schedule_iii_view(&source, &decisions).unwrap();

    assert_eq!(
        not_applied(&view, 12),
        Some(&NotApplied::BalanceNotEstablished)
    );
    assert_eq!(not_applied(&view, 13), Some(&NotApplied::ReadIncomplete));
    assert_eq!(view.exclusions.len(), 2);
    assert!(view.lines.is_empty());
    assert_eq!(view.finality, Finality::NotFinal);
}

#[test]
fn derivations_are_what_a_decision_is_made_against() {
    let source = book(vec![
        row("Customer", "Sundry Debtors", "-100"),
        row("Customer advance", "Sundry Debtors", "250"),
        row("Term loan", "Loans (Liability)", "500"),
    ]);
    assert_eq!(
        outcomes(&source),
        vec![
            DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
            DerivedOutcome::Undetermined(Undetermined::OppositeSide(
                GroupSubtotalKind::SundryDebtors
            )),
            unmapped("loans (liability)"),
        ]
    );
}

// --- The conservation check, proven to fire ---

fn conserved_fixture() -> (
    PartyLedgerMasterSource,
    Vec<ScheduleIIILine>,
    Vec<ScheduleIIIExclusion>,
) {
    let book = book(vec![
        row("Customer", "Sundry Debtors", "-100"),
        row("Term loan", "Loans (Liability)", "500"),
    ]);
    let view = build_schedule_iii_view(&book, &[]).unwrap();
    (book.source().clone(), view.lines, view.exclusions)
}

#[test]
fn conservation_holds_for_an_honest_view() {
    let (source, lines, exclusions) = conserved_fixture();
    assert!(check_conservation(&source, &lines, &exclusions).is_ok());
}

#[test]
fn conservation_refuses_a_ledger_placed_twice() {
    let (source, lines, mut exclusions) = conserved_fixture();
    exclusions.push(ScheduleIIIExclusion {
        row_index: 0,
        reason: "counted again".to_string(),
    });
    assert!(matches!(
        check_conservation(&source, &lines, &exclusions),
        Err(ScheduleIIIError::NotConserved)
    ));
}

#[test]
fn conservation_refuses_a_ledger_left_out() {
    let (source, lines, _) = conserved_fixture();
    assert!(matches!(
        check_conservation(&source, &lines, &[]),
        Err(ScheduleIIIError::NotConserved)
    ));
}

#[test]
fn conservation_refuses_a_line_total_that_is_not_its_ledgers_balance() {
    let (source, mut lines, exclusions) = conserved_fixture();
    lines[0].total = ExactDecimal::parse("-99").unwrap();
    assert!(matches!(
        check_conservation(&source, &lines, &exclusions),
        Err(ScheduleIIIError::NotConserved)
    ));
}

#[test]
fn conservation_refuses_a_row_index_outside_the_read() {
    let (source, mut lines, exclusions) = conserved_fixture();
    lines[0].row_indices.push(source.rows.len());
    assert!(matches!(
        check_conservation(&source, &lines, &exclusions),
        Err(ScheduleIIIError::NotConserved)
    ));
}
