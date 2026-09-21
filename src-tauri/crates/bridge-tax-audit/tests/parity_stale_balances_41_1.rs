// SPDX-License-Identifier: Apache-2.0
//! CI parity for `stale_balances_41_1`: the Rust port over the committed synthetic read must produce the
//! canonical dump the Python reference produced over the same bytes
//! (`golden/synthetic.stale_balances_41_1.json`, see PROVENANCE.md), and the comparison must be able to fail.

mod common;

use bridge_tax_audit::compare::compare;
use serde_json::{json, Value};

fn rust_dump() -> Value {
    common::run_stale_balances_41_1(&common::fixtures().join("synthetic-read"), false).unwrap()
}

fn diffs(rust: &Value) -> Vec<String> {
    compare(
        &common::golden_named("synthetic.stale_balances_41_1"),
        rust,
        None,
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

fn value(doc: &Value, id: &str) -> Value {
    doc["figures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == id)
        .unwrap()["value"]
        .clone()
}

#[test]
fn synthetic_read_matches_the_python_golden() {
    let rust = rust_dump();
    let d = diffs(&rust);
    assert!(d.is_empty(), "parity failed:\n{}", d.join("\n"));
    // Anchor the pass to content, so an empty or trivial dump cannot be what passed.
    assert_eq!(rust["figures"].as_array().unwrap().len(), 12);
    assert_eq!(rust["findings"].as_array().unwrap().len(), 2);
    assert_eq!(
        value(&rust, "stale_balances_41_1.creditor_stale_count"),
        json!(1)
    );
    assert_eq!(
        value(
            &rust,
            "stale_balances_41_1.creditor_stale_closing_total_paise"
        ),
        json!(-1234500)
    );
    assert_eq!(
        value(&rust, "stale_balances_41_1.creditor_active_count"),
        json!(10)
    );
    assert_eq!(
        rust["module_invariants_evaluated"],
        json!(["stale_balances_41_1.check_invariants"])
    );
}

#[test]
fn a_changed_value_is_reported() {
    let mut rust = rust_dump();
    figure_mut(
        &mut rust,
        "stale_balances_41_1.creditor_stale_closing_total_paise",
    )["value"] = json!(1i64);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("stale_balances_41_1.creditor_stale_closing_total_paise: value differs"),
        "{d:?}"
    );
}
