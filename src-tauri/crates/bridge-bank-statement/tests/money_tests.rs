//! Amounts, control values and the balance chain, ported from
//! `scripts/bank_statement_import.test.py`.

mod common;

use bridge_bank_statement::money::{
    balance, control_value, money, reconcile, verify_against_statement,
};
use bridge_tally_primitives::ExactDecimal;
use common::*;

fn d(text: &str) -> ExactDecimal {
    ExactDecimal::parse(text).unwrap()
}

fn same(left: &ExactDecimal, right: &str) -> bool {
    left.numeric_eq(&d(right))
}

#[test]
fn control_values_take_the_operator_at_their_word() {
    assert!(same(
        &control_value("1,00,000.00", "opening", true).unwrap(),
        "100000.00"
    ));
    assert!(same(
        &control_value("-248044.20", "closing", true).unwrap(),
        "-248044.20"
    ));
    refuses(
        control_value("-1.00", "debits", false),
        "malformed_control_value",
    );
    refuses(
        control_value("1.005", "opening", true),
        "malformed_control_value",
    );
    refuses(
        control_value("abc", "opening", true),
        "malformed_control_value",
    );
}

#[test]
fn amount_parsing_is_strict() {
    assert!(money("", "dr", 0).unwrap().is_none());
    assert!(money("  ", "dr", 0).unwrap().is_none());
    assert!(same(
        &money("1234.50", "dr", 0).unwrap().unwrap(),
        "1234.50"
    ));
    refuses(money("1.005", "dr", 0), "malformed_amount");
    refuses(money("garbage", "dr", 0), "malformed_amount");
    assert!(same(
        &balance("-248044.20", "bal", 0).unwrap().unwrap(),
        "-248044.20"
    ));
    refuses(balance("248044.20Dr", "bal", 0), "malformed_balance");
    // deliberate divergence from Python's `\d`: other scripts' digits refuse
    refuses(money("١٠.٠٠", "dr", 0), "malformed_amount");
}

fn chain() -> Vec<bridge_bank_statement::parse::Row> {
    vec![
        row(&[
            ("date", "01/08/26"),
            ("dr", ""),
            ("cr", "100.00"),
            ("bal", "1100.00"),
        ]),
        row(&[
            ("date", "02/08/26"),
            ("dr", "50.00"),
            ("cr", ""),
            ("bal", "1050.00"),
        ]),
    ]
}

#[test]
fn reconcile_replays_the_running_balance() {
    let rows = chain();
    assert!(same(
        &reconcile(&rows, &d("1000.00"), &d("1050.00")).unwrap(),
        "1050.00"
    ));

    let mut broken = rows.clone();
    broken[1].set("bal", "1049.00");
    let refusal = refuses(
        reconcile(&broken, &d("1000.00"), &d("1049.00")),
        "balance_chain_broken",
    );
    assert_eq!(refusal.row, Some(2));

    // a self-consistent prefix is still not the statement
    refuses(
        reconcile(&rows[..1], &d("1000.00"), &d("1050.00")),
        "extent_unproven",
    );
    refuses(
        reconcile(&[], &d("1000.00"), &d("1000.00")),
        "empty_statement",
    );

    let two_sided = [row(&[
        ("date", "01/08/26"),
        ("dr", "50.00"),
        ("cr", "150.00"),
        ("bal", "1100.00"),
    ])];
    refuses(
        reconcile(&two_sided, &d("1000.00"), &d("1100.00")),
        "two_sided_row",
    );

    let overdrawn = [row(&[
        ("date", "01/08/26"),
        ("dr", "1500.00"),
        ("cr", ""),
        ("bal", "-500.00"),
    ])];
    assert!(same(
        &reconcile(&overdrawn, &d("1000.00"), &d("-500.00")).unwrap(),
        "-500.00"
    ));

    // a blank balance cell is a broken chain, not a skipped check
    let blank = [row(&[
        ("date", "01/08/26"),
        ("dr", "1.00"),
        ("cr", ""),
        ("bal", ""),
    ])];
    refuses(
        reconcile(&blank, &d("1.00"), &d("0.00")),
        "balance_chain_broken",
    );
}

#[test]
fn closing_balance_alone_cannot_prove_extent() {
    let full = vec![
        row(&[
            ("date", "01/08/26"),
            ("dr", ""),
            ("cr", "100.00"),
            ("bal", "1100.00"),
        ]),
        row(&[
            ("date", "02/08/26"),
            ("dr", "50.00"),
            ("cr", ""),
            ("bal", "1050.00"),
        ]),
        row(&[
            ("date", "03/08/26"),
            ("dr", ""),
            ("cr", "50.00"),
            ("bal", "1100.00"),
        ]),
    ];
    let truncated = &full[..1];
    assert!(same(
        &reconcile(&full, &d("1000.00"), &d("1100.00")).unwrap(),
        "1100.00"
    ));
    assert!(same(
        &reconcile(truncated, &d("1000.00"), &d("1100.00")).unwrap(),
        "1100.00"
    ));
    // the debit total does see it
    verify_against_statement(&full, &d("50.00"), &d("150.00")).unwrap();
    refuses(
        verify_against_statement(truncated, &d("50.00"), &d("150.00")),
        "control_total_mismatch",
    );
}

#[test]
fn control_totals_are_all_compared() {
    let rows = chain();
    let totals = verify_against_statement(&rows, &d("50.00"), &d("100.00")).unwrap();
    assert!(same(&totals.debits, "50.00") && same(&totals.credits, "100.00"));
    refuses(
        verify_against_statement(&rows, &d("60.00"), &d("100.00")),
        "control_total_mismatch",
    );
    refuses(
        verify_against_statement(&rows, &d("50.00"), &d("110.00")),
        "control_total_mismatch",
    );
}

#[test]
fn a_zero_in_one_amount_column_is_still_two_sided() {
    let rows = [row(&[
        ("date", "01/08/26"),
        ("narr", "x"),
        ("ref", "1"),
        ("dr", "0.00"),
        ("cr", "10.00"),
        ("bal", "10.00"),
    ])];
    let refusal = refuses(reconcile(&rows, &d("0"), &d("10.00")), "two_sided_row");
    assert!(refusal.message.contains("both amount columns"));
}
