//! s.43B(h) creditor ageing: a FIFO reconstruction of each creditor's closing balance into dated
//! lots, because bill-wise tracking is off, so Tally cannot report per-invoice ageing directly. A
//! port of the reference Python implementation's `creditor_ageing_43bh` test module, version 2.
//!
//! * An opening CREDIT balance (TB opening, Dr+/Cr-) becomes one lot dated the period start; an
//!   opening DEBIT balance becomes an advance.
//! * Population lines on the creditor are walked in date order; on one day, bills (credit lines)
//!   come before payments (debit lines).
//! * A bill first consumes any outstanding advance, and the remainder opens a lot. A payment
//!   consumes the oldest open lots first; what is left after every lot is exhausted becomes an
//!   advance. A lot is never negative.
//! * Age is `as_of` (the period end) less (lot date + `acceptance_lag_days`), in buckets 0-15,
//!   16-45, 46-90 and over 90 days.
//!
//! Whether s.43B(h) reaches a creditor at all depends on its Udyam classification, which is
//! client configuration and never inferred: an unclassified creditor is reported as needing the
//! certificate and is kept out of every disallowance-candidate total. `check_invariants` is AGE-1,
//! which re-derives what is owed from the Trial Balance alone.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;

use crate::book::{Book, Voucher};
use crate::depreciation::civil_day_number;
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::{iso, Window};
use crate::rules::Rules;
use crate::support;

pub const TEST_ID: &str = "creditor_ageing_43bh";
pub const VERSION: &str = "2";

const BUCKET_NAMES: [&str; 4] = [
    "bucket_0_15",
    "bucket_16_45",
    "bucket_46_90",
    "bucket_over_90",
];

/// Supplier classifications, per the Udyam Registration Certificate. "unknown" is the default for
/// any creditor the client configuration does not tag.
pub const CLASSIFICATIONS: [&str; 6] = [
    "micro",
    "small",
    "medium",
    "trader",
    "not_registered",
    "unknown",
];

fn applicable(classification: &str) -> bool {
    matches!(classification, "micro" | "small")
}

fn bucket(age_days: i64) -> usize {
    if age_days <= 15 {
        0
    } else if age_days <= 45 {
        1
    } else if age_days <= 90 {
        2
    } else {
        3
    }
}

/// The Python repr of a tuple of strings, for the reference's own refusal message.
fn py_tuple_repr(items: &[&str]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("({})", inner.join(", "))
}

fn classify<'a>(
    name: &str,
    supplier_classification: &'a BTreeMap<String, String>,
) -> Result<&'a str> {
    let cls = supplier_classification
        .get(name)
        .map_or("unknown", String::as_str);
    if !CLASSIFICATIONS.contains(&cls) {
        return Err(AuditError::Config(format!(
            "supplier_classification[{name:?}] = {cls:?} is not one of {}",
            py_tuple_repr(&CLASSIFICATIONS)
        )));
    }
    Ok(cls)
}

/// One open FIFO lot: its day number (see [`civil_day_number`]) and remaining paise, over zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lot {
    pub day: i64,
    pub paise: i64,
}

/// One creditor's reconstruction: open lots oldest-first, residual advance, and the opening
/// lot's own remaining paise (0 if there was none, or it was paid down).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    pub lots: Vec<Lot>,
    pub advance: i64,
    pub opening_remaining: i64,
}

/// Reconstruct one creditor's open FIFO lots and residual advance from its population lines.
/// `lines` are `(day, amount_paise)` in the Dr+/Cr- convention, in any order: they are sorted
/// here, stably, by day and then bills before payments, as the reference sorts them.
///
/// The opening lot is always the first lot pushed and payments only ever consume from the front,
/// so it is the front lot for as long as any of it remains; that is how its remainder is tracked,
/// never by matching its date (an in-year bill can fall on the period start too).
pub fn walk_creditor(period_start: i64, opening_paise: i64, lines: &[(i64, i64)]) -> Result<Walk> {
    let overflow = || support::overflow(TEST_ID);
    let mut lots: std::collections::VecDeque<Lot> = std::collections::VecDeque::new();
    let mut advance: i64 = 0;
    let mut opening_live = false;
    if opening_paise < 0 {
        lots.push_back(Lot {
            day: period_start,
            paise: opening_paise.checked_neg().ok_or_else(overflow)?,
        });
        opening_live = true;
    } else if opening_paise > 0 {
        advance = opening_paise;
    }
    let mut sorted: Vec<(i64, i64)> = lines.to_vec();
    sorted.sort_by_key(|&(day, amt)| (day, amt > 0));
    for (day, amt) in sorted {
        if amt < 0 {
            let mag = amt.checked_neg().ok_or_else(overflow)?;
            let used = mag.min(advance);
            advance -= used;
            let rem = mag - used;
            if rem > 0 {
                lots.push_back(Lot { day, paise: rem });
            }
        } else if amt > 0 {
            let mut pay = amt;
            while pay > 0 {
                let Some(front) = lots.front_mut() else {
                    break;
                };
                let take = pay.min(front.paise);
                front.paise -= take;
                pay -= take;
                if front.paise == 0 {
                    lots.pop_front();
                    opening_live = false;
                }
            }
            advance = advance.checked_add(pay).ok_or_else(overflow)?;
        }
    }
    let opening_remaining = if opening_live {
        lots.front().map_or(0, |l| l.paise)
    } else {
        0
    };
    Ok(Walk {
        lots: lots.into_iter().collect(),
        advance,
        opening_remaining,
    })
}

/// What a next-year payment walk says about one lot open at `as_of`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostYear {
    pub day: i64,
    pub settled_paise: i64,
    /// The payment day that last reduced this lot to zero, if one did.
    pub settled_day: Option<i64>,
    pub remaining_paise: i64,
}

/// Continue the FIFO walk past `as_of` using only next-year payment lines (sorted by day,
/// stably), to see whether a lot still open at year end was settled inside its window. With no
/// lines, every lot comes back wholly unresolved.
pub fn apply_post_year_payments(lots: &[Lot], post_year_lines: &[(i64, i64)]) -> Vec<PostYear> {
    let mut work: Vec<i64> = lots.iter().map(|l| l.paise).collect();
    let mut out: Vec<PostYear> = lots
        .iter()
        .map(|l| PostYear {
            day: l.day,
            settled_paise: 0,
            settled_day: None,
            remaining_paise: 0,
        })
        .collect();
    let mut sorted = post_year_lines.to_vec();
    sorted.sort_by_key(|&(day, _)| day);
    for (pday, pamt) in sorted {
        let mut pay = pamt;
        for (i, lot) in work.iter_mut().enumerate() {
            if pay <= 0 {
                break;
            }
            if *lot <= 0 {
                continue;
            }
            let take = pay.min(*lot);
            *lot -= take;
            pay -= take;
            out[i].settled_paise += take;
            if *lot == 0 {
                out[i].settled_day = Some(pday);
            }
        }
    }
    for (i, lot) in work.iter().enumerate() {
        out[i].remaining_paise = *lot;
    }
    out
}

/// One creditor's population lines and the vouchers carrying them.
struct Rows<'a> {
    walk: Walk,
    vouchers: BTreeMap<&'a str, &'a Voucher>,
    has_tb_row: bool,
    bills_in_year_paise: i64,
}

fn compute_ageing<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    creditors: &BTreeSet<String>,
    period_start: i64,
) -> Result<BTreeMap<String, Rows<'a>>> {
    let overflow = || support::overflow(TEST_ID);
    let mut lines: BTreeMap<&str, Vec<(i64, i64)>> =
        creditors.iter().map(|n| (n.as_str(), Vec::new())).collect();
    let mut vouchers: BTreeMap<&str, BTreeMap<&'a str, &'a Voucher>> = creditors
        .iter()
        .map(|n| (n.as_str(), BTreeMap::new()))
        .collect();
    for v in pop {
        let day = civil_day_number(&v.date);
        for l in &v.lines {
            if l.amount_paise == 0 {
                continue;
            }
            if let Some(ls) = lines.get_mut(l.ledger.as_str()) {
                ls.push((day, l.amount_paise));
                if let Some(vs) = vouchers.get_mut(l.ledger.as_str()) {
                    vs.insert(v.guid.as_str(), *v);
                }
            }
        }
    }
    let mut rows = BTreeMap::new();
    for name in creditors {
        let tb_row = book.tb.get(name);
        let opening_paise = tb_row.map_or(0, |t| t.opening_paise);
        let ls = &lines[name.as_str()];
        let walk = walk_creditor(period_start, opening_paise, ls)?;
        let bills_in_year_paise =
            ls.iter()
                .filter(|(_, a)| *a < 0)
                .try_fold(0i64, |acc, (_, a)| {
                    acc.checked_add(a.checked_neg().ok_or_else(overflow)?)
                        .ok_or_else(overflow)
                })?;
        rows.insert(
            name.clone(),
            Rows {
                walk,
                vouchers: vouchers.remove(name.as_str()).unwrap_or_default(),
                has_tb_row: tb_row.is_some(),
                bills_in_year_paise,
            },
        );
    }
    Ok(rows)
}

/// `s43b_h` rule values: (days with a written agreement, days without one).
fn s43b_h_days(rules: &Rules) -> Result<(i64, i64)> {
    rules
        .s43b_h_msme_days
        .ok_or_else(|| AuditError::Config("rules: no [s43b_h] table".to_string()))
}

/// The parameters the reference's `run()` takes beyond the book and the creditor set.
#[derive(Debug, Clone, Default)]
pub struct Params {
    pub acceptance_lag_days: i64,
    pub supplier_classification: BTreeMap<String, String>,
    /// Next-year payment lines per creditor, `(date, paise)`. The reference's pack never passes
    /// any; its `run()` takes them, so this port does too.
    pub post_year_payments: BTreeMap<String, Vec<(TallyDate, i64)>>,
    pub mse_interest_ledgers: BTreeSet<String>,
}

#[allow(clippy::too_many_lines)]
pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    creditors: &BTreeSet<String>,
    params: &Params,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let overflow = || support::overflow(TEST_ID);
    let add = |a: i64, b: i64| a.checked_add(b).ok_or_else(overflow);
    let lag = params.acceptance_lag_days;
    let as_of_iso = iso(&period.to);
    let as_of = civil_day_number(&period.to);
    let period_start = civil_day_number(&period.from);
    let (over_45_days, over_15_days) = s43b_h_days(rules)?;
    let lag_note = format!(
        "Age is computed from bill date + acceptance_lag_days ({lag} day{}), an assumption that \
acceptance or deemed acceptance of the goods/services (MSMED s.2(b)) happened this long after the \
bill date; the true acceptance date is not in these books -- confirm against GRNs/delivery \
records.",
        if lag == 1 { "" } else { "s" }
    );
    let pop = book.population()?;
    let pop_note = "Books population (optional, cancelled and post-dated vouchers excluded).";
    r.population_note = pop_note.to_string();

    let rows = compute_ageing(&pop, book, creditors, period_start)?;
    let no_tb_row: Vec<&String> = rows
        .iter()
        .filter(|(_, d)| !d.has_tb_row)
        .map(|(n, _)| n)
        .collect();

    let mut bucket_totals = [0i64; 4];
    let (mut reconstructed_total, mut advances_total, mut over_45_total) = (0i64, 0i64, 0i64);
    let mut over_15_total = 0i64;
    let mut over_45_creditor_count = 0usize;
    let mut clause22_ii_total = 0i64;
    let mut clause22_iii_b_45day_total = 0i64;
    let mut clause22_iii_b_15day_total = 0i64;
    let mut unknown_classification_total = 0i64;
    let mut unknown_classification_creditor_count = 0usize;
    let mut post_year_confirmed_breach_total = 0i64;
    let mut post_year_pending_confirmation_total = 0i64;
    let mut opening_dues_26a_candidate_total = 0i64;
    let mut opening_dues_26a_candidate_creditor_count = 0usize;
    let mut opening_dues_26a_evidence: Vec<EvidenceRef> = Vec::new();

    for (name, d) in &rows {
        let h = stable_ledger_tag(book, name)?;
        let classification = classify(name, &params.supplier_classification)?;
        let mut creditor_ev = vec![EvidenceRef::new("ledger", name)];
        creditor_ev.extend(
            d.vouchers
                .iter()
                .map(|(g, v)| EvidenceRef::with_label("voucher", g, &support::voucher_label(v))),
        );
        let creditor_recon = d
            .walk
            .lots
            .iter()
            .try_fold(0i64, |acc, l| add(acc, l.paise))?;
        reconstructed_total = add(reconstructed_total, creditor_recon)?;
        advances_total = add(advances_total, d.walk.advance)?;
        let f_recon = r.fig(
            &format!("creditor_reconstructed_{h}"),
            Value::Int(creditor_recon),
            Unit::Paise,
            &format!("Sum of open FIFO lots for one creditor ledger (tag {h}) at {as_of_iso}."),
            creditor_ev.clone(),
        );
        let f_adv = r.fig(
            &format!("creditor_advance_{h}"),
            Value::Int(d.walk.advance),
            Unit::Paise,
            &format!(
                "Residual advance (payments in excess of open lots) for one creditor ledger (tag \
{h}) at {as_of_iso}."
            ),
            creditor_ev.clone(),
        );

        if applicable(classification) {
            clause22_ii_total = add(clause22_ii_total, d.bills_in_year_paise)?;
        }

        let opening_remaining = d.walk.opening_remaining;
        let opening_age = as_of - add(period_start, lag)?;
        let opening_over_45 = if opening_age > over_45_days {
            opening_remaining
        } else {
            0
        };
        let opening_over_15 = if opening_age > over_15_days {
            opening_remaining
        } else {
            0
        };

        let (mut creditor_over_45, mut creditor_over_15) = (0i64, 0i64);
        let mut not_yet_over_15: Vec<Lot> = Vec::new();
        for lot in &d.walk.lots {
            let age = as_of - add(lot.day, lag)?;
            let b = bucket(age);
            bucket_totals[b] = add(bucket_totals[b], lot.paise)?;
            if age > over_45_days {
                creditor_over_45 = add(creditor_over_45, lot.paise)?;
            }
            if age > over_15_days {
                creditor_over_15 = add(creditor_over_15, lot.paise)?;
                over_15_total = add(over_15_total, lot.paise)?;
            } else {
                not_yet_over_15.push(*lot);
            }
        }
        if creditor_over_45 > 0 {
            over_45_creditor_count += 1;
            over_45_total = add(over_45_total, creditor_over_45)?;
        }

        let creditor_over_45_clause22 = creditor_over_45 - opening_over_45;
        let creditor_over_15_clause22 = creditor_over_15 - opening_over_15;

        if applicable(classification) && opening_remaining > 0 {
            opening_dues_26a_candidate_total =
                add(opening_dues_26a_candidate_total, opening_remaining)?;
            opening_dues_26a_candidate_creditor_count += 1;
            opening_dues_26a_evidence.extend(creditor_ev.iter().cloned());
        }

        if applicable(classification) {
            if creditor_over_45_clause22 > 0 || creditor_over_15_clause22 > 0 {
                let f_o45 = r.fig(
                    &format!("creditor_over45_{h}"),
                    Value::Int(creditor_over_45_clause22),
                    Unit::Paise,
                    &format!(
                        "Sum of one creditor ledger's (tag {h}) FIFO lots older than \
{over_45_days} days as of {as_of_iso}, EXCLUDING the opening balance's own lot (GN 42.25 -- see \
opening_dues_26a_candidate_total). {lag_note}"
                    ),
                    creditor_ev.clone(),
                );
                let f_o15 = r.fig(
                    &format!("creditor_over15_{h}"),
                    Value::Int(creditor_over_15_clause22),
                    Unit::Paise,
                    &format!(
                        "Sum of one creditor ledger's (tag {h}) FIFO lots older than \
{over_15_days} days as of {as_of_iso} (no-written-agreement view), EXCLUDING the opening \
balance's own lot (GN 42.25). {lag_note}"
                    ),
                    creditor_ev.clone(),
                );
                clause22_iii_b_45day_total =
                    add(clause22_iii_b_45day_total, creditor_over_45_clause22)?;
                clause22_iii_b_15day_total =
                    add(clause22_iii_b_15day_total, creditor_over_15_clause22)?;
                r.findings.push(Finding {
                    id: format!("{TEST_ID}/over45/{h}"),
                    clauses: vec!["s.43B(h)".to_string(), "3CD-22(iii)(b)".to_string()],
                    title: format!(
                        "Creditor ledger has FIFO-reconstructed lots older than the MSME limit as \
of {as_of_iso}"
                    ),
                    facts: vec![
                        ("over_15_amount".to_string(), f_o15),
                        ("with_agreement_over_45_amount".to_string(), f_o45),
                        ("reconstructed_total".to_string(), f_recon.clone()),
                        ("advance".to_string(), f_adv.clone()),
                    ],
                    evidence: creditor_ev.clone(),
                    confidence: Confidence::Indicative,
                    limits: vec![
                        "Not bill-wise: ISBILLWISEON is off (or unknown) for this ledger, so this \
ages a FIFO reconstruction of ledger postings, not Tally's own bill-by-bill outstanding report."
                            .to_string(),
                        format!(
                            "Supplier classified '{classification}' by client config (Udyam \
Registration Certificate; GN 42.7 -- never recomputed from financials here), so s.43B(h) is taken \
to apply. With no written agreement on file the MSMED limit is 15 days (GN 42.14(b)), so the \
15-day, no-agreement figure is the default amount for this clause; the 45-day, with-agreement \
figure is shown beside it only as the alternative that applies once a written agreement (or \
invoice/purchase-order terms constituting one, GN 42.29) on this creditor's credit period is \
confirmed."
                        ),
                        lag_note.clone(),
                        "Excludes the opening (pre-year) balance's own lot, which is reported \
separately as a clause 26(i)(A) candidate, not counted here (GN 42.25: '22-1')."
                            .to_string(),
                        "This provision disallows the amount of a lot unpaid beyond the window at \
31 March for the year it was claimed (clause 22(iii)(b), GN 42.26(b)), and the s.139(1) \
return-due-date proviso available to ordinary s.43B items does NOT extend to s.43B(h) (GN 42.27, \
46.7). A payment recorded after 31 March, if it exists, is outside this FY's voucher population \
and cannot be seen from these books -- it does not, on its own, cure this disallowance."
                            .to_string(),
                    ],
                    ask_client: vec![
                        "Udyam registration certificate, confirming category and activity \
(micro/small vs medium vs trader), for this creditor."
                            .to_string(),
                        "Any written agreement on credit period with this creditor (the MSMED Act \
limit is 15 days without one, 45 days with one)."
                            .to_string(),
                        "Acceptance/deemed-acceptance date (GRN or delivery record) for each open \
bill, to replace the acceptance_lag_days assumption."
                            .to_string(),
                    ],
                });
            }
        } else if classification == "unknown" && (creditor_over_45 > 0 || creditor_over_15 > 0) {
            let f_o45 = r.fig(
                &format!("creditor_over45_{h}"),
                Value::Int(creditor_over_45),
                Unit::Paise,
                &format!(
                    "Sum of one creditor ledger's (tag {h}) FIFO lots older than {over_45_days} \
days as of {as_of_iso}. {lag_note}"
                ),
                creditor_ev.clone(),
            );
            let f_o15 = r.fig(
                &format!("creditor_over15_{h}"),
                Value::Int(creditor_over_15),
                Unit::Paise,
                &format!(
                    "Sum of one creditor ledger's (tag {h}) FIFO lots older than {over_15_days} \
days as of {as_of_iso} (no-written-agreement view). {lag_note}"
                ),
                creditor_ev.clone(),
            );
            unknown_classification_creditor_count += 1;
            unknown_classification_total = add(unknown_classification_total, creditor_over_15)?;
            r.findings.push(Finding {
                id: format!("{TEST_ID}/unknown_classification/{h}"),
                clauses: vec!["s.43B(h)".to_string()],
                title: "Creditor ledger has aged open lots but its MSME classification is not on \
file"
                    .to_string(),
                facts: vec![
                    ("over_15_amount".to_string(), f_o15),
                    ("with_agreement_over_45_amount".to_string(), f_o45),
                ],
                evidence: creditor_ev.clone(),
                confidence: Confidence::NeedsDocument,
                limits: vec![
                    "Whether s.43B(h) reaches this creditor at all depends on its Udyam \
registration category (micro/small vs medium) and activity (trader suppliers and unregistered \
suppliers are outside s.43B(h) -- GN 42.9, 42.6, 42.24); none of this is recorded in Tally, so \
this amount is excluded from every s.43B(h) disallowance-candidate total until classified, never \
assumed either way. This includes any opening-balance portion: whether that portion belongs to \
clause 22(iii)(b) or is excluded in favour of a clause 26(i)(A) candidate (GN 42.25, '22-1') \
itself depends on the same unresolved classification."
                        .to_string(),
                ],
                ask_client: vec![
                    "Udyam registration certificate for this creditor, or confirmation that none \
exists."
                        .to_string(),
                ],
            });
        }

        if applicable(classification) && !not_yet_over_15.is_empty() {
            let py_lines: Vec<(i64, i64)> = params
                .post_year_payments
                .get(name)
                .map(|ls| ls.iter().map(|(d, a)| (civil_day_number(d), *a)).collect())
                .unwrap_or_default();
            if py_lines.is_empty() {
                for lot in &not_yet_over_15 {
                    post_year_pending_confirmation_total =
                        add(post_year_pending_confirmation_total, lot.paise)?;
                }
            } else {
                for res in apply_post_year_payments(&not_yet_over_15, &py_lines) {
                    let Some(settled_day) = res.settled_day else {
                        post_year_pending_confirmation_total =
                            add(post_year_pending_confirmation_total, res.remaining_paise)?;
                        continue;
                    };
                    let settled_days = settled_day - add(res.day, lag)?;
                    if settled_days > over_15_days {
                        post_year_confirmed_breach_total =
                            add(post_year_confirmed_breach_total, res.settled_paise)?;
                    }
                    if res.remaining_paise > 0 {
                        post_year_pending_confirmation_total =
                            add(post_year_pending_confirmation_total, res.remaining_paise)?;
                    }
                }
            }
        }
    }

    for (i, b) in BUCKET_NAMES.iter().enumerate() {
        r.fig(
            b,
            Value::Int(bucket_totals[i]),
            Unit::Paise,
            &format!(
                "Sum of FIFO lots aged in this bucket as of {as_of_iso}. {lag_note} {pop_note}"
            ),
            Vec::new(),
        );
    }
    r.fig(
        "reconstructed_total",
        Value::Int(reconstructed_total),
        Unit::Paise,
        &format!("Sum of every creditor's open FIFO lots. {pop_note}"),
        Vec::new(),
    );
    r.fig(
        "advances_total",
        Value::Int(advances_total),
        Unit::Paise,
        &format!("Sum of every creditor's residual advance. {pop_note}"),
        Vec::new(),
    );
    r.fig(
        "over_45_total",
        Value::Int(over_45_total),
        Unit::Paise,
        &format!(
            "Books-fact sum of FIFO lots older than {over_45_days} days \
(rules.s43b_h.msme_days_with_agreement), across every creditor in scope regardless of MSME \
classification -- a raw ageing fact, not a s.43B(h) applicability conclusion (see \
clause22_iii_b_45day_total for the classification-scoped candidate). {lag_note}"
        ),
        Vec::new(),
    );
    r.fig(
        "over_15_total",
        Value::Int(over_15_total),
        Unit::Paise,
        &format!(
            "Books-fact sum of FIFO lots older than {over_15_days} days \
(rules.s43b_h.msme_days_without_agreement), same scope as over_45_total, shown beside it, never \
instead of it. {lag_note}"
        ),
        Vec::new(),
    );
    r.fig(
        "over_45_creditor_count",
        support::count(TEST_ID, over_45_creditor_count)?,
        Unit::Count,
        "Creditor ledgers with at least one FIFO lot older than the MSME limit (books fact, \
unconditional on classification).",
        Vec::new(),
    );
    if !no_tb_row.is_empty() {
        r.fig(
            "creditors_without_tb_row_count",
            support::count(TEST_ID, no_tb_row.len())?,
            Unit::Count,
            "Creditor ledgers in scope with no Trial Balance row at all (opening treated as nil).",
            no_tb_row
                .iter()
                .map(|n| EvidenceRef::new("ledger", n))
                .collect(),
        );
    }

    r.fig(
        "clause22_ii_total",
        Value::Int(clause22_ii_total),
        Unit::Paise,
        "3CD-22(ii): total amount required to be paid to micro/small (non-trader, non-medium, \
registered) enterprise suppliers during the year (GN 42.26(a)) -- every bill raised on a creditor \
classified micro/small in client config, whether paid during the year, outstanding at year end, \
or capitalised (a credit line to the creditor ledger either way). Suppliers classified 'unknown' \
are excluded (see the per-creditor NEEDS_DOCUMENT findings above), so this is a floor, not the \
true total, until every supplier is classified.",
        Vec::new(),
    );
    r.fig(
        "clause22_iii_b_45day_total",
        Value::Int(clause22_iii_b_45day_total),
        Unit::Paise,
        "3CD-22(iii)(b), with-written-agreement (45-day) view: claimed as a deduction, \
outstanding on the last day of the PY, and not paid within the s.15 MSMED time -- summed only \
over creditors classified micro/small (non-trader, registered) (CPC OI 11h <-> 22(iii)(b)). \
EXCLUDES the opening (pre-year) balance's own lot on every creditor (GN 42.25: a due of an earlier \
year is not a 22(iii)(b) amount -- see opening_dues_26a_candidate_total below). Shown beside the \
15-day figure, never in its place: with no written agreement on file the default amount for this \
clause is the 15-day figure (GN 42.14(b)).",
        Vec::new(),
    );
    r.fig(
        "clause22_iii_b_15day_total",
        Value::Int(clause22_iii_b_15day_total),
        Unit::Paise,
        "3CD-22(iii)(b), no-written-agreement (15-day) view -- the DEFAULT amount for this clause \
(GN 42.14(b): with no written agreement on file the MSMED limit is 15 days, not 45), same \
population as the 45-day figure above (including the same opening-lot exclusion), shown beside it \
because whether a written agreement exists is not in these books.",
        Vec::new(),
    );
    if opening_dues_26a_candidate_creditor_count > 0 {
        let f_amt = r.fig(
            "opening_dues_26a_candidate_total",
            Value::Int(opening_dues_26a_candidate_total),
            Unit::Paise,
            "3CD-22-1 (GN 42.25): the still-unpaid remainder of each micro/small creditor's \
OPENING (pre-year) balance -- excluded from every clause 22 figure above because it relates to a \
period earlier than this PY, not to this year's own claimed dues. Reported here as a candidate \
for clause 26(i)(A)(b) instead (GN 46.5: only a sum NOT allowable in an earlier year belongs in \
26(i)(A); that needs last year's own return/3CD, which this module does not have).",
            Vec::new(),
        );
        let f_count = r.fig(
            "opening_dues_26a_candidate_creditor_count",
            support::count(TEST_ID, opening_dues_26a_candidate_creditor_count)?,
            Unit::Count,
            "Micro/small creditor ledgers with a still-unpaid opening-balance remainder reported \
above.",
            Vec::new(),
        );
        // `dict.fromkeys`: first occurrence of each whole ref, in order.
        let mut evidence: Vec<EvidenceRef> = Vec::new();
        for e in opening_dues_26a_evidence {
            if !evidence.contains(&e) {
                evidence.push(e);
            }
        }
        r.findings.push(Finding {
            id: format!("{TEST_ID}/opening_dues_26a_candidate"),
            clauses: vec!["s.43B(h)".to_string(), "3CD-26(i)(A)(b)".to_string()],
            title: "Opening creditor balance on a micro/small supplier, still unpaid: a clause \
26(i)(A) candidate, not a clause 22 item"
                .to_string(),
            facts: vec![
                ("opening_dues_26a_candidate_total".to_string(), f_amt),
                (
                    "opening_dues_26a_candidate_creditor_count".to_string(),
                    f_count,
                ),
            ],
            evidence,
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "Excluded from clause 22 because it relates to a period earlier than this \
previous year (GN 42.25); it is only a clause 26(i)(A) item if it was NOT already allowable in an \
earlier year (GN 46.5) -- that requires last year's own income-tax return/Form 3CD, which this \
module does not have, so this amount is a candidate only, never asserted as this year's 26(i)(A) \
figure."
                    .to_string(),
            ],
            ask_client: vec![
                "Last year's income-tax return / Form 3CD (or working papers), to \
confirm whether this opening amount was already claimed as not allowable in an earlier year, and \
if so, in what amount."
                    .to_string(),
            ],
        });
    }
    if unknown_classification_creditor_count > 0 {
        r.fig(
            "unknown_classification_total",
            Value::Int(unknown_classification_total),
            Unit::Paise,
            "Sum of the (larger, 15-day-view) aged amount on creditors with an aged open lot but \
no MSME classification in client config -- pending Udyam evidence, excluded from every clause \
22(ii)/22(iii)(b) s.43B(h) total above until classified.",
            Vec::new(),
        );
        let f_unknown = r.fig(
            "unknown_classification_creditor_count",
            support::count(TEST_ID, unknown_classification_creditor_count)?,
            Unit::Count,
            "Creditor ledgers with an aged open lot and no MSME classification in client config.",
            Vec::new(),
        );
        r.findings.push(Finding {
            id: format!("{TEST_ID}/clause22_undetermined"),
            clauses: vec!["s.43B(h)".to_string(), "3CD-22(iii)(b)".to_string()],
            title: "Clause 22(iii)(b) cannot be settled: suppliers' MSME category is not on file"
                .to_string(),
            facts: vec![(
                "unknown_classification_creditor_count".to_string(),
                f_unknown,
            )],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "Only micro and small enterprise suppliers fall under s.43B(h); traders \
and medium enterprises do not (GN 42.9, 42.24). Until each creditor's category is known, the \
clause 22(iii)(b) amount is undetermined, not nil."
                    .to_string(),
            ],
            ask_client: vec![
                "Udyam registration certificate (or a written statement that the \
supplier is not registered) for each creditor with an open balance at 31 March."
                    .to_string(),
            ],
        });
    }
    r.fig(
        "post_year_confirmed_breach_total",
        Value::Int(post_year_confirmed_breach_total),
        Unit::Paise,
        "Sum of lots not yet past the MSME window as of 31 March (micro/small creditors only) \
that `post_year_payments` shows were actually settled after that window in the following year (or \
never settled) -- a disallowance for the claim year under audit, confirmed from next-year payment \
data, not assumed. Not folded into over_45_total/over_15_total/clause22_iii_b_* above (those are \
the at-31-March view only); add this figure to them for the full-year picture.",
        Vec::new(),
    );
    let f_pending = r.fig(
        "post_year_pending_confirmation_total",
        Value::Int(post_year_pending_confirmation_total),
        Unit::Paise,
        "Sum of lots not yet past the MSME window as of 31 March (micro/small creditors only) \
with no next-year payment data supplied at all -- whether the window was eventually met cannot be \
determined from these books; NEEDS_DOCUMENT, never assumed met or missed.",
        Vec::new(),
    );
    if post_year_pending_confirmation_total != 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/post_year_pending"),
            clauses: vec!["s.43B(h)".to_string()],
            title: "Creditor lots open at year end, still inside the MSME window as of 31 March, \
whose eventual payment date this book cannot show"
                .to_string(),
            facts: vec![("pending".to_string(), f_pending)],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "Paying within the MSME window after the year end is not a disallowance \
(GN 42.14); paying after the window elapses is, even though the window itself falls in the next \
FY. This FY's voucher population ends at 31 March, so this cannot be resolved without next year's \
payment vouchers or a challan."
                    .to_string(),
            ],
            ask_client: vec![
                "Payment date (bank statement or challan) for each such creditor's \
open balance, for the period just after 31 March."
                    .to_string(),
            ],
        });
    }

    let mut mse_interest_paise = 0i64;
    for v in &pop {
        for l in &v.lines {
            if l.amount_paise > 0 && params.mse_interest_ledgers.contains(&l.ledger) {
                mse_interest_paise = add(mse_interest_paise, l.amount_paise)?;
            }
        }
    }
    r.fig(
        "clause22_i_mse_interest_paise",
        Value::Int(mse_interest_paise),
        Unit::Paise,
        "3CD-22(i): s.16 MSMED interest debited to P&L (inadmissible under s.23), summed over \
ledgers client config tags as MSMED-interest ledgers (CPC OI 17 <-> 22(i)). NIL (0) if the \
auditee neither provided nor paid any such interest, per GN 42.21 -- not itself a finding that \
none is due; see the mercantile-system observation in GN 42.22 (a CA judgement, not computed \
here).",
        Vec::new(),
    );
    Ok(r)
}

/// AGE-1, independent of the walk: per creditor with a `creditor_reconstructed_<tag>` figure, a
/// nil or debit TB closing means nothing may be aged as owed; a credit closing caps the aged
/// amount; and lots less the advance must tie the TB closing within a rupee.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let prefix = format!("{}.", result.test_id);
    let marker = format!("{prefix}creditor_reconstructed_");
    let tol_paise: i128 = 100;
    let hash_to_name = support::ledgers_by_tag(book)?;
    let int_of = |v: &Value| -> Result<i64> {
        match v {
            Value::Int(n) => Ok(*n),
            _ => Err(AuditError::Config(format!(
                "{TEST_ID}: AGE-1 read a non-integer figure"
            ))),
        }
    };
    let mut figures: Vec<&crate::findings::Figure> = result.figures.iter().collect();
    figures.sort_by(|a, b| a.id.cmp(&b.id));
    for fig in figures {
        let Some(h) = fig.id.strip_prefix(&marker) else {
            continue;
        };
        let Some(name) = hash_to_name.get(h) else {
            out.push(format!(
                "AGE-1: cannot resolve a creditor ledger for figure {} (tag {h})",
                fig.id
            ));
            continue;
        };
        let closing_paise = i128::from(book.tb.get(*name).map_or(0, |t| t.closing_paise));
        let recon = i128::from(int_of(&fig.value)?);
        let adv_fid = format!("{prefix}creditor_advance_{h}");
        let advance = match result.figures.iter().find(|f| f.id == adv_fid) {
            Some(f) => i128::from(int_of(&f.value)?),
            None => 0,
        };
        if closing_paise >= 0 {
            if recon != 0 {
                out.push(format!(
                    "AGE-1: {name} has a nil/debit TB closing balance ({closing_paise}p) but \
{recon}p is still aged as owed ({})",
                    fig.id
                ));
            }
        } else if recon > -closing_paise {
            out.push(format!(
                "AGE-1: {name} aged amount {recon}p exceeds abs(TB closing credit balance) {}p ({})",
                -closing_paise, fig.id
            ));
        }
        let signed_recon = advance - recon;
        let diff = signed_recon - closing_paise;
        if diff.abs() > tol_paise {
            out.push(format!(
                "AGE-1: {name} reconstructed lots minus advance ({signed_recon}p = advance \
{advance}p - reconstructed {recon}p) does not tie the TB closing balance ({closing_paise}p); \
difference {diff}p"
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_day_bills_come_before_payments_and_excess_becomes_an_advance() {
        // A payment and a bill on one day: the bill is walked first, so the payment settles it.
        let w = walk_creditor(0, 0, &[(5, 1_000), (5, -1_000)]).unwrap();
        assert_eq!(
            w,
            Walk {
                lots: vec![],
                advance: 0,
                opening_remaining: 0
            }
        );
        // An overpayment carries forward as an advance and the next bill consumes it first.
        let w = walk_creditor(0, 0, &[(1, -500), (2, 800), (3, -1_000)]).unwrap();
        assert_eq!(w.lots, vec![Lot { day: 3, paise: 700 }]);
        assert_eq!(w.advance, 0);
    }

    #[test]
    fn the_opening_lot_is_tracked_by_position_not_date() {
        // An in-year bill on the period start is a second lot on the same day as the opening
        // lot; paying the opening lot off leaves the bill, whose remainder is not the opening's.
        let w = walk_creditor(0, -300, &[(0, -200), (4, 300)]).unwrap();
        assert_eq!(w.lots, vec![Lot { day: 0, paise: 200 }]);
        assert_eq!(w.opening_remaining, 0);
        let w = walk_creditor(0, -300, &[(0, -200), (4, 100)]).unwrap();
        assert_eq!(w.opening_remaining, 200);
        // An opening debit is an advance, never a negative lot.
        let w = walk_creditor(0, 250, &[(1, -100)]).unwrap();
        assert_eq!((w.lots.len(), w.advance, w.opening_remaining), (0, 150, 0));
    }

    #[test]
    fn a_post_year_payment_settles_the_oldest_lot_first() {
        let lots = [
            Lot {
                day: 10,
                paise: 100,
            },
            Lot { day: 12, paise: 50 },
        ];
        let res = apply_post_year_payments(&lots, &[(40, 120), (30, 10)]);
        assert_eq!(res[0].settled_day, Some(40));
        assert_eq!((res[0].settled_paise, res[0].remaining_paise), (100, 0));
        assert_eq!(
            (
                res[1].settled_paise,
                res[1].remaining_paise,
                res[1].settled_day
            ),
            (30, 20, None)
        );
        let none = apply_post_year_payments(&lots, &[]);
        assert_eq!(none[1].remaining_paise, 50);
    }

    #[test]
    fn a_classification_outside_the_list_is_refused() {
        let mut m = BTreeMap::new();
        m.insert("A".to_string(), "large".to_string());
        assert!(classify("A", &m).is_err());
        assert_eq!(classify("B", &m).unwrap(), "unknown");
    }
}
