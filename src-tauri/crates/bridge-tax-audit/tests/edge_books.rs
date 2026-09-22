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
use bridge_tax_audit::{
    cash_book_integrity, ledger_scrutiny, stale_balances_41_1, tds_payees, trial_balance, TdsConfig,
};
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
            "s194j" => rules.s194j_aggregate_paise = None,
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

/// The `[tds]`/`[tds_payees]` values `parity/edge_golden.py` passes `tds_payees`: a
/// `s194j_category_by_ledger` value that is not a string is kept as `None`, as `TdsConfig` keeps it.
fn tds_config(s: &Value) -> TdsConfig {
    let map = |key: &str| -> BTreeMap<String, String> {
        s[key]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    TdsConfig {
        nature_by_ledger: map("nature_by_ledger"),
        payee_aliases: map("payee_aliases"),
        previous_year_turnover_paise: s["previous_year_turnover_paise"].as_i64(),
        s194j_category_by_ledger: s["s194j_category_by_ledger"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
                    .collect()
            })
            .unwrap_or_default(),
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
                let want: Vec<String> = strs(&common::golden_named(&format!(
                    "edge.{name}.trial_balance.order"
                )));
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
            "tds_payees" => {
                let entity_type = s["entity_type"].as_str().unwrap_or("individual");
                let r = tds_payees::run(&book, &rules, entity_type, &tds_config(&s)).unwrap();
                // The reference module has no check_invariants: an empty evaluated list.
                let rust = canonical_test_result(&book, &r, None).unwrap();
                let golden = common::golden_named(&format!("edge.{name}.{test}"));
                let diffs = compare(&golden, &rust, None).unwrap();
                assert!(diffs.is_empty(), "{name} {test}:\n{}", diffs.join("\n"));
                continue;
            }
            other => panic!("{name}: no edge dispatch for {other} (EDGE_TESTS: {EDGE_TESTS:?})"),
        };
        let rust = canonical_test_result(&book, &result, Some(module_check)).unwrap();
        let golden = common::golden_named(&format!("edge.{name}.{test}"));
        let diffs = compare(&golden, &rust, None).unwrap();
        assert!(diffs.is_empty(), "{name} {test}:\n{}", diffs.join("\n"));
    }
}

/// The tests an edge book may name: the arms of `check` above, and exactly the keys of
/// `parity/edge_golden.py`'s `runners` (`edge_runners_agree_across_the_two_sides`).
const EDGE_TESTS: [&str; 5] = [
    "cash_book_integrity",
    "ledger_scrutiny",
    "stale_balances_41_1",
    "tds_payees",
    "trial_balance",
];

/// Synthetic goldens other than `synthetic.<id>.json`, each read by a named test:
/// `financial_statements.noreport` by `tests/common/mod.rs` (`golden_financial_statements(false)`).
const SYNTHETIC_VARIANTS: [&str; 1] = ["financial_statements.noreport"];

fn book_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(common::fixtures().join("edge-books"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| p.file_stem().unwrap().to_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

/// Every edge book in the directory is built and compared -- no hand-kept list to fall behind.
/// Each book runs in its own panic boundary so one failure names its book and the rest still run.
#[test]
fn every_edge_book_matches_the_reference() {
    let names = book_names();
    assert!(!names.is_empty());
    let failed: Vec<String> = names
        .iter()
        .filter_map(|name| {
            std::panic::catch_unwind(|| check(name)).err().map(|e| {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                format!("{name}: {msg}")
            })
        })
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// Every edge golden belongs to a book that names its test, and every test a book names has its
/// golden; every synthetic golden is a registered test's. A stray or orphaned golden fails here.
#[test]
fn every_golden_belongs_to_a_book_or_a_registered_test() {
    let books: BTreeMap<String, Vec<String>> = book_names()
        .into_iter()
        .map(|n| {
            let tests = strs(&spec(&n)["tests"]);
            (n, tests)
        })
        .collect();
    let registered: Vec<&str> = bridge_tax_audit::registry::PORTED
        .iter()
        .map(|t| t.id)
        .collect();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for entry in std::fs::read_dir(common::fixtures().join("golden")).unwrap() {
        let file = entry.unwrap().file_name().into_string().unwrap();
        let stem = file
            .strip_suffix(".json")
            .unwrap_or_else(|| panic!("{file}: not json"));
        if let Some(rest) = stem.strip_prefix("edge.") {
            let (rest, order) = match rest.strip_suffix(".order") {
                Some(r) => (r, true),
                None => (rest, false),
            };
            let (book, test) = rest.split_once('.').unwrap_or_else(|| panic!("{file}"));
            let tests = books
                .get(book)
                .unwrap_or_else(|| panic!("{file}: no edge book {book}"));
            assert!(
                tests.iter().any(|t| t == test),
                "{file}: {book} does not name {test}"
            );
            assert!(
                !order || test == "trial_balance",
                "{file}: only trial_balance has an order file"
            );
            if !order {
                seen.insert((book.to_string(), test.to_string()));
            }
        } else if let Some(rest) = stem.strip_prefix("synthetic.") {
            // A registered test's golden, or one of the named variants a test reads.
            let (id, variant) = rest.split_once('.').unwrap_or((rest, ""));
            assert!(
                registered.contains(&id),
                "{file}: {id} is not a registered test"
            );
            assert!(
                variant.is_empty() || SYNTHETIC_VARIANTS.contains(&rest),
                "{file}: variant {variant:?} is not in SYNTHETIC_VARIANTS (add it with the test that reads it)"
            );
        } else {
            panic!("{file}: neither an edge nor a synthetic golden");
        }
    }
    for (book, tests) in &books {
        for test in tests {
            assert!(
                seen.contains(&(book.clone(), test.clone())),
                "{book} names {test} but golden/edge.{book}.{test}.json is missing"
            );
        }
    }
}

/// `parity/edge_golden.py`'s runners and this file's dispatch name the same tests, all
/// registered. Read as text: the Python module needs the reference engine to import.
#[test]
fn edge_runners_agree_across_the_two_sides() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("parity/edge_golden.py"),
    )
    .unwrap();
    let block = text
        .split("    runners = {\n")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("edge_golden.py has a `runners = { ... }` block");
    let mut python: Vec<&str> = block
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"').and_then(|r| r.split('"').next()))
        .collect();
    python.sort_unstable();
    assert_eq!(python, EDGE_TESTS.to_vec());
    for t in EDGE_TESTS {
        assert!(
            bridge_tax_audit::registry::find(t).is_some(),
            "{t} is not registered"
        );
    }
}
