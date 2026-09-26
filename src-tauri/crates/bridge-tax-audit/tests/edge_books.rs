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
use bridge_tax_audit::book::{
    Book, InventoryLine, Ledger, LedgerLine, TbRow, Voucher, VoucherStatus,
};
use bridge_tax_audit::canonical::canonical_test_result;
use bridge_tax_audit::compare::compare;
use bridge_tax_audit::documents::{bank_statement_from_json, traces_documents_from_json};
use bridge_tax_audit::read::Window;
use bridge_tax_audit::rules::Rules;
use bridge_tax_audit::{
    bank_reconciliation, book_keeping_quality, cash_book_integrity, creditor_ageing_43bh,
    high_value_register, ledger_scrutiny, loans_interest, partners_40b_194t, stale_balances_41_1,
    statutory_dues_43b, stock, stock_read, tds_payees, tds_tcs_26as, trial_balance,
    twentysixas_receipts, PartnersConfig, Tds26asConfig, TdsConfig,
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

/// `key` of `obj`: `None` when absent, or when null and `nullable`; otherwise `read` must accept
/// the value, or the test panics -- a mistyped key fails here as it does in `parity/edge_golden.py`,
/// rather than the two sides building different books.
fn typed<T>(
    obj: &Value,
    key: &str,
    nullable: bool,
    what: &str,
    read: impl Fn(&Value) -> Option<T>,
) -> Option<T> {
    match obj.get(key) {
        None => None,
        Some(Value::Null) if nullable => None,
        Some(v) => Some(read(v).unwrap_or_else(|| panic!("{key} must be {what}, got {v}"))),
    }
}

/// One `inventory` entry: `{item, qty?, rate?, amount?, direction?, qty_field_present?}`, the
/// numbers as the reference model holds them (`qty` a number, read as a float; `rate`/`amount`
/// integer paise, debit positive; `direction` 1 or -1), absent or null meaning `None`;
/// `qty_field_present` a boolean, true when absent. Any other type is refused.
fn inventory_line(i: &Value) -> InventoryLine {
    InventoryLine {
        item: typed(i, "item", false, "text", |v| v.as_str().map(str::to_string))
            .expect("item is required"),
        qty: typed(i, "qty", true, "a number or null", Value::as_f64),
        rate_paise: typed(i, "rate", true, "an integer or null", Value::as_i64),
        amount_paise: typed(i, "amount", true, "an integer or null", Value::as_i64),
        direction: typed(i, "direction", true, "1, -1 or null", |v| {
            match v.as_i64() {
                Some(1) => Some(1),
                Some(-1) => Some(-1),
                _ => None,
            }
        }),
        qty_field_present: typed(
            i,
            "qty_field_present",
            false,
            "true or false",
            Value::as_bool,
        )
        .unwrap_or(true),
    }
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
                party_field: text("party", ""),
                reference: typed(v, "reference", false, "text", |r| {
                    r.as_str().map(str::to_string)
                })
                .unwrap_or_default(),
                masterid: typed(v, "masterid", false, "text", |m| {
                    m.as_str().map(str::to_string)
                }),
                inventory: v["inventory"]
                    .as_array()
                    .map(|a| a.iter().map(inventory_line).collect())
                    .unwrap_or_default(),
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
        ..Default::default()
    }
}

fn rules(s: &Value) -> Rules {
    let mut rules = Rules::vendored().unwrap();
    for table in strs(&s["rules_without"]) {
        match table.as_str() {
            "ledger_scrutiny" => rules.ledger_scrutiny_large_entry_paise = None,
            "s43b" => rules.s43b = None,
            "s36_1_va" => rules.s36_1_va_due_day = None,
            "s194j" => rules.s194j_aggregate_paise = None,
            "s194t" => rules.s194t = None,
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

/// `creditor_ageing_43bh`'s parameters from a spec's `creditor_ageing` table, each defaulting as
/// the reference's `run()` defaults it.
fn creditor_ageing_params(c: &Value) -> creditor_ageing_43bh::Params {
    creditor_ageing_43bh::Params {
        acceptance_lag_days: c["acceptance_lag_days"].as_i64().unwrap_or(0),
        supplier_classification: c["supplier_classification"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        post_year_payments: c["post_year_payments"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, rows)| {
                        let rows = rows
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|r| (date(r[0].as_str().unwrap()), int(&r[1])))
                            .collect();
                        (k.clone(), rows)
                    })
                    .collect()
            })
            .unwrap_or_default(),
        mse_interest_ledgers: strs(&c["mse_interest_ledgers"]).into_iter().collect(),
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

/// A spec's JSON value as the TOML value a client config would carry (integers, text, booleans,
/// lists and tables; anything else is refused, so the two sides cannot read different configs).
fn toml_of(v: &Value) -> toml::Value {
    match v {
        Value::String(t) => toml::Value::String(t.clone()),
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => toml::Value::Integer(n.as_i64().expect("an integer")),
        Value::Array(a) => toml::Value::Array(a.iter().map(toml_of).collect()),
        Value::Object(m) => {
            toml::Value::Table(m.iter().map(|(k, x)| (k.clone(), toml_of(x))).collect())
        }
        Value::Null => panic!("null is not a TOML value"),
    }
}

/// The `partners` and `deed` `parity/edge_golden.py` passes `partners_40b_194t`, as a bound
/// `[partners]` table.
fn partners(s: &Value) -> PartnersConfig {
    PartnersConfig {
        partners: s["partners"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), toml_of(v))).collect())
            .unwrap_or_default(),
        deed: (!s["deed"].is_null()).then(|| toml_of(&s["deed"])),
    }
}

/// The `loans` table `parity/edge_golden.py` passes `loans_interest`, typed as the crate types a
/// bound `[loans.loan_ledgers]`.
fn loans(s: &Value) -> BTreeMap<String, loans_interest::LoanConfig> {
    s["loans"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(k, v)| {
                    let text = |key: &str| v[key].as_str().unwrap().to_string();
                    let cfg = loans_interest::LoanConfig {
                        lender: text("lender"),
                        lender_type: text("lender_type"),
                        interest_ledger: v["interest_ledger"].as_str().map(str::to_string),
                    };
                    (k.clone(), cfg)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `[tds_tcs_26as]` values `parity/edge_golden.py` passes both 26AS tests.
fn tds_26as_config(s: &Value) -> Tds26asConfig {
    let set = |key: &str| strs(&s[key]).into_iter().collect();
    Tds26asConfig {
        tds_ledgers: set("tds_ledgers"),
        tcs_ledgers: set("tcs_ledgers"),
        advance_tax_ledgers: set("advance_tax_ledgers"),
        deductor_aliases: s["deductor_aliases"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// `book_keeping_quality`'s inputs from the spec's `book_keeping_quality` table, as
/// `parity/edge_golden.py` passes them: every key optional and empty when absent, `tax_ledgers`
/// flattened to ledger -> head.
fn bkq_inputs(s: &Value) -> book_keeping_quality::Inputs {
    let b = &s["book_keeping_quality"];
    let set = |k: &str| strs(&b[k]).into_iter().collect();
    let mut tax_ledgers_by_head = BTreeMap::new();
    if let Some(heads) = b["tax_ledgers"].as_object() {
        for (head, ledgers) in heads {
            for ledger in strs(ledgers) {
                assert!(
                    tax_ledgers_by_head.insert(ledger, head.clone()).is_none(),
                    "an edge book lists a ledger under one GST head only"
                );
            }
        }
    }
    book_keeping_quality::Inputs {
        payment_channel_debtors: set("payment_channel_debtors"),
        tax_ledgers_by_head,
        gst_payment_ledgers: set("gst_payment_ledgers"),
        reissue_narration_terms: strs(&b["reissue_narration_terms"]),
        writeoff_discount_ledgers: set("writeoff_discount_ledgers"),
    }
}

/// `stock`'s inputs from the spec, typed as `parity/edge_golden.py` types them: `stock_items`
/// ({name: {base_unit?, guid?, opening_qty?, opening_value?, closing_qty?, closing_value?}}),
/// `stock_opening`/`stock_closing` ({as_of, rows: {name: {qty?, value?, rate?}}}) and
/// `is_integrated` (a boolean, absent or null for unknown).
fn stock_inputs(s: &Value) -> stock_read::StockInputs {
    let qty = |d: &Value, key: &str| typed(d, key, true, "a number or null", Value::as_f64);
    let paise = |d: &Value, key: &str| typed(d, key, true, "an integer or null", Value::as_i64);
    let text = |d: &Value, key: &str| {
        typed(d, key, false, "text", |v| v.as_str().map(str::to_string)).unwrap_or_default()
    };
    let items = s["stock_items"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(n, m)| {
                    let master = stock_read::StockItemMaster {
                        name: n.clone(),
                        guid: text(m, "guid"),
                        parent: String::new(),
                        base_unit: text(m, "base_unit"),
                        opening_qty: qty(m, "opening_qty"),
                        opening_value_paise: paise(m, "opening_value"),
                        closing_qty: qty(m, "closing_qty"),
                        closing_value_paise: paise(m, "closing_value"),
                    };
                    (n.clone(), master)
                })
                .collect()
        })
        .unwrap_or_default();
    let snapshot = |key: &str| stock_read::StockSnapshot {
        as_of: date(s[key]["as_of"].as_str().unwrap()),
        rows: s[key]["rows"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(n, r)| {
                let row = stock_read::StockSnapshotRow {
                    name: n.clone(),
                    guid: String::new(),
                    qty: qty(r, "qty"),
                    value_paise: paise(r, "value"),
                    rate_paise: paise(r, "rate"),
                };
                (n.clone(), row)
            })
            .collect(),
    };
    stock_read::StockInputs {
        items,
        opening: snapshot("stock_opening"),
        closing: snapshot("stock_closing"),
        is_integrated: typed(
            s,
            "is_integrated",
            true,
            "true, false or null",
            Value::as_bool,
        ),
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
            "creditor_ageing_43bh" => {
                let creditors: BTreeSet<String> = strs(&s["creditors"]).into_iter().collect();
                let r = creditor_ageing_43bh::run(
                    &book,
                    &rules,
                    &period(&s),
                    &creditors,
                    &creditor_ageing_params(&s["creditor_ageing"]),
                )
                .unwrap();
                let c = creditor_ageing_43bh::check_invariants(&book, &r).unwrap();
                (r, c)
            }
            "statutory_dues_43b" => {
                let sd = &s["statutory_dues"];
                let nature_by_ledger: BTreeMap<String, String> = sd["nature_by_ledger"]
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let salary: BTreeSet<String> =
                    strs(&sd["salary_expense_ledgers"]).into_iter().collect();
                let r =
                    statutory_dues_43b::run(&book, &rules, &period(&s), &nature_by_ledger, &salary)
                        .unwrap();
                let c = statutory_dues_43b::check_invariants(&book, &r).unwrap();
                (r, c)
            }
            "book_keeping_quality" => {
                let r = book_keeping_quality::run(&book, &rules, &cash, &bkq_inputs(&s)).unwrap();
                let c = book_keeping_quality::check_invariants(&book, &r).unwrap();
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
            "loans_interest" => {
                let entity_type = s["entity_type"].as_str().unwrap_or("individual");
                let shared: BTreeSet<String> =
                    strs(&s["shared_interest_ledgers"]).into_iter().collect();
                let loans = loans(&s);
                let turnover = s["previous_year_turnover_paise"].as_i64();
                // Without a net_reversals key the book runs the rule in force, through run() and
                // check_invariants(), so the default switch is what that golden pins.
                match typed(&s, "net_reversals", false, "true or false", Value::as_bool) {
                    None => {
                        let r = loans_interest::run(
                            &book,
                            &rules,
                            entity_type,
                            &loans,
                            turnover,
                            &cash,
                            &bank,
                            &shared,
                        )
                        .unwrap();
                        let c = loans_interest::check_invariants(&book, &r).unwrap();
                        (r, c)
                    }
                    Some(net_reversals) => {
                        let r = loans_interest::run_with(
                            &book,
                            &rules,
                            entity_type,
                            &loans,
                            turnover,
                            &cash,
                            &bank,
                            &shared,
                            net_reversals,
                        )
                        .unwrap();
                        let c = loans_interest::check_invariants_with(&book, &r, net_reversals)
                            .unwrap();
                        (r, c)
                    }
                }
            }
            "partners_40b_194t" => {
                let entity_type = s["entity_type"].as_str().unwrap_or("individual");
                let r =
                    partners_40b_194t::run(&book, &rules, &period(&s), entity_type, &partners(&s))
                        .unwrap();
                // The reference module has no check_invariants: an empty evaluated list.
                let rust = canonical_test_result(&book, &r, None).unwrap();
                let golden = common::golden_named(&format!("edge.{name}.{test}"));
                let diffs = compare(&golden, &rust, None).unwrap();
                assert!(diffs.is_empty(), "{name} {test}:\n{}", diffs.join("\n"));
                continue;
            }
            "bank_reconciliation" => {
                // The statement is caller data; its rows feed BANK-1, as the reference's pack sets
                // `eng.bank` to them.
                let statement = bank_statement_from_json(&s["bank_statement"]).unwrap();
                let terms: BTreeSet<String> = strs(&s["bank_charge_terms"]).into_iter().collect();
                let ledger = s["bank_reconciliation_ledger"].as_str().unwrap();
                let r = bank_reconciliation::run(
                    &book,
                    &rules,
                    &period(&s),
                    &statement,
                    ledger,
                    &terms,
                    bank_reconciliation::MATCH_MAX_DAYS,
                )
                .unwrap();
                let c = bank_reconciliation::check_invariants(&statement.rows, &r).unwrap();
                (r, c)
            }
            "high_value_register" => {
                // As `parity/edge_golden.py` runs it: the statement and the AIS rows optional, the
                // counterparty types already merged, the recipient type from `entity_type` unless
                // the spec names one ("unknown" meaning none).
                let statement = match &s["bank_statement"] {
                    Value::Null => None,
                    v => Some(bank_statement_from_json(v).unwrap()),
                };
                let docs = traces_documents_from_json(&s).unwrap();
                let set = |k: &str| -> BTreeSet<String> { strs(&s[k]).into_iter().collect() };
                let (terms, round_off) = (set("s194n_terms"), set("round_off_ledgers"));
                let types: BTreeMap<String, String> = s["counterparty_types"]
                    .as_object()
                    .map(|o| {
                        o.iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let recipient = match s["s194n_recipient_type"].as_str() {
                    None => high_value_register::s194n_recipient_type(Some(
                        s["entity_type"].as_str().unwrap_or("individual"),
                    )),
                    Some("unknown") => None,
                    Some(high_value_register::RECIPIENT_CO_OPERATIVE) => {
                        Some(high_value_register::Recipient::CoOperative)
                    }
                    Some(high_value_register::RECIPIENT_NOT_CO_OPERATIVE) => {
                        Some(high_value_register::Recipient::NotCoOperative)
                    }
                    Some(t) => panic!("{name}: s194n_recipient_type {t:?} is not a recipient type"),
                };
                let inputs = high_value_register::Inputs {
                    cash: &cash,
                    bank: &bank,
                    threshold_paise: None,
                    bank_statement: statement.as_ref(),
                    s194n_narration_terms: &terms,
                    ais_rows: &docs.ais,
                    s194n_recipient_type: recipient,
                    round_off_ledgers: &round_off,
                    counterparty_type_by_ledger: &types,
                };
                let r = high_value_register::run(&book, &rules, &inputs).unwrap();
                // The reference module has no check_invariants: an empty evaluated list.
                let rust = canonical_test_result(&book, &r, None).unwrap();
                let golden = common::golden_named(&format!("edge.{name}.{test}"));
                let diffs = compare(&golden, &rust, None).unwrap();
                assert!(diffs.is_empty(), "{name} {test}:\n{}", diffs.join("\n"));
                continue;
            }
            "stock" => {
                let inputs = stock_inputs(&s);
                let r = stock::run(&book, &rules, &inputs).unwrap();
                let c = stock::check_invariants(&book, &r, &inputs).unwrap();
                (r, c)
            }
            "twentysixas_receipts" => {
                let docs = traces_documents_from_json(&s).unwrap();
                let aliases = tds_26as_config(&s).deductor_aliases;
                let r = twentysixas_receipts::run(&book, &rules, &docs.form26as, &aliases).unwrap();
                let c = twentysixas_receipts::check_invariants(&book, &docs.form26as, &r).unwrap();
                (r, c)
            }
            "tds_tcs_26as" => {
                let docs = traces_documents_from_json(&s).unwrap();
                let r = tds_tcs_26as::run(
                    &book,
                    &rules,
                    &period(&s),
                    &docs.form26as,
                    &docs.ais,
                    &docs.tis,
                    &tds_26as_config(&s),
                )
                .unwrap();
                let c = tds_tcs_26as::check_invariants(&book, &docs.form26as, &r).unwrap();
                (r, c)
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
const EDGE_TESTS: [&str; 15] = [
    "bank_reconciliation",
    "book_keeping_quality",
    "cash_book_integrity",
    "creditor_ageing_43bh",
    "high_value_register",
    "ledger_scrutiny",
    "loans_interest",
    "partners_40b_194t",
    "stale_balances_41_1",
    "statutory_dues_43b",
    "stock",
    "tds_payees",
    "tds_tcs_26as",
    "trial_balance",
    "twentysixas_receipts",
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

/// The edge-book builder refuses a mistyped `masterid` or inventory key rather than reading it
/// differently from `parity/edge_golden.py`, which refuses the same specs.
#[test]
fn mistyped_voucher_keys_are_refused() {
    let cases = [
        serde_json::json!({"item": "x", "qty_field_present": null}),
        serde_json::json!({"item": "x", "qty": "5"}),
        serde_json::json!({"item": "x", "rate": 1.5}),
        serde_json::json!({"item": "x", "direction": 2}),
        serde_json::json!({"qty": 1}),
    ];
    for case in cases {
        let refused = std::panic::catch_unwind(|| inventory_line(&case)).is_err();
        assert!(refused, "{case} was not refused");
    }
    let accepted = inventory_line(&serde_json::json!({"item": "x", "qty": null, "direction": -1}));
    assert_eq!(
        (accepted.qty, accepted.direction, accepted.qty_field_present),
        (None, Some(-1), true)
    );
    for masterid in [serde_json::json!(42), Value::Null] {
        let refused = std::panic::catch_unwind(|| {
            typed(
                &serde_json::json!({ "masterid": masterid }),
                "masterid",
                false,
                "text",
                |m| m.as_str().map(str::to_string),
            )
        })
        .is_err();
        assert!(refused, "masterid {masterid} was not refused");
    }
}

/// A voucher's `reference` reaches the built book as text, absent meaning empty; any other type
/// refuses the whole spec, as `parity/edge_golden.py` refuses it.
#[test]
fn a_voucher_reference_is_built_as_text_or_refused() {
    let with = |reference: Option<Value>| {
        let mut s = spec("bkq_quiet");
        if let Some(r) = reference {
            s["vouchers"][0]["reference"] = r;
        }
        s
    };
    assert_eq!(build(&with(None)).vouchers[0].reference, "");
    let s = with(Some(serde_json::json!("INV/7")));
    assert_eq!(build(&s).vouchers[0].reference, "INV/7");
    for bad in [serde_json::json!(42), Value::Null] {
        let s = with(Some(bad.clone()));
        let refused = std::panic::catch_unwind(|| build(&s)).is_err();
        assert!(refused, "reference {bad} was not refused");
    }
}

/// The 26AS module invariants catch what a correct run never produces, so each is driven here with a
/// tampered result: TR-2 (a supply figure's evidence naming a Part VI row) and TT-4 (a TDS match
/// pair's evidence naming a Part VI row). The untampered `tds_tcs_26as` run is clean, its control;
/// `each_26as_invariant_fires_on_its_own_tampering` shows the untampered receipts run raises no
/// TR-2.
#[test]
fn the_26as_invariants_catch_a_row_of_the_wrong_part() {
    let s = spec("tds26as_receipts");
    let (book, rules) = (build(&s), crate::rules(&s));
    let docs = traces_documents_from_json(&s).unwrap();
    let aliases = tds_26as_config(&s).deductor_aliases;
    let mut r = twentysixas_receipts::run(&book, &rules, &docs.form26as, &aliases).unwrap();
    let part_vi = docs.form26as.iter().find(|a| a.part == "VI").unwrap();
    let vi_id = format!("{}#{}", part_vi.doc, part_vi.row);
    let supply = r
        .figures
        .iter_mut()
        .find(|f| f.id.starts_with("twentysixas_receipts.supply_26as_amount_"))
        .unwrap();
    let doc = supply
        .evidence
        .iter_mut()
        .find(|e| e.kind == "document_row")
        .unwrap();
    doc.id = vi_id.clone();
    let v = twentysixas_receipts::check_invariants(&book, &docs.form26as, &r).unwrap();
    assert!(
        v.iter()
            .any(|m| m.starts_with("TR-2:") && m.contains("Part VI row")),
        "{v:?}"
    );

    let s = spec("tds26as_matching");
    let (book, rules) = (build(&s), crate::rules(&s));
    let docs = traces_documents_from_json(&s).unwrap();
    let cfg = tds_26as_config(&s);
    let mut r = tds_tcs_26as::run(
        &book,
        &rules,
        &period(&s),
        &docs.form26as,
        &docs.ais,
        &docs.tis,
        &cfg,
    )
    .unwrap();
    assert!(tds_tcs_26as::check_invariants(&book, &docs.form26as, &r)
        .unwrap()
        .is_empty());
    let part_vi = docs.form26as.iter().find(|a| a.part == "VI").unwrap();
    let pair = r
        .figures
        .iter_mut()
        .find(|f| f.id.starts_with("tds_tcs_26as.match_pair_tds_"))
        .unwrap();
    let doc = pair
        .evidence
        .iter_mut()
        .find(|e| e.kind == "document_row")
        .unwrap();
    doc.id = format!("{}#{}", part_vi.doc, part_vi.row);
    let v = tds_tcs_26as::check_invariants(&book, &docs.form26as, &r).unwrap();
    assert!(
        v.iter()
            .any(|m| m.starts_with("TT-4:") && m.contains("(kind tds) matches a 26AS part-VI row")),
        "{v:?}"
    );
}

/// Each remaining 26AS module invariant fires on a result tampered the one way it guards against,
/// and only there: the untampered runs are the control (clean for `tds_tcs_26as`; for
/// `twentysixas_receipts`, nothing beyond the TR-4 missing-ledger report that book is built to
/// raise), and each tampering must add a violation matching its own check.
#[test]
fn each_26as_invariant_fires_on_its_own_tampering() {
    use bridge_tax_audit::findings::{TestResult, Value as V};
    fn int(f: &bridge_tax_audit::findings::Figure) -> i64 {
        match f.value {
            V::Int(n) => n,
            _ => panic!("{} is not an integer", f.id),
        }
    }
    fn by_prefix<'a>(
        r: &'a mut TestResult,
        prefix: &str,
    ) -> impl Iterator<Item = &'a mut bridge_tax_audit::findings::Figure> {
        let prefix = prefix.to_string();
        r.figures
            .iter_mut()
            .filter(move |f| f.id.starts_with(&prefix))
    }

    let s = spec("tds26as_receipts");
    let (book, rules) = (build(&s), crate::rules(&s));
    let docs = traces_documents_from_json(&s).unwrap();
    let aliases = tds_26as_config(&s).deductor_aliases;
    let clean = twentysixas_receipts::run(&book, &rules, &docs.form26as, &aliases).unwrap();
    let check =
        |r: &TestResult| twentysixas_receipts::check_invariants(&book, &docs.form26as, r).unwrap();
    let base = check(&clean);
    assert!(
        !base.is_empty()
            && base
                .iter()
                .all(|m| m.starts_with("TR-4:") && m.contains("no resolvable ledger")),
        "{base:?}"
    );
    let fires = |tamper: &dyn Fn(&mut TestResult), needle: &str| {
        let mut r = clean.clone();
        tamper(&mut r);
        let added: Vec<String> = check(&r)
            .into_iter()
            .filter(|m| !base.contains(m))
            .collect();
        assert!(
            added.iter().any(|m| m.contains(needle)),
            "{needle}: {added:?}"
        );
    };
    let p = "twentysixas_receipts.";
    fires(
        &|r| {
            let f = by_prefix(r, &format!("{p}supply_26as_amount_"))
                .next()
                .unwrap();
            f.value = V::Int(int(f) + 1);
        },
        "but the sum of its own referenced 26AS rows is",
    );
    fires(
        &|r| {
            let f = by_prefix(r, &format!("{p}supply_26as_amount_"))
                .next()
                .unwrap();
            let e = f
                .evidence
                .iter_mut()
                .find(|e| e.kind == "document_row")
                .unwrap();
            e.id = "form26as:nowhere#0".to_string();
        },
        "evidence form26as:nowhere#0 does not resolve to a Form 26AS row",
    );
    fires(
        &|r| {
            let row = by_prefix(r, &format!("{p}supply_26as_amount_"))
                .next()
                .unwrap()
                .evidence
                .iter()
                .find(|e| e.kind == "document_row")
                .unwrap()
                .clone();
            by_prefix(r, &format!("{p}interest_26as_amount_"))
                .next()
                .unwrap()
                .evidence
                .push(row);
        },
        "TR-3: 26AS row",
    );
    fires(
        &|r| {
            let resolvable = by_prefix(r, &format!("{p}supply_books_amount_"))
                .find(|f| {
                    !base
                        .iter()
                        .any(|m| m.contains(&format!("{} carries no", f.id)))
                })
                .unwrap();
            resolvable.value = V::Int(int(resolvable) + 1);
        },
        "but a fresh population walk for",
    );

    let s = spec("tds26as_matching");
    let (book, rules) = (build(&s), crate::rules(&s));
    let docs = traces_documents_from_json(&s).unwrap();
    let clean = tds_tcs_26as::run(
        &book,
        &rules,
        &period(&s),
        &docs.form26as,
        &docs.ais,
        &docs.tis,
        &tds_26as_config(&s),
    )
    .unwrap();
    let check = |r: &TestResult| tds_tcs_26as::check_invariants(&book, &docs.form26as, r).unwrap();
    assert!(check(&clean).is_empty());
    for (name, needle) in [
        ("twentysixas_agg_count", "!= twentysixas_agg_count"),
        ("books_claim_count", "!= books_claim_count"),
        (
            "twentysixas_only_unclassified_count",
            "TT-2: twentysixas_only_unclassified_count = 1 (must be zero)",
        ),
        (
            "books_only_unclassified_count",
            "TT-2: books_only_unclassified_count = 1 (must be zero)",
        ),
        (
            "books_tds_ledger_movement_paise",
            "TT-3: books_tds_ledger_movement_paise",
        ),
    ] {
        let mut r = clean.clone();
        let f = r
            .figures
            .iter_mut()
            .find(|f| f.id == format!("tds_tcs_26as.{name}"))
            .unwrap();
        f.value = V::Int(int(f) + 1);
        let v = check(&r);
        assert!(v.iter().any(|m| m.contains(needle)), "{name}: {v:?}");
    }
}

/// A TIS category given twice would repeat a figure id: the reference raises, and so does this.
#[test]
fn a_repeated_tis_category_is_refused() {
    let s = spec("tds26as_matching");
    let (book, rules) = (build(&s), crate::rules(&s));
    let mut docs = traces_documents_from_json(&s).unwrap();
    let mut again = docs.tis[0].clone();
    again.row = 99;
    docs.tis.push(again);
    let Err(err) = tds_tcs_26as::run(
        &book,
        &rules,
        &period(&s),
        &docs.form26as,
        &docs.ais,
        &docs.tis,
        &tds_26as_config(&s),
    ) else {
        panic!("a repeated TIS category is refused");
    };
    assert!(format!("{err}").contains("would repeat"), "{err}");
}

/// Two matched books rows sharing a GUID would repeat `match_pair_<hash>`: the reference's `fig`
/// raises ("duplicate figure id bank_reconciliation.match_pair_093394bf", checked on this same
/// change to `bankrec_paths`), and the port refuses with an error rather than panicking.
#[test]
fn a_repeated_books_guid_is_refused_not_panicked() {
    let mut s = spec("bankrec_paths");
    for v in s["vouchers"].as_array_mut().unwrap() {
        if v["guid"] == "p07" {
            v["guid"] = Value::from("p01");
        }
    }
    let (book, rules) = (build(&s), rules(&s));
    let statement = bank_statement_from_json(&s["bank_statement"]).unwrap();
    let terms: BTreeSet<String> = strs(&s["bank_charge_terms"]).into_iter().collect();
    let result = std::panic::catch_unwind(|| {
        bank_reconciliation::run(
            &book,
            &rules,
            &period(&s),
            &statement,
            "Edge Bank",
            &terms,
            bank_reconciliation::MATCH_MAX_DAYS,
        )
    })
    .expect("refused, not panicked");
    let err = result.expect_err("a repeated figure id is refused");
    assert!(format!("{err}").contains("match_pair_093394bf"), "{err}");
}

/// A two-line journal whose lines are both on one party ledger gives two figures one id; the
/// reference raises `duplicate figure id ...journal_transfer_amount_0bce8b28_0431f39b` on this
/// book, and the port refuses with an error, never a panic.
#[test]
fn a_journal_on_one_ledger_is_refused_not_panicked() {
    let mut s = spec("hvr_bare");
    for v in s["vouchers"].as_array_mut().unwrap() {
        if v["guid"] == "b02" {
            v["lines"] =
                serde_json::json!([["Customer A", 20_000_000], ["Customer A", -20_000_000]]);
        }
    }
    let (book, rules) = (build(&s), rules(&s));
    let cash: BTreeSet<String> = strs(&s["cash"]).into_iter().collect();
    let (none, no_types) = (BTreeSet::new(), BTreeMap::new());
    let inputs = high_value_register::Inputs {
        cash: &cash,
        bank: &none,
        threshold_paise: None,
        bank_statement: None,
        s194n_narration_terms: &none,
        ais_rows: &[],
        s194n_recipient_type: None,
        round_off_ledgers: &none,
        counterparty_type_by_ledger: &no_types,
    };
    let result = std::panic::catch_unwind(|| high_value_register::run(&book, &rules, &inputs))
        .expect("refused, not panicked");
    let err = result.expect_err("a repeated figure id is refused");
    assert!(
        format!("{err}").contains("journal_transfer_amount_0bce8b28_0431f39b"),
        "{err}"
    );
}

/// A goods inventory line whose quantity field is absent means the read did not carry quantities:
/// the reference raises, naming the count and the first line, and the port refuses the same book
/// with an error rather than skipping the line as a value-only one.
/// Three rules no edge book reaches: the Stock-in-Hand voucher count reads the population only,
/// an opening Stock Summary row with no quantity starts the walk at nil (not the master's
/// opening), and negative at close counts goods items only.
#[test]
fn stock_counts_the_population_seeds_nil_and_goods_only_at_close() {
    let mut s = spec("stock_quiet");
    s["ledgers"].as_array_mut().unwrap().push(serde_json::json!(
        {"name": "Shop Stock", "chain": ["Stock-in-Hand", "Current Assets"], "guid": "edge-stock_quiet-t01"}
    ));
    s["vouchers"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!(
            {"guid": "q02", "date": "2025-05-01", "base_type": "Journal", "status": "cancelled",
             "lines": [["Shop Stock", 500], ["Cash", -500]]}
        ));
    s["vouchers"][0]["inventory"] =
        serde_json::json!([{"item": "Widget", "qty": 3, "amount": -300}]);
    s["stock_items"] = serde_json::json!({
        "Widget": {"base_unit": "Nos", "opening_qty": 5, "opening_value": 500},
        "Freight Placeholder": {"base_unit": "\u{fffd}#4; Not Applicable"}
    });
    s["stock_opening"]["rows"] = serde_json::json!({"Widget": {"value": 500}});
    s["stock_closing"]["rows"] = serde_json::json!({
        "Widget": {"qty": 2, "value": 200},
        "Freight Placeholder": {"qty": -1, "value": -100}
    });
    let (book, rules) = (build(&s), rules(&s));
    let r = stock::run(&book, &rules, &stock_inputs(&s)).unwrap();
    let figure = |id: &str| {
        let f = r.figures.iter().find(|f| f.id == format!("stock.{id}"));
        f.unwrap_or_else(|| panic!("no figure {id}")).value.clone()
    };
    let int = bridge_tax_audit::findings::Value::Int;
    assert_eq!(
        figure("stock_in_hand_voucher_count"),
        int(0),
        "a cancelled voucher"
    );
    assert_eq!(figure("opening_seed_from_summary_count"), int(1));
    // Seeded at nil, Widget goes to -3; from the master's 5 it would stay at 2.
    assert_eq!(figure("negative_any_point_range_min"), int(1));
    assert_eq!(
        figure("negative_at_close_count"),
        int(0),
        "a value-only item"
    );
    assert_eq!(figure("non_goods_negative_value_item_count"), int(1));
}

#[test]
fn a_goods_line_without_a_quantity_field_is_refused() {
    let mut s = spec("stock_quiet");
    s["vouchers"][0]["inventory"] =
        serde_json::json!([{"item": "Widget", "amount": 100, "qty_field_present": false}]);
    let (book, rules) = (build(&s), rules(&s));
    let err = stock::run(&book, &rules, &stock_inputs(&s)).expect_err("refused");
    assert!(
        format!("{err}")
            .contains("1 goods inventory line(s) in the population carry no quantity field (BILLEDQTY/ACTUALQTY) at all, first 'Widget' on Receipt q01 on 2025-04-05;"),
        "{err}"
    );
    // Outside the population (a cancelled voucher) the same line is not read.
    s["vouchers"][0]["status"] = Value::from("cancelled");
    let book = build(&s);
    stock::run(&book, &rules, &stock_inputs(&s)).expect("a cancelled voucher's line is not read");
}
