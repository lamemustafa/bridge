// SPDX-License-Identifier: Apache-2.0
//! Parity on the edge books: small invented books, written by hand to reach the boundaries and
//! branches the synthetic read does not (`tests/fixtures/edge-books/*.json`, each with a comment
//! naming what it reaches). The reference implementation built each book with its own model and
//! wrote its canonical dumps (`golden/edge.NAME.TEST.json`, by `parity/edge_golden.py`, see
//! PROVENANCE.md); this file builds the same book in Rust, runs the same test with its module check,
//! and requires the whole dump to compare equal. Because the canonical dump sorts figures by id,
//! `trial_balance`'s row order is compared separately against the reference's emission order
//! (`golden/edge.NAME.trial_balance.order.json`).
//!
//! These books are not Tally reads: they prove the port and the reference agree on the same book,
//! and nothing about reading Tally.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;
use bridge_tax_audit::book::{Book, Ledger, LedgerLine, TbRow, Voucher, VoucherStatus};
use bridge_tax_audit::canonical::canonical_test_result;
use bridge_tax_audit::compare::compare;
use bridge_tax_audit::read::Window;
use bridge_tax_audit::rules::Rules;
use bridge_tax_audit::{cash_book_integrity, ledger_scrutiny, stale_balances_41_1, trial_balance};
use serde_json::Value;

fn spec(name: &str) -> Value {
    let path = common::fixtures().join(format!("edge-books/{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn date(iso: &str) -> TallyDate {
    TallyDate::parse(iso.replace('-', "")).unwrap()
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

/// The book `parity/edge_golden.py` builds from the same spec.
fn build(s: &Value) -> Book {
    let groups = s["groups"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(n, p)| (n.clone(), p.as_str().map(str::to_string)))
        .collect();
    let ledgers = s["ledgers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let chain = strs(&l["chain"]);
            let name = l["name"].as_str().unwrap().to_string();
            let ledger = Ledger {
                name: name.clone(),
                parent: chain.first().cloned().unwrap_or_default(),
                chain,
                chain_complete: true,
                master_opening_paise: 0,
                guid: l["guid"].as_str().unwrap().to_string(),
                masterid: None,
            };
            (name, ledger)
        })
        .collect();
    let vouchers = s["vouchers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            let text = |k: &str, default: &str| v[k].as_str().unwrap_or(default).to_string();
            let guid = text("guid", "");
            let base_type = text("base_type", "");
            Voucher {
                date: date(v["date"].as_str().unwrap()),
                vtype: text("vtype", &base_type),
                number: text("number", &guid),
                status: match v["status"].as_str().unwrap_or("regular") {
                    "regular" => VoucherStatus::Regular,
                    "optional" => VoucherStatus::Optional,
                    "cancelled" => VoucherStatus::Cancelled,
                    "postdated" => VoucherStatus::Postdated,
                    other => panic!("unknown status {other}"),
                },
                lines: v["lines"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| LedgerLine {
                        ledger: l[0].as_str().unwrap().to_string(),
                        amount_paise: int(&l[1]),
                    })
                    .collect(),
                narration: text("narration", ""),
                guid,
                base_type,
            }
        })
        .collect();
    let tb = s["tb"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let row = TbRow {
                opening_paise: int(&t["opening"]),
                debit_paise: int(&t["debit"]),
                credit_paise: int(&t["credit"]),
                closing_paise: int(&t["closing"]),
            };
            (t["ledger"].as_str().unwrap().to_string(), row)
        })
        .collect();
    Book {
        company_name: "Invented edge book".to_string(),
        company_guid: "invented-edge-company".to_string(),
        read_at: String::new(),
        groups,
        group_masters: BTreeMap::new(),
        ledgers,
        vouchers,
        tb,
    }
}

fn rules(s: &Value) -> Rules {
    let mut rules = Rules::vendored().unwrap();
    for table in strs(&s["rules_without"]) {
        match table.as_str() {
            "ledger_scrutiny" => rules.ledger_scrutiny_large_entry_paise = None,
            other => panic!("rules_without {other} is not wired here"),
        }
    }
    rules
}

fn period(s: &Value) -> Window {
    let p = strs(&s["period"]);
    let (from, to) = match p.as_slice() {
        [] => ("2025-04-01", "2026-03-31"),
        [from, to] => (from.as_str(), to.as_str()),
        _ => panic!("period is [start, end]"),
    };
    Window {
        from: date(from),
        to: date(to),
    }
}

/// Build the book, run every test the spec names, and compare each whole dump with the reference's.
fn check(name: &str) {
    let s = spec(name);
    let (book, rules) = (build(&s), rules(&s));
    let cash: BTreeSet<String> = strs(&s["cash"]).into_iter().collect();
    let bank: BTreeSet<String> = strs(&s["bank"]).into_iter().collect();
    let tests = strs(&s["tests"]);
    assert!(!tests.is_empty(), "{name} names no test");
    for test in tests {
        let (result, module_check) = match test.as_str() {
            "trial_balance" => {
                let r = trial_balance::run(&book, &rules).unwrap();
                let order: Vec<&str> = r
                    .figures
                    .iter()
                    .map(|f| f.id.as_str())
                    .filter(|id| id.starts_with("trial_balance.tb_group_"))
                    .collect();
                let want: Vec<String> =
                    strs(&common::golden_named(&format!("edge.{name}.trial_balance.order")));
                assert_eq!(order, want, "{name}: trial_balance row order");
                let c = trial_balance::check_invariants(&book, &r).unwrap();
                (r, c)
            }
            "stale_balances_41_1" => {
                let r = stale_balances_41_1::run(&book, &rules).unwrap();
                let c = stale_balances_41_1::check_invariants(&book, &r).unwrap();
                (r, c)
            }
            "ledger_scrutiny" => {
                let r = ledger_scrutiny::run(&book, &rules, &period(&s), &cash).unwrap();
                let c = ledger_scrutiny::check_invariants(&book, &r).unwrap();
                (r, c)
            }
            "cash_book_integrity" => {
                let terms = strs(&s["own_account_terms"]);
                let r = cash_book_integrity::run(&book, &rules, &cash, &bank, &terms).unwrap();
                let c = cash_book_integrity::check_invariants(&book, &r).unwrap();
                (r, c)
            }
            other => panic!("{name}: unknown test {other}"),
        };
        let rust = canonical_test_result(&book, &result, Some(module_check)).unwrap();
        let golden = common::golden_named(&format!("edge.{name}.{test}"));
        let diffs = compare(&golden, &rust, None).unwrap();
        assert!(diffs.is_empty(), "{name} {test}:\n{}", diffs.join("\n"));
    }
}

#[test]
fn tb_rows() {
    check("tb_rows");
}

#[test]
fn stale() {
    check("stale");
}

#[test]
fn scrutiny() {
    check("scrutiny");
}

#[test]
fn scrutiny_default() {
    check("scrutiny_default");
}

#[test]
fn scrutiny_short() {
    check("scrutiny_short");
}

#[test]
fn cash_book() {
    check("cash_book");
}

/// Every committed edge book is checked above, and every edge golden has its book.
#[test]
fn every_edge_book_is_checked() {
    let checked = [
        "tb_rows",
        "stale",
        "scrutiny",
        "scrutiny_default",
        "scrutiny_short",
        "cash_book",
    ];
    let mut books: Vec<String> = std::fs::read_dir(common::fixtures().join("edge-books"))
        .unwrap()
        .map(|e| {
            let p = e.unwrap().path();
            p.file_stem().unwrap().to_str().unwrap().to_string()
        })
        .collect();
    books.sort();
    let mut want: Vec<String> = checked.iter().map(|s| (*s).to_string()).collect();
    want.sort();
    assert_eq!(books, want);
    for entry in std::fs::read_dir(common::fixtures().join("golden")).unwrap() {
        let file = entry.unwrap().file_name().into_string().unwrap();
        if let Some(rest) = file.strip_prefix("edge.") {
            let book = rest.split('.').next().unwrap();
            assert!(want.iter().any(|b| b == book), "{file} has no edge book");
        }
    }
}
