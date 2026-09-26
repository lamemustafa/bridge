//! Two tests use one company's captures of one window: the Trial Balance,
//! group tree and both of Tally's own statements. They prove the derivation
//! ties, and that the gate passes, on a full year and on a part-year window.
//!
//! The rest pair a captured Trial Balance from one synthetic company with a
//! verbatim group tree captured from another. Every `PARENT` in those Trial
//! Balances names a group Tally creates in every company, so each hop resolves
//! through Tally's own default tree. That pairing crosses companies and proves
//! classification, arithmetic and the gate's clauses only. Tally's Balance
//! Sheet in those tests is synthetic, built to tie or to fail one clause.
//!
//! Tests marked "synthetic mutation" alter a captured row or group to reach a
//! branch the captures do not; they make no claim about Tally output.

use super::*;
use bridge_tally_protocol::{
    native_outstandings::parse_native_group_snapshot,
    native_statement_reports::{parse_native_statement, NativeStatementLine},
    native_trial_balance::{parse_native_trial_balance, NativeTrialBalance},
    PartyLedgerMasterFieldObservation,
};

const KNOWN_LAB: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
);
const KNOWN_LAB_GUID: &str = "eebb9a9f-1679-4468-9e8f-814c729674cb";
const OPENING_YEAR: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_opening_year.xml"
);
const OPENING_YEAR_GUID: &str = "915d42f8-42ae-4b03-8291-55f596e3a2ea";
const GROUPS: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_aarav_with_computed_company_guid.xml"
);
const GROUPS_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";

fn groups() -> Vec<TallyNamedMaster> {
    parse_native_group_snapshot(GROUPS, GROUPS_GUID).unwrap()
}

fn known_lab() -> NativeTrialBalance {
    parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap()
}

fn admitted(report: NativeTrialBalance) -> SingleCurrencyTrialBalance {
    SingleCurrencyTrialBalance::admitted_for_tests(report)
}

fn decimal(value: &str) -> ExactDecimal {
    ExactDecimal::parse(value).unwrap()
}

fn statement(kind: NativeStatementKind, lines: &[(&str, &str, &str)]) -> NativeStatement {
    let amount = |value: &str| {
        if value.is_empty() {
            NativeStatementAmount::Empty
        } else {
            NativeStatementAmount::Present(decimal(value))
        }
    };
    NativeStatement {
        kind,
        lines: lines
            .iter()
            .map(|(name, sub, main)| NativeStatementLine {
                name: name.to_string(),
                sub: amount(sub),
                main: amount(main),
            })
            .collect(),
    }
}

fn balance_sheet(lines: &[(&str, &str, &str)]) -> NativeStatement {
    statement(NativeStatementKind::BalanceSheet, lines)
}

/// A synthetic Tally Balance Sheet that ties to the known-lab derivation.
fn known_lab_balance_sheet() -> NativeStatement {
    balance_sheet(&[
        ("Capital Account", "", ""),
        ("Current Assets", "", "-11027.00"),
        ("Profit & Loss A/c", "", "11027.00"),
    ])
}

fn derive(report: NativeTrialBalance, tally: &NativeStatement) -> DerivedStatements {
    derive_statements(&admitted(report), &groups(), tally).unwrap()
}

fn assert_sum(sum: &StatementSum, value: &str, present: usize, empty: usize) {
    assert!(sum.sum.numeric_eq(&decimal(value)), "{} != {value}", sum.sum.as_str());
    assert_eq!((sum.present_count, sum.empty_count), (present, empty));
}

fn assert_established(result: &Established, value: &str) {
    match result {
        Established::Established { value: got } => {
            assert!(got.numeric_eq(&decimal(value)), "{} != {value}", got.as_str())
        }
        other => panic!("expected {value}, got {other:?}"),
    }
}

fn blocked(reason: &'static str) -> Established {
    Established::NotEstablished {
        reason,
        lines: Vec::new(),
    }
}

fn differs(lines: &[&str]) -> Established {
    Established::NotEstablished {
        reason: "tally_balance_sheet_differs",
        lines: lines.iter().map(|line| line.to_string()).collect(),
    }
}

fn assert_every_result(derived: &DerivedStatements, expected: &Established) {
    assert_eq!(&derived.gross_result, expected);
    assert_eq!(&derived.net_result, expected);
    assert_eq!(&derived.balance_sheet_profit_and_loss, expected);
}

fn line<'a>(lines: &'a [PrimaryGroupLine], reserved: &str) -> &'a PrimaryGroupLine {
    lines
        .iter()
        .find(|line| line.reserved_name == reserved)
        .unwrap_or_else(|| panic!("no {reserved} line"))
}

fn root_parent(report: &NativeTrialBalance) -> PartyLedgerMasterFieldObservation {
    report
        .rows
        .iter()
        .find(|row| row.name == "Profit & Loss A/c")
        .unwrap()
        .parent
        .clone()
}

fn row<'a>(report: &'a mut NativeTrialBalance, name: &str) -> &'a mut NativeTrialBalanceRow {
    report.rows.iter_mut().find(|row| row.name == name).unwrap()
}

#[test]
fn a_captured_trial_balance_splits_into_statement_lines() {
    let derived = derive(known_lab(), &known_lab_balance_sheet());

    // The captured tree holds all fifteen reserved primary groups.
    assert_eq!(derived.profit_and_loss.len(), 6);
    assert_eq!(derived.balance_sheet.len(), 9);
    assert_eq!(
        derived
            .profit_and_loss
            .iter()
            .chain(&derived.balance_sheet)
            .filter(|line| line.ledger_count > 0)
            .count(),
        2
    );
    let sales = line(&derived.profit_and_loss, "Sales Accounts");
    // Movement: an empty debit and a 4027.00 credit.
    assert_sum(&sales.amount, "4027.00", 1, 1);
    assert_eq!(sales.ledger_count, 1);

    let current_assets = line(&derived.balance_sheet, "Current Assets");
    // Four closings, one of them empty: debtors and cash.
    assert_sum(&current_assets.amount, "-11027.00", 3, 1);
    assert_eq!(current_assets.ledger_count, 4);

    let root = derived.profit_and_loss_ledger.as_ref().unwrap();
    assert_eq!(root.name, "Profit & Loss A/c");
    assert!(derived.unclassified.is_empty());
    assert_eq!(derived.stock_ledger_count, 0);
    assert_established(&derived.gross_result, "4027.00");
    assert_established(&derived.net_result, "4027.00");
    // 7000.00 carried on the ledger plus the window's 4027.00: §5.6's
    // balance-sheet figure, and the negation of the asset line.
    assert_established(&derived.balance_sheet_profit_and_loss, "11027.00");
}

#[test]
fn an_opening_only_book_keeps_its_opening_difference_visible() {
    let report = parse_native_trial_balance(OPENING_YEAR, OPENING_YEAR_GUID).unwrap();
    let tally = balance_sheet(&[
        ("Capital Account", "", "125000.00"),
        ("Current Liabilities", "", "88000.00"),
        ("Profit & Loss A/c", "", ""),
        ("Current Assets", "", "-262833.50"),
    ]);
    let derived = derive(report, &tally);

    assert!(derived.profit_and_loss.iter().all(|line| line.ledger_count == 0));
    assert_sum(&line(&derived.balance_sheet, "Capital Account").amount, "125000.00", 1, 0);
    assert_sum(&line(&derived.balance_sheet, "Current Liabilities").amount, "88000.00", 1, 0);
    assert_sum(&line(&derived.balance_sheet, "Current Assets").amount, "-262833.50", 4, 1);
    // No line is invented to balance: the three lines net to §5.6's -49833.50.
    let net: ExactDecimal = derived
        .balance_sheet
        .iter()
        .try_fold(ExactDecimal::zero(), |total, line| total.checked_add(&line.amount.sum))
        .unwrap();
    assert!(net.numeric_eq(&decimal("-49833.50")));
    assert_established(&derived.net_result, "0");
}

#[test]
fn a_p_and_l_line_is_the_window_movement_not_the_closing() {
    // Synthetic mutation: the sales ledger opens at 100.00, so its closing
    // (4127.00) and its movement (4027.00) differ.
    let mut report = known_lab();
    let sales = row(&mut report, "Ageing Sales");
    sales.opening = NativeTrialBalanceAmount::Present(decimal("100.00"));
    sales.closing = NativeTrialBalanceAmount::Present(decimal("4127.00"));
    let tally = balance_sheet(&[
        ("Current Assets", "", "-11027.00"),
        ("Profit & Loss A/c", "", "11127.00"),
    ]);
    let derived = derive(report, &tally);

    assert_sum(&line(&derived.profit_and_loss, "Sales Accounts").amount, "4027.00", 1, 1);
    assert_established(&derived.net_result, "4027.00");
    // The carried line is closings: 7000.00 on the ledger and 4127.00 of sales.
    assert_established(&derived.balance_sheet_profit_and_loss, "11127.00");
}

#[test]
fn a_ledger_under_a_user_created_primary_group_blocks_every_result() {
    // Synthetic mutation: a user-created primary group, and one captured
    // ledger moved under it.
    let mut report = known_lab();
    let mut groups = groups();
    groups.push(TallyNamedMaster {
        name: "BRIDGE Synthetic Primary".to_string(),
        parent: root_parent(&report),
        reserved_name: Some(String::new()),
    });
    row(&mut report, "Ageing Sales").parent =
        PartyLedgerMasterFieldObservation::Returned("BRIDGE Synthetic Primary".to_string());

    let derived =
        derive_statements(&admitted(report), &groups, &known_lab_balance_sheet()).unwrap();
    assert_eq!(line(&derived.profit_and_loss, "Sales Accounts").ledger_count, 0);
    assert_eq!(derived.unclassified.len(), 1);
    assert_eq!(derived.unclassified[0].reason, "primary_group_user_created");
    assert_every_result(&derived, &blocked("unclassified_ledger_carries_an_amount"));
}

#[test]
fn an_unclassified_ledger_with_no_amount_blocks_nothing() {
    // Synthetic mutation: the captured all-empty ledger loses its parent group.
    let mut report = known_lab();
    row(&mut report, "Ageing Bank").parent =
        PartyLedgerMasterFieldObservation::Returned("BRIDGE Absent Group".to_string());

    let derived = derive(report, &known_lab_balance_sheet());
    assert_eq!(derived.unclassified.len(), 1);
    assert_eq!(derived.unclassified[0].reason, "group_absent");
    assert_established(&derived.net_result, "4027.00");
}

#[test]
fn a_stock_balance_blocks_every_result_including_the_carried_line() {
    // Synthetic mutation: a captured ledger with a non-zero closing moved under
    // Stock-in-Hand. Tally's carried line includes the change in stock, which
    // the Trial Balance cannot give, so the carried line is blocked too.
    let mut report = known_lab();
    row(&mut report, "Cash").parent =
        PartyLedgerMasterFieldObservation::Returned("Stock-in-Hand".to_string());

    let derived = derive(report, &known_lab_balance_sheet());
    assert_eq!(derived.stock_ledger_count, 1);
    assert_every_result(&derived, &blocked("closing_stock_not_derivable_from_trial_balance"));
}

#[test]
fn a_tally_line_that_differs_blocks_every_result_and_is_named() {
    let tally = balance_sheet(&[
        ("Current Assets", "", "-11026.00"),
        ("Profit & Loss A/c", "", "11027.00"),
    ]);
    let derived = derive(known_lab(), &tally);
    assert_every_result(&derived, &differs(&["Current Assets"]));
}

#[test]
fn a_tally_line_with_an_amount_nothing_derived_matches_blocks_every_result() {
    // What an inventory book's statement is expected to add: a line the Trial
    // Balance cannot produce.
    let tally = balance_sheet(&[
        ("Current Assets", "", "-11027.00"),
        ("Closing Stock", "", "500.00"),
        ("Profit & Loss A/c", "", "11027.00"),
    ]);
    let derived = derive(known_lab(), &tally);
    assert_every_result(&derived, &differs(&["Closing Stock"]));

    // Both columns present is uncompared too, and carries an amount.
    let tally = balance_sheet(&[
        ("Current Assets", "-1.00", "-11027.00"),
        ("Profit & Loss A/c", "", "11027.00"),
    ]);
    let derived = derive(known_lab(), &tally);
    assert_every_result(&derived, &differs(&["Current Assets"]));
}

#[test]
fn a_derived_line_that_tally_does_not_show_blocks_every_result() {
    let tally = balance_sheet(&[("Profit & Loss A/c", "", "11027.00")]);
    let derived = derive(known_lab(), &tally);
    assert_every_result(&derived, &differs(&["Current Assets"]));
}

#[test]
fn a_missing_profit_and_loss_ledger_blocks_the_carried_line() {
    // Synthetic mutation: the reserved-root ledger removed from the capture.
    let mut report = known_lab();
    report.rows.retain(|row| row.name != "Profit & Loss A/c");
    let derived = derive(report, &known_lab_balance_sheet());
    assert_eq!(
        derived.balance_sheet_profit_and_loss,
        blocked("profit_and_loss_ledger_not_returned")
    );
    // Tally's carried line then has nothing to tie to, so the gate holds too.
    assert_eq!(derived.net_result, differs(&["Profit & Loss A/c"]));
}

#[test]
fn a_row_whose_columns_do_not_add_up_is_refused() {
    // Synthetic mutation: the one captured row with all four amounts present.
    let mut report = known_lab();
    row(&mut report, "Ageing Customer A").closing =
        NativeTrialBalanceAmount::Present(decimal("-7277.01"));
    assert_eq!(
        derive_statements(&admitted(report), &groups(), &known_lab_balance_sheet()).unwrap_err(),
        StatementsError::RowInconsistent
    );
}

#[test]
fn a_closing_that_no_movement_explains_is_refused_even_with_empty_columns() {
    // Synthetic mutation: the sales ledger keeps its 4027.00 closing but loses
    // its credit, so its closing is in the carried line and not in the P&L.
    let mut report = known_lab();
    row(&mut report, "Ageing Sales").credit = NativeTrialBalanceAmount::PresentEmpty;
    assert_eq!(
        derive_statements(&admitted(report), &groups(), &known_lab_balance_sheet()).unwrap_err(),
        StatementsError::RowInconsistent
    );
}

#[test]
fn a_second_root_ledger_is_refused() {
    // Synthetic mutation: another captured ledger moved to the root.
    let mut report = known_lab();
    let root = root_parent(&report);
    row(&mut report, "Cash").parent = root;
    assert_eq!(
        derive_statements(&admitted(report), &groups(), &known_lab_balance_sheet()).unwrap_err(),
        StatementsError::RootLedgerRepeated
    );
}

#[test]
fn the_gate_takes_only_a_balance_sheet() {
    let tally = statement(NativeStatementKind::ProfitAndLoss, &[("Sales Accounts", "", "4027.00")]);
    assert_eq!(
        derive_statements(&admitted(known_lab()), &groups(), &tally).unwrap_err(),
        StatementsError::GateNotABalanceSheet
    );
}

#[test]
fn the_profit_and_loss_tie_is_reported_and_gates_nothing() {
    let derived = derive(known_lab(), &known_lab_balance_sheet());
    let tie = profit_and_loss_tie(
        &derived,
        &statement(NativeStatementKind::ProfitAndLoss, &[("Cost of Sales :", "", "-1.00")]),
    );
    assert_eq!(
        tie.lines[0].status,
        TieStatus::NotCompared {
            reason: "no_derived_line_of_that_name"
        }
    );
    assert_eq!(
        tie.derived_only,
        vec![line(&derived.profit_and_loss, "Sales Accounts").display_name.clone()]
    );
    assert_established(&derived.net_result, "4027.00");
}

/// Decoded as production decodes a response: BOM-less UTF-16LE under the
/// `charset=utf-16` content type Tally sent.
fn utf16(bytes: &[u8]) -> String {
    bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .unwrap()
    .text
}

struct Capture {
    guid: &'static str,
    trial_balance: &'static [u8],
    groups: &'static [u8],
    balance_sheet: &'static [u8],
    profit_and_loss: &'static [u8],
}

impl Capture {
    fn derive(&self) -> (DerivedStatements, TieOut) {
        let report = parse_native_trial_balance(&utf16(self.trial_balance), self.guid).unwrap();
        let groups = parse_native_group_snapshot(&utf16(self.groups), self.guid).unwrap();
        let tally_balance_sheet =
            parse_native_statement(NativeStatementKind::BalanceSheet, &utf16(self.balance_sheet))
                .unwrap();
        let tally_profit_and_loss = parse_native_statement(
            NativeStatementKind::ProfitAndLoss,
            &utf16(self.profit_and_loss),
        )
        .unwrap();
        let derived =
            derive_statements(&admitted(report), &groups, &tally_balance_sheet).unwrap();
        let tie = profit_and_loss_tie(&derived, &tally_profit_and_loss);
        (derived, tie)
    }
}

fn statuses(tie: &TieOut) -> Vec<(&str, TieStatus)> {
    tie.lines
        .iter()
        .map(|line| (line.name.as_str(), line.status.clone()))
        .collect()
}

/// One company, one window, four captures read within two minutes: the Trial
/// Balance and group tree derive the statements, Tally's own Balance Sheet
/// passes the gate, and Tally's own Profit and Loss ties.
#[test]
fn a_same_company_full_year_passes_the_gate() {
    let (derived, tie) = Capture {
        guid: "de2e15f2-6d42-4715-b6e7-b7a95a68abe8",
        trial_balance: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_trial_balance_fy_live.utf16le.xml"
        ),
        groups: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_groups_fy_live.utf16le.xml"
        ),
        balance_sheet: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_balance_sheet_fy_live.utf16le.xml"
        ),
        profit_and_loss: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_profit_and_loss_fy_live.utf16le.xml"
        ),
    }
    .derive();

    assert!(derived.unclassified.is_empty());
    assert_sum(&line(&derived.profit_and_loss, "Purchase Accounts").amount, "-4250.00", 1, 1);
    assert_sum(&line(&derived.balance_sheet, "Current Liabilities").amount, "4250.00", 1, 3);
    assert_established(&derived.gross_result, "-4250.00");
    assert_established(&derived.net_result, "-4250.00");
    assert_established(&derived.balance_sheet_profit_and_loss, "-4250.00");
    assert_eq!(
        statuses(&derived.balance_sheet_tie),
        vec![
            ("Capital Account", TieStatus::MatchedEmptyAsZero),
            ("Loans (Liability)", TieStatus::MatchedEmptyAsZero),
            ("Current Liabilities", TieStatus::Matched),
            ("Profit & Loss A/c", TieStatus::Matched),
            ("Current Assets", TieStatus::MatchedEmptyAsZero),
        ]
    );
    assert!(derived.balance_sheet_tie.derived_only.is_empty());
    assert_eq!(
        statuses(&tie),
        vec![
            (
                "Cost of Sales :",
                TieStatus::NotCompared {
                    reason: "no_derived_line_of_that_name"
                }
            ),
            ("Purchase Accounts", TieStatus::Matched),
        ]
    );
    assert!(tie.derived_only.is_empty());
}

/// One month of a 29,900-voucher book. In a part-year window a P&L ledger's
/// Trial Balance covers the window only, and the year's earlier result sits in
/// the Profit & Loss A/c ledger's opening. The gate still passes.
#[test]
fn a_same_company_part_year_window_on_a_heavy_book_passes_the_gate() {
    let (derived, tie) = Capture {
        guid: "d45bc1b0-e5e3-4261-b3b2-cce3915f42d3",
        trial_balance: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_trial_balance_dense_month_live.utf16le.xml"
        ),
        groups: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_groups_dense_live.utf16le.xml"
        ),
        balance_sheet: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_balance_sheet_dense_month_live.utf16le.xml"
        ),
        profit_and_loss: include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/statement_profit_and_loss_dense_month_live.utf16le.xml"
        ),
    }
    .derive();

    assert!(derived.unclassified.is_empty());
    assert_sum(&line(&derived.profit_and_loss, "Sales Accounts").amount, "113726661.73", 1, 1);
    assert_eq!(line(&derived.balance_sheet, "Current Assets").ledger_count, 121);
    let root = derived.profit_and_loss_ledger.as_ref().unwrap();
    assert_eq!(
        root.closing,
        NativeTrialBalanceAmount::Present(decimal("109235760.65"))
    );
    assert_established(&derived.net_result, "113726661.73");
    assert_established(&derived.balance_sheet_profit_and_loss, "222962422.38");
    assert_eq!(
        statuses(&derived.balance_sheet_tie),
        vec![
            ("Capital Account", TieStatus::MatchedEmptyAsZero),
            ("Loans (Liability)", TieStatus::MatchedEmptyAsZero),
            ("Current Liabilities", TieStatus::MatchedEmptyAsZero),
            ("Profit & Loss A/c", TieStatus::Matched),
            ("Current Assets", TieStatus::Matched),
        ]
    );
    assert!(derived.balance_sheet_tie.derived_only.is_empty());
    assert_eq!(statuses(&tie), vec![("Sales Accounts", TieStatus::Matched)]);
    assert!(tie.derived_only.is_empty());
}
