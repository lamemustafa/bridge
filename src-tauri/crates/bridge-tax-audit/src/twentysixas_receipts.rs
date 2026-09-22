// SPDX-License-Identifier: Apache-2.0
//! The reference implementation's `twentysixas_receipts` test: the AMOUNT each Form 26AS deductor
//! reported as paid or credited to the assessee, against what the books record from the same
//! party. A line-for-line port; the reference module's docstring is the design record.
//!
//! * The deductor's books party comes from client configuration (`[tds_tcs_26as]
//!   .deductor_aliases`, keyed by TAN, never a name). Only Part I rows are read.
//! * Supply sections compare with the taxable value the books BILLED that party (Sales Accounts
//!   lines on its Sales / Debit Note / Credit Note vouchers); s.194A with the income credited on
//!   vouchers carrying its ledger (Direct/Indirect Incomes lines). Other sections are counted,
//!   never compared.
//! * A difference over Re 1 is a finding; every finding states the timing limit.
//!
//! Divergences: a total that overflows i64 paise is refused, where Python's integers are
//! unbounded.
//!
//! Refused as the reference raises: a figure id repeated by a ledger-tag collision.

use std::collections::{BTreeMap, BTreeSet};

use crate::book::{Book, Voucher};
use crate::documents::Form26asRow;
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::iso;
use crate::rules::Rules;
use crate::support::{overflow, py_upper};

pub const TEST_ID: &str = "twentysixas_receipts";

/// Add a figure, refusing a repeated id as the reference's `fig` raises on one.
fn fig(
    r: &mut TestResult,
    name: &str,
    value: Value,
    unit: Unit,
    definition: &str,
    evidence: Vec<EvidenceRef>,
) -> Result<String> {
    let id = format!("{TEST_ID}.{name}");
    if r.figures.iter().any(|f| f.id == id) {
        return Err(AuditError::Config(format!(
            "{TEST_ID}: figure id {id} would repeat"
        )));
    }
    Ok(r.fig(name, value, unit, definition, evidence))
}
pub const VERSION: &str = "1";

const SUPPLY_SECTIONS: [&str; 8] = [
    "194C", "194Q", "194J", "194H", "194I", "194-I", "194O", "194-O",
];
const INTEREST_SECTIONS: [&str; 1] = ["194A"];
const SALES_GROUP: &str = "Sales Accounts";
const INCOME_GROUPS: [&str; 2] = ["Direct Incomes", "Indirect Incomes"];
const TOL_PAISE: i64 = 100;
const BILLED_TYPES: [&str; 3] = ["Sales", "Debit Note", "Credit Note"];

/// The reference's `_sec`: spaces removed, then Python's `upper()`.
fn sec(section: &str) -> String {
    py_upper(&section.replace(' ', ""))
}

fn class_of(section: &str) -> &'static str {
    let s = sec(section);
    if SUPPLY_SECTIONS.contains(&s.as_str()) {
        "supply"
    } else if INTEREST_SECTIONS.contains(&s.as_str()) {
        "interest"
    } else {
        "other"
    }
}

fn sum_paise(mut values: impl Iterator<Item = i64>) -> Result<i64> {
    values
        .try_fold(0_i64, |a, b| a.checked_add(b))
        .ok_or_else(|| overflow(TEST_ID))
}

fn doc_ref(a: &Form26asRow) -> EvidenceRef {
    EvidenceRef::with_label(
        "document_row",
        &format!("{}#{}", a.doc, a.row),
        &format!("{} {} {}", a.deductor_tan, a.section, iso(&a.txn_date)),
    )
}

/// This module's own voucher label: `"<type> <number> on <date>"` with the number as read, even
/// when empty (the reference writes `v.number` here, not the GUID fallback other modules use).
fn voucher_ref(v: &Voucher) -> EvidenceRef {
    EvidenceRef::with_label(
        "voucher",
        &v.guid,
        &format!("{} {} on {}", v.vtype, v.number, iso(&v.date)),
    )
}

/// The reference's `vev`: a set of voucher refs, sorted by id.
fn voucher_refs(vs: &[&Voucher]) -> Vec<EvidenceRef> {
    let mut out: Vec<EvidenceRef> = vs.iter().map(|v| voucher_ref(v)).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.label.cmp(&b.label)));
    out.dedup();
    out
}

/// What the books record from `party`: (billed, its vouchers, income, its vouchers).
fn books_side<'a>(
    book: &Book,
    pop: &[&'a Voucher],
    party: &str,
) -> Result<(i64, Vec<&'a Voucher>, i64, Vec<&'a Voucher>)> {
    let (mut billed, mut billed_v, mut income, mut income_v) =
        (0_i64, Vec::new(), 0_i64, Vec::new());
    for &v in pop {
        if !v.lines.iter().any(|l| l.ledger == party) {
            continue;
        }
        for l in &v.lines {
            let Some(led) = book.ledgers.get(&l.ledger) else {
                continue;
            };
            let amount = l
                .amount_paise
                .checked_neg()
                .ok_or_else(|| overflow(TEST_ID))?;
            if BILLED_TYPES.contains(&v.base_type.as_str()) && led.under(SALES_GROUP) {
                billed = billed
                    .checked_add(amount)
                    .ok_or_else(|| overflow(TEST_ID))?;
                billed_v.push(v);
            } else if INCOME_GROUPS.iter().any(|g| led.under(g)) {
                income = income
                    .checked_add(amount)
                    .ok_or_else(|| overflow(TEST_ID))?;
                income_v.push(v);
            }
        }
    }
    Ok((billed, billed_v, income, income_v))
}

pub fn run(
    book: &Book,
    rules: &Rules,
    form26as: &[Form26asRow],
    deductor_aliases: &BTreeMap<String, String>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = "Form 26AS Part I rows whose deductor TAN the client configuration maps \
to a books ledger; books population (optional, cancelled and post-dated vouchers excluded)."
        .to_string();
    // party -> (supply, interest, other) rows, in 26AS order within each.
    let mut by_party: BTreeMap<&str, [Vec<&Form26asRow>; 3]> = BTreeMap::new();
    for a in form26as {
        if a.part != "I" {
            continue;
        }
        let Some(party) = deductor_aliases
            .get(&a.deductor_tan)
            .filter(|p| !p.is_empty())
        else {
            continue;
        };
        let slot = match class_of(&a.section) {
            "supply" => 0,
            "interest" => 1,
            _ => 2,
        };
        by_party.entry(party.as_str()).or_default()[slot].push(a);
    }

    let pop = book.population()?;
    for (party, groups) in &by_party {
        let h = stable_ledger_tag(book, party)?;
        let (billed, billed_v, income, income_v) = books_side(book, &pop, party)?;
        let led_ref = EvidenceRef::with_label("ledger", party, party);
        for (cls, slot, books_amt, books_ev, what) in [
            ("supply", 0, billed, &billed_v, "billed to"),
            ("interest", 1, income, &income_v, "income credited from"),
        ] {
            let rs = &groups[slot];
            if rs.is_empty() {
                continue;
            }
            let amt26 = sum_paise(rs.iter().map(|a| a.amount_paise))?;
            let secs = rs
                .iter()
                .map(|a| a.section.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            let ev26: Vec<EvidenceRef> = rs.iter().map(|a| doc_ref(a)).collect();
            let with_ledger = |ev: &[EvidenceRef]| {
                let mut out = ev.to_vec();
                out.push(led_ref.clone());
                out
            };
            let f26 = fig(
                &mut r,
                &format!("{cls}_26as_amount_{h}"),
                Value::Int(amt26),
                Unit::Paise,
                &format!("Form 26AS amount paid/credited by this party under section(s) {secs}."),
                with_ledger(&ev26),
            )?;
            let vev = voucher_refs(books_ev);
            let fb = fig(
                &mut r,
                &format!("{cls}_books_amount_{h}"),
                Value::Int(books_amt),
                Unit::Paise,
                &if cls == "supply" {
                    format!("Books: taxable value {what} this party in the year.")
                } else {
                    format!("Books: {what} this party in the year (Direct/Indirect Incomes lines).")
                },
                with_ledger(&vev),
            )?;
            let diff = amt26
                .checked_sub(books_amt)
                .ok_or_else(|| overflow(TEST_ID))?;
            let fd = fig(
                &mut r,
                &format!("{cls}_difference_{h}"),
                Value::Int(diff),
                Unit::Paise,
                "Form 26AS amount less the books amount.",
                vec![led_ref.clone()],
            )?;
            if diff.checked_abs().ok_or_else(|| overflow(TEST_ID))? > TOL_PAISE {
                let mut evidence = ev26.clone();
                evidence.extend(vev.iter().cloned());
                evidence.push(led_ref.clone());
                r.findings.push(Finding {
                    id: format!("{TEST_ID}/{cls}/{h}"),
                    clauses: vec!["s.199".to_string()],
                    title: if cls == "supply" {
                        "Form 26AS reports a different amount paid or credited by a customer than \
the books bill it"
                    } else {
                        "Form 26AS reports interest paid or credited by a party that the books do \
not match"
                    }
                    .to_string(),
                    facts: vec![
                        ("amount_26as".to_string(), f26),
                        ("amount_books".to_string(), fb),
                        ("difference".to_string(), fd),
                    ],
                    evidence,
                    confidence: Confidence::NeedsDocument,
                    limits: vec![
                        "Tax on a supply falls due on credit or payment, whichever is earlier, so \
part of a difference can belong to an invoice of the previous or next year."
                            .to_string(),
                        "Credit for tax deducted is allowed in the year the related income is \
offered (s.199); a 26AS amount with no matching income is a question about the income, not only the \
credit."
                            .to_string(),
                    ],
                    ask_client: vec![
                        "Obtain the deductor's own ledger or TDS certificate (Form 16A) for the \
rows that differ, and explain each."
                            .to_string(),
                        "State whether any invoice or income behind those rows is outside the \
books."
                            .to_string(),
                    ],
                });
            }
        }
        let other = &groups[2];
        if !other.is_empty() {
            fig(
                &mut r,
                &format!("other_sections_26as_amount_{h}"),
                Value::Int(sum_paise(other.iter().map(|a| a.amount_paise))?),
                Unit::Paise,
                "Form 26AS amount under sections that are not a supply or interest (not compared).",
                vec![led_ref.clone()],
            )?;
        }
    }
    Ok(r)
}

/// The reference's `check_invariants` (TR-1..TR-4), independent of `run`'s accumulators: each
/// 26AS-amount figure against the rows its own evidence names, each row's part and section
/// against the figure's bucket, no row claimed twice, and each books-amount figure against a
/// fresh walk of the population for the party in its own ledger evidence.
pub fn check_invariants(
    book: &Book,
    form26as: &[Form26asRow],
    result: &TestResult,
) -> Result<Vec<String>> {
    let prefix = format!("{}.", result.test_id);
    let doc_index: BTreeMap<String, &Form26asRow> = form26as
        .iter()
        .map(|a| (format!("{}#{}", a.doc, a.row), a))
        .collect();
    let mut out = Vec::new();
    let mut claim_owner: BTreeMap<&str, &str> = BTreeMap::new();
    let int = |v: &Value| match v {
        Value::Int(n) => Some(*n),
        _ => None,
    };
    let shown = |v: &Value| match v {
        Value::Int(n) => n.to_string(),
        Value::Undefined => "None".to_string(),
        Value::Text(t) => t.clone(),
    };

    for fig in &result.figures {
        let fid = fig.id.as_str();
        let cls = if fid.starts_with(&format!("{prefix}supply_26as_amount_")) {
            "supply"
        } else if fid.starts_with(&format!("{prefix}interest_26as_amount_")) {
            "interest"
        } else {
            continue;
        };
        let mut rows = Vec::new();
        for e in fig.evidence.iter().filter(|e| e.kind == "document_row") {
            let Some(a) = doc_index.get(&e.id) else {
                out.push(format!(
                    "TR-1: {fid} evidence {} does not resolve to a Form 26AS row",
                    e.id
                ));
                continue;
            };
            rows.push(*a);
            if let Some(prior) = claim_owner.get(e.id.as_str()) {
                if *prior != fid {
                    out.push(format!(
                        "TR-3: 26AS row {} is claimed as evidence by both {prior} and {fid}",
                        e.id
                    ));
                }
            }
            claim_owner.insert(e.id.as_str(), fid);
        }
        let total = sum_paise(rows.iter().map(|a| a.amount_paise))?;
        if int(&fig.value) != Some(total) {
            out.push(format!(
                "TR-1: {fid} = {} but the sum of its own referenced 26AS rows is {total}",
                shown(&fig.value)
            ));
        }
        for a in rows {
            if a.part != "I" {
                out.push(format!(
                    "TR-2: {fid} references a Part {} row, not Part I",
                    a.part
                ));
                continue;
            }
            let s = sec(&a.section);
            if cls == "supply" && !SUPPLY_SECTIONS.contains(&s.as_str()) {
                out.push(format!(
                    "TR-2: {fid} (supply) references section {}, not a supply section",
                    a.section
                ));
            } else if cls == "interest" && !INTEREST_SECTIONS.contains(&s.as_str()) {
                out.push(format!(
                    "TR-2: {fid} (interest) references section {}, not s.194A",
                    a.section
                ));
            }
        }
    }

    let pop = book.population()?;
    for fig in &result.figures {
        let fid = fig.id.as_str();
        let supply = if fid.starts_with(&format!("{prefix}supply_books_amount_")) {
            true
        } else if fid.starts_with(&format!("{prefix}interest_books_amount_")) {
            false
        } else {
            continue;
        };
        let party = fig
            .evidence
            .iter()
            .find(|e| e.kind == "ledger")
            .map(|e| e.id.as_str());
        let Some(party) = party.filter(|p| book.ledgers.contains_key(*p)) else {
            out.push(format!(
                "TR-4: {fid} carries no resolvable ledger evidence for its party"
            ));
            continue;
        };
        let (billed, _, income, _) = books_side(book, &pop, party)?;
        let recomputed = if supply { billed } else { income };
        if int(&fig.value) != Some(recomputed) {
            out.push(format!(
                "TR-4: {fid} = {} but a fresh population walk for {} finds {recomputed}",
                shown(&fig.value),
                crate::support::py_repr_str(party)
            ));
        }
    }
    Ok(out)
}
