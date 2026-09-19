// SPDX-License-Identifier: Apache-2.0
//! CI parity for `depreciation`: the Rust slice over the committed synthetic read must produce
//! the canonical dump the Python reference engine produced over the same bytes
//! (`tests/fixtures/golden/synthetic.depreciation.json`, see PROVENANCE.md), and the comparison
//! must be able to fail. Mirrors `tests/parity_40a3.rs`'s discipline: each seeded test alters one
//! thing and asserts the specific difference is reported, not merely that something was.
//!
//! Two of the fixture's cases sit exactly AT a legal limit this module tests (masterid 43-49,
//! `parity/generate_fixture.py`): an addition put to use for exactly 180 days (full rate, "Office
//! Furniture") beside one at 179 days (half rate, one calendar day later, "Showroom Furniture"),
//! and a cash-paid addition of exactly Rs 10,000 -- the s.43(1) second proviso limit itself -- not
//! flagged ("Office Computers") beside one Rs 0.01 over it, flagged ("Reception Computers", plus
//! the pre-existing "Delivery Van" from `cash_payments_40a3`'s own fixture, cash-paid over the
//! limit). The seeded tests below mutate exactly the figures that boundary produces, so a flipped
//! `>=`/`>` in the Rust port -- which would leave every non-boundary test passing -- changes this
//! fixture's result and is caught here, not only by the crate's own unit tests on hand-built books
//! (`src/depreciation.rs`).

mod common;

use bridge_tax_audit::compare::{compare, ParityMismatch};
use serde_json::{json, Value};

fn rust_dump() -> Value {
    common::run_depreciation(&common::fixtures().join("synthetic-read"), false).unwrap()
}

fn figure_mut<'a>(doc: &'a mut Value, id: &str) -> &'a mut Value {
    doc["figures"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|f| f["id"] == id)
        .unwrap()
}

fn diffs(rust: &Value) -> Vec<String> {
    compare(&common::golden_depreciation(), rust, None).unwrap()
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
    assert_eq!(rust["figures"].as_array().unwrap().len(), 41);
    assert_eq!(rust["findings"].as_array().unwrap().len(), 3);
    assert_eq!(rust["rules_version"], "2026-09-17.1");
    assert_eq!(
        rust["module_invariants_evaluated"],
        json!(["depreciation.check_invariants"])
    );
    assert_eq!(rust["module_invariant_violations"], json!([]));
    // The 180/179-day boundary: Office Furniture (exactly 180 days used) is full rate, Showroom
    // Furniture (179 days, one day later) is half rate -- never the other way round.
    assert_eq!(
        figure_mut(
            &mut rust.clone(),
            "depreciation.additions_ge180_furniture_10"
        )["value"],
        10_000_000i64
    );
    assert_eq!(
        figure_mut(
            &mut rust.clone(),
            "depreciation.additions_lt180_furniture_10"
        )["value"],
        6_000_000i64
    );
    // The s.43(1) cash-addition-limit boundary: exactly Rs 10,000 (Office Computers) is not
    // flagged; Rs 10,000.01 (Reception Computers) is. Anchored two ways: the sensitivity figure
    // that excludes only the flagged additions from cost, and the exact finding count (a `>=`
    // mutation would flag Office Computers too, adding a third s43_1 finding).
    assert_eq!(
        figure_mut(
            &mut rust.clone(),
            "depreciation.dep_total_act_sensitivity_excl_cash_computers_40"
        )["value"],
        8_000_000i64
    );
    let s43_1: Vec<&Value> = rust["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["id"].as_str().unwrap().starts_with("depreciation/s43_1/"))
        .collect();
    assert_eq!(s43_1.len(), 2, "{s43_1:?}");
    assert_eq!(
        rust["book_invariant_violations"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn a_changed_value_is_reported() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "depreciation.act_dep_total")["value"] = json!(1i64);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("depreciation.act_dep_total: value differs"),
        "{d:?}"
    );
}

/// The 180-day boundary itself: flipping which pool an addition put to use on the exact threshold
/// falls into (as a `>` vs `>=` mutation in the Rust port would) must be caught.
#[test]
fn a_changed_value_at_the_180_day_boundary_is_reported() {
    let mut rust = rust_dump();
    let id = "depreciation.additions_ge180_furniture_10";
    assert_eq!(figure_mut(&mut rust, id)["value"], 10_000_000i64);
    figure_mut(&mut rust, id)["value"] = json!(0i64);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with(&format!("{id}: value differs")), "{d:?}");
}

/// The s.43(1) cash-addition-limit boundary itself: the sensitivity figure that excludes exactly
/// the flagged additions from cost must reflect Reception Computers (Rs 10,000.01) being flagged
/// and Office Computers (exactly Rs 10,000) not.
#[test]
fn a_changed_value_at_the_cash_limit_boundary_is_reported() {
    let mut rust = rust_dump();
    let id = "depreciation.dep_total_act_sensitivity_excl_cash_computers_40";
    assert_eq!(figure_mut(&mut rust, id)["value"], 8_000_000i64);
    figure_mut(&mut rust, id)["value"] = json!(14_000_000i64); // as if neither addition were flagged
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].starts_with(&format!("{id}: value differs")), "{d:?}");
}

/// A `>=` mutation at the cash-limit boundary would flag Office Computers too, adding a third
/// s43_1 finding for it; simulate that here and confirm the key-set mismatch is reported.
#[test]
fn an_extra_s43_1_finding_is_reported() {
    let mut rust = rust_dump();
    let mut extra = rust["findings"].as_array().unwrap()[0].clone();
    extra["id"] = json!("depreciation/s43_1/simulated_boundary_mutation");
    rust["findings"].as_array_mut().unwrap().push(extra);
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.starts_with("findings present on the right only")
                && l.contains("depreciation/s43_1/simulated_boundary_mutation")),
        "{d:?}"
    );
}

#[test]
fn a_missing_figure_is_reported() {
    let mut rust = rust_dump();
    rust["figures"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "depreciation.gst_tcs_addition_lines_seen_count");
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.starts_with("figures present on the left only")
                && l.contains("depreciation.gst_tcs_addition_lines_seen_count")),
        "{d:?}"
    );
}

/// Unlike `cash_44ab` (which always emits exactly 7 figures), `depreciation`'s figure count is
/// data-dependent: only the 2 always-present figures are guaranteed (see `compare::
/// default_min_figures`'s own doc comment for why). Dropping below that floor -- as an empty or
/// near-empty dump would -- must still be caught.
#[test]
fn a_dump_below_the_two_figure_floor_is_reported() {
    let mut rust = rust_dump();
    rust["figures"].as_array_mut().unwrap().truncate(1);
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.contains("figure count below the minimum (2)")),
        "{d:?}"
    );
}

#[test]
fn a_flipped_confidence_is_reported() {
    let mut rust = rust_dump();
    let idx = rust["findings"]
        .as_array()
        .unwrap()
        .iter()
        .position(|f| f["id"] == "depreciation/book_vs_act")
        .unwrap();
    rust["findings"][idx]["confidence"] = json!("judgement_required");
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].contains("confidence differs"), "{d:?}");
}

/// DEP-1/DEP-2 (this module's own `check_invariants`) must not silently read as "no violations"
/// on the side that never evaluated it at all.
#[test]
fn a_module_invariant_not_evaluated_is_reported_even_without_violations() {
    let mut rust = rust_dump();
    rust["module_invariants_evaluated"] = json!([]);
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.starts_with("module_invariants_evaluated differ")),
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
    let mut golden = common::golden_depreciation();
    golden["figures"] = json!([]);
    assert!(matches!(
        compare(&golden, &rust, None),
        Err(ParityMismatch(_))
    ));
}
