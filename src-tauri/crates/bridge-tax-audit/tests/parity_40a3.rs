// SPDX-License-Identifier: Apache-2.0
//! CI parity for `cash_payments_40a3`: the Rust slice over the committed synthetic read must
//! produce the canonical dump the Python reference engine produced over the same bytes
//! (`tests/fixtures/golden/synthetic.cash_payments_40a3.json`, see PROVENANCE.md), and the
//! comparison must be able to fail. Mirrors `tests/parity.rs`'s discipline for `cash_44ab`: each
//! seeded test alters one thing and asserts the specific difference is reported, not merely
//! that something was.

mod common;

use bridge_tax_audit::compare::{compare, ParityMismatch};
use serde_json::{json, Value};

fn rust_dump() -> Value {
    common::run_40a3(&common::fixtures().join("synthetic-read"), false).unwrap()
}

fn figure_mut<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["figures"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["id"] == id)
        .unwrap()
}

fn finding_mut<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["findings"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["id"] == id)
        .unwrap()
}

fn diffs(rust: &Value) -> Vec<String> {
    compare(&common::golden_40a3(), rust, None).unwrap()
}

#[test]
fn synthetic_read_matches_the_python_golden() {
    let rust = rust_dump();
    let differences = diffs(&rust);
    assert!(
        differences.is_empty(),
        "parity failed:\n{}",
        differences.join("\n")
    );
    // Anchor the pass to content, so an empty or trivial dump cannot be what passed.
    assert_eq!(rust["figures"].as_array().unwrap().len(), 50);
    assert_eq!(rust["findings"].as_array().unwrap().len(), 22);
    assert_eq!(rust["rules_version"], "2026-09-17.1");
    assert_eq!(
        figure_mut(
            &mut rust.clone(),
            "cash_payments_40a3.s40a3_over_limit_in_scope_count"
        )["value"],
        10
    );
    assert_eq!(
        figure_mut(
            &mut rust.clone(),
            "cash_payments_40a3.s269ss269t_candidate_count"
        )["value"],
        3
    );
    assert_eq!(
        rust["book_invariant_violations"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn a_changed_value_is_reported() {
    let mut rust = rust_dump();
    figure_mut(
        &mut rust,
        "cash_payments_40a3.s40a3_payee_days_any_amount_total",
    )["value"] = json!(68_585_051i64);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("cash_payments_40a3.s40a3_payee_days_any_amount_total: value differs"),
        "{d:?}"
    );
}

#[test]
fn a_missing_figure_is_reported() {
    let mut rust = rust_dump();
    rust["figures"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "cash_payments_40a3.s269ss269t_candidate_total");
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.starts_with("figures present on the left only")
                && l.contains("cash_payments_40a3.s269ss269t_candidate_total")),
        "{d:?}"
    );
}

/// Unlike `cash_44ab` (which always emits exactly 7 figures, so its floor and its total
/// coincide), `cash_payments_40a3`'s figure count is data-dependent: only the 18 always-present
/// summary figures are guaranteed. Dropping below that floor -- as an empty or near-empty dump
/// would -- must still be caught.
#[test]
fn a_dump_below_the_eighteen_figure_floor_is_reported() {
    let mut rust = rust_dump();
    rust["figures"].as_array_mut().unwrap().truncate(10);
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.contains("figure count below the minimum (18)")),
        "{d:?}"
    );
}

/// The goods-carriage heuristic is a proviso-limit exemption on the s.40A(3) daily cap: flipping
/// its "yes"/"no" text value must be caught, the same way a flipped applicability flag would be
/// for any other exemption.
#[test]
fn a_flipped_goods_carriage_exemption_is_reported() {
    let mut rust = rust_dump();
    let id = "cash_payments_40a3.s40a3_goods_carriage_candidate_2025-06-13_026540de";
    assert_eq!(figure_mut(&mut rust, id)["value"], "yes");
    figure_mut(&mut rust, id)["value"] = json!("no");
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with(&format!("{id}: value differs")), "{d:?}");
}

/// The 2026-09-17 double-count fix (module docstring): an uncovered loan ledger's s.269T finding
/// carries an extra Clause 31 tag a covered one does not. Dropping that tag (as if the ledger
/// were wrongly treated as configuration-covered) must be caught by the clause comparison.
#[test]
fn a_flipped_loan_coverage_clause_is_reported() {
    let mut rust = rust_dump();
    let uncovered = rust["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["id"]
                .as_str()
                .unwrap()
                .starts_with("cash_payments_40a3/s269ss269t/")
                && f["clauses"].as_array().unwrap().len() == 2
        })
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    finding_mut(&mut rust, &uncovered)["clauses"] = json!(["s.269T"]);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with(&format!("{uncovered}: clauses differs")),
        "{d:?}"
    );
}

#[test]
fn a_flipped_confidence_is_reported() {
    let mut rust = rust_dump();
    rust["findings"][0]["confidence"] = json!("computed");
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains("confidence differs"), "{d:?}");
}

#[test]
fn an_invariant_not_evaluated_is_reported_even_without_violations() {
    let mut rust = rust_dump();
    rust["result_invariants_evaluated"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c != "EVID-1");
    let d = diffs(&rust);
    assert!(
        d[0].starts_with("result_invariants_evaluated differ"),
        "{d:?}"
    );
}

#[test]
fn a_reordered_but_equal_dump_passes() {
    let mut rust = rust_dump();
    rust["figures"].as_array_mut().unwrap().reverse();
    rust["findings"].as_array_mut().unwrap().reverse();
    rust["book_invariant_violations"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert!(diffs(&rust).is_empty());
}

#[test]
fn empty_against_empty_is_refused() {
    let mut rust = rust_dump();
    rust["figures"] = json!([]);
    let mut golden = common::golden_40a3();
    golden["figures"] = json!([]);
    assert!(matches!(
        compare(&golden, &rust, None),
        Err(ParityMismatch(_))
    ));
}
