// SPDX-License-Identifier: Apache-2.0
//! CI parity for `ledger_scrutiny`: the Rust port over the committed synthetic read must produce the
//! canonical dump the Python reference produced over the same bytes
//! (`golden/synthetic.ledger_scrutiny.json`, see PROVENANCE.md), and the comparison must be able to fail.

mod common;

use bridge_tax_audit::compare::compare;
use serde_json::{json, Value};

fn rust_dump() -> Value {
    common::run_ledger_scrutiny(&common::fixtures().join("synthetic-read"), false).unwrap()
}

fn diffs(rust: &Value) -> Vec<String> {
    compare(
        &common::golden_named("synthetic.ledger_scrutiny"),
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
    assert_eq!(rust["figures"].as_array().unwrap().len(), 87);
    assert_eq!(rust["findings"].as_array().unwrap().len(), 7);
    assert_eq!(
        value(&rust, "ledger_scrutiny.cash_share_bp_690d194d"),
        json!(8630)
    );
    assert_eq!(
        value(&rust, "ledger_scrutiny.cash_paid_paise_2d3194aa"),
        json!(2000000)
    );
    assert_eq!(
        rust["module_invariants_evaluated"],
        json!(["ledger_scrutiny.check_invariants"])
    );
}

#[test]
fn a_changed_value_is_reported() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "ledger_scrutiny.cash_paid_paise_690d194d")["value"] = json!(1i64);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("ledger_scrutiny.cash_paid_paise_690d194d: value differs"),
        "{d:?}"
    );
}
