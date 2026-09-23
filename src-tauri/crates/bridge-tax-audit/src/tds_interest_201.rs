// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `tds_interest_201`: s.201(1A) TDS interest and s.206C(7) TCS
//! interest, as a range, on shortfalls other tests already identified.
//!
//! The test decides no payee, nature or amount itself. [`defaults_from`] builds its rows from
//! `tds_payees`' and `partners_40b_194t`'s own findings and figures, as the reference's pack
//! (`_tds_interest_defaults`) does: each 194C/194I/194J finding twice (at the section's lower and
//! higher rate, the payee's type being unknown there) and each s.194T finding once, dated by the
//! latest voucher it cites. [`run`] prices each row by "month or part of a month" (calendar-month
//! parts, not elapsed days), and an unsupplied date gives a range: the minimum assumes the leg
//! complete with no further delay, the maximum assumes it still outstanding on `as_of`.
//!
//! Every amount is carried in i128 and each figure is checked back into i64.

use std::collections::HashMap;

use bridge_tally_primitives::TallyDate;
use sha1::{Digest, Sha1};

use crate::book::{Book, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Figure, Finding, TestResult, Unit, Value};
use crate::read::iso;
use crate::rules::Rules;
use crate::support::{overflow, py_repr_str, py_upper};

pub const TEST_ID: &str = "tds_interest_201";
pub const VERSION: &str = "1";

/// The reference's `DEFAULT_S201_1A` and `DEFAULT_S206C_7`, used (and flagged) without the tables.
const DEFAULT_S201_1A_BEFORE_BP: i64 = 100;
const DEFAULT_S201_1A_AFTER_BP: i64 = 150;
const DEFAULT_S206C_7_BP: i64 = 100;
const DEFAULT_STATUS: &str = "confirm";

/// One row the test prices: a (section, payee) tax amount another test already identified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestDefault {
    pub section: String,
    pub payee_key: String,
    pub tax_paise: i64,
    pub deductible_date: TallyDate,
    pub deducted_date: Option<TallyDate>,
    pub paid_date: Option<TallyDate>,
    pub payee_return_filed_date: Option<TallyDate>,
    pub deductor_status_uncertain: bool,
    pub evidence: Vec<EvidenceRef>,
}

fn hash8(text: &str) -> String {
    crate::canonical::hex(&Sha1::digest(text.as_bytes()))[..8].to_string()
}

/// `(n + d / 2).div_euclid(d)`: Python's `(n + d // 2) // d` for a positive `d`.
fn floor_half_up(n: i128, d: i128) -> i128 {
    (n + d / 2).div_euclid(d)
}

/// The rows the reference's pack builds from `tds_payees`' and `partners_40b_194t`'s results
/// (`_tds_interest_defaults`), in its order. A cited voucher is looked up among every voucher by
/// GUID, the last with a GUID winning as the reference's dict does; a finding citing none is left
/// out.
pub fn defaults_from(
    book: &Book,
    rules: &Rules,
    tds_payees: &TestResult,
    partners: &TestResult,
    deductor_status_uncertain: bool,
) -> Result<Vec<InterestDefault>> {
    let rates = rules
        .tds_rates
        .ok_or_else(|| AuditError::Config(format!("{TEST_ID} needs rules [tds_rates]")))?;
    let by_guid: HashMap<&str, &Voucher> =
        book.vouchers.iter().map(|v| (v.guid.as_str(), v)).collect();
    let figure = |r: &TestResult, id: &str| -> Result<i64> {
        match r.figures.iter().find(|f| f.id == id).map(|f| &f.value) {
            Some(Value::Int(n)) => Ok(*n),
            _ => Err(AuditError::Config(format!(
                "{TEST_ID}: {id} is not an integer figure"
            ))),
        }
    };
    let latest = |evidence: &[EvidenceRef]| -> Option<TallyDate> {
        evidence
            .iter()
            .filter(|e| e.kind == "voucher")
            .filter_map(|e| by_guid.get(e.id.as_str()))
            .map(|v| v.date.clone())
            .max()
    };
    let mut out = Vec::new();
    for f in &tds_payees.findings {
        let rid = f.id.split_once('/').map_or("", |(_, rest)| rest);
        let nature = rid.split_once('_').map_or(rid, |(n, _)| n);
        let pair = match nature {
            "194C" => (rates.s194c_individual_huf_bp, rates.s194c_other_bp),
            "194I" => (rates.s194i_plant_machinery_bp, rates.s194i_land_building_bp),
            "194J" => (rates.s194j_technical_bp, rates.s194j_professional_bp),
            _ => continue,
        };
        let Some((_, credited_id)) = f.facts.iter().find(|(k, _)| k == "credited") else {
            continue;
        };
        let credited = i128::from(figure(tds_payees, credited_id)?);
        let Some(deductible_date) = latest(&f.evidence) else {
            continue;
        };
        for (rate_bp, tag) in [(pair.0, "lower_rate"), (pair.1, "higher_rate")] {
            let tax = floor_half_up(credited * i128::from(rate_bp), 10_000);
            out.push(InterestDefault {
                section: nature.to_string(),
                payee_key: format!("{rid}_{tag}"),
                tax_paise: i64::try_from(tax).map_err(|_| overflow(TEST_ID))?,
                deductible_date: deductible_date.clone(),
                deducted_date: None,
                paid_date: None,
                payee_return_filed_date: None,
                deductor_status_uncertain,
                evidence: f.evidence.clone(),
            });
        }
    }
    let prefix = format!("{}/s194t/", crate::partners_40b_194t::TEST_ID);
    for f in &partners.findings {
        let Some(h) = f.id.strip_prefix(&prefix) else {
            continue;
        };
        let Some((_, tds_id)) = f.facts.iter().find(|(k, _)| k == "tds_expected") else {
            continue;
        };
        let tax_paise = figure(partners, tds_id)?;
        let Some(deductible_date) = latest(&f.evidence) else {
            continue;
        };
        out.push(InterestDefault {
            section: "194T".to_string(),
            payee_key: h.to_string(),
            tax_paise,
            deductible_date,
            deducted_date: None,
            paid_date: None,
            payee_return_filed_date: None,
            deductor_status_uncertain: false,
            evidence: f.evidence.clone(),
        });
    }
    Ok(out)
}

/// Any TCS section ("206C(1)", "206 c", ...) is priced under s.206C(7), every other under
/// s.201(1A): the reference's light dispatch on the caller's section string.
fn is_206c(section: &str) -> bool {
    py_upper(section)
        .replace([' ', '.'], "")
        .starts_with("206C")
}

fn year_month(d: &TallyDate) -> (i64, i64) {
    let s = d.as_str();
    (s[0..4].parse().unwrap_or(0), s[4..6].parse().unwrap_or(0))
}

/// "Month or part of a month": 0 when `end` is not after `start`, otherwise every calendar month
/// from `start`'s through `end`'s, inclusive.
fn months_or_part(start: &TallyDate, end: &TallyDate) -> i64 {
    if end <= start {
        return 0;
    }
    let ((sy, sm), (ey, em)) = (year_month(start), year_month(end));
    (ey - sy) * 12 + (em - sm) + 1
}

/// `tax * rate * months / 10000`, rounded half-up; 0 unless all three are positive.
fn interest_amount(tax: i128, rate_bp: i64, months: i64) -> i128 {
    if tax <= 0 || rate_bp <= 0 || months <= 0 {
        return 0;
    }
    floor_half_up(tax * i128::from(rate_bp) * i128::from(months), 10_000)
}

#[allow(clippy::too_many_lines)]
pub fn run(rules: &Rules, defaults: &[InterestDefault], as_of: &TallyDate) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = "Not walked from the books population: every row is one (section, payee) \
tax amount already identified as a shortfall by 'TDS on payments made' or 'Partners (s.40(b), \
s.194T)', with whatever deduction/payment dates the books or documents establish. An unsupplied \
date is never guessed; it drives a range instead."
        .to_string();

    let (rate1, rate2, s201_authority, s201_status, s201_is_default) = match &rules.s201_1a {
        Some(t) => (
            t.rate_before_deduction_bp,
            t.rate_after_deduction_bp,
            t.authority.as_str(),
            t.status.as_str(),
            false,
        ),
        None => (
            DEFAULT_S201_1A_BEFORE_BP,
            DEFAULT_S201_1A_AFTER_BP,
            "s.201(1A)",
            DEFAULT_STATUS,
            true,
        ),
    };
    let (rate3, s206c_authority, s206c_status, s206c_is_default) = match &rules.s206c_7 {
        Some(t) => (t.rate_bp, t.authority.as_str(), t.status.as_str(), false),
        None => (DEFAULT_S206C_7_BP, "s.206C(7)", DEFAULT_STATUS, true),
    };
    let as_of_iso = iso(as_of);

    r.fig(
        "as_of",
        Value::Text(as_of_iso.clone()),
        Unit::Text,
        "Reference date for every maximum bound: the return due date from rules[due_dates], or the \
report date, as supplied by the caller -- this test never reads rules[due_dates] itself.",
        Vec::new(),
    );
    r.fig(
        "rate_before_deduction_bp",
        Value::Int(rate1),
        Unit::BasisPoints,
        &format!(
            "s.201(1A) pre-deduction rate, per month or part of a month ({s201_authority}, \
status={s201_status})."
        ),
        Vec::new(),
    );
    r.fig(
        "rate_after_deduction_bp",
        Value::Int(rate2),
        Unit::BasisPoints,
        &format!(
            "s.201(1A) post-deduction rate, per month or part of a month ({s201_authority}, \
status={s201_status})."
        ),
        Vec::new(),
    );
    r.fig(
        "rate_206c_bp",
        Value::Int(rate3),
        Unit::BasisPoints,
        &format!(
            "s.206C(7) rate, per month or part of a month ({s206c_authority}, \
status={s206c_status})."
        ),
        Vec::new(),
    );

    let int = |x: i128| -> Result<Value> {
        i64::try_from(x)
            .map(Value::Int)
            .map_err(|_| overflow(TEST_ID))
    };
    let count = |x: i64| Value::Int(x);
    let (mut total_tax, mut total_min, mut total_max, mut total_26a) = (0i128, 0i128, 0i128, 0i128);
    let (mut count_26a, mut count_no_26a_date) = (0i64, 0i64);

    for (i, d) in defaults.iter().enumerate() {
        let section = d.section.as_str();
        let tax = i128::from(d.tax_paise);
        let dd = &d.deductible_date;
        let (ed, pd) = (d.deducted_date.as_ref(), d.paid_date.as_ref());
        let evidence = d.evidence.clone();
        let tcs = is_206c(section);
        let h = hash8(&format!("{section}:{}:{i}", d.payee_key));
        let payee = py_repr_str(&d.payee_key);
        total_tax += tax;

        r.fig(
            &format!("section_{h}"),
            Value::Text(section.to_string()),
            Unit::Text,
            &format!(
                "Section this row's interest is computed under (row {i}, payee {payee}, tag {h})."
            ),
            Vec::new(),
        );
        let f_tax = r.fig(
            &format!("tax_{h}"),
            Value::Int(d.tax_paise),
            Unit::Paise,
            &format!(
                "Tax this row's interest is computed on (row {i}, payee {payee}, tag {h}) -- \
carried in from tds_payees/partners_40b_194t, not re-derived here."
            ),
            evidence.clone(),
        );
        let f_dd = r.fig(
            &format!("deductible_date_{h}"),
            Value::Text(iso(dd)),
            Unit::Text,
            &format!(
                "Date tax became deductible/collectible (row {i}, tag {h}): the earlier of credit \
and payment for most sections."
            ),
            Vec::new(),
        );
        let or_not_supplied =
            |x: Option<&TallyDate>| Value::Text(x.map_or("not supplied".to_string(), iso));
        r.fig(
            &format!("deducted_date_{h}"),
            or_not_supplied(ed),
            Unit::Text,
            &format!(
                "Date tax was actually deducted/collected (row {i}, tag {h}); 'not supplied' when \
books/documents do not establish it -- drives the range below, never guessed."
            ),
            Vec::new(),
        );
        r.fig(
            &format!("paid_date_{h}"),
            or_not_supplied(pd),
            Unit::Text,
            &format!(
                "Date the deducted/collected tax was actually paid/deposited (row {i}, tag {h}); \
'not supplied' when books/documents do not establish it."
            ),
            Vec::new(),
        );

        let (authority_label, law_status_note, interest_min, interest_max);
        if tcs {
            authority_label = "s.206C(7)";
            law_status_note = s206c_is_default;
            let (months_min, months_max) = match pd {
                Some(pd) => {
                    let m = months_or_part(dd, pd);
                    (m, m)
                }
                None => (
                    months_or_part(dd, ed.unwrap_or(dd)),
                    months_or_part(dd, as_of),
                ),
            };
            interest_min = interest_amount(tax, rate3, months_min);
            interest_max = interest_amount(tax, rate3, months_max);
            r.fig(
                &format!("months_min_{h}"),
                count(months_min),
                Unit::Count,
                &format!("s.206C(7) months (or part), minimum bound, row {i} tag {h}."),
                Vec::new(),
            );
            r.fig(
                &format!("months_max_{h}"),
                count(months_max),
                Unit::Count,
                &format!("s.206C(7) months (or part), maximum bound, row {i} tag {h}."),
                Vec::new(),
            );
        } else {
            authority_label = "s.201(1A)";
            law_status_note = s201_is_default;
            let (m1_min, m1_max, m2_min, m2_max);
            if let Some(ed) = ed {
                let months1 = months_or_part(dd, ed);
                let amt1 = interest_amount(tax, rate1, months1);
                (m1_min, m1_max) = (months1, months1);
                (m2_min, m2_max) = match pd {
                    Some(pd) => {
                        let m = months_or_part(ed, pd);
                        (m, m)
                    }
                    None => (0, months_or_part(ed, as_of)),
                };
                interest_min = amt1 + interest_amount(tax, rate2, m2_min);
                interest_max = amt1 + interest_amount(tax, rate2, m2_max);
            } else {
                (m1_min, m1_max) = (0, months_or_part(dd, as_of));
                (m2_min, m2_max) = (0, 0);
                interest_min = interest_amount(tax, rate1, m1_min);
                interest_max = interest_amount(tax, rate1, m1_max);
            }
            for (name, value, which) in [
                (
                    "months_stage1_min",
                    m1_min,
                    "pre-deduction months (or part), minimum",
                ),
                (
                    "months_stage1_max",
                    m1_max,
                    "pre-deduction months (or part), maximum",
                ),
                (
                    "months_stage2_min",
                    m2_min,
                    "post-deduction months (or part), minimum",
                ),
                (
                    "months_stage2_max",
                    m2_max,
                    "post-deduction months (or part), maximum",
                ),
            ] {
                r.fig(
                    &format!("{name}_{h}"),
                    count(value),
                    Unit::Count,
                    &format!("s.201(1A) {which} bound, row {i} tag {h}."),
                    Vec::new(),
                );
            }
        }

        let f_min = r.fig(
            &format!("interest_min_{h}"),
            int(interest_min)?,
            Unit::Paise,
            &format!(
                "{authority_label} interest, minimum bound (row {i} tag {h}): known dates used \
where supplied; any unsupplied leg is assumed complete with zero further delay for this floor."
            ),
            evidence.clone(),
        );
        let f_max = r.fig(
            &format!("interest_max_{h}"),
            int(interest_max)?,
            Unit::Paise,
            &format!(
                "{authority_label} interest, maximum bound (row {i} tag {h}): any unsupplied leg is \
assumed still outstanding through as_of ({as_of_iso})."
            ),
            evidence.clone(),
        );
        total_min += interest_min;
        total_max += interest_max;

        let mut f_26a = None;
        if !tcs && ed.is_none() {
            if let Some(filed) = &d.payee_return_filed_date {
                let months_26a = months_or_part(dd, filed);
                let amt_26a = interest_amount(tax, rate1, months_26a);
                r.fig(
                    &format!("payee_return_filed_date_{h}"),
                    Value::Text(iso(filed)),
                    Unit::Text,
                    &format!(
                        "Payee's own return-filing date supplied for the Form 26A route (row {i} \
tag {h})."
                    ),
                    Vec::new(),
                );
                r.fig(
                    &format!("months_26a_relief_{h}"),
                    count(months_26a),
                    Unit::Count,
                    &format!(
                        "Months (or part), deductible date to the payee's return-filing date, row \
{i} tag {h}."
                    ),
                    Vec::new(),
                );
                f_26a = Some(r.fig(
                    &format!("interest_26a_relief_{h}"),
                    int(amt_26a)?,
                    Unit::Paise,
                    &format!(
                        "s.201(1A) interest to the payee's Form 26A return-filing date -- the first \
proviso to s.201(1) relief from being an assessee in default does not stop this clock (row {i} tag \
{h})."
                    ),
                    evidence.clone(),
                ));
                total_26a += amt_26a;
                count_26a += 1;
            } else {
                count_no_26a_date += 1;
            }
        }

        if interest_max > 0 {
            let mut facts = vec![
                ("tax".to_string(), f_tax),
                ("deductible_date".to_string(), f_dd),
                ("interest_min".to_string(), f_min),
                ("interest_max".to_string(), f_max),
            ];
            let mut limits = vec!["Books/documents alone cannot establish the actual \
deduction/collection and deposit dates; challans, the TAN and (for the pre-deduction leg) Form 26A \
are needed before any single interest amount can be confirmed -- this is a range, not a computed \
liability."
                .to_string()];
            if law_status_note {
                limits.push(format!(
                    "The {authority_label} rate used here is a local prototype default \
(status=\"confirm\"), pending confirmation against the bare Act/notification text -- not yet a \
verified rule."
                ));
            }
            if tcs {
                limits.push(
                    "s.206C(7) is modelled here as a single rate for the whole not-collected/not-paid \
period, per the LAW text this port was given; confirm whether the Act instead splits \
pre-/post-collection like s.201(1A) before relying on this figure."
                        .to_string(),
                );
            }
            if f_26a.is_none() && ed.is_none() && !tcs {
                limits.push(
                    "No payee return-filing date was supplied, so the Form 26A (first proviso to \
s.201(1)) relief scenario could not be computed for this row; the maximum bound above assumes tax \
remains undeducted through as_of."
                        .to_string(),
                );
            }
            if let Some(id) = f_26a {
                facts.push(("interest_26a_relief".to_string(), id));
            }
            if d.deductor_status_uncertain {
                limits.push(
                    "Whether this assessee is a deductor at all for the year is itself unresolved \
(previous-year turnover unconfirmed -- see tds_payees' own deductor_status finding); this interest \
range is conditional on deductor status being confirmed, not a standalone conclusion."
                        .to_string(),
                );
            }
            r.findings.push(Finding {
                id: format!("{TEST_ID}/{h}"),
                clauses: vec![
                    authority_label.to_string(),
                    "3CD-21(b)".to_string(),
                    "3CD-34(a)".to_string(),
                    "3CD-34(c)".to_string(),
                ],
                title: format!(
                    "Possible {authority_label} interest on a {section} shortfall for one payee -- \
range pending deposit evidence"
                ),
                facts,
                evidence,
                confidence: Confidence::NeedsDocument,
                limits,
                ask_client: vec![
                    "Provide TDS/TCS challans and the TAN to confirm actual deduction/collection \
and deposit dates for this payee."
                        .to_string(),
                    "If not deducted/collected, confirm whether Form 26A (or the equivalent TCS \
relief) is available, and the payee's own return-filing date."
                        .to_string(),
                ],
            });
        }
    }

    r.fig(
        "total_tax_paise",
        int(total_tax)?,
        Unit::Paise,
        "Sum of tax_<row> across every default supplied.",
        Vec::new(),
    );
    r.fig(
        "total_interest_min_paise",
        int(total_min)?,
        Unit::Paise,
        "Sum of interest_min_<row> across every default -- the floor of the combined range.",
        Vec::new(),
    );
    r.fig(
        "total_interest_max_paise",
        int(total_max)?,
        Unit::Paise,
        "Sum of interest_max_<row> across every default -- the ceiling of the combined range.",
        Vec::new(),
    );
    r.fig(
        "s201_1a_defaults_with_26a_relief_date_count",
        count(count_26a),
        Unit::Count,
        "Rows (deducted_date unknown, s.201(1A)) where a payee return-filing date was supplied for \
the Form 26A relief scenario.",
        Vec::new(),
    );
    r.fig(
        "s201_1a_defaults_without_26a_relief_date_count",
        count(count_no_26a_date),
        Unit::Count,
        "Rows (deducted_date unknown, s.201(1A)) where no payee return-filing date was supplied -- \
the Form 26A relief scenario could not be computed.",
        Vec::new(),
    );
    r.fig(
        "total_interest_26a_relief_paise",
        int(total_26a)?,
        Unit::Paise,
        "Sum of interest_26a_relief_<row> across only the rows where a payee filing date was \
supplied -- a partial sum over a subset, never compared directly to total_interest_min/max.",
        Vec::new(),
    );
    Ok(r)
}

/// TDSI-1: every interest_min/interest_max pair is non-negative, ordered, and equal to tax x rate
/// x months recomputed here from the published figures alone. The month count and the amount are
/// this function's own copies, never the builder's, so a bug the two share cannot pass (the
/// reference's own tautology guard).
pub fn check_invariants(result: &TestResult) -> Result<Vec<String>> {
    fn month_span(start: &str, end: &str) -> Option<i128> {
        let ymd = |s: &str| -> Option<(i128, i128, i128)> {
            let mut parts = s.splitn(3, '-');
            Some((
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ))
        };
        let (a, b) = (ymd(start)?, ymd(end)?);
        if b <= a {
            return Some(0);
        }
        Some((b.0 - a.0) * 12 + (b.1 - a.1) + 1)
    }
    fn amount(tax: i128, rate: i128, months: i128) -> i128 {
        if tax <= 0 || rate <= 0 || months <= 0 {
            return 0;
        }
        (tax * rate * months + 5000).div_euclid(10_000)
    }

    let mut out = Vec::new();
    let prefix = format!("{}.", result.test_id);
    let figures: HashMap<&str, &Figure> =
        result.figures.iter().map(|f| (f.id.as_str(), f)).collect();
    let fv = |name: &str| {
        figures
            .get(format!("{prefix}{name}").as_str())
            .map(|f| &f.value)
    };
    let int_of = |name: &str| match fv(name) {
        Some(Value::Int(n)) => Some(i128::from(*n)),
        _ => None,
    };
    let text_of = |name: &str| match fv(name) {
        Some(Value::Text(t)) => Some(t.clone()),
        _ => None,
    };
    let bad_date =
        |h: &str| AuditError::Config(format!("{TEST_ID}: {h}: a date figure is not ISO"));
    let as_of = text_of("as_of").unwrap_or_default();
    let (rate1, rate2, rate3) = (
        int_of("rate_before_deduction_bp").unwrap_or(0),
        int_of("rate_after_deduction_bp").unwrap_or(0),
        int_of("rate_206c_bp").unwrap_or(0),
    );
    let min_prefix = format!("{prefix}interest_min_");
    let mut hashes: Vec<&str> = figures
        .keys()
        .filter_map(|id| id.strip_prefix(min_prefix.as_str()))
        .collect();
    hashes.sort_unstable();

    for h in hashes {
        let (tax, section, dd, imin, imax) = (
            int_of(&format!("tax_{h}")),
            text_of(&format!("section_{h}")),
            text_of(&format!("deductible_date_{h}")),
            int_of(&format!("interest_min_{h}")),
            int_of(&format!("interest_max_{h}")),
        );
        let (Some(tax), Some(section), Some(dd), Some(imin), Some(imax)) =
            (tax, section, dd, imin, imax)
        else {
            out.push(format!(
                "TDSI-1: {h}: missing a figure needed for independent recomputation"
            ));
            continue;
        };
        if imin < 0 || imax < 0 {
            out.push(format!(
                "TDSI-1: {h}: negative bound (min={imin}, max={imax})"
            ));
        }
        if imin > imax {
            out.push(format!("TDSI-1: {h}: min {imin} > max {imax}"));
        }
        let known = |name: &str| text_of(&format!("{name}_{h}")).filter(|t| t != "not supplied");
        let (ed, pd) = (known("deducted_date"), known("paid_date"));
        let span = |a: &str, b: &str| month_span(a, b).ok_or_else(|| bad_date(h));
        let (re_min, re_max) = if is_206c(&section) {
            let (m_min, m_max) = match &pd {
                Some(pd) => {
                    let m = span(&dd, pd)?;
                    (m, m)
                }
                None => (span(&dd, ed.as_deref().unwrap_or(&dd))?, span(&dd, &as_of)?),
            };
            (amount(tax, rate3, m_min), amount(tax, rate3, m_max))
        } else if let Some(ed) = &ed {
            let amt1 = amount(tax, rate1, span(&dd, ed)?);
            match &pd {
                Some(pd) => {
                    let both = amt1 + amount(tax, rate2, span(ed, pd)?);
                    (both, both)
                }
                None => (
                    amt1 + amount(tax, rate2, 0),
                    amt1 + amount(tax, rate2, span(ed, &as_of)?),
                ),
            }
        } else {
            (
                amount(tax, rate1, 0),
                amount(tax, rate1, span(&dd, &as_of)?),
            )
        };
        if re_min != imin {
            out.push(format!(
                "TDSI-1: {h}: published interest_min {imin} != independently recomputed {re_min}"
            ));
        }
        if re_max != imax {
            out.push(format!(
                "TDSI-1: {h}: published interest_max {imax} != independently recomputed {re_max}"
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(iso: &str) -> TallyDate {
        TallyDate::parse(iso.replace('-', "")).unwrap()
    }

    #[test]
    fn a_month_or_part_counts_calendar_months_touched() {
        // The reference's own boundary cases: same day, within a month, one day across months.
        assert_eq!(months_or_part(&d("2026-03-31"), &d("2026-03-31")), 0);
        assert_eq!(months_or_part(&d("2026-03-01"), &d("2026-03-31")), 1);
        assert_eq!(months_or_part(&d("2026-03-31"), &d("2026-04-01")), 2);
        assert_eq!(months_or_part(&d("2025-12-31"), &d("2026-01-01")), 2);
        assert_eq!(months_or_part(&d("2026-04-01"), &d("2026-03-31")), 0);
    }

    #[test]
    fn tdsi_1_reports_a_bound_the_figures_do_not_support() {
        let rules = Rules::vendored().unwrap();
        let row = InterestDefault {
            section: "194C".to_string(),
            payee_key: "p".to_string(),
            tax_paise: 100_000,
            deductible_date: d("2025-06-10"),
            deducted_date: None,
            paid_date: None,
            payee_return_filed_date: None,
            deductor_status_uncertain: false,
            evidence: Vec::new(),
        };
        let mut r = run(&rules, &[row], &d("2026-09-30")).unwrap();
        assert!(check_invariants(&r).unwrap().is_empty());
        let max = r
            .figures
            .iter_mut()
            .find(|f| f.id.starts_with("tds_interest_201.interest_max_"))
            .unwrap();
        max.value = Value::Int(1);
        let out = check_invariants(&r).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("published interest_max 1 != independently recomputed"));
    }

    #[test]
    fn defaults_need_the_rate_table() {
        let rules = Rules {
            tds_rates: None,
            ..Rules::vendored().unwrap()
        };
        let empty = TestResult::new("t", "1", "r");
        let book = Book {
            company_name: String::new(),
            company_guid: String::new(),
            read_at: String::new(),
            groups: std::collections::BTreeMap::new(),
            group_masters: std::collections::BTreeMap::new(),
            ledgers: std::collections::BTreeMap::new(),
            vouchers: Vec::new(),
            tb: std::collections::BTreeMap::new(),
        };
        assert!(matches!(
            defaults_from(&book, &rules, &empty, &empty, false),
            Err(AuditError::Config(_))
        ));
    }
}
