// SPDX-License-Identifier: Apache-2.0
//! CI parity for `financial_statements`: the Rust slice over the committed synthetic read must
//! produce the canonical dumps the Python reference engine produced over the same bytes -- with the
//! committed synthetic report totals (`golden/synthetic.financial_statements.json`) and with none
//! (`golden/synthetic.financial_statements.noreport.json`), see PROVENANCE.md -- and the comparison
//! must be able to fail. Mirrors `tests/parity_depreciation.rs`'s discipline: each seeded test
//! alters one thing and asserts the specific difference is reported, not merely that something was.
//!
//! The fixture's financial_statements cases (masterid 50-53 and the two Stock-in-Hand ledgers,
//! `parity/generate_fixture.py`): a Direct Expenses ledger one subgroup below its primary, a Direct
//! Incomes and an Indirect Incomes ledger (the credit-balance sign flip), a partners'-interest
//! ledger configured in `[partners.*]`, a Stock-in-Hand ledger whose TB closing field is stale
//! (closing stock must come from opening + debit - credit) beside one that is not, and report
//! totals exactly Rs 1 from the derived figures -- FS-1's inclusive tolerance, so an off-by-one in
//! the tie comparison changes this fixture's result.

mod common;

use bridge_tax_audit::compare::compare;
use serde_json::{json, Value};

fn rust_dump(with_report: bool) -> Value {
    let report = common::synthetic_report_totals();
    common::run_financial_statements(
        &common::fixtures().join("synthetic-read"),
        false,
        with_report.then_some(&report),
    )
    .unwrap()
}

fn figure_mut<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["figures"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["id"] == id)
        .unwrap()
}

fn value(doc: &Value, name: &str) -> Value {
    doc["figures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == format!("financial_statements.{name}"))
        .unwrap()["value"]
        .clone()
}

fn diffs(rust: &Value, with_report: bool) -> Vec<String> {
    compare(
        &common::golden_financial_statements(with_report),
        rust,
        None,
    )
    .unwrap()
}

#[test]
fn synthetic_read_matches_the_python_golden_with_report_totals() {
    let rust = rust_dump(true);
    let d = diffs(&rust, true);
    assert!(d.is_empty(), "parity failed:\n{}", d.join("\n"));
    // Anchor the pass to content, so an empty or trivial dump cannot be what passed.
    assert_eq!(rust["figures"].as_array().unwrap().len(), 28);
    assert_eq!(rust["findings"][0]["confidence"], "computed");
    assert_eq!(
        value(&rust, "report_tie_status"),
        json!("performed: net profit and closing stock")
    );
    assert_eq!(
        rust["module_invariants_evaluated"],
        json!(["financial_statements.check_invariants"])
    );
    assert_eq!(rust["module_invariant_violations"], json!([]));
    assert_eq!(value(&rust, "direct_expenses"), json!(450_000)); // via a subgroup
    assert_eq!(value(&rust, "direct_incomes"), json!(1_200_000)); // sign-flipped
    assert_eq!(value(&rust, "other_income"), json!(185_025));
    assert_eq!(value(&rust, "partner_interest"), json!(750_000));
    // Stale closing field: closing stock is (60,000 + 75,000 - 5,000) + 10,000, not the field's
    // 70,000.
    assert_eq!(value(&rust, "closing_stock"), json!(14_000_000));
    assert_eq!(value(&rust, "closing_stock_tb_field"), json!(7_000_000));
    assert_eq!(
        value(
            &rust,
            "stock_in_hand_ledgers_with_stale_tb_closing_field_count"
        ),
        json!(1)
    );
    // Exactly at the inclusive Rs 1 tolerance on both sides of the tie.
    assert_eq!(value(&rust, "report_net_profit_diff"), json!(-100));
    assert_eq!(value(&rust, "report_closing_stock_diff"), json!(-100));
}

#[test]
fn synthetic_read_matches_the_python_golden_without_report_totals() {
    let rust = rust_dump(false);
    let d = diffs(&rust, false);
    assert!(d.is_empty(), "parity failed:\n{}", d.join("\n"));
    assert_eq!(rust["figures"].as_array().unwrap().len(), 24);
    assert!(rust["findings"][0]["title_text"]
        .as_str()
        .unwrap()
        .starts_with("Report tie not performed"));
    assert_eq!(rust["findings"][0]["confidence"], "needs_document");
    assert_eq!(
        value(&rust, "report_tie_status"),
        json!("not performed: no report part in this read")
    );
}

#[test]
fn a_changed_value_is_reported() {
    let mut rust = rust_dump(true);
    figure_mut(&mut rust, "financial_statements.sales")["value"] = json!(1i64);
    let d = diffs(&rust, true);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("financial_statements.sales: value differs"),
        "{d:?}"
    );
}

/// Reading closing stock off the stale TB field (the defect this test exists to avoid) must be
/// caught.
#[test]
fn closing_stock_from_the_stale_field_is_reported() {
    let mut rust = rust_dump(true);
    let id = "financial_statements.closing_stock";
    figure_mut(&mut rust, id)["value"] = json!(7_000_000i64);
    let d = diffs(&rust, true);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with(&format!("{id}: value differs")), "{d:?}");
}

/// One paisa past the tolerance: a Rust port comparing with `<` instead of `<=` would flip the
/// tie's confidence at exactly Rs 1, and the golden (computed) must disagree with that.
#[test]
fn a_tie_one_paisa_past_the_tolerance_is_reported() {
    let mut report = common::synthetic_report_totals();
    report.net_profit_paise += 1;
    let rust = common::run_financial_statements(
        &common::fixtures().join("synthetic-read"),
        false,
        Some(&report),
    )
    .unwrap();
    assert_eq!(rust["findings"][0]["confidence"], "judgement_required");
    assert_eq!(
        rust["module_invariant_violations"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let d = diffs(&rust, true);
    assert!(d.iter().any(|x| x.contains("confidence differs")), "{d:?}");
}

#[test]
fn a_missing_partner_figure_is_reported() {
    let mut rust = rust_dump(true);
    rust["figures"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "financial_statements.partner_interest");
    let d = diffs(&rust, true);
    assert!(
        d.iter()
            .any(|x| x.contains("financial_statements.partner_interest")),
        "{d:?}"
    );
}

#[test]
fn a_dump_below_the_structural_floor_is_reported() {
    let mut rust = rust_dump(true);
    rust["figures"].as_array_mut().unwrap().truncate(17);
    let d = compare(&rust, &rust, None).unwrap();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("figure count below the minimum (18) for test \"financial_statements\""),
        "{d:?}"
    );
}
