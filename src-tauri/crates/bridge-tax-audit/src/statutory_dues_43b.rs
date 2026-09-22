//! s.43B and s.36(1)(va): statutory dues actually paid, and employees' welfare-fund
//! contributions deposited by their own due date. A port of the reference Python implementation's
//! `statutory_dues_43b` test module, version 2.
//!
//! Each liability ledger is mapped to a nature by client configuration, never by its name. Per
//! nature, opening and closing liability come from the Trial Balance, and charged and paid from
//! the population's own lines on the mapped ledgers.
//!
//! * `pf_employee` / `esi_employee` (s.36(1)(va)): each calendar month's deduction is one lot, due
//!   on `due_day` of the following month; the opening liability is the oldest lot, due on the real
//!   due date of the month before the period. Payments are matched FIFO in date order, on time or
//!   late against each lot's own due date. A lot due after the period end cannot be resolved from
//!   these books and is never called late or on time.
//! * `tds_payable` is not a s.43B sum: figures only.
//! * Every other nature: the closing liability is a books fact whose allowability needs a challan
//!   dated after the year end (clause 26(i)(B)); the opening liability is walked FIFO against this
//!   year's payments for clause 26(i)(A), and its unpaid remainder is kept out of 26(i)(B).
//!
//! `check_invariants` is S43B-1: per nature, opening + charged - paid ties the closing liability,
//! re-derived from the Trial Balance alone.

use std::collections::{BTreeMap, BTreeSet};

use crate::book::{Book, Voucher};
use crate::depreciation::civil_day_number;
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::read::{iso, Window};
use crate::rules::Rules;
use crate::support;

pub const TEST_ID: &str = "statutory_dues_43b";
pub const VERSION: &str = "2";

const EMPLOYEE_CONTRIBUTION_NATURES: [&str; 2] = ["pf_employee", "esi_employee"];
const NOT_S43B_NATURES: [&str; 1] = ["tds_payable"];
const PASS_THROUGH_NATURES: [&str; 2] = ["gst_payable", "gst_payable_rcm"];
const RCM_NATURES: [&str; 1] = ["gst_payable_rcm"];
const PF_NATURES: [&str; 2] = ["pf_employer", "pf_employee"];
const ESI_NATURES: [&str; 2] = ["esi_employer", "esi_employee"];

/// The reference's own fallbacks when the rules carry no `[s43b]` / `[s36_1_va]` table.
const DEFAULT_S43B_AUTHORITY: &str = "s.43B and its proviso (payment on or before the s.139(1) \
return due date); Explanation 5 (employees' contributions are governed by s.36(1)(va), not this \
proviso) -- local prototype default, not yet in rules/ay2026-27.toml";
const DEFAULT_S43B_STATUS: &str = "confirm";
const DEFAULT_S36_1_VA_DUE_DAY: i64 = 15;

/// A calendar month, (year, month).
type Month = (i64, i64);

fn month_of(day_number: i64) -> Month {
    // Inverse of `civil_day_number` (Howard Hinnant's `civil_from_days`), for year and month only.
    let z = day_number + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m)
}

fn next_month((y, m): Month) -> Month {
    if m == 12 {
        (y + 1, 1)
    } else {
        (y, m + 1)
    }
}

/// Day number of `due_day` of the month after `month`; refused, as the reference's `date()`
/// raises, when that day does not exist in that month.
fn due_date(month: Month, due_day: i64) -> Result<i64> {
    let (y, m) = next_month(month);
    let text = format!("{y:04}{m:02}{due_day:02}");
    let d = bridge_tally_primitives::TallyDate::parse(text).map_err(|_| {
        AuditError::Config(format!(
            "{TEST_ID}: due day {due_day} does not exist in {y:04}-{m:02}"
        ))
    })?;
    Ok(civil_day_number(&d))
}

/// One deduction lot of an employee-contribution nature. `month` is `None` for the opening lot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionLot {
    pub month: Option<Month>,
    pub due_day_number: i64,
    pub charged_paise: i64,
    pub paid_on_time_paise: i64,
    pub paid_late_paise: i64,
    pub remaining_paise: i64,
}

/// The reference's `walk_contribution_due_dates`: `lines` are `(day, paise)` in any order.
pub fn walk_contribution_due_dates(
    lines: &[(i64, i64)],
    due_day: i64,
    opening_paise: i64,
    period_start: i64,
) -> Result<(Vec<ContributionLot>, i64)> {
    let overflow = || support::overflow(TEST_ID);
    let mut charges: BTreeMap<Month, i64> = BTreeMap::new();
    let mut payments: Vec<(i64, i64)> = Vec::new();
    for &(d, amt) in lines {
        if amt < 0 {
            let e = charges.entry(month_of(d)).or_insert(0);
            *e = e
                .checked_add(amt.checked_neg().ok_or_else(overflow)?)
                .ok_or_else(overflow)?;
        } else if amt > 0 {
            payments.push((d, amt));
        }
    }
    let mut lots: Vec<ContributionLot> = Vec::new();
    if opening_paise > 0 {
        lots.push(ContributionLot {
            month: None,
            due_day_number: due_date(month_of(period_start - 1), due_day)?,
            charged_paise: opening_paise,
            paid_on_time_paise: 0,
            paid_late_paise: 0,
            remaining_paise: opening_paise,
        });
    }
    for (mk, amt) in charges {
        lots.push(ContributionLot {
            month: Some(mk),
            due_day_number: due_date(mk, due_day)?,
            charged_paise: amt,
            paid_on_time_paise: 0,
            paid_late_paise: 0,
            remaining_paise: amt,
        });
    }
    payments.sort_by_key(|&(d, _)| d);
    let mut unmatched_advance = 0i64;
    for (pdate, pamt) in payments {
        let mut remaining_pay = pamt;
        for lot in &mut lots {
            if remaining_pay <= 0 {
                break;
            }
            if lot.remaining_paise <= 0 {
                continue;
            }
            let take = remaining_pay.min(lot.remaining_paise);
            if pdate <= lot.due_day_number {
                lot.paid_on_time_paise += take;
            } else {
                lot.paid_late_paise += take;
            }
            lot.remaining_paise -= take;
            remaining_pay -= take;
        }
        if remaining_pay > 0 {
            unmatched_advance = unmatched_advance
                .checked_add(remaining_pay)
                .ok_or_else(overflow)?;
        }
    }
    Ok((lots, unmatched_advance))
}

/// One lot of the clause 26(i)(A) walk; never popped once paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExhaustionLot {
    pub is_opening: bool,
    pub paid_paise: i64,
    pub remaining_paise: i64,
}

/// The reference's `walk_opening_exhaustion`: the opening liability as the oldest lot, then this
/// year's bills and payments sorted (stably) by day, bills before payments on one day.
pub fn walk_opening_exhaustion(
    opening_paise: i64,
    lines: &[(i64, i64)],
) -> Result<Vec<ExhaustionLot>> {
    let overflow = || support::overflow(TEST_ID);
    let mut lots: Vec<ExhaustionLot> = Vec::new();
    if opening_paise > 0 {
        lots.push(ExhaustionLot {
            is_opening: true,
            paid_paise: 0,
            remaining_paise: opening_paise,
        });
    }
    let mut sorted = lines.to_vec();
    sorted.sort_by_key(|&(d, amt)| (d, amt > 0));
    for (_, amt) in sorted {
        if amt < 0 {
            lots.push(ExhaustionLot {
                is_opening: false,
                paid_paise: 0,
                remaining_paise: amt.checked_neg().ok_or_else(overflow)?,
            });
        } else if amt > 0 {
            let mut pay = amt;
            for lot in &mut lots {
                if pay <= 0 {
                    break;
                }
                if lot.remaining_paise <= 0 {
                    continue;
                }
                let take = pay.min(lot.remaining_paise);
                lot.paid_paise += take;
                lot.remaining_paise -= take;
                pay -= take;
            }
        }
    }
    Ok(lots)
}

struct NatureLines<'a> {
    charged: i64,
    paid: i64,
    lines: Vec<(i64, i64)>,
    vouchers: BTreeMap<&'a str, &'a Voucher>,
}

fn compute_nature_lines<'a>(
    pop: &[&'a Voucher],
    ledgers: &BTreeSet<&str>,
) -> Result<NatureLines<'a>> {
    let overflow = || support::overflow(TEST_ID);
    let mut out = NatureLines {
        charged: 0,
        paid: 0,
        lines: Vec::new(),
        vouchers: BTreeMap::new(),
    };
    for v in pop {
        let day = civil_day_number(&v.date);
        for l in &v.lines {
            if l.amount_paise == 0 || !ledgers.contains(l.ledger.as_str()) {
                continue;
            }
            out.lines.push((day, l.amount_paise));
            out.vouchers.insert(v.guid.as_str(), *v);
            if l.amount_paise < 0 {
                out.charged = out
                    .charged
                    .checked_add(l.amount_paise.checked_neg().ok_or_else(overflow)?)
                    .ok_or_else(overflow)?;
            } else {
                out.paid = out.paid.checked_add(l.amount_paise).ok_or_else(overflow)?;
            }
        }
    }
    Ok(out)
}

fn voucher_refs(vouchers: &BTreeMap<&str, &Voucher>) -> Vec<EvidenceRef> {
    vouchers
        .iter()
        .map(|(g, v)| EvidenceRef::with_label("voucher", g, &support::voucher_label(v)))
        .collect()
}

fn ledger_refs<'a>(names: impl Iterator<Item = &'a str>) -> Vec<EvidenceRef> {
    names.map(|n| EvidenceRef::new("ledger", n)).collect()
}

fn s(text: &str) -> String {
    text.to_string()
}

pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    nature_by_ledger: &BTreeMap<String, String>,
    salary_expense_ledgers: &BTreeSet<String>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let overflow = || support::overflow(TEST_ID);
    let pop = book.population()?;
    r.population_note = "Books population (optional, cancelled and post-dated vouchers excluded). \
Each nature's opening/closing liability is the Trial Balance sum across every ledger client config \
maps to it; charged/paid are population voucher-line sums on the same ledgers (TOT-1: never a \
filtered sub-table)."
        .to_string();

    let (s43b_authority, s43b_status) = match &rules.s43b {
        Some((a, st)) => (a.as_str(), st.as_str()),
        None => (DEFAULT_S43B_AUTHORITY, DEFAULT_S43B_STATUS),
    };
    let due_day = rules.s36_1_va_due_day.unwrap_or(DEFAULT_S36_1_VA_DUE_DAY);

    let mut ledgers_by_nature: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (ledger, nature) in nature_by_ledger {
        ledgers_by_nature
            .entry(nature.as_str())
            .or_default()
            .insert(ledger.as_str());
    }
    let period_start = civil_day_number(&period.from);
    let period_end = civil_day_number(&period.to);

    for (nature, ledgers) in &ledgers_by_nature {
        let nature = *nature;
        let ledger_ev = ledger_refs(ledgers.iter().copied());
        let no_tb_row: Vec<&str> = ledgers
            .iter()
            .copied()
            .filter(|n| !book.tb.contains_key(*n))
            .collect();
        let (mut opening, mut closing) = (0i64, 0i64);
        for n in ledgers {
            if let Some(t) = book.tb.get(*n) {
                opening = opening.checked_sub(t.opening_paise).ok_or_else(overflow)?;
                closing = closing.checked_sub(t.closing_paise).ok_or_else(overflow)?;
            }
        }
        let f_open = r.fig(
            &format!("opening_liability_{nature}"),
            Value::Int(opening),
            Unit::Paise,
            &format!(
                "Trial Balance opening balance (Dr+/Cr- flipped to a liability-positive figure), \
summed across every ledger client config maps to nature '{nature}'."
            ),
            ledger_ev.clone(),
        );
        let f_close = r.fig(
            &format!("closing_liability_{nature}"),
            Value::Int(closing),
            Unit::Paise,
            &format!(
                "Trial Balance closing balance (Dr+/Cr- flipped to a liability-positive figure), \
summed across every ledger client config maps to nature '{nature}'."
            ),
            ledger_ev.clone(),
        );
        if !no_tb_row.is_empty() {
            r.fig(
                &format!("ledgers_without_tb_row_count_{nature}"),
                support::count(TEST_ID, no_tb_row.len())?,
                Unit::Count,
                &format!(
                    "Ledgers mapped to nature '{nature}' with no Trial Balance row at all \
(opening/closing treated as nil)."
                ),
                ledger_refs(no_tb_row.iter().copied()),
            );
        }

        let nl = compute_nature_lines(&pop, ledgers)?;
        let voucher_ev = voucher_refs(&nl.vouchers);
        let f_charged = r.fig(
            &format!("charged_{nature}"),
            Value::Int(nl.charged),
            Unit::Paise,
            &format!(
                "Sum of population voucher CREDIT lines (amount added to the liability) on ledgers \
mapped to nature '{nature}'."
            ),
            voucher_ev.clone(),
        );
        r.fig(
            &format!("paid_{nature}"),
            Value::Int(nl.paid),
            Unit::Paise,
            &format!(
                "Sum of population voucher DEBIT lines (amount paid against the liability) on \
ledgers mapped to nature '{nature}'."
            ),
            voucher_ev,
        );

        if NOT_S43B_NATURES.contains(&nature) {
            continue;
        }
        if EMPLOYEE_CONTRIBUTION_NATURES.contains(&nature) {
            employee_contribution(
                &mut r,
                nature,
                &nl,
                due_day,
                period_start,
                &period.to,
                period_end,
                opening,
                &f_charged,
                &f_close,
            )?;
        } else {
            plain_s43b_nature(
                &mut r,
                nature,
                ledgers,
                &nl,
                rules,
                (s43b_authority, s43b_status),
                opening,
                closing,
                (&f_open, &f_charged),
                &ledger_ev,
            )?;
        }
    }

    coverage_findings(&mut r, nature_by_ledger, salary_expense_ledgers);
    Ok(r)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn plain_s43b_nature(
    r: &mut TestResult,
    nature: &str,
    ledgers: &BTreeSet<&str>,
    nl: &NatureLines,
    rules: &Rules,
    (s43b_authority, s43b_status): (&str, &str),
    opening: i64,
    closing: i64,
    (f_open, f_charged): (&str, &str),
    ledger_ev: &[EvidenceRef],
) -> Result<()> {
    let opening_lot = if opening > 0 {
        walk_opening_exhaustion(opening, &nl.lines)?
            .into_iter()
            .find(|l| l.is_opening)
    } else {
        None
    };
    let opening_unpaid_remainder = opening_lot.as_ref().map_or(0, |l| l.remaining_paise);
    let this_year_closing = closing
        .checked_sub(opening_unpaid_remainder)
        .ok_or_else(|| support::overflow(TEST_ID))?;
    let spaced = nature.replace('_', " ");

    if this_year_closing > 0 {
        // The vendored rules always carry `[due_dates]`, and its return date is never empty.
        let due_text = format!(
            "the return due date under s.139(1) (rules.due_dates.return_audit_case = {}, \
status={})",
            rules.due_date_return_audit_case, rules.due_dates_status
        );
        let f_this_year_close = r.fig(
            &format!("this_year_closing_liability_{nature}"),
            Value::Int(this_year_closing),
            Unit::Paise,
            &format!(
                "3CD-26(i)(B): THIS YEAR's own unpaid s.43B liability for nature '{nature}' -- the \
Trial Balance closing balance LESS any still-unpaid remainder of the OPENING (pre-existing) \
liability reported separately under clause 26(i)(A)(b) below, so the same rupee is never counted \
under both clauses (26-7 fix, GN 46.4-46.7)."
            ),
            ledger_ev.to_vec(),
        );
        let mut limits = vec![
            format!(
                "Allowability under s.43B turns on payment on or before {due_text}; a challan or \
payment proof dated after 31 March is needed and is not in this book. ({s43b_authority}, \
status={s43b_status})"
            ),
            s("Clause 26(i)(B) splits (a) paid on or before the s.139(1) due date from (b) not so \
paid; this closing balance alone cannot be placed in either without post-year payment evidence (a \
challan or bank statement dated after 31 March), so this finding stays tagged the bare 26(i)(B), \
not (a) or (b)."),
        ];
        if PASS_THROUGH_NATURES.contains(&nature) {
            limits.push(s(
                "Under the exclusive method (GST kept outside profit and loss, clause \
26(ii) answered 'No'), s.43B does not disallow this amount in computing income because it was \
never claimed as a deduction; the Guidance Note still requires it to be shown as a NOTE under \
clause 26 -- the amount collected but not paid, and the date/amount of any payment made before the \
due date (GN 46.17, 46.23). Confirm the accounting method (exclusive vs inclusive) before wording \
clause 26.",
            ));
        }
        if RCM_NATURES.contains(&nature) {
            limits.push(s(
                "This is reverse-charge (RCM) GST payable: GN 46.26 separately requires \
RCM GST booked but not paid by the due date to be reported under clause 26, in addition to the \
exclusive-method note above.",
            ));
        }
        r.findings.push(Finding {
            id: format!("{TEST_ID}/unpaid/{nature}"),
            clauses: vec![s("s.43B"), s("3CD-26(i)(B)")],
            title: format!(
                "Closing balance under s.43B for {spaced}, incurred this year, as recorded in the \
books"
            ),
            facts: vec![
                (s("closing_liability"), f_this_year_close),
                (s("opening_liability"), f_open.to_string()),
                (s("charged"), f_charged.to_string()),
            ],
            evidence: ledger_ev.to_vec(),
            confidence: Confidence::NeedsDocument,
            limits,
            ask_client: vec![format!(
                "Challan(s) or payment proof for '{nature}' dated after 31 March, on or before \
the s.139(1) return due date."
            )],
        });
    }

    let Some(opening_lot) = opening_lot else {
        return Ok(());
    };
    let payment_ev: Vec<EvidenceRef> = nl
        .vouchers
        .iter()
        .filter(|(_, v)| {
            v.lines
                .iter()
                .any(|l| ledgers.contains(l.ledger.as_str()) && l.amount_paise > 0)
        })
        .map(|(g, v)| EvidenceRef::with_label("voucher", g, &support::voucher_label(v)))
        .collect();
    let f_paid = r.fig(
        &format!("opening_s43b_paid_this_year_{nature}"),
        Value::Int(opening_lot.paid_paise),
        Unit::Paise,
        &format!(
            "Clause 26(i)(A)(a) CANDIDATE: portion of nature '{nature}'s opening (pre-existing) \
Trial Balance liability matched, FIFO oldest-first, to this year's own payment vouchers on its \
mapped ledgers. NOT a settled 'allowable' fact: GN 46.5 limits 26(i)(A) to a sum that was NOT \
already allowable in an earlier year, which can only be verified from last year's own income-tax \
return/Form 3CD (not available to this module) -- treating the whole TB opening balance as \
26(i)(A) risks double-deducting an amount already allowed last year."
        ),
        payment_ev.clone(),
    );
    let f_unpaid = r.fig(
        &format!("opening_s43b_still_unpaid_{nature}"),
        Value::Int(opening_lot.remaining_paise),
        Unit::Paise,
        &format!(
            "Clause 26(i)(A)(b) CANDIDATE: portion of nature '{nature}'s opening (pre-existing) \
liability still unmatched to any payment in this year's population. Same GN 46.5 limitation as \
the paid portion above: this is a candidate needing last year's return to confirm it was not \
already allowable in an earlier year, not a settled figure; it carries forward as next year's own \
opening balance either way, and is EXCLUDED from clause 26(i)(B) above (26-7 fix) so it is never \
counted twice."
        ),
        ledger_ev.to_vec(),
    );
    if opening_lot.paid_paise > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/opening_paid/{nature}"),
            clauses: vec![s("s.43B"), s("3CD-26(i)(A)(a)")],
            title: format!(
                "Sum under s.43B for '{spaced}' incurred in an earlier year, paid during this \
year: a clause 26(i)(A) candidate"
            ),
            facts: vec![
                (s("opening_paid"), f_paid),
                (s("opening_liability"), f_open.to_string()),
            ],
            evidence: payment_ev,
            confidence: Confidence::Indicative,
            limits: vec![
                s("FIFO order is an assumption about which specific bill or liability instalment a \
payment settled first -- the ledger itself does not say (the same assumption used for creditor \
ageing elsewhere in this pack); confirm against the challan/payment advice if the exact \
instalment paid matters."),
                s("Candidate only (GN 46.5): this is only a clause 26(i)(A)(a) amount if it was \
NOT already allowable in an earlier year -- that needs last year's own income-tax return/Form \
3CD, which this module does not have."),
            ],
            ask_client: vec![
                s("Challan(s)/payment proof identifying which specific prior-year dues these \
payments settled, to confirm the FIFO attribution."),
                s("Last year's income-tax return/Form 3CD, to confirm this amount was not already \
allowable in an earlier year."),
            ],
        });
    }
    if opening_lot.remaining_paise > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/opening_unpaid/{nature}"),
            clauses: vec![s("s.43B"), s("3CD-26(i)(A)(b)")],
            title: format!(
                "Sum under s.43B for '{spaced}' incurred in an earlier year, still unpaid at this \
year end: a clause 26(i)(A) candidate"
            ),
            facts: vec![
                (s("opening_unpaid"), f_unpaid),
                (s("opening_liability"), f_open.to_string()),
            ],
            evidence: ledger_ev.to_vec(),
            confidence: Confidence::NeedsDocument,
            limits: vec![s(
                "Candidate only (GN 46.5): this is only a clause 26(i)(A)(b) amount if \
it was NOT already allowable in an earlier year -- that needs last year's own income-tax \
return/Form 3CD, which this module does not have, so this is NOT a settled COMPUTED figure. If \
and when it is paid, it is allowed only in the year of actual payment (GN 46.4-46.5: no \
return-due-date relief for a pre-existing liability); it carries forward as next year's own \
opening balance either way.",
            )],
            ask_client: vec![s(
                "Last year's income-tax return/Form 3CD, to confirm whether this \
amount was already allowable in an earlier year.",
            )],
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn employee_contribution(
    r: &mut TestResult,
    nature: &str,
    nl: &NatureLines,
    due_day: i64,
    period_start: i64,
    period_end_date: &bridge_tally_primitives::TallyDate,
    period_end: i64,
    opening: i64,
    f_charged: &str,
    f_close: &str,
) -> Result<()> {
    let overflow = || support::overflow(TEST_ID);
    let add = |a: i64, b: i64| a.checked_add(b).ok_or_else(overflow);
    let (lots, unmatched_advance) =
        walk_contribution_due_dates(&nl.lines, due_day, opening, period_start)?;
    let end_iso = iso(period_end_date);
    let (mut on_time, mut late, mut not_visible, mut visible_unpaid) = (0i64, 0i64, 0i64, 0i64);
    for l in lots.iter().filter(|l| l.month.is_some()) {
        on_time = add(on_time, l.paid_on_time_paise)?;
        late = add(late, l.paid_late_paise)?;
        if l.due_day_number > period_end {
            not_visible = add(not_visible, l.remaining_paise)?;
        } else if l.remaining_paise > 0 {
            visible_unpaid = add(visible_unpaid, l.remaining_paise)?;
        }
    }
    let opening_lot = lots.iter().find(|l| l.month.is_none());
    let lot_ev = voucher_refs(&nl.vouchers);

    let f_on_time = r.fig(
        &format!("deposited_on_time_{nature}"),
        Value::Int(on_time),
        Unit::Paise,
        &format!(
            "Sum of '{nature}' deduction lots' payments dated on or before that month's own due \
date (FIFO, oldest lot first; s.36(1)(va)). Excludes any opening (prior-year) liability lot -- see \
opening_liability_paid_this_year_<nature> below."
        ),
        lot_ev.clone(),
    );
    let f_late = r.fig(
        &format!("deposited_late_{nature}"),
        Value::Int(late),
        Unit::Paise,
        &format!(
            "Sum of '{nature}' deduction lots' payments dated after that month's own due date \
(FIFO, oldest lot first). Excludes any opening (prior-year) liability lot."
        ),
        lot_ev.clone(),
    );
    let f_not_visible = r.fig(
        &format!("not_visible_after_year_end_{nature}"),
        Value::Int(not_visible),
        Unit::Paise,
        &format!(
            "Sum of '{nature}' deduction lots whose due date falls after {end_iso} (the FY's \
voucher population has no postings after this date, so on-time/late cannot be determined from \
this book at all)."
        ),
        lot_ev.clone(),
    );
    let f_visible_unpaid = r.fig(
        &format!("unpaid_due_date_passed_{nature}"),
        Value::Int(visible_unpaid),
        Unit::Paise,
        &format!(
            "Sum of '{nature}' deduction lots whose due date fell on or before {end_iso} and still \
had no matching payment recorded in the books by then."
        ),
        lot_ev.clone(),
    );
    if unmatched_advance != 0 {
        r.fig(
            &format!("unmatched_advance_{nature}"),
            Value::Int(unmatched_advance),
            Unit::Paise,
            &format!(
                "Payments on '{nature}'-mapped ledgers left over after every open deduction lot \
(incl. any opening lot) was exhausted (an advance deposit, or a payment the books cannot match to \
a specific month's deduction)."
            ),
            lot_ev.clone(),
        );
    }

    if let Some(opening_lot) = opening_lot {
        let opening_paid_on_time = opening_lot.paid_on_time_paise;
        let opening_paid_late = opening_lot.paid_late_paise;
        let opening_paid = add(opening_paid_on_time, opening_paid_late)?;
        r.fig(
            &format!("opening_liability_paid_this_year_{nature}"),
            Value::Int(opening_paid),
            Unit::Paise,
            &format!(
                "Sum of this year's payments on '{nature}'-mapped ledgers FIFO-matched to the \
OPENING (prior-year) liability lot, seeded so a payment early in the year settles last year's \
carried-over deduction first rather than being misattributed to this year's own lots (Phase 1b, \
relay item 8). Split by s.36(1)(va) due date below (on_time_paise/late_paise), not assumed \
disallowed as a whole."
            ),
            lot_ev.clone(),
        );
        let f_open_paid_on_time = r.fig(
            &format!("opening_liability_paid_on_time_{nature}"),
            Value::Int(opening_paid_on_time),
            Unit::Paise,
            "Of the opening-lot payment above, the portion paid on or before that deduction \
month's OWN due date (s.36(1)(va)) -- allowable in the earlier year it was deducted, not \
disallowed.",
            lot_ev.clone(),
        );
        let f_open_paid_late = r.fig(
            &format!("opening_liability_paid_late_{nature}"),
            Value::Int(opening_paid_late),
            Unit::Paise,
            "Of the opening-lot payment above, the portion paid AFTER that deduction month's own \
due date (s.36(1)(va)) -- permanently disallowed for the earlier year it was deducted (Checkmate \
Services P Ltd v CIT (2022) SC), not reassessed this year.",
            lot_ev.clone(),
        );
        let f_open_unpaid = r.fig(
            &format!("opening_liability_unpaid_{nature}"),
            Value::Int(opening_lot.remaining_paise),
            Unit::Paise,
            &format!(
                "Remaining balance of the opening (prior-year) '{nature}' liability lot with no \
payment matched to it in this year's population -- its own due date has already passed (it \
pre-dates this PY), so it is permanently disallowed for the earlier year it was deducted \
(s.36(1)(va), Checkmate), and still an unresolved PF/ESI Act deposit besides."
            ),
            lot_ev.clone(),
        );
        if opening_paid_late > 0 {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/opening_paid_late/{nature}"),
                clauses: vec![s("s.36(1)(va)")],
                title: format!(
                    "Employees' contribution ('{nature}') carried over from a prior year, paid \
this year AFTER its own due date"
                ),
                facts: vec![(s("opening_paid_late"), f_open_paid_late)],
                evidence: lot_ev.clone(),
                confidence: Confidence::Indicative,
                limits: vec![s(
                    "Permanently disallowed for the earlier year in which it was \
deducted (s.36(1)(va), Checkmate Services P Ltd v CIT (2022) SC) -- not an income-tax question for \
THIS year, but flagged because the deposit is only now showing up in this year's payment \
vouchers.",
                )],
                ask_client: vec![s(
                    "PF/ESI challans for this amount, to confirm the actual deposit \
date against the voucher date used here.",
                )],
            });
        }
        if opening_paid_on_time > 0 {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/opening_paid_on_time/{nature}"),
                clauses: vec![s("s.36(1)(va)")],
                title: format!(
                    "Employees' contribution ('{nature}') carried over from a prior year, paid \
this year by its own due date"
                ),
                facts: vec![(s("opening_paid_on_time"), f_open_paid_on_time)],
                evidence: lot_ev.clone(),
                confidence: Confidence::Indicative,
                limits: vec![s(
                    "Deposited by its own s.36(1)(va) due date, so allowable in the \
earlier year it was deducted -- not disallowed; shown here only because the matching payment \
voucher falls inside this year's population.",
                )],
                ask_client: Vec::new(),
            });
        }
        if opening_lot.remaining_paise > 0 {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/opening_still_unpaid/{nature}"),
                clauses: vec![s("s.36(1)(va)")],
                title: format!(
                    "Employees' contribution ('{nature}') carried over from a prior year is still \
unpaid"
                ),
                facts: vec![(s("opening_unpaid"), f_open_unpaid)],
                evidence: lot_ev.clone(),
                confidence: Confidence::NeedsDocument,
                limits: vec![s(
                    "Already permanently disallowed for income tax in the year the \
deduction was made, since its own due date has passed unpaid (s.36(1)(va), Checkmate Services P \
Ltd v CIT (2022) SC); flagged here only because it remains an open PF/ESI Act deposit question, \
not an income-tax question for this year.",
                )],
                ask_client: vec![s("Confirm whether and when this carried-over amount was \
eventually deposited.")],
            });
        }
    }

    if late > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/late/{nature}"),
            clauses: vec![s("s.36(1)(va)"), s("3CD-20(b)")],
            title: format!("Employees' contribution ('{nature}') deposited after its own due date"),
            facts: vec![
                (s("deposited_late"), f_late),
                (s("deposited_on_time"), f_on_time),
                (s("charged"), f_charged.to_string()),
            ],
            evidence: lot_ev.clone(),
            confidence: Confidence::Indicative,
            limits: vec![s(
                "The deposit date used here is the payment voucher's own date in the \
books, not the PF/ESI challan's acknowledgement date -- confirm against the actual challans. \
Checkmate Services P Ltd v CIT (2022) (SC): the s.43B return-date proviso does not extend to \
employees' contributions, so a late deposit is not allowable however small the delay; a CA \
judgement applies only to a specific fact affecting the due date itself (e.g. the 15th falling on \
a bank holiday), never to the rule.",
            )],
            ask_client: vec![s(
                "PF/ESI challans for the late-deposited amount, to confirm the \
actual deposit date against the voucher date used here.",
            )],
        });
    }
    if visible_unpaid > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/unpaid/{nature}"),
            clauses: vec![s("s.36(1)(va)"), s("3CD-20(b)")],
            title: format!(
                "Employees' contribution ('{nature}') charged but no deposit recorded by its due \
date, which has already passed"
            ),
            facts: vec![
                (s("unpaid_due_date_passed"), f_visible_unpaid),
                (s("charged"), f_charged.to_string()),
                (s("closing_liability"), f_close.to_string()),
            ],
            evidence: lot_ev.clone(),
            confidence: Confidence::Computed,
            limits: vec![s(
                "Checkmate Services P Ltd v CIT (2022) (SC): not allowable for this \
year regardless of whether it is eventually deposited later.",
            )],
            ask_client: vec![s(
                "Confirm whether and when this amount was eventually deposited, \
for interest exposure under the PF/ESI Act (a separate question from income-tax allowability).",
            )],
        });
    }
    if not_visible > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/not_visible/{nature}"),
            clauses: vec![s("s.36(1)(va)")],
            title: format!(
                "Employees' contribution ('{nature}') for the year's last deduction month(s): due \
date falls after year end, not visible in this book"
            ),
            facts: vec![
                (s("not_visible"), f_not_visible),
                (s("charged"), f_charged.to_string()),
            ],
            evidence: lot_ev,
            confidence: Confidence::NeedsDocument,
            limits: vec![s(
                "This FY's voucher export ends 31 March; a due date of 15 April or \
later falls outside it, so whether this amount was deposited on time cannot be determined from \
the books at all, on either side.",
            )],
            ask_client: vec![s("PF/ESI challans dated after 31 March for this month's \
deduction.")],
        });
    }
    Ok(())
}

fn coverage_findings(
    r: &mut TestResult,
    nature_by_ledger: &BTreeMap<String, String>,
    salary_expense_ledgers: &BTreeSet<String>,
) {
    if salary_expense_ledgers.is_empty() {
        return;
    }
    let mapped: BTreeSet<&str> = nature_by_ledger.values().map(String::as_str).collect();
    for (label, natures) in [("PF", PF_NATURES), ("ESI", ESI_NATURES)] {
        if natures.iter().any(|n| mapped.contains(n)) {
            continue;
        }
        r.findings.push(Finding {
            id: format!("{TEST_ID}/coverage/{}", label.to_lowercase()),
            clauses: vec![s("s.36(1)(va)"), s("s.43B"), s("3CD-20(b)")],
            title: format!("Salary expense exists but no ledger is mapped to a {label} nature"),
            facts: Vec::new(),
            evidence: ledger_refs(salary_expense_ledgers.iter().map(String::as_str)),
            confidence: Confidence::JudgementRequired,
            limits: vec![format!(
                "Whether the establishment is covered under the {label} Act depends on \
employee-count thresholds measured monthly; headcount per month from salary vouchers is out of \
scope for this test and is not computed here."
            )],
            ask_client: vec![format!(
                "Confirm whether the establishment is registered/covered under the {label} Act, \
and if not, the basis (e.g. headcount below the threshold every month)."
            )],
        });
    }
}

/// S43B-1, re-derived from the Trial Balance alone for the ledgers each `closing_liability_<nature>`
/// figure cites.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let prefix = format!("{}.", result.test_id);
    let marker = format!("{prefix}closing_liability_");
    let tol_paise: i128 = 100;
    let int_of = |v: &Value| -> Result<i128> {
        match v {
            Value::Int(n) => Ok(i128::from(*n)),
            _ => Err(AuditError::Config(format!(
                "{TEST_ID}: S43B-1 read a non-integer figure"
            ))),
        }
    };
    let mut figures: Vec<&crate::findings::Figure> = result.figures.iter().collect();
    figures.sort_by(|a, b| a.id.cmp(&b.id));
    for fig in figures {
        let Some(nature) = fig.id.strip_prefix(&marker) else {
            continue;
        };
        let mut ledgers: Vec<&str> = fig
            .evidence
            .iter()
            .filter(|e| e.kind == "ledger")
            .map(|e| e.id.as_str())
            .collect();
        ledgers.sort_unstable();
        if ledgers.is_empty() {
            out.push(format!(
                "S43B-1: {} carries no ledger evidence to re-derive from the Trial Balance",
                fig.id
            ));
            continue;
        }
        let (mut opening_r, mut closing_r, mut charged_r, mut paid_r) =
            (0i128, 0i128, 0i128, 0i128);
        for n in &ledgers {
            if let Some(t) = book.tb.get(*n) {
                opening_r -= i128::from(t.opening_paise);
                closing_r -= i128::from(t.closing_paise);
                charged_r += i128::from(t.credit_paise);
                paid_r += i128::from(t.debit_paise);
            }
        }
        let open_fid = format!("{prefix}opening_liability_{nature}");
        if let Some(f) = result.figures.iter().find(|f| f.id == open_fid) {
            let reported = int_of(&f.value)?;
            if reported != opening_r {
                out.push(format!(
                    "S43B-1: {nature} opening_liability figure ({reported}p) does not match the \
Trial Balance sum for its own evidenced ledgers ({opening_r}p)"
                ));
            }
        }
        let value = int_of(&fig.value)?;
        if value != closing_r {
            out.push(format!(
                "S43B-1: {nature} closing_liability figure ({value}p) does not match the Trial \
Balance sum for its own evidenced ledgers ({closing_r}p)"
            ));
        }
        let derived_closing = opening_r + charged_r - paid_r;
        let diff = derived_closing - closing_r;
        if diff.abs() > tol_paise {
            out.push(format!(
                "S43B-1: {nature} opening ({opening_r}p) + charged ({charged_r}p) - paid \
({paid_r}p) = {derived_closing}p does not equal the Trial Balance closing ({closing_r}p); \
difference {diff}p"
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_tally_primitives::TallyDate;

    fn day(iso: &str) -> i64 {
        civil_day_number(&TallyDate::parse(iso.replace('-', "")).unwrap())
    }

    #[test]
    fn month_of_inverts_the_day_number() {
        for d in [
            "2025-03-31",
            "2025-04-01",
            "2024-02-29",
            "2025-12-31",
            "2026-01-01",
        ] {
            let (y, m) = month_of(day(d));
            assert_eq!(format!("{y:04}-{m:02}"), d[..7], "{d}");
        }
    }

    #[test]
    fn the_opening_lot_is_due_the_month_after_the_last_pre_period_month() {
        // Period from 1 April: the opening lot is March's deduction, due 15 April.
        let (lots, adv) = walk_contribution_due_dates(
            &[(day("2025-04-15"), 100), (day("2025-04-16"), 50)],
            15,
            120,
            day("2025-04-01"),
        )
        .unwrap();
        assert_eq!(lots[0].month, None);
        assert_eq!(lots[0].due_day_number, day("2025-04-15"));
        assert_eq!(
            (lots[0].paid_on_time_paise, lots[0].paid_late_paise),
            (100, 20)
        );
        assert_eq!(adv, 30);
    }

    #[test]
    fn a_period_starting_mid_month_takes_the_opening_lot_from_the_day_before() {
        // The reference, walk_contribution_due_dates([(16 Apr, 50)], 15, 120, 2 Apr 2025): the day
        // before the period is 1 April, so the opening lot is April's, due 15 May, and a payment on
        // 16 April is on time.
        let (lots, adv) =
            walk_contribution_due_dates(&[(day("2025-04-16"), 50)], 15, 120, day("2025-04-02"))
                .unwrap();
        assert_eq!(lots.len(), 1);
        assert_eq!(lots[0].due_day_number, day("2025-05-15"));
        assert_eq!(
            (
                lots[0].paid_on_time_paise,
                lots[0].paid_late_paise,
                lots[0].remaining_paise
            ),
            (50, 0, 70)
        );
        assert_eq!(adv, 0);
    }

    /// S43B-1's three reporting branches, which `run()` never reaches (its own figures always carry
    /// their ledgers and match the TB). The expected messages are the reference's own
    /// `check_invariants` on the same hand-made result.
    #[test]
    fn s43b_1_reports_mismatches_and_missing_evidence_as_the_reference_does() {
        use crate::book::{Ledger, TbRow};
        let book = Book {
            company_name: "c".to_string(),
            company_guid: "g".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: BTreeMap::from([(
                "X".to_string(),
                Ledger {
                    name: "X".to_string(),
                    parent: "Duties & Taxes".to_string(),
                    chain: vec!["Duties & Taxes".to_string()],
                    chain_complete: true,
                    master_opening_paise: 0,
                    guid: "gx".to_string(),
                    masterid: None,
                },
            )]),
            vouchers: Vec::new(),
            tb: BTreeMap::from([(
                "X".to_string(),
                TbRow {
                    opening_paise: -100,
                    debit_paise: 50,
                    credit_paise: 250,
                    closing_paise: -300,
                },
            )]),
        };
        let mut r = TestResult::new(TEST_ID, VERSION, "v");
        let x = || vec![EvidenceRef::new("ledger", "X")];
        r.fig("opening_liability_a", Value::Int(1), Unit::Paise, "d", x());
        r.fig(
            "closing_liability_a",
            Value::Int(999),
            Unit::Paise,
            "d",
            x(),
        );
        r.fig(
            "closing_liability_b",
            Value::Int(0),
            Unit::Paise,
            "d",
            Vec::new(),
        );
        assert_eq!(
            check_invariants(&book, &r).unwrap(),
            vec![
                "S43B-1: a opening_liability figure (1p) does not match the Trial Balance sum for \
its own evidenced ledgers (100p)",
                "S43B-1: a closing_liability figure (999p) does not match the Trial Balance sum for \
its own evidenced ledgers (300p)",
                "S43B-1: statutory_dues_43b.closing_liability_b carries no ledger evidence to \
re-derive from the Trial Balance",
            ]
        );
    }

    #[test]
    fn a_due_day_that_does_not_exist_is_refused() {
        assert!(due_date((2025, 1), 30).is_err());
        assert!(due_date((2025, 1), 28).is_ok());
    }
}
