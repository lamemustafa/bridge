//! Most tests pair a captured Trial Balance from one synthetic company with a
//! verbatim group tree captured from another. Every `PARENT` in those Trial
//! Balances names a group Tally creates in every company, so each hop resolves
//! through Tally's own default tree. That pairing crosses companies and proves
//! classification and arithmetic only. The tie itself is proven by the last
//! test, where the Trial Balance, group tree and both statements are one
//! company's captures of one window (#692).
//!
//! Tests marked "synthetic mutation" alter a captured row or group to reach a
//! branch the captures do not; they make no claim about Tally output.

use super::*;
use bridge_tally_protocol::{
    native_outstandings::parse_native_group_snapshot,
    native_statement_reports::{NativeStatementKind, NativeStatementLine},
    native_trial_balance::parse_native_trial_balance,
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

fn decimal(value: &str) -> ExactDecimal {
    ExactDecimal::parse(value).unwrap()
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

fn line<'a>(lines: &'a [PrimaryGroupLine], reserved: &str) -> &'a PrimaryGroupLine {
    lines
        .iter()
        .find(|line| line.reserved_name == reserved)
        .unwrap_or_else(|| panic!("no {reserved} line"))
}

#[test]
fn a_captured_trial_balance_splits_into_statement_lines() {
    let report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let derived = derive_statements(&report, &groups()).unwrap();

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
    let derived = derive_statements(&report, &groups()).unwrap();

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

fn root_parent(report: &NativeTrialBalance) -> PartyLedgerMasterFieldObservation {
    report
        .rows
        .iter()
        .find(|row| row.name == "Profit & Loss A/c")
        .unwrap()
        .parent
        .clone()
}

#[test]
fn a_ledger_under_a_user_created_primary_group_blocks_every_result() {
    // Synthetic mutation: a user-created primary group, and one captured
    // ledger moved under it.
    let mut report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let mut groups = groups();
    groups.push(TallyNamedMaster {
        name: "BRIDGE Synthetic Primary".to_string(),
        parent: root_parent(&report),
        reserved_name: Some(String::new()),
    });
    let row = report.rows.iter_mut().find(|row| row.name == "Ageing Sales").unwrap();
    row.parent = PartyLedgerMasterFieldObservation::Returned("BRIDGE Synthetic Primary".to_string());

    let derived = derive_statements(&report, &groups).unwrap();
    assert_eq!(line(&derived.profit_and_loss, "Sales Accounts").ledger_count, 0);
    assert_eq!(derived.unclassified.len(), 1);
    assert_eq!(derived.unclassified[0].reason, "primary_group_user_created");
    let blocked = Established::NotEstablished {
        reason: "unclassified_ledger_carries_an_amount",
    };
    assert_eq!(derived.gross_result, blocked);
    assert_eq!(derived.net_result, blocked);
    assert_eq!(derived.balance_sheet_profit_and_loss, blocked);
}

#[test]
fn an_unclassified_ledger_with_no_amount_blocks_nothing() {
    // Synthetic mutation: the captured all-empty ledger loses its parent group.
    let mut report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let row = report.rows.iter_mut().find(|row| row.name == "Ageing Bank").unwrap();
    row.parent = PartyLedgerMasterFieldObservation::Returned("BRIDGE Absent Group".to_string());

    let derived = derive_statements(&report, &groups()).unwrap();
    assert_eq!(derived.unclassified.len(), 1);
    assert_eq!(derived.unclassified[0].reason, "group_absent");
    assert_established(&derived.net_result, "4027.00");
}

#[test]
fn a_stock_balance_blocks_the_profit_but_not_the_carried_line() {
    // Synthetic mutation: a captured ledger with a non-zero closing moved
    // under Stock-in-Hand.
    let mut report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let row = report.rows.iter_mut().find(|row| row.name == "Cash").unwrap();
    row.parent = PartyLedgerMasterFieldObservation::Returned("Stock-in-Hand".to_string());

    let derived = derive_statements(&report, &groups()).unwrap();
    assert_eq!(derived.stock_ledger_count, 1);
    let blocked = Established::NotEstablished {
        reason: "closing_stock_not_derivable_from_trial_balance",
    };
    assert_eq!(derived.gross_result, blocked);
    assert_eq!(derived.net_result, blocked);
    assert_established(&derived.balance_sheet_profit_and_loss, "11027.00");
}

#[test]
fn a_row_whose_columns_do_not_add_up_is_refused() {
    // Synthetic mutation: the one captured row with all four amounts present.
    let mut report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let row = report
        .rows
        .iter_mut()
        .find(|row| row.name == "Ageing Customer A")
        .unwrap();
    row.closing = NativeTrialBalanceAmount::Present(decimal("-7277.01"));
    assert_eq!(
        derive_statements(&report, &groups()).unwrap_err(),
        StatementsError::RowInconsistent
    );
}

#[test]
fn a_second_root_ledger_is_refused() {
    // Synthetic mutation: another captured ledger moved to the root.
    let mut report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let root = root_parent(&report);
    report.rows.iter_mut().find(|row| row.name == "Cash").unwrap().parent = root;
    assert_eq!(
        derive_statements(&report, &groups()).unwrap_err(),
        StatementsError::RootLedgerRepeated
    );
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

#[test]
fn tie_out_reports_each_line_and_enforces_nothing() {
    // Synthetic statements shaped like the captured Balance Sheet (a main
    // amount per line), over the derived lines above.
    let report = parse_native_trial_balance(KNOWN_LAB, KNOWN_LAB_GUID).unwrap();
    let derived = derive_statements(&report, &groups()).unwrap();
    let current_assets = line(&derived.balance_sheet, "Current Assets").display_name.clone();

    let tie = tie_out(
        &derived,
        &statement(
            NativeStatementKind::BalanceSheet,
            &[
                ("Capital Account", "", ""),
                ("Sources of Funds :", "", "-1.00"),
                (&current_assets, "", "-11027.00"),
                ("Profit & Loss A/c", "", "11027.00"),
            ],
        ),
    );
    let statuses: Vec<_> = tie.lines.iter().map(|line| line.status.clone()).collect();
    assert_eq!(
        statuses,
        vec![
            // A primary group with no ledger, which Tally shows empty.
            TieStatus::MatchedEmptyAsZero,
            TieStatus::NotCompared {
                reason: "no_derived_line_of_that_name"
            },
            TieStatus::Matched,
            TieStatus::Matched,
        ]
    );
    assert!(tie.derived_only.is_empty());

    let tie = tie_out(
        &derived,
        &statement(
            NativeStatementKind::BalanceSheet,
            &[(&current_assets, "-1.00", "-11027.00"), ("Profit & Loss A/c", "", "11026.00")],
        ),
    );
    assert_eq!(
        tie.lines[0].status,
        TieStatus::NotCompared {
            reason: "both_tally_columns_present"
        }
    );
    assert_eq!(
        tie.lines[1].status,
        TieStatus::Differs {
            derived: match &derived.balance_sheet_profit_and_loss {
                Established::Established { value } => value.clone(),
                other => panic!("{other:?}"),
            }
        }
    );

    // A P&L without the derived Sales line names it as derived-only.
    let tie = tie_out(
        &derived,
        &statement(NativeStatementKind::ProfitAndLoss, &[("Cost of Sales :", "", "")]),
    );
    assert_eq!(tie.derived_only, vec![line(&derived.profit_and_loss, "Sales Accounts").display_name.clone()]);
}

const READS_LAB_GUID: &str = "de2e15f2-6d42-4715-b6e7-b7a95a68abe8";
const READS_LAB_TB: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/statement_trial_balance_fy_live.utf16le.xml"
);
const READS_LAB_GROUPS: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/statement_groups_fy_live.utf16le.xml"
);
const READS_LAB_BS: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/statement_balance_sheet_fy_live.utf16le.xml"
);
const READS_LAB_PL: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/statement_profit_and_loss_fy_live.utf16le.xml"
);

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

/// One company, one window, four captures read within two minutes: the Trial
/// Balance and group tree derive the statements, and Tally's own two
/// statements are the tie.
#[test]
fn a_same_company_capture_ties_to_tallys_own_statements() {
    let report = parse_native_trial_balance(&utf16(READS_LAB_TB), READS_LAB_GUID).unwrap();
    let groups = parse_native_group_snapshot(&utf16(READS_LAB_GROUPS), READS_LAB_GUID).unwrap();
    let derived = derive_statements(&report, &groups).unwrap();

    assert!(derived.unclassified.is_empty());
    assert_sum(&line(&derived.profit_and_loss, "Purchase Accounts").amount, "-4250.00", 1, 1);
    assert_sum(&line(&derived.balance_sheet, "Current Liabilities").amount, "4250.00", 1, 3);
    assert_established(&derived.gross_result, "-4250.00");
    assert_established(&derived.net_result, "-4250.00");
    assert_established(&derived.balance_sheet_profit_and_loss, "-4250.00");

    let balance_sheet = bridge_tally_protocol::native_statement_reports::parse_native_statement(
        NativeStatementKind::BalanceSheet,
        &utf16(READS_LAB_BS),
    )
    .unwrap();
    let tie = tie_out(&derived, &balance_sheet);
    let statuses: Vec<_> = tie
        .lines
        .iter()
        .map(|line| (line.name.as_str(), line.status.clone()))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("Capital Account", TieStatus::MatchedEmptyAsZero),
            ("Loans (Liability)", TieStatus::MatchedEmptyAsZero),
            ("Current Liabilities", TieStatus::Matched),
            ("Profit & Loss A/c", TieStatus::Matched),
            ("Current Assets", TieStatus::MatchedEmptyAsZero),
        ]
    );
    assert!(tie.derived_only.is_empty());

    let profit_and_loss = bridge_tally_protocol::native_statement_reports::parse_native_statement(
        NativeStatementKind::ProfitAndLoss,
        &utf16(READS_LAB_PL),
    )
    .unwrap();
    let tie = tie_out(&derived, &profit_and_loss);
    let statuses: Vec<_> = tie
        .lines
        .iter()
        .map(|line| (line.name.as_str(), line.status.clone()))
        .collect();
    assert_eq!(
        statuses,
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
