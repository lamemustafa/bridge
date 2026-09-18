//! Diff two canonical dumps by the same rules as the reference's `tae/parity/compare.py`
//! (PARITY-SPEC-v1 section 7): type validation on both sides first, refusal of an
//! empty-vs-empty comparison, a minimum figure count, identical figure and finding key sets
//! (the full symmetric difference is reported), then per-figure unit, value, definition hash
//! and evidence; per-finding clauses (ordered), confidence, facts, evidence and prose hashes;
//! the population note; and the invariant reports. Every list is re-sorted before comparing,
//! so a producer's order never decides the result.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Json;

const NUMERIC_UNITS: [&str; 4] = ["paise", "bp", "count", "days"];

/// The reference's `DEFAULT_MIN_FIGURES`: anchored to the reference fixture's figure counts.
pub fn default_min_figures(test_id: &str) -> usize {
    match test_id {
        "cash_44ab" => 7,
        "cash_payments_40a3" => 28,
        _ => 1,
    }
}

/// A structural defect that stops the comparison (a bad type, or nothing to compare).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParityMismatch(pub String);

fn list<'a>(doc: &'a Json, key: &str) -> &'a [Json] {
    doc.get(key)
        .and_then(Json::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text<'a>(doc: &'a Json, key: &str) -> &'a str {
    doc.get(key).and_then(Json::as_str).unwrap_or_default()
}

fn is_integer(value: &Json) -> bool {
    value.as_i64().is_some() || value.as_u64().is_some()
}

/// Both documents' figure values, checked against their units independently.
pub fn validate_types(doc: &Json, side: &str) -> Result<(), ParityMismatch> {
    for fig in list(doc, "figures") {
        let (unit, value, id) = (
            fig.get("unit").and_then(Json::as_str),
            fig.get("value").unwrap_or(&Json::Null),
            fig.get("id"),
        );
        match unit {
            Some(u) if NUMERIC_UNITS.contains(&u) => {
                if !value.is_null() && !is_integer(value) {
                    return Err(ParityMismatch(format!(
                        "[{side}] figure {id:?} unit {u:?} but value {value} is not int|null -- float leakage or a bad type"
                    )));
                }
            }
            Some("text") => {
                if !value.is_string() {
                    return Err(ParityMismatch(format!(
                        "[{side}] figure {id:?} unit 'text' but value {value} is not a string"
                    )));
                }
            }
            other => {
                return Err(ParityMismatch(format!(
                    "[{side}] figure {id:?} has an unrecognised unit {other:?}"
                )))
            }
        }
    }
    Ok(())
}

fn by_id(items: &[Json]) -> BTreeMap<String, &Json> {
    items
        .iter()
        .map(|item| (text(item, "id").to_string(), item))
        .collect()
}

fn sorted_by(items: &[Json], key: impl Fn(&Json) -> Vec<String>) -> Vec<Json> {
    let mut items = items.to_vec();
    items.sort_by_key(|item| key(item));
    items
}

fn sorted_evidence(items: &[Json]) -> Vec<Json> {
    sorted_by(items, |e| {
        vec![
            format!("{}:{}", text(e, "kind"), text(e, "id")),
            text(e, "label").to_string(),
        ]
    })
}

fn sorted_facts(items: &[Json]) -> Vec<Json> {
    sorted_by(items, |f| vec![text(f, "name").to_string()])
}

fn sorted_violations(items: &[Json]) -> Vec<Json> {
    sorted_by(items, |v| {
        ["invariant", "subject", "detail"]
            .iter()
            .map(|k| text(v, k).to_string())
            .collect()
    })
}

fn key_sets(
    what: &str,
    a: &BTreeMap<String, &Json>,
    b: &BTreeMap<String, &Json>,
    out: &mut Vec<String>,
) {
    let left: BTreeSet<&String> = a.keys().collect();
    let right: BTreeSet<&String> = b.keys().collect();
    let only_left: Vec<&&String> = left.difference(&right).collect();
    let only_right: Vec<&&String> = right.difference(&left).collect();
    if !only_left.is_empty() {
        out.push(format!("{what} present on the left only: {only_left:?}"));
    }
    if !only_right.is_empty() {
        out.push(format!("{what} present on the right only: {only_right:?}"));
    }
}

fn diff_figures(a: &Json, b: &Json, out: &mut Vec<String>) {
    let (fa, fb) = (by_id(list(a, "figures")), by_id(list(b, "figures")));
    key_sets("figures", &fa, &fb, out);
    for (id, x) in &fa {
        let Some(y) = fb.get(id) else { continue };
        for field in ["unit", "value"] {
            if x.get(field) != y.get(field) {
                out.push(format!(
                    "{id}: {field} differs left={:?} right={:?}",
                    x.get(field),
                    y.get(field)
                ));
            }
        }
        if x.get("definition_sha256_16") != y.get("definition_sha256_16") {
            out.push(format!(
                "{id}: definition differs (hash mismatch)\n    left:  {:?}\n    right: {:?}",
                text(x, "definition_text"),
                text(y, "definition_text")
            ));
        }
        let (ex, ey) = (
            sorted_evidence(list(x, "evidence")),
            sorted_evidence(list(y, "evidence")),
        );
        if ex != ey {
            out.push(format!(
                "{id}: evidence differs\n    left:  {ex:?}\n    right: {ey:?}"
            ));
        }
    }
}

fn diff_findings(a: &Json, b: &Json, out: &mut Vec<String>) {
    let (fa, fb) = (by_id(list(a, "findings")), by_id(list(b, "findings")));
    key_sets("findings", &fa, &fb, out);
    for (id, x) in &fa {
        let Some(y) = fb.get(id) else { continue };
        for field in ["clauses", "confidence"] {
            if x.get(field) != y.get(field) {
                out.push(format!(
                    "{id}: {field} differs left={:?} right={:?}",
                    x.get(field),
                    y.get(field)
                ));
            }
        }
        let (fx, fy) = (
            sorted_facts(list(x, "facts")),
            sorted_facts(list(y, "facts")),
        );
        if fx != fy {
            out.push(format!("{id}: facts differ left={fx:?} right={fy:?}"));
        }
        let (ex, ey) = (
            sorted_evidence(list(x, "evidence")),
            sorted_evidence(list(y, "evidence")),
        );
        if ex != ey {
            out.push(format!("{id}: evidence differs left={ex:?} right={ey:?}"));
        }
        for field in ["title", "limits", "ask_client"] {
            let hash = format!("{field}_sha256_16");
            if x.get(&hash) != y.get(&hash) {
                let prose = format!("{field}_text");
                out.push(format!(
                    "{id}: {field} text differs (hash mismatch)\n    left:  {:?}\n    right: {:?}",
                    x.get(&prose),
                    y.get(&prose)
                ));
            }
        }
    }
}

/// Every difference between two canonical dumps (empty means parity holds), or a
/// [`ParityMismatch`] for a defect that stops the comparison.
pub fn compare(
    a: &Json,
    b: &Json,
    min_figures: Option<usize>,
) -> Result<Vec<String>, ParityMismatch> {
    validate_types(a, "left")?;
    validate_types(b, "right")?;
    let (n_a, n_b) = (list(a, "figures").len(), list(b, "figures").len());
    if n_a == 0 && n_b == 0 {
        return Err(ParityMismatch(
            "both sides report zero figures; refusing an empty-vs-empty comparison".to_string(),
        ));
    }
    let test_id = text(a, "test_id");
    let min = min_figures.unwrap_or_else(|| default_min_figures(test_id));

    let mut out = Vec::new();
    for field in ["test_id", "rules_version"] {
        if a.get(field) != b.get(field) {
            out.push(format!(
                "{field} differs left={:?} right={:?}",
                a.get(field),
                b.get(field)
            ));
        }
    }
    if n_a < min || n_b < min {
        out.push(format!(
            "figure count below the minimum ({min}) for test {test_id:?}: left={n_a} right={n_b}"
        ));
    }
    diff_figures(a, b, &mut out);
    diff_findings(a, b, &mut out);
    if a.get("population_note_sha256_16") != b.get("population_note_sha256_16") {
        out.push(format!(
            "population_note differs\n    left:  {:?}\n    right: {:?}",
            text(a, "population_note_text"),
            text(b, "population_note_text")
        ));
    }
    for key in [
        "book_invariants_evaluated",
        "result_invariants_evaluated",
        "module_invariants_evaluated",
    ] {
        let sorted = |doc: &Json| {
            let mut codes: Vec<String> = list(doc, key)
                .iter()
                .map(|c| c.as_str().map_or_else(|| c.to_string(), str::to_string))
                .collect();
            codes.sort();
            codes
        };
        let (ca, cb) = (sorted(a), sorted(b));
        if ca != cb {
            out.push(format!(
                "{key} differ left={ca:?} right={cb:?} -- an invariant evaluated on only one side reads as 'no violations' otherwise"
            ));
        }
    }
    for key in [
        "book_invariant_violations",
        "result_invariant_violations",
        "module_invariant_violations",
    ] {
        let (va, vb) = (
            sorted_violations(list(a, key)),
            sorted_violations(list(b, key)),
        );
        if va != vb {
            out.push(format!(
                "{key} differ\n    left:  {va:?}\n    right: {vb:?}"
            ));
        }
    }
    Ok(out)
}
