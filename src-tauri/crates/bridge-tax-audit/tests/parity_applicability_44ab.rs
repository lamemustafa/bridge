// SPDX-License-Identifier: Apache-2.0
//! CI parity for `applicability_44ab`: the Rust slice over the committed synthetic read, with the
//! committed comparison turnover (`synthetic-turnover-inputs.json`), must produce the canonical dump
//! the Python reference engine produced (`golden/synthetic.applicability_44ab.json`, see
//! PROVENANCE.md), and the comparison must be able to fail.
//!
//! The dump exercises the chain the reference's own pack runs: books turnover from
//! `financial_statements`' `sales` (Rs 2,97,500), the cash share from `cash_44ab`, a full-coverage
//! GSTR-1 figure that is differenced and a partial-coverage GSTR-3B figure that must not be, an
//! absent AIS figure, the vendored due dates and a supplied presumptive history. Every s.44AB(a)
//! threshold boundary is tested on the pure function in `src/applicability_44ab.rs`.

mod common;

use bridge_tax_audit::compare::compare;
use serde_json::{json, Value};

fn rust_dump() -> Value {
    common::run_applicability_44ab(
        &common::fixtures().join("synthetic-read"),
        &common::synthetic_turnover_inputs(),
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
        .find(|f| f["id"] == format!("applicability_44ab.{name}"))
        .unwrap()["value"]
        .clone()
}

fn diffs(rust: &Value) -> Vec<String> {
    compare(&common::golden_applicability_44ab(), rust, None).unwrap()
}

#[test]
fn synthetic_read_matches_the_python_golden() {
    let rust = rust_dump();
    let d = diffs(&rust);
    assert!(d.is_empty(), "parity failed:\n{}", d.join("\n"));
    assert_eq!(rust["figures"].as_array().unwrap().len(), 12);
    assert_eq!(rust["findings"].as_array().unwrap().len(), 2);
    assert_eq!(value(&rust, "turnover"), json!(29_750_000)); // financial_statements' sales
    assert_eq!(value(&rust, "audit_required_44ab_a"), json!("no"));
    assert_eq!(
        value(&rust, "gstr1_turnover_diff_from_books"),
        json!(250_000)
    );
    assert!(!rust["figures"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["id"] == "applicability_44ab.gstr3b_turnover_diff_from_books"));
    assert_eq!(
        value(&rust, "presumptive_history_status"),
        json!("supplied")
    );
    assert_eq!(value(&rust, "due_date_audit_report"), json!("2026-09-30"));
}

#[test]
fn a_changed_call_is_reported() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "applicability_44ab.audit_required_44ab_a")["value"] = json!("yes");
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("applicability_44ab.audit_required_44ab_a: value differs"),
        "{d:?}"
    );
}

/// Differencing a partial-coverage source (the defect the coverage rule exists to prevent) must be
/// caught as an extra figure.
#[test]
fn a_partial_source_differenced_is_reported() {
    let mut inputs = common::synthetic_turnover_inputs();
    inputs.gstr3b.as_mut().unwrap().coverage = "full".to_string();
    let rust = common::run_applicability_44ab(&common::fixtures().join("synthetic-read"), &inputs)
        .unwrap();
    let d = diffs(&rust);
    assert!(
        d.iter()
            .any(|x| x.contains("applicability_44ab.gstr3b_turnover_diff_from_books")),
        "{d:?}"
    );
}

#[test]
fn a_changed_due_date_is_reported() {
    let mut rust = rust_dump();
    figure_mut(&mut rust, "applicability_44ab.due_date_audit_report")["value"] =
        json!("2026-10-31");
    let d = diffs(&rust);
    assert_eq!(d.len(), 1, "{d:?}");
}

#[test]
fn a_dump_below_the_structural_floor_is_reported() {
    let mut rust = rust_dump();
    rust["figures"].as_array_mut().unwrap().truncate(8);
    let d = compare(&rust, &rust, None).unwrap();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(
        d[0].starts_with("figure count below the minimum (9) for test \"applicability_44ab\""),
        "{d:?}"
    );
}
