// SPDX-License-Identifier: Apache-2.0
//! CI parity for every registered test at once: each entry in `registry::PORTED` has a synthetic
//! golden (`golden/synthetic.<id>.json`) and matches it over the committed synthetic read, and the
//! Python side's `parity/python_golden.py` `RUNNERS` names exactly the same tests. A newly ported
//! test is covered here by its registry entry and golden alone; its own `parity_<id>.rs` adds the
//! anchored checks (figure counts, named values, a changed value reported).

mod common;

use bridge_tax_audit::compare::compare;
use bridge_tax_audit::registry::{self, CallerData, PORTED};
use bridge_tax_audit::rules_for;

fn caller(id: &str) -> CallerData {
    let json = |name: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(common::fixtures().join(name)).unwrap())
            .unwrap()
    };
    let mut c = CallerData::default();
    match id {
        "financial_statements" => {
            c.report_totals = Some(
                registry::report_totals_from_json(&json("synthetic-report-totals.json")).unwrap(),
            );
        }
        "applicability_44ab" => {
            c.turnover_inputs =
                registry::turnover_inputs_from_json(&json("synthetic-turnover-inputs.json"))
                    .unwrap();
        }
        _ => {}
    }
    c
}

#[test]
fn every_registered_test_matches_its_synthetic_golden() {
    let e = common::engagement(&common::fixtures().join("synthetic-read"), false);
    let rules = rules_for(&e).unwrap();
    for test in PORTED {
        let rust = registry::run_canonical(test.id, &e, &rules, &caller(test.id)).unwrap();
        let golden = common::golden_named(&format!("synthetic.{}", test.id));
        let diffs = compare(&golden, &rust, None).unwrap();
        assert!(diffs.is_empty(), "{}:\n{}", test.id, diffs.join("\n"));
    }
}

#[test]
fn the_python_runners_name_the_same_tests() {
    // Read as text: the Python module needs the reference engine on its path to import.
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("parity/python_golden.py"),
    )
    .unwrap();
    let block = text
        .split("\nRUNNERS = {\n")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("python_golden.py has a RUNNERS = { ... } block");
    let python: Vec<&str> = block
        .lines()
        .map(|l| l.trim().split('"').nth(1).expect("one quoted id per line"))
        .collect();
    let rust: Vec<&str> = PORTED.iter().map(|t| t.id).collect();
    assert_eq!(python, rust);
}

#[test]
fn an_unregistered_test_is_refused() {
    let e = common::engagement(&common::fixtures().join("synthetic-read"), false);
    let rules = rules_for(&e).unwrap();
    let err = registry::run_canonical("itr_3cd_tally", &e, &rules, &CallerData::default())
        .unwrap_err()
        .to_string();
    assert!(err.contains("itr_3cd_tally is not a ported test"), "{err}");
}
