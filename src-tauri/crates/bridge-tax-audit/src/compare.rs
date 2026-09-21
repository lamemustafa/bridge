//! Diff two canonical dumps by the same rules as the reference Python implementation's own
//! comparison tool (`docs/tax-audit/parity-spec-v1.md` section 7): type validation on both
//! sides first, refusal of an empty-vs-empty comparison, a minimum figure count, identical
//! figure and finding key sets (the full symmetric difference is reported), then per-figure
//! unit, value, definition hash and evidence; per-finding clauses (ordered), confidence, facts,
//! evidence and prose hashes; the population note; and the invariant reports. Every list is
//! re-sorted before comparing, so a producer's order never decides the result.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Json;

const NUMERIC_UNITS: [&str; 4] = ["paise", "bp", "count", "days"];

/// A minimum figure count below which a dump cannot be trusted as complete, one entry per test
/// id. `cash_44ab` always emits exactly 7 figures regardless of the book, so its fixture total
/// and its structural floor are the same number.
///
/// `cash_payments_40a3` is not, and its 18 here is a DIFFERENT KIND of number from the reference
/// Python implementation's own `DEFAULT_MIN_FIGURES["cash_payments_40a3"] = 28`
/// (`tae/parity/compare.py`): that 28 is fixture-anchored -- it is the synthetic fixture's own
/// actual figure count as of the commit that set it, chosen so the floor fails loudly if the
/// fixture or the engine ever drifts the count without the constant being updated to match, and
/// it moves (28 -> some other number) every time this crate's own synthetic fixture gains or
/// loses figures, exactly as the 40 -> 49 in `tests/parity_40a3.rs` just did.
///
/// This 18 is STRUCTURAL instead: it is the count this module always emits regardless of the
/// book's content, computed from the code, not read off any one fixture -- 4 s.40A(3) summary
/// figures, plus 5 `s40a3_excluded_total_<kind>` figures (one per entry in
/// `rules.s40a3.excluded_group_roles`, which the vendored AY 2026-27 rules always list as five:
/// capital, loans_liability, loans_advances_asset, fixed_assets, duties_taxes -- even a kind with
/// no rows still emits its total as zero), plus 3 s.269ST receipt summary figures, 3 s.269ST
/// payment summary figures and 3 s.269SS/269T summary figures: 4 + 5 + 3 + 3 + 3 = 18. On top of
/// that structural floor, the module emits 2 more figures per in-scope s.40A(3) over-limit
/// payee-day and 1 more per s.269ST/s.269SS/269T row or candidate at or over its own limit -- so
/// a real book with few such rows legitimately produces far fewer figures than this crate's own
/// synthetic fixture's total (a three-client local parity run measured a real client at 22,
/// still full, correct parity), and this 18 never needs to move when that fixture does. 18 is the
/// right floor for that reason: low enough to admit a quiet real book, still high enough that an
/// empty or near-empty dump cannot pass.
/// `depreciation` always emits at least these 2 figures, in every code path -- even the "unmapped
/// Fixed Assets ledger" fail-loud path (which returns before any block, cash-flag or totals
/// figure) still carries them: `gst_tcs_addition_lines_seen_count` (a plain verification counter)
/// and `dep_expense_ledgers_count`. Unlike `cash_payments_40a3`'s 18, this floor is deliberately
/// small: every other figure is conditioned on at least one Fixed Assets ledger or one configured
/// block existing, which a real client book is never guaranteed to have (a services business with
/// no fixed assets at all is a legitimate, quiet book, not a broken dump). 2 is still high enough
/// that an empty or near-empty dump cannot pass.
/// `financial_statements` always emits these 18, whatever the book: 15 statement figures (the six
/// P&L group totals, opening stock, closing stock, the TB closing field, the stale-field count,
/// gross profit and its ratio, the stock-to-turnover ratio, net profit and its ratio) and 3
/// voucher-population counts. Partner, report-tie and per-exclusion figures come on top.
pub fn default_min_figures(test_id: &str) -> usize {
    match test_id {
        "cash_44ab" => 7,
        "cash_payments_40a3" => 18,
        "depreciation" => 2,
        "financial_statements" => 18,
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
