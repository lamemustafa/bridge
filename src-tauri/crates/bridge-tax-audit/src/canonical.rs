//! The canonical, language-neutral serialisation of one test result
//! (`docs/tax-audit/parity-spec-v1.md`; the reference is the Python implementation's own
//! canonical serialiser). Integer paise and basis points only, never a float; every list sorted
//! by plain code-point order; prose compared by a 16-hex-character sha256 of its NFC form, with
//! the text riding along for a human reading a diff.

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::book::Book;
use crate::error::Result;
use crate::findings::{EvidenceRef, Figure, Finding, TestResult, Value};
use crate::invariants::{book_invariants, result_invariants, Violation};

pub const SPEC_VERSION: &str = "1.0.0";

/// ASCII unit separator: joins an ordered list of strings before hashing, so `["ab", "c"]`
/// and `["a", "bc"]` hash differently.
const JOIN: &str = "\u{1f}";

pub fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Lowercase hex of some bytes.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// sha256 of the NFC-normalised UTF-8 text, first 16 hex characters.
pub fn sha16(text: &str) -> String {
    hex(&Sha256::digest(nfc(text).as_bytes())[..8])
}

fn hash_ordered(items: &[String]) -> String {
    sha16(&items.join(JOIN))
}

fn evidence(refs: &[EvidenceRef]) -> Json {
    let mut items: Vec<(String, String, String)> = refs
        .iter()
        .map(|e| (e.kind.clone(), nfc(&e.id), nfc(&e.label)))
        .collect();
    items.sort_by(|a, b| {
        (format!("{}:{}", a.0, a.1), &a.2).cmp(&(format!("{}:{}", b.0, b.1), &b.2))
    });
    Json::Array(
        items
            .into_iter()
            .map(|(kind, id, label)| json!({"kind": kind, "id": id, "label": label}))
            .collect(),
    )
}

fn figure(f: &Figure) -> Json {
    let value = match &f.value {
        Value::Int(n) => json!(n),
        Value::Undefined => Json::Null,
        Value::Text(s) => json!(s),
    };
    json!({
        "id": f.id,
        "value": value,
        "unit": f.unit.as_str(),
        "definition_sha256_16": sha16(&f.definition),
        "definition_text": nfc(&f.definition),
        "evidence": evidence(&f.evidence),
    })
}

fn finding(f: &Finding) -> Json {
    let mut facts = f.facts.clone();
    facts.sort();
    json!({
        "id": f.id,
        "clauses": f.clauses,
        "confidence": f.confidence.as_str(),
        "facts": facts.iter().map(|(name, id)| json!({"name": name, "figure_id": id})).collect::<Vec<_>>(),
        "evidence": evidence(&f.evidence),
        "title_sha256_16": sha16(&f.title),
        "title_text": nfc(&f.title),
        "limits_sha256_16": hash_ordered(&f.limits),
        "limits_text": f.limits.iter().map(|s| nfc(s)).collect::<Vec<_>>(),
        "ask_client_sha256_16": hash_ordered(&f.ask_client),
        "ask_client_text": f.ask_client.iter().map(|s| nfc(s)).collect::<Vec<_>>(),
    })
}

fn report(codes: &[&str], violations: Vec<Violation>) -> (Json, Json) {
    let mut codes: Vec<&str> = codes.to_vec();
    codes.sort_unstable();
    let mut violations: Vec<Violation> = violations
        .into_iter()
        .map(|v| Violation {
            invariant: v.invariant,
            subject: nfc(&v.subject),
            detail: nfc(&v.detail),
        })
        .collect();
    violations.sort();
    let violations = violations
        .into_iter()
        .map(|v| json!({"invariant": v.invariant, "subject": v.subject, "detail": v.detail}))
        .collect();
    (json!(codes), Json::Array(violations))
}

/// The test module's OWN `check_invariants(book, result)`, if it has one, in the standard
/// 2-arg-in/`Vec<String>`-out shape: `None` when the test has no module-level invariant function
/// yet (`cash_44ab` and `cash_payments_40a3` do not; both sides of a parity comparison must show
/// an EMPTY evaluated-codes list for that test until one is added), `Some(violations)` -- even an
/// empty vec -- once one exists and was evaluated (`depreciation`'s DEP-1/DEP-2). Mirrors the
/// reference engine's own `module_invariant_report`: a single code `"<test_id>.check_invariants"`
/// stands for the whole module function, and each returned string becomes one violation with that
/// code, `result.test_id` as `subject`, and the string itself (NFC-normalised) as `detail`.
fn module_report(test_id: &str, module_check: Option<Vec<String>>) -> (Json, Json) {
    match module_check {
        None => (json!([]), json!([])),
        Some(raw) => {
            let code = format!("{test_id}.check_invariants");
            let mut violations: Vec<Violation> = raw
                .into_iter()
                .map(|s| Violation {
                    invariant: code.clone(),
                    subject: nfc(test_id),
                    detail: nfc(&s),
                })
                .collect();
            violations.sort();
            let violations = violations
                .into_iter()
                .map(
                    |v| json!({"invariant": v.invariant, "subject": v.subject, "detail": v.detail}),
                )
                .collect();
            (json!([code]), Json::Array(violations))
        }
    }
}

/// The full canonical dump for one test result on one book. `module_check` is this test's own
/// `check_invariants` output, or `None` for a test with no module-level invariant function yet
/// (see [`module_report`]).
pub fn canonical_test_result(
    book: &Book,
    result: &TestResult,
    module_check: Option<Vec<String>>,
) -> Result<Json> {
    let mut figures: Vec<&Figure> = result.figures.iter().collect();
    figures.sort_by(|a, b| a.id.cmp(&b.id));
    let mut findings: Vec<&Finding> = result.findings.iter().collect();
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let (book_codes, book_violations) = book_invariants(book)?;
    let (book_codes, book_violations) = report(&book_codes, book_violations);
    let (result_codes, result_violations) = result_invariants(book, result);
    let (result_codes, result_violations) = report(&result_codes, result_violations);
    let (module_codes, module_violations) = module_report(&result.test_id, module_check);
    Ok(json!({
        "spec_version": SPEC_VERSION,
        "test_id": result.test_id,
        "test_version": result.test_version,
        "rules_version": result.rules_version,
        "population_note_sha256_16": sha16(&result.population_note),
        "population_note_text": nfc(&result.population_note),
        "figures": figures.into_iter().map(figure).collect::<Vec<_>>(),
        "findings": findings.into_iter().map(finding).collect::<Vec<_>>(),
        "book_invariants_evaluated": book_codes,
        "book_invariant_violations": book_violations,
        "result_invariants_evaluated": result_codes,
        "result_invariant_violations": result_violations,
        "module_invariants_evaluated": module_codes,
        "module_invariant_violations": module_violations,
    }))
}
