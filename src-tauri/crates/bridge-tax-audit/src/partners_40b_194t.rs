// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `partners_40b_194t` (firm/LLP only): s.40(b) interest on a
//! partner's capital against the allowable rate, remuneration credited, and s.194T TDS on payments
//! to partners.
//!
//! Applicability is read from the rules, never from the entity type's name: the test applies when
//! `[entity.<type>].s194t` is true or `s40b_interest_rate_bp` is above zero. When it does not, the
//! result is one figure (`applicable` = "no") and nothing else, and no voucher is read.
//!
//! A voucher's line on a partner's capital ledger is an interest (or remuneration) credit when that
//! same voucher also carries a line on the partner's own interest (or remuneration) ledger; those
//! lines are left out of the day-by-day capital walk. Allowable interest is simple interest at
//! min(deed rate, rules rate) on each day's capital, a day in debit contributing zero, over actual
//! days / 365, with three named sensitivities (days / 360, opening balance only, no reduction).
//!
//! Every amount is carried in i128 (a year of capital-paise-days times a rate exceeds i64) and
//! each figure is checked back into i64.
//!
//! One narrowing, typed at the boundary: the deed's `interest_rate_bp` must be an integer. The
//! reference would also take a float (and fail on text); no real deed carries either.

use std::collections::{BTreeMap, BTreeSet};

use sha1::{Digest, Sha1};

use crate::book::{Book, Voucher};
use crate::depreciation::civil_day_number;
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::read::Window;
use crate::rules::Rules;
use crate::support::{overflow, py_lower, voucher_label};
use crate::PartnersConfig;

pub const TEST_ID: &str = "partners_40b_194t";
pub const VERSION: &str = "1";

const DUTIES_TAXES_GROUP: &str = "Duties & Taxes";

/// The reference's `DEFAULT_S194T`, used (and said so) when the rules carry no `[s194t]` table.
const DEFAULT_S194T_RATE_BP: i64 = 1000;
const DEFAULT_S194T_LIMIT_PAISE: i64 = 2_000_000;

/// One `[partners.<key>]` entry, typed when the test runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partner {
    pub capital_ledgers: Vec<String>,
    pub interest_ledger: Option<String>,
    pub remuneration_ledger: Option<String>,
}

/// The partners and the deed's interest rate, typed from the bound `[partners]` table. A partner
/// without `capital_ledgers` is refused, as the reference's `p["capital_ledgers"]` raises; the
/// shapes of the three name locations were already checked when they were bound.
pub fn partners_config(cfg: &PartnersConfig) -> Result<(BTreeMap<String, Partner>, Option<i64>)> {
    let mut partners = BTreeMap::new();
    for (key, entry) in &cfg.partners {
        let t = entry.as_table().ok_or_else(|| {
            AuditError::Config(format!("{TEST_ID}: [partners].{key} is not a table"))
        })?;
        let text = |name: &str| -> Result<Option<String>> {
            t.get(name)
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        AuditError::Config(format!(
                            "{TEST_ID}: [partners].{key}.{name} is not a name"
                        ))
                    })
                })
                .transpose()
        };
        let capital_ledgers = t
            .get("capital_ledgers")
            .ok_or_else(|| {
                AuditError::Config(format!(
                    "{TEST_ID}: [partners].{key} has no capital_ledgers"
                ))
            })?
            .as_array()
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
            .ok_or_else(|| {
                AuditError::Config(format!(
                    "{TEST_ID}: [partners].{key}.capital_ledgers is not a list of names"
                ))
            })?;
        partners.insert(
            key.clone(),
            Partner {
                capital_ledgers,
                interest_ledger: text("interest_ledger")?,
                remuneration_ledger: text("remuneration_ledger")?,
            },
        );
    }
    let deed_rate = match &cfg.deed {
        None => None,
        Some(deed) => {
            let t = deed.as_table().ok_or_else(|| {
                AuditError::Config(format!("{TEST_ID}: [partners].deed is not a table"))
            })?;
            match t.get("interest_rate_bp") {
                None => None,
                Some(v) => Some(v.as_integer().ok_or_else(|| {
                    AuditError::Config(format!(
                        "{TEST_ID}: [partners].deed.interest_rate_bp is not an integer"
                    ))
                })?),
            }
        }
    };
    Ok((partners, deed_rate))
}

/// The reference's `_hash`: the first 8 hex characters of the key's SHA-1.
fn hash8(key: &str) -> String {
    let digest = Sha1::digest(key.as_bytes());
    crate::canonical::hex(&digest)[..8].to_string()
}

/// `numerator / denominator` rounded half-up; 0 when either is not positive.
fn round_half_up(numerator: i128, denominator: i128) -> i128 {
    if numerator <= 0 || denominator <= 0 {
        return 0;
    }
    (numerator + denominator / 2) / denominator
}

/// One partner's pass over the population (the reference's `compute_partner_walk`).
struct Walk<'a> {
    opening_paise: i128,
    /// (day number, delta paise), sorted by day.
    events: Vec<(i64, i128)>,
    interest_credited_paise: i128,
    interest_vouchers: BTreeMap<String, &'a Voucher>,
    remuneration_credited_paise: i128,
    remuneration_vouchers: BTreeMap<String, &'a Voucher>,
}

fn walk<'a>(pop: &[&'a Voucher], book: &Book, p: &Partner) -> Walk<'a> {
    let capital: BTreeSet<&str> = p.capital_ledgers.iter().map(String::as_str).collect();
    // A ledger named twice in capital_ledgers counts twice, as the reference's sum over the list does.
    let opening_paise = p
        .capital_ledgers
        .iter()
        .filter_map(|n| book.tb.get(n))
        .map(|row| i128::from(row.opening_paise))
        .sum();
    let mut w = Walk {
        opening_paise,
        events: Vec::new(),
        interest_credited_paise: 0,
        interest_vouchers: BTreeMap::new(),
        remuneration_credited_paise: 0,
        remuneration_vouchers: BTreeMap::new(),
    };
    for v in pop {
        let lines_here: Vec<_> = v
            .lines
            .iter()
            .filter(|l| capital.contains(l.ledger.as_str()))
            .collect();
        if lines_here.is_empty() {
            continue;
        }
        let carries = |ledger: &Option<String>| {
            ledger
                .as_deref()
                .is_some_and(|n| v.lines.iter().any(|l| l.ledger == n))
        };
        let is_interest = carries(&p.interest_ledger);
        let is_remuneration = carries(&p.remuneration_ledger);
        for l in lines_here {
            let amount = i128::from(l.amount_paise);
            if is_interest {
                w.interest_credited_paise -= amount;
                w.interest_vouchers.insert(v.guid.clone(), v);
            } else if is_remuneration {
                w.remuneration_credited_paise -= amount;
                w.remuneration_vouchers.insert(v.guid.clone(), v);
            } else {
                w.events.push((civil_day_number(&v.date), amount));
            }
        }
    }
    w.events.sort_by_key(|e| e.0);
    w
}

/// Sum over every day of the period of max(-(ledger balance), 0): each event applies on its own
/// day, before that day counts (the reference's `capital_paise_days`).
fn capital_paise_days(opening_paise: i128, events: &[(i64, i128)], period: &Window) -> i128 {
    let (start, end) = (civil_day_number(&period.from), civil_day_number(&period.to));
    let mut balance = opening_paise;
    let mut idx = 0;
    let mut total = 0;
    for day in start..=end {
        while idx < events.len() && events[idx].0 <= day {
            balance += events[idx].1;
            idx += 1;
        }
        let economic = -balance;
        if economic > 0 {
            total += economic;
        }
    }
    total
}

/// `days_paise * rate_bp / (denom_days * 10000)`, rounded half-up, floored at 0.
fn interest_from_capital_days(days_paise: i128, rate_bp: i64, denom_days: i128) -> i128 {
    if days_paise <= 0 || rate_bp <= 0 {
        return 0;
    }
    round_half_up(days_paise * i128::from(rate_bp), denom_days * 10_000)
}

pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    entity_type: &str,
    cfg: &PartnersConfig,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let s40b_rate = rules.s40b_interest_rate_bp(entity_type)?;
    let s194t_on = rules.s194t_applies(entity_type)?;
    let applicable = s194t_on || s40b_rate > 0;
    r.fig(
        "applicable",
        Value::Text(if applicable { "yes" } else { "no" }.to_string()),
        Unit::Text,
        "Whether s.40(b)/s.194T apply to this engagement: read from rules.entity(...).\
s40b_interest_rate_bp and rules.entity(...).s194t for this engagement's entity_type -- never from \
the entity_type string itself. 'no' when both are absent/zero/false.",
        Vec::new(),
    );
    if !applicable {
        return Ok(r);
    }

    let pop = book.population()?;
    r.population_note = "Books population (optional, cancelled and post-dated vouchers excluded). \
A capital-ledger line is excluded from the daily-balance walk, and counted as interest/remuneration \
credited instead, iff that SAME voucher also carries a line on the partner's own interest or \
remuneration ledger -- identified from the voucher's other lines, never from a date, number or \
narration."
        .to_string();

    let (partners, deed_rate) = partners_config(cfg)?;
    // Each partner's figures are tagged `hash8(key)`, as the reference tags them, so two keys whose
    // tags coincide would name one figure twice. The reference raises there (its `fig` refuses a
    // duplicate id); refuse the same way before any partner figure is built, never panic in `fig`.
    let mut tags: BTreeMap<String, &str> = BTreeMap::new();
    for key in partners.keys() {
        let tag = hash8(key);
        if let Some(first) = tags.insert(tag.clone(), key) {
            return Err(AuditError::Config(format!(
                "{TEST_ID}: [partners].{first} and [partners].{key} share the figure tag {tag}; \
the reference refuses a duplicate figure id"
            )));
        }
    }
    let deed_missing = deed_rate.is_none();
    let rate_bp = deed_rate.map_or(s40b_rate, |d| d.min(s40b_rate));
    let (tds_rate_bp, tds_limit_paise, s194t_is_default) = match rules.s194t {
        Some(t) => (t.rate_bp, t.limit_paise, false),
        None => (DEFAULT_S194T_RATE_BP, DEFAULT_S194T_LIMIT_PAISE, true),
    };

    let f_rate = r.fig(
        "interest_rate_bp_used",
        Value::Int(rate_bp),
        Unit::BasisPoints,
        &match deed_rate {
            Some(d) => format!(
                "Rate used for s.40(b) allowable interest: min(deed interest_rate_bp {d}, rules \
s40b_interest_rate_bp {s40b_rate})."
            ),
            None => format!(
                "Deed interest rate not available; rules.s40b_interest_rate_bp used as an upper \
bound ({s40b_rate} bp), not a confirmed authorised rate."
            ),
        },
        Vec::new(),
    );
    if deed_missing {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/deed_missing"),
            clauses: vec!["s.40(b)".to_string(), "3CD-21(c)".to_string()],
            title: "Partnership deed not received; s.40(b) interest rate assumed at the statutory \
cap"
            .to_string(),
            facts: vec![("rate_used".to_string(), f_rate)],
            evidence: Vec::new(),
            confidence: Confidence::JudgementRequired,
            limits: vec![format!(
                "s.40(b) allows interest only as authorised by, and not exceeding the amount \
specified in, the partnership deed; without the deed the actual authorised rate is unknown, so the \
statutory cap ({s40b_rate} bp) is used here as an upper bound on allowable interest, not a \
confirmed rate."
            )],
            ask_client: vec![
                "Provide the partnership deed (and any supplementary deed in force \
during the year) showing the authorised interest rate and remuneration clause."
                    .to_string(),
            ],
        });
    }

    let int = |x: i128| -> Result<Value> {
        i64::try_from(x)
            .map(Value::Int)
            .map_err(|_| overflow(TEST_ID))
    };
    let mut remuneration_facts: Vec<(String, String)> = Vec::new();
    let mut excess_total: i128 = 0;
    let mut credited_total: i128 = 0;

    for (key, p) in &partners {
        let h = hash8(key);
        let w = walk(&pop, book, p);
        let days_primary = capital_paise_days(w.opening_paise, &w.events, period);
        let days_opening_only = capital_paise_days(w.opening_paise, &[], period);
        let reductions_skipped: Vec<(i64, i128)> =
            w.events.iter().copied().filter(|e| e.1 < 0).collect();
        let days_no_reduction = capital_paise_days(w.opening_paise, &reductions_skipped, period);
        let allowable_365 = interest_from_capital_days(days_primary, rate_bp, 365);
        let allowable_360 = interest_from_capital_days(days_primary, rate_bp, 360);
        let allowable_opening_only = interest_from_capital_days(days_opening_only, rate_bp, 365);
        let allowable_no_reduction = interest_from_capital_days(days_no_reduction, rate_bp, 365);
        let credited = w.interest_credited_paise;
        let excess = (credited - allowable_365).max(0);

        let ev_capital: Vec<EvidenceRef> = p
            .capital_ledgers
            .iter()
            .map(|n| EvidenceRef::new("ledger", n))
            .collect();
        r.fig(
            &format!("capital_opening_{h}"),
            int(w.opening_paise)?,
            Unit::Paise,
            &format!("Opening (1-4) balance of partner (tag {h})'s capital ledger(s), Dr+/Cr-."),
            ev_capital.clone(),
        );
        let f_allow = r.fig(
            &format!("allowable_interest_{h}"),
            int(allowable_365)?,
            Unit::Paise,
            &format!(
                "s.40(b) allowable interest for partner (tag {h}): simple interest at {rate_bp} bp \
on the capital balance walked day by day (opening TB balance, then every population voucher line \
on the capital ledger(s) other than an interest/remuneration credit, changing the balance ON its \
date), actual days / 365, a debit (negative) capital day contributing zero."
            ),
            ev_capital.clone(),
        );
        r.fig(
            &format!("allowable_interest_sensitivity_360day_{h}"),
            int(allowable_360)?,
            Unit::Paise,
            &format!("Same daily-balance walk for partner (tag {h}), but actual days / 360."),
            ev_capital.clone(),
        );
        r.fig(
            &format!("allowable_interest_sensitivity_opening_only_{h}"),
            int(allowable_opening_only)?,
            Unit::Paise,
            &format!(
                "s.40(b) interest for partner (tag {h}) as if the capital balance stayed at its \
opening value for the whole year (no intra-year voucher line applied at all)."
            ),
            ev_capital.clone(),
        );
        r.fig(
            &format!("allowable_interest_sensitivity_no_reduction_{h}"),
            int(allowable_no_reduction)?,
            Unit::Paise,
            &format!(
                "s.40(b) interest for partner (tag {h}) as if capital were never reduced by a \
withdrawal or transfer (every debit line on the capital ledger(s) skipped; a credit/increase line, \
e.g. capital introduced, still applied)."
            ),
            ev_capital,
        );

        let voucher_refs = |vs: &BTreeMap<String, &Voucher>| -> Vec<EvidenceRef> {
            vs.iter()
                .map(|(g, v)| EvidenceRef::with_label("voucher", g, &voucher_label(v)))
                .collect()
        };
        let ev_interest_v = voucher_refs(&w.interest_vouchers);
        let f_credited = r.fig(
            &format!("interest_credited_{h}"),
            int(credited)?,
            Unit::Paise,
            &format!(
                "Interest actually credited to partner (tag {h})'s capital ledger(s), in a voucher \
also carrying a line on their interest_ledger."
            ),
            ev_interest_v.clone(),
        );
        let f_excess = r.fig(
            &format!("s40b_excess_{h}"),
            int(excess)?,
            Unit::Paise,
            &format!("interest_credited_{h} minus allowable_interest_{h}, floor 0."),
            Vec::new(),
        );
        excess_total += excess;
        credited_total += credited;

        if excess > 0 {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/s40b_excess/{h}"),
                clauses: vec!["s.40(b)".to_string(), "3CD-21(c)".to_string()],
                title: "Interest credited to a partner exceeds the s.40(b) allowable amount"
                    .to_string(),
                facts: vec![
                    ("credited".to_string(), f_credited),
                    ("allowable".to_string(), f_allow),
                    ("excess".to_string(), f_excess),
                ],
                evidence: ev_interest_v,
                confidence: if deed_missing {
                    Confidence::JudgementRequired
                } else {
                    Confidence::Computed
                },
                limits: vec![if deed_missing {
                    "No deed was available; the excess shown uses the statutory cap as the \
assumed authorised rate -- confirm the deed's actual rate and terms before relying on this figure."
                        .to_string()
                } else {
                    "Allowable interest here assumes the capital base is exactly the capital \
ledger(s) supplied and that interest was authorised for the whole year; confirm both against the \
deed."
                        .to_string()
                }],
                ask_client: vec![
                    "Confirm the deed's authorised interest rate and the capital \
base it applies to."
                        .to_string(),
                ],
            });
        }

        let f_rem = r.fig(
            &format!("remuneration_credited_{h}"),
            int(w.remuneration_credited_paise)?,
            Unit::Paise,
            &format!(
                "Remuneration credited to partner (tag {h}), in a voucher also carrying a line on \
their remuneration_ledger. The s.40(b)(v) book-profit limit is NOT computed here."
            ),
            voucher_refs(&w.remuneration_vouchers),
        );
        remuneration_facts.push((key.clone(), f_rem));

        let subject_paise = credited + w.remuneration_credited_paise;
        let f_subject = r.fig(
            &format!("s194t_amount_credited_{h}"),
            int(subject_paise)?,
            Unit::Paise,
            &format!(
                "Interest + remuneration credited to partner (tag {h}) in the year -- the s.194T \
base (salary, remuneration, commission, bonus and interest to a partner)."
            ),
            Vec::new(),
        );
        let tds_expected = round_half_up(subject_paise * i128::from(tds_rate_bp), 10_000);
        let f_tds = r.fig(
            &format!("s194t_tds_expected_{h}"),
            int(tds_expected)?,
            Unit::Paise,
            &format!("TDS @ {tds_rate_bp} bp on s194t_amount_credited_{h}."),
            Vec::new(),
        );
        // The remuneration voucher wins a GUID both maps hold, as the reference's
        // `{**interest, **remuneration}` does.
        let mut touched = w.interest_vouchers.clone();
        touched.extend(w.remuneration_vouchers.clone());
        let tds_seen = touched.values().any(|v| {
            v.lines.iter().any(|l| {
                book.ledgers
                    .get(&l.ledger)
                    .is_some_and(|led| led.under(DUTIES_TAXES_GROUP))
                    && py_lower(&l.ledger).contains("tds")
            })
        });
        r.fig(
            &format!("s194t_tds_ledger_seen_{h}"),
            Value::Text(if tds_seen { "yes" } else { "no" }.to_string()),
            Unit::Text,
            &format!(
                "Whether a line under '{DUTIES_TAXES_GROUP}' with 'tds' in its name appears in the \
voucher(s) crediting partner (tag {h})'s interest/remuneration."
            ),
            Vec::new(),
        );

        if subject_paise > i128::from(tds_limit_paise) {
            let mut limits = vec![
                "Books only: TAN registration, challans filed and any Form 26A \
route are not visible from vouchers."
                    .to_string(),
            ];
            if s194t_is_default {
                limits.push(format!(
                    "The s.194T rate/limit used here (rate {tds_rate_bp} bp, limit \
{tds_limit_paise} paise) is a local prototype default (status=\"confirm\"), pending confirmation in \
the rules table -- not yet a verified rule."
                ));
            }
            r.findings.push(Finding {
                id: format!("{TEST_ID}/s194t/{h}"),
                clauses: vec![
                    "s.194T".to_string(),
                    "3CD-34(a)".to_string(),
                    "3CD-34(c)".to_string(),
                ],
                title: "Payments/credits to a partner over the s.194T threshold with no TDS \
ledger line seen"
                    .to_string(),
                facts: vec![
                    ("credited".to_string(), f_subject),
                    ("tds_expected".to_string(), f_tds),
                ],
                evidence: voucher_refs(&touched),
                confidence: Confidence::NeedsDocument,
                limits,
                ask_client: vec![
                    "Confirm the firm's TAN and whether TDS under s.194T was deposited (this is \
not visible from the books shown here if it went through a ledger not captured in \
touched_vouchers)."
                        .to_string(),
                    "If not deducted, confirm whether Form 26A relief is available (partner \
returned the income, tax paid)."
                        .to_string(),
                ],
            });
        }
    }

    r.fig(
        "s40b_excess_total",
        int(excess_total)?,
        Unit::Paise,
        "Sum of s40b_excess_<partner> across all partners.",
        Vec::new(),
    );
    r.fig(
        "s194t_interest_credited_total",
        int(credited_total)?,
        Unit::Paise,
        "Sum of interest_credited_<partner> across all partners -- cross-check this against a \
shared interest_ledger's own TB movement when every partner uses the same ledger name.",
        Vec::new(),
    );

    if !remuneration_facts.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/remuneration_book_profit_required"),
            clauses: vec!["s.40(b)(v)".to_string(), "3CD-21(c)".to_string()],
            title: "Remuneration to working partners: the s.40(b)(v) limit depends on book \
profit, not computed here"
                .to_string(),
            facts: remuneration_facts,
            evidence: Vec::new(),
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "The s.40(b)(v) remuneration ceiling is a slab on 'book profit' \
(s.28-44D profit plus remuneration debited, per Explanation 3), which this test does not compute; \
whether remuneration is authorised and quantified by the deed, and from what date, is a deed \
reading, not a books fact."
                    .to_string(),
            ],
            ask_client: vec![
                "Provide the computation of book profit under s.40(b) Explanation \
3, and the deed clause authorising and quantifying remuneration."
                    .to_string(),
            ],
        });
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::{LedgerLine, VoucherStatus};
    use bridge_tally_primitives::TallyDate;

    fn cfg(text: &str) -> PartnersConfig {
        let mut partners: BTreeMap<String, toml::Value> = toml::from_str::<toml::Table>(text)
            .unwrap()
            .into_iter()
            .collect();
        let deed = partners.remove("deed");
        PartnersConfig { partners, deed }
    }

    fn year() -> Window {
        Window {
            from: TallyDate::parse("20250401".to_string()).unwrap(),
            to: TallyDate::parse("20260331".to_string()).unwrap(),
        }
    }

    /// A book whose one voucher has an unknown status: the population refuses it.
    fn unreadable_book() -> Book {
        Book {
            company_name: "Invented".to_string(),
            company_guid: "invented".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            vouchers: vec![Voucher {
                guid: "g-1".to_string(),
                date: TallyDate::parse("20250501".to_string()).unwrap(),
                vtype: "Journal".to_string(),
                base_type: "Journal".to_string(),
                number: "1".to_string(),
                status: VoucherStatus::Unknown,
                lines: vec![LedgerLine {
                    ledger: "A Capital".to_string(),
                    amount_paise: -1,
                }],
                narration: String::new(),
                party_field: String::new(),
                masterid: None,
                inventory: Vec::new(),
                ..Default::default()
            }],
            tb: BTreeMap::new(),
        }
    }

    #[test]
    fn the_partner_tag_is_the_references() {
        // As the reference's golden names partner_a: sha1("partner_a")[:8].
        assert_eq!(hash8("partner_a"), "d0893259");
    }

    #[test]
    fn a_partner_or_deed_the_reference_cannot_read_is_refused() {
        for text in [
            "[a]\ninterest_ledger = \"I\"\n",
            "deed = 5\n[a]\ncapital_ledgers = [\"A\"]\n",
            "[a]\ncapital_ledgers = [\"A\"]\n[deed]\ninterest_rate_bp = \"12%\"\n",
        ] {
            assert!(
                matches!(partners_config(&cfg(text)), Err(AuditError::Config(_))),
                "{text}"
            );
        }
        let (partners, rate) = partners_config(&cfg(
            "[a]\ncapital_ledgers = [\"A\", \"A\"]\n[deed]\ninterest_rate_bp = 1000\n",
        ))
        .unwrap();
        assert_eq!(partners["a"].capital_ledgers, ["A", "A"]);
        assert_eq!(rate, Some(1000));
    }

    #[test]
    fn two_partner_keys_sharing_a_figure_tag_are_refused_not_a_panic() {
        // SHA-1 of "p30395" and of "p89343" both begin 47ff8a3d.
        assert_eq!(hash8("p30395"), hash8("p89343"));
        let book = Book {
            vouchers: Vec::new(),
            ..unreadable_book()
        };
        let rules = Rules::vendored().unwrap();
        let two = "[p30395]\ncapital_ledgers = [\"A Capital\"]\n\
[p89343]\ncapital_ledgers = [\"B Capital\"]\n";
        match run(&book, &rules, &year(), "firm", &cfg(two)) {
            Err(AuditError::Config(m)) => {
                assert!(m.contains("[partners].p30395 and [partners].p89343"), "{m}");
                assert!(m.contains("47ff8a3d"), "{m}");
            }
            other => panic!("expected a Config refusal, got {other:?}"),
        }
        // Either key alone runs.
        let one = "[p30395]\ncapital_ledgers = [\"A Capital\"]\n";
        assert!(run(&book, &rules, &year(), "firm", &cfg(one)).is_ok());
    }

    #[test]
    fn not_applicable_reads_no_voucher_and_rules_without_entities_refuse() {
        let rules = Rules::vendored().unwrap();
        let r = run(&unreadable_book(), &rules, &year(), "company", &cfg("")).unwrap();
        assert_eq!(r.figures.len(), 1);
        // A firm reads the population, which refuses the unknown status.
        assert!(run(&unreadable_book(), &rules, &year(), "firm", &cfg("")).is_err());
        // Either key alone makes the test apply (it then reads the population, which refuses).
        let only = |s40b: Option<i64>, s194t: Option<bool>| Rules {
            entity: Some(BTreeMap::from([(
                "x".to_string(),
                crate::rules::EntityRules {
                    s40b_interest_rate_bp: s40b,
                    s194t,
                },
            )])),
            ..rules.clone()
        };
        for r in [only(Some(1200), None), only(None, Some(true))] {
            assert!(run(&unreadable_book(), &r, &year(), "x", &cfg("")).is_err());
        }
        let neither = only(Some(0), Some(false));
        assert_eq!(
            run(&unreadable_book(), &neither, &year(), "x", &cfg(""))
                .unwrap()
                .figures
                .len(),
            1
        );
        let without = Rules {
            entity: None,
            ..rules
        };
        assert!(matches!(
            run(&unreadable_book(), &without, &year(), "company", &cfg("")),
            Err(AuditError::Config(_))
        ));
    }
}
