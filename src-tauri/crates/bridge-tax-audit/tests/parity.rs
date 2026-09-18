// SPDX-License-Identifier: Apache-2.0
//! CI parity: the Rust slice over the committed synthetic read must produce the canonical
//! `cash_44ab` dump the Python reference engine produced over the same bytes
//! (`tests/fixtures/golden/synthetic.cash_44ab.json`, see PROVENANCE.md), and the comparison
//! must be able to fail. Each seeded test alters one thing and asserts the specific
//! difference is reported, not merely that something was.

mod common;

use bridge_tax_audit::compare::{compare, ParityMismatch};
use serde_json::{json, Value};

fn rust_dump() -> Value {
    common::run(&common::fixtures().join("synthetic-read"), false).unwrap()
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
    compare(&common::golden(), rust, None).unwrap()
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
    assert_eq!(rust["figures"].as_array().unwrap().len(), 7);
    assert_eq!(rust["rules_version"], "2026-09-17.1");
    assert_eq!(
        figure_mut(&mut rust.clone(), "cash_44ab.cash_receipts")["value"],
        104_373_457
    );
    assert_eq!(
        rust["book_invariant_violations"].as_array().unwrap().len(),
        4
    );
}

#[test]
fn a_changed_value_is_reported() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "cash_44ab.cash_receipts")["value"] = json!(9_873_458);
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("cash_44ab.cash_receipts: value differs"),
        "{d:?}"
    );
}

#[test]
fn a_missing_figure_is_reported() {
    let mut rust = rust_dump();
    rust["figures"]
        .as_array_mut()
        .unwrap()
        .retain(|f| f["id"] != "cash_44ab.bank_payments");
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.contains("figure count below the minimum (7)")),
        "{d:?}"
    );
    assert!(
        d.iter()
            .any(|l| l.starts_with("figures present on the left only")
                && l.contains("cash_44ab.bank_payments")),
        "{d:?}"
    );
}

#[test]
fn a_changed_unit_is_reported() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "cash_44ab.cash_share_receipts")["unit"] = json!("count");
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("cash_44ab.cash_share_receipts: unit differs"),
        "{d:?}"
    );
}

#[test]
fn a_float_value_stops_the_comparison() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "cash_44ab.cash_receipts")["value"] = json!(9_873_457.0);
    let err = compare(&common::golden(), &rust, None).unwrap_err();
    assert!(err.0.contains("float leakage"), "{err}");
}

#[test]
fn an_undefined_ratio_is_not_zero() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "cash_44ab.cash_share_payments")["value"] = Value::Null;
    let d = diffs(&rust);
    assert!(
        d[0].starts_with("cash_44ab.cash_share_payments: value differs"),
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
    let mut golden = common::golden();
    golden["figures"] = json!([]);
    assert!(matches!(
        compare(&golden, &rust, None),
        Err(ParityMismatch(_))
    ));
}

/// One paisa changed in the read's own bytes, with the manifest re-hashed so the consumer
/// rules admit it: every book invariant still passes (the TB tie tolerates a rupee), and only
/// the parity comparison sees the difference.
#[test]
fn one_paisa_in_the_read_bytes_is_reported() {
    let scratch = common::ScratchRead::new("paisa");
    scratch.edit_part(
        "vouchers-2025-04-01",
        "<AMOUNT TYPE=\"Amount\">-42000.00</AMOUNT>",
        "<AMOUNT TYPE=\"Amount\">-42000.01</AMOUNT>",
        true,
    );
    let rust = common::run(&scratch.dir, false).unwrap();
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|l| l.starts_with("cash_44ab.cash_receipts: value differs")
                && l.contains("104373457")
                && l.contains("104373458")),
        "{d:?}"
    );
    assert_eq!(
        rust["book_invariant_violations"],
        common::golden()["book_invariant_violations"]
    );
}
