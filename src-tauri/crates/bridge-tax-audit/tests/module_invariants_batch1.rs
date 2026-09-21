// SPDX-License-Identifier: Apache-2.0
//! Batch 1's edges the synthetic golden does not reach.
//!
//! Module invariants: the synthetic golden exercises only TB-2 (its Trial Balance carries a
//! deliberate skew), so every other check is driven here on a small invented book, by a book whose
//! facts contradict what the test reports or by tampering with a reported figure.
//!
//! Boundaries and branches (the `edge_*` tests): each expected value was produced by the Python
//! reference implementation's own module on the identical book (recorded in PROVENANCE.md), not
//! derived by hand, and the ledger tags are the reference's own. Every name and amount below is
//! invented.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;
use bridge_tax_audit::book::{Book, Ledger, LedgerLine, TbRow, Voucher, VoucherStatus};
use bridge_tax_audit::findings::Value;
use bridge_tax_audit::read::Window;
use bridge_tax_audit::rules::Rules;
use bridge_tax_audit::{cash_book_integrity, ledger_scrutiny, stale_balances_41_1, trial_balance};

fn ledger(name: &str, chain: &[&str]) -> Ledger {
    Ledger {
        name: name.to_string(),
        parent: chain[0].to_string(),
        chain: chain.iter().map(|g| (*g).to_string()).collect(),
        chain_complete: true,
        master_opening_paise: 0,
        guid: String::new(),
        masterid: None,
    }
}

fn tb(opening: i64, debit: i64, credit: i64, closing: i64) -> TbRow {
    TbRow {
        opening_paise: opening,
        debit_paise: debit,
        credit_paise: credit,
        closing_paise: closing,
    }
}

fn voucher(guid: &str, date: &str, base: &str, lines: &[(&str, i64)]) -> Voucher {
    Voucher {
        guid: guid.to_string(),
        date: TallyDate::parse(date).unwrap(),
        vtype: base.to_string(),
        base_type: base.to_string(),
        number: guid.to_string(),
        status: VoucherStatus::Regular,
        lines: lines
            .iter()
            .map(|(l, a)| LedgerLine {
                ledger: (*l).to_string(),
                amount_paise: *a,
            })
            .collect(),
        narration: String::new(),
    }
}

fn book(ledgers: Vec<Ledger>, tb_rows: Vec<(&str, TbRow)>, vouchers: Vec<Voucher>) -> Book {
    Book {
        company_name: "Invented Traders".to_string(),
        company_guid: "invented-guid".to_string(),
        read_at: String::new(),
        groups: BTreeMap::new(),
        group_masters: BTreeMap::new(),
        ledgers: ledgers.into_iter().map(|l| (l.name.clone(), l)).collect(),
        vouchers,
        tb: tb_rows
            .into_iter()
            .map(|(n, t)| (n.to_string(), t))
            .collect(),
    }
}

fn rules() -> Rules {
    Rules::vendored().unwrap()
}

fn set_int(result: &mut bridge_tax_audit::findings::TestResult, suffix: &str, v: i64) {
    let f = result
        .figures
        .iter_mut()
        .find(|f| f.id.ends_with(suffix))
        .unwrap_or_else(|| panic!("no figure ending {suffix}"));
    f.value = Value::Int(v);
}

#[test]
fn tb1_fires_when_a_listed_closing_differs_from_the_trial_balance() {
    let b = book(
        vec![ledger("Stores", &["Indirect Expenses"])],
        vec![
            ("Stores", tb(0, 500, 0, 500)),
            ("Cash", tb(0, 0, 500, -500)),
        ],
        Vec::new(),
    );
    let mut r = trial_balance::run(&b, &rules()).unwrap();
    assert!(trial_balance::check_invariants(&b, &r).unwrap().is_empty());
    let tag = bridge_tax_audit::ledger_ids::stable_ledger_tag(&b, "Stores").unwrap();
    set_int(&mut r, &format!("tb_closing_{tag}"), 499);
    let v = trial_balance::check_invariants(&b, &r).unwrap();
    assert!(v.iter().any(|m| m.starts_with("TB-1")), "{v:?}");
    // Moving one closing by a paisa also unbalances the period's movements.
    assert!(v.iter().any(|m| m.starts_with("TB-2")), "{v:?}");
}

#[test]
fn stl1_fires_when_a_stale_ledger_has_a_population_line() {
    // The TB says no movement, but a regular voucher posts to the debtor: contradictory books.
    let b = book(
        vec![
            ledger("Quiet Debtor", &["Sundry Debtors"]),
            ledger("Cash", &["Cash-in-Hand"]),
        ],
        vec![
            ("Quiet Debtor", tb(20_000, 0, 0, 20_000)),
            ("Cash", tb(0, 0, 0, 0)),
        ],
        vec![voucher(
            "v1",
            "20250601",
            "Receipt",
            &[("Quiet Debtor", -100), ("Cash", 100)],
        )],
    );
    let r = stale_balances_41_1::run(&b, &rules()).unwrap();
    let v = stale_balances_41_1::check_invariants(&b, &r).unwrap();
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].starts_with("STL-1: Quiet Debtor reported as an unmoved debtor"));
}

#[test]
fn stl1_reports_a_tag_it_cannot_resolve() {
    let b = book(
        vec![ledger("Quiet Debtor", &["Sundry Debtors"])],
        vec![("Quiet Debtor", tb(20_000, 0, 0, 20_000))],
        Vec::new(),
    );
    let mut r = stale_balances_41_1::run(&b, &rules()).unwrap();
    let f = r
        .figures
        .iter_mut()
        .find(|f| f.id.contains("_unmoved_ledger_"))
        .unwrap();
    f.id = "stale_balances_41_1.debtor_unmoved_ledger_nosuchtag".to_string();
    let v = stale_balances_41_1::check_invariants(&b, &r).unwrap();
    assert_eq!(
        v,
        vec!["STL-1: cannot resolve a debtor ledger for figure stale_balances_41_1.debtor_unmoved_ledger_nosuchtag (tag nosuchtag)"]
    );
}

#[test]
fn lsc1_fires_when_the_walk_exceeds_the_tb_debit_by_more_than_a_rupee() {
    let period = Window {
        from: TallyDate::parse("20250401").unwrap(),
        to: TallyDate::parse("20260331").unwrap(),
    };
    let b = book(
        vec![
            ledger("Repairs", &["Indirect Expenses"]),
            ledger("Cash", &["Cash-in-Hand"]),
        ],
        // The TB claims only Rs 1 of debits; the walk finds Rs 5,000.
        vec![
            ("Repairs", tb(0, 100, 0, 100)),
            ("Cash", tb(0, 0, 100, -100)),
        ],
        vec![voucher(
            "v1",
            "20250601",
            "Payment",
            &[("Repairs", 500_000), ("Cash", -500_000)],
        )],
    );
    let cash: BTreeSet<String> = ["Cash".to_string()].into();
    let r = ledger_scrutiny::run(&b, &rules(), &period, &cash).unwrap();
    let v = ledger_scrutiny::check_invariants(&b, &r).unwrap();
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].starts_with("LSC-1: Repairs population-walk total_debit_paise (500000p)"));
    // Exactly Re 1 over the TB is within tolerance.
    let b2 = book(
        vec![ledger("Repairs", &["Indirect Expenses"])],
        vec![("Repairs", tb(0, 499_900, 0, 499_900))],
        vec![voucher(
            "v1",
            "20250601",
            "Journal",
            &[("Repairs", 500_000)],
        )],
    );
    let r2 = ledger_scrutiny::run(&b2, &rules(), &period, &BTreeSet::new()).unwrap();
    assert!(ledger_scrutiny::check_invariants(&b2, &r2)
        .unwrap()
        .is_empty());
}

#[test]
fn cbi1_fires_on_a_tampered_opening_difference_and_cbi2_on_an_unwalked_negative_close() {
    let cash: BTreeSet<String> = ["Cash".to_string()].into();
    // Cash closes negative in the TB, but no voucher moves it, so the walk finds no negative day.
    let b = book(
        vec![ledger("Cash", &["Cash-in-Hand"])],
        vec![("Cash", tb(0, 0, 0, -1_000))],
        Vec::new(),
    );
    let mut r = cash_book_integrity::run(&b, &rules(), &cash, &BTreeSet::new(), &[]).unwrap();
    let v = cash_book_integrity::check_invariants(&b, &r).unwrap();
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].starts_with("CBI-2: cash ledger tag"), "{v:?}");
    set_int(&mut r, "opening_difference", 7);
    let v = cash_book_integrity::check_invariants(&b, &r).unwrap();
    assert!(v
        .iter()
        .any(|m| m == "CBI-1: opening_difference != Trial Balance opening sum"));
}

fn fig(result: &bridge_tax_audit::findings::TestResult, id: &str) -> Value {
    result
        .figures
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("no figure {id}"))
        .value
        .clone()
}

fn period() -> Window {
    Window {
        from: TallyDate::parse("20250401").unwrap(),
        to: TallyDate::parse("20260331").unwrap(),
    }
}

fn narrated(mut v: Voucher, text: &str) -> Voucher {
    v.narration = text.to_string();
    v
}

/// "Active" is abs(closing) OVER Re 1: a closing of exactly 100 paise is not active.
#[test]
fn edge_stale_active_boundary_is_strictly_over_one_rupee() {
    let b = book(
        vec![
            ledger("D100", &["Sundry Debtors"]),
            ledger("D101", &["Sundry Debtors"]),
        ],
        vec![("D100", tb(100, 0, 0, 100)), ("D101", tb(101, 0, 0, 101))],
        Vec::new(),
    );
    let r = stale_balances_41_1::run(&b, &rules()).unwrap();
    let id = |n: &str| format!("stale_balances_41_1.{n}");
    assert_eq!(fig(&r, &id("debtor_ledger_count")), Value::Int(2));
    assert_eq!(fig(&r, &id("debtor_active_count")), Value::Int(1));
    assert_eq!(fig(&r, &id("debtor_stale_count")), Value::Int(1));
    assert_eq!(
        fig(&r, &id("debtor_stale_closing_total_paise")),
        Value::Int(101)
    );
    assert_eq!(
        fig(&r, &id("debtor_unmoved_ledger_2845edf9")),
        Value::Int(101)
    );
}

/// Large is abs(net) OVER the threshold (exactly Rs 50,000 is not large), and a credit entry
/// on the expense ledger is never cash-paid even when its voucher has a cash leg.
#[test]
fn edge_ledger_scrutiny_large_boundary_and_cash_paid_debits_only() {
    let b = book(
        vec![
            ledger("Repairs", &["Indirect Expenses"]),
            ledger("Cash", &["Cash-in-Hand"]),
            ledger("Sundry", &["Suspense"]),
        ],
        vec![
            ("Repairs", tb(0, 10_000_001, 100_000, 9_900_001)),
            ("Cash", tb(0, 100_000, 5_000_000, -4_900_000)),
        ],
        vec![
            voucher(
                "v1",
                "20250601",
                "Payment",
                &[("Repairs", 5_000_000), ("Cash", -5_000_000)],
            ),
            voucher(
                "v2",
                "20250701",
                "Receipt",
                &[("Repairs", -100_000), ("Cash", 100_000)],
            ),
            voucher(
                "v3",
                "20250801",
                "Journal",
                &[("Repairs", 5_000_001), ("Sundry", -5_000_001)],
            ),
        ],
    );
    let cash: BTreeSet<String> = ["Cash".to_string()].into();
    let r = ledger_scrutiny::run(&b, &rules(), &period(), &cash).unwrap();
    let id = |n: &str| format!("ledger_scrutiny.{n}_cad740c5");
    assert_eq!(fig(&r, &id("large_entry_count")), Value::Int(1));
    assert_eq!(fig(&r, &id("round_sum_count")), Value::Int(2));
    assert_eq!(fig(&r, &id("cash_paid_paise")), Value::Int(5_000_000));
    assert_eq!(fig(&r, &id("total_debit_paise")), Value::Int(10_000_001));
    assert_eq!(fig(&r, &id("cash_share_bp")), Value::Int(5000));
}

/// Narrations that differ only in whitespace runs and case are the same text; an empty one is
/// unnarrated; money received into cash that credits an expense ledger is an expense credit.
#[test]
fn edge_cash_book_narrations_and_expense_credits() {
    let b = book(
        vec![
            ledger("Cash", &["Cash-in-Hand"]),
            ledger("Wages", &["Direct Expenses"]),
            ledger("Sundry", &["Suspense"]),
        ],
        vec![("Cash", tb(0, 0, 0, 0)), ("Wages", tb(0, 0, 0, 0))],
        vec![
            voucher(
                "c1",
                "20250601",
                "Payment",
                &[("Cash", -100), ("Sundry", 100)],
            ),
            voucher(
                "c2",
                "20250602",
                "Receipt",
                &[("Cash", 50), ("Sundry", -50)],
            ),
            voucher(
                "c3",
                "20250603",
                "Payment",
                &[("Cash", -50), ("Sundry", 50)],
            ),
            narrated(
                voucher(
                    "j1",
                    "20250701",
                    "Journal",
                    &[("Wages", 200), ("Cash", -200)],
                ),
                "Wages  week\t1",
            ),
            narrated(
                voucher(
                    "j2",
                    "20250702",
                    "Journal",
                    &[("Wages", 300), ("Cash", -300)],
                ),
                "wages week 1",
            ),
            voucher("j3", "20250703", "Journal", &[("Wages", 10), ("Cash", -10)]),
            voucher(
                "r1",
                "20250801",
                "Receipt",
                &[("Cash", 700), ("Wages", -700)],
            ),
        ],
    );
    let cash: BTreeSet<String> = ["Cash".to_string()].into();
    let r = cash_book_integrity::run(&b, &rules(), &cash, &BTreeSet::new(), &[]).unwrap();
    let w = |n: &str| format!("cash_book_integrity.{n}_060718ae");
    assert_eq!(fig(&r, &w("journal_cash_total")), Value::Int(510));
    assert_eq!(fig(&r, &w("journal_cash_count")), Value::Int(3));
    assert_eq!(
        fig(&r, &w("journal_cash_repeated_narration_count")),
        Value::Int(2)
    );
    assert_eq!(
        fig(&r, &w("journal_cash_repeated_narration_total")),
        Value::Int(500)
    );
    assert_eq!(fig(&r, &w("journal_cash_unnarrated_count")), Value::Int(1));
    assert_eq!(fig(&r, &w("journal_cash_unnarrated_total")), Value::Int(10));
    assert_eq!(fig(&r, &w("expense_credit_total")), Value::Int(700));
    assert_eq!(fig(&r, &w("expense_credit_count")), Value::Int(1));
    let c = |n: &str| format!("cash_book_integrity.{n}_758ec54e");
    assert_eq!(fig(&r, &c("negative_days_best_case")), Value::Int(6));
    assert_eq!(fig(&r, &c("negative_days_worst_case")), Value::Int(7));
    assert_eq!(fig(&r, &c("lowest_best_case_balance")), Value::Int(-610));
    assert_eq!(
        fig(&r, &c("lowest_best_case_date")),
        Value::Text("2025-07-03".to_string())
    );
}

/// Two days at the same lowest closing: the first is reported, as Python's min() returns the
/// first minimum.
#[test]
fn edge_cash_book_lowest_day_is_the_first_of_equal_minima() {
    let b = book(
        vec![
            ledger("Cash", &["Cash-in-Hand"]),
            ledger("Sundry", &["Suspense"]),
        ],
        vec![("Cash", tb(0, 50, 150, -100))],
        vec![
            voucher(
                "c1",
                "20250601",
                "Payment",
                &[("Cash", -100), ("Sundry", 100)],
            ),
            voucher(
                "c2",
                "20250602",
                "Receipt",
                &[("Cash", 50), ("Sundry", -50)],
            ),
            voucher(
                "c3",
                "20250603",
                "Payment",
                &[("Cash", -50), ("Sundry", 50)],
            ),
        ],
    );
    let cash: BTreeSet<String> = ["Cash".to_string()].into();
    let r = cash_book_integrity::run(&b, &rules(), &cash, &BTreeSet::new(), &[]).unwrap();
    let c = |n: &str| format!("cash_book_integrity.{n}_758ec54e");
    assert_eq!(fig(&r, &c("negative_days_best_case")), Value::Int(3));
    assert_eq!(fig(&r, &c("lowest_best_case_balance")), Value::Int(-100));
    assert_eq!(
        fig(&r, &c("lowest_best_case_date")),
        Value::Text("2025-06-01".to_string())
    );
}
