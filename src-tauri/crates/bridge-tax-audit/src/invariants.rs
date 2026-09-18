//! Invariants: properties that must hold before a figure leaves the engine (the reference
//! Python implementation's own invariants module). Each re-derives what it needs from the book,
//! never from a test's own computation. Codes and violation strings match the reference exactly,
//! because the parity dump compares them.
//!
//! POL-1 is not here: the reference implementation calls it an observation, not a gate, and the
//! parity dump excludes it.

use std::collections::{BTreeMap, BTreeSet};

use crate::book::{Book, TbRow, VoucherStatus};
use crate::error::Result;
use crate::findings::TestResult;
use crate::read::iso;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub invariant: String,
    pub subject: String,
    pub detail: String,
}

fn violation(invariant: &str, subject: &str, detail: String) -> Violation {
    Violation {
        invariant: invariant.to_string(),
        subject: subject.to_string(),
        detail,
    }
}

/// POP-1 tolerance: the TB tie tolerates one rupee.
const TIE_TOLERANCE_PAISE: i64 = 100;

/// The book invariants, in the reference's `BOOK_INVARIANTS` order: the codes evaluated, and
/// every violation. Like the reference, this refuses (through `population`) when a voucher's
/// status is unknown, so POP-0 can only ever report on a book that is already refused.
pub fn book_invariants(book: &Book) -> Result<(Vec<&'static str>, Vec<Violation>)> {
    let mut out = Vec::new();

    // ID-1 (Bridge ADR 0002): the book is bound to a company GUID.
    if book.company_guid.is_empty() {
        out.push(violation(
            "ID-1",
            &book.company_name,
            "no company GUID".to_string(),
        ));
    }

    // POP-0: every voucher's status is established.
    for v in &book.vouchers {
        if v.status == VoucherStatus::Unknown {
            out.push(violation("POP-0", &v.guid, "status unknown".to_string()));
        }
    }

    let population = book.population()?;

    // POP-1: per ledger, the sum of in-books voucher lines equals TB closing minus opening.
    let mut movement: BTreeMap<&str, i64> = BTreeMap::new();
    for v in &population {
        for l in &v.lines {
            *movement.entry(l.ledger.as_str()).or_default() += l.amount_paise;
        }
    }
    let names: BTreeSet<&str> = movement
        .keys()
        .copied()
        .chain(book.tb.keys().map(String::as_str))
        .collect();
    for name in names {
        let want = book.tb.get(name).map_or(0, TbRow::movement_paise);
        let got = movement.get(name).copied().unwrap_or(0);
        if (got - want).abs() > TIE_TOLERANCE_PAISE {
            out.push(violation(
                "POP-1",
                name,
                format!("vouchers {got} vs TB movement {want} paise"),
            ));
        }
    }

    // POP-2: every in-books voucher's lines sum to zero.
    for v in &population {
        let sum: i64 = v.lines.iter().map(|l| l.amount_paise).sum();
        if !v.lines.is_empty() && sum.abs() > 1 {
            out.push(violation(
                "POP-2",
                &v.guid,
                format!("{} {} {} lines sum {sum}", v.vtype, v.number, iso(&v.date)),
            ));
        }
    }

    // POP-3: TB opening and closing each sum to zero.
    for (key, sum) in [
        (
            "opening_paise",
            book.tb.values().map(|t| t.opening_paise).sum::<i64>(),
        ),
        (
            "closing_paise",
            book.tb.values().map(|t| t.closing_paise).sum::<i64>(),
        ),
    ] {
        if sum.abs() > TIE_TOLERANCE_PAISE {
            out.push(violation(
                "POP-3",
                key,
                format!("TB {key} sums to {sum} paise"),
            ));
        }
    }

    // MAP-1: every ledger with vouchers resolves to a primary group.
    let used: BTreeSet<&str> = population
        .iter()
        .flat_map(|v| v.lines.iter().map(|l| l.ledger.as_str()))
        .collect();
    for name in used {
        if let Some(ledger) = book.ledgers.get(name) {
            if !ledger.chain_complete {
                out.push(violation(
                    "MAP-1",
                    name,
                    format!("chain incomplete: {}", ledger.chain.join(" > ")),
                ));
            }
        }
    }

    Ok((
        vec!["ID-1", "POP-0", "POP-1", "POP-2", "POP-3", "MAP-1"],
        out,
    ))
}

/// The three result-level invariants, evaluated with this one result (REND-0, EVID-1, POP-4).
pub fn result_invariants(book: &Book, result: &TestResult) -> (Vec<&'static str>, Vec<Violation>) {
    let mut out = Vec::new();

    // REND-0: every fact in a finding points at a figure that exists.
    let figures: BTreeSet<&str> = result.figures.iter().map(|f| f.id.as_str()).collect();
    for finding in &result.findings {
        for (name, figure_id) in &finding.facts {
            if !figures.contains(figure_id.as_str()) {
                out.push(violation(
                    "REND-0",
                    &finding.id,
                    format!("fact {name} -> missing figure {figure_id}"),
                ));
            }
        }
    }

    let refs: Vec<_> = result
        .figures
        .iter()
        .flat_map(|f| f.evidence.iter())
        .chain(result.findings.iter().flat_map(|f| f.evidence.iter()))
        .collect();

    // EVID-1: every voucher or ledger evidence ref resolves in the book.
    let guids: BTreeSet<&str> = book.vouchers.iter().map(|v| v.guid.as_str()).collect();
    for e in &refs {
        let unresolved = (e.kind == "voucher" && !guids.contains(e.id.as_str()))
            || (e.kind == "ledger" && !book.ledgers.contains_key(&e.id));
        if unresolved {
            out.push(violation(
                "EVID-1",
                &result.test_id,
                format!("unresolved {}", e.key()),
            ));
        }
    }

    // POP-4: no figure or finding cites a voucher outside the books population.
    let excluded: BTreeSet<&str> = book.excluded().map(|v| v.guid.as_str()).collect();
    for e in &refs {
        if e.kind == "voucher" && excluded.contains(e.id.as_str()) {
            out.push(violation("POP-4", &result.test_id, e.key()));
        }
    }

    (vec!["REND-0", "EVID-1", "POP-4"], out)
}
