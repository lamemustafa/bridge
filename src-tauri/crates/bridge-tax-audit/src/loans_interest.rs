// SPDX-License-Identifier: Apache-2.0
//! Loans and deposits: s.194A interest TDS, Form 3CD Clause 31(a)/(c) (loans and deposits taken
//! and repaid in the year) and the s.269SS/269T candidates within them, with the interest on a
//! declared-shared interest ledger that no configured loan accounts for. A line-for-line port;
//! the reference module's docstring is the design record, and only what differs here is stated:
//!
//! * Every amount is summed in i128 and refused if a figure does not fit i64, where the
//!   reference's integers are unbounded.
//! * A loan entry's `lender` and `lender_type` must be strings; the reference formats any value
//!   with `str()`.
//! * Two vouchers with the same GUID on one loan ledger, in the same direction, would repeat a
//!   Clause 31 row's figure id: the reference raises there, and this refuses.
//! * Voucher identity (the reference's `id(v)`) is the voucher's position in the population.
//! * The shared-ledger rule is switched by `net_reversals`: [`run`] and [`check_invariants`] pass
//!   [`NET_REVERSALS`], and the tests reach the dormant reversal rule through [`run_with`] and
//!   [`check_invariants_with`] instead of rebinding a module global.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sha1::{Digest, Sha1};

use crate::book::{Book, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::rules::Rules;
use crate::support::{
    count, ledgers_by_tag, overflow, py_lower, py_repr_str, py_strip, voucher_label,
};
use crate::tds_payees::{deductor_status, py_format_g};

pub const TEST_ID: &str = "loans_interest";
pub const VERSION: &str = "1";

const MODE_CASH: &str = "cash";
const MODE_BANK: &str = "bank";
const MODE_JOURNAL: &str = "journal";
const MODE_OTHER: &str = "other";
const NON_ACCOUNT_PAYEE_MODES: [&str; 3] = [MODE_CASH, MODE_JOURNAL, MODE_OTHER];

const DEFAULT_269_EXEMPT_LENDER_TYPES: [&str; 2] = ["bank", "cooperative_bank"];
const DEFAULT_269_REPORTING_EXEMPT_LENDER_TYPES: [&str; 4] = [
    "government",
    "government_company",
    "bank",
    "statutory_corporation",
];

/// The note the unattributed figure on a shared interest ledger carries while no credit reduces
/// it (owner decision, 2026-09-23).
pub const NEVER_NET_NOTE: &str = "No credit reduces it: every credit to this ledger on these \
vouchers (a reversal, interest received, interest charged on to a debtor, a rectification) is shown \
beside it as a separate figure, never netted.";

/// The earlier reversal rule's note (owner decision (b), 2026-09-22), kept with the rule.
pub const REVERSAL_RULE_NOTE: &str = "Only a credit that looks like the reversal of a specific \
earlier debit reduces it: a credit on another voucher, not a Receipt, of the same amount, with the \
same other ledgers, dated on or after the debit, each debit reversed at most once and matched \
earliest first. Every other credit (interest received on a Receipt, interest charged on to a \
debtor, an unmatched rectification) is shown beside it but not netted. The match is read from the \
amounts and ledgers, not from the vouchers' purpose: a credit that is not in fact a reversal but \
meets every condition (for example interest received booked as a Journal against the same bank) \
would reduce it.";

/// Whether a credit may reduce the unattributed figure at all. False by owner decision,
/// 2026-09-23 (never subtract); true restores the reversal rule.
pub const NET_REVERSALS: bool = false;

/// The note for the rule in force.
pub fn shared_netting_note(net_reversals: bool) -> &'static str {
    if net_reversals {
        REVERSAL_RULE_NOTE
    } else {
        NEVER_NET_NOTE
    }
}

/// One `[loans.loan_ledgers]` entry, typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoanConfig {
    pub lender: String,
    pub lender_type: String,
    /// `None` for an interest-free loan (no `interest_ledger` key).
    pub interest_ledger: Option<String>,
}

/// Type the bound `[loans.loan_ledgers]` entries, as the reference's `run()` reads them.
pub fn loan_config(
    entries: &BTreeMap<String, toml::Value>,
) -> Result<BTreeMap<String, LoanConfig>> {
    entries
        .iter()
        .map(|(ledger, entry)| {
            let t = entry.as_table().ok_or_else(|| {
                AuditError::Config(format!("[loans.loan_ledgers].{ledger:?} is not a table"))
            })?;
            let text = |key: &str| -> Result<String> {
                t.get(key)
                    .ok_or_else(|| {
                        AuditError::Config(format!("[loans.loan_ledgers].{ledger:?} has no {key}"))
                    })?
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AuditError::Config(format!(
                            "[loans.loan_ledgers].{ledger:?}.{key} is not a string"
                        ))
                    })
            };
            let interest_ledger = match t.get("interest_ledger") {
                None => None,
                Some(v) => Some(v.as_str().map(str::to_string).ok_or_else(|| {
                    AuditError::Config(format!(
                        "[loans.loan_ledgers].{ledger:?}.interest_ledger is not a string"
                    ))
                })?),
            };
            Ok((
                ledger.clone(),
                LoanConfig {
                    lender: text("lender")?,
                    lender_type: text("lender_type")?,
                    interest_ledger,
                },
            ))
        })
        .collect()
}

fn hash8(text: &str) -> String {
    crate::canonical::hex(&Sha1::digest(text.as_bytes()))[..8].to_string()
}

/// Tally's Receipt base type, whatever its case or surrounding space.
fn is_receipt(base_type: &str) -> bool {
    py_lower(py_strip(base_type)) == "receipt"
}

fn to_i64(n: i128) -> Result<i64> {
    i64::try_from(n).map_err(|_| overflow(TEST_ID))
}

fn net(v: &Voucher, ledger: &str) -> i128 {
    v.lines
        .iter()
        .filter(|l| l.ledger == ledger)
        .map(|l| i128::from(l.amount_paise))
        .sum()
}

fn others_than<'a>(v: &'a Voucher, ledger: &str) -> BTreeSet<&'a str> {
    v.lines
        .iter()
        .filter(|l| l.ledger != ledger)
        .map(|l| l.ledger.as_str())
        .collect()
}

/// Whether `others` is exactly `{ledger}`.
fn only(others: &BTreeSet<&str>, ledger: &str) -> bool {
    others.len() == 1 && others.contains(ledger)
}

/// A Python list of strings as `repr()` prints it.
fn py_repr_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| py_repr_str(s)).collect();
    format!("[{}]", inner.join(", "))
}

fn voucher_ref(v: &Voucher) -> EvidenceRef {
    EvidenceRef::with_label("voucher", &v.guid, &voucher_label(v))
}

/// Voucher refs, each once: the reference's `sorted({EvidenceRef(...)})`.
fn voucher_refs<'a>(vs: impl Iterator<Item = &'a Voucher>) -> Vec<EvidenceRef> {
    let set: BTreeSet<(String, String)> = vs.map(|v| (v.guid.clone(), voucher_label(v))).collect();
    set.into_iter()
        .map(|(id, label)| EvidenceRef::with_label("voucher", &id, &label))
        .collect()
}

fn mode(
    counter: &BTreeSet<&str>,
    base_type: &str,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
) -> &'static str {
    if counter.iter().any(|l| cash.contains(*l)) {
        MODE_CASH
    } else if counter.iter().any(|l| bank.contains(*l)) {
        MODE_BANK
    } else if base_type == "Journal" {
        MODE_JOURNAL
    } else {
        MODE_OTHER
    }
}

/// The Form 3CD utility's Note 1 code for a mode and direction; bank carries none.
pub fn mode_code(mode: &str, direction: &str) -> &'static str {
    match (mode, direction) {
        ("cash", "taken") => "B",
        ("cash", "repaid") => "A",
        ("journal", "taken") => "J",
        ("journal", "repaid") => "I",
        ("other", "taken") => "L",
        ("other", "repaid") => "K",
        _ => "",
    }
}

/// A line on an interest ledger: ((date, population position, line index), population position,
/// amount, the voucher's other ledgers). The key's order is the reference's (date, entry order).
type InterestLine<'a> = (
    (&'a bridge_tally_primitives::TallyDate, usize, usize),
    usize,
    i128,
    BTreeSet<&'a str>,
);

/// A taken or repaid row on one loan ledger.
struct PrincipalRow<'a> {
    voucher: &'a Voucher,
    amount: i128,
    mode: &'static str,
}

/// The reference's `compute_loan_rows`: (interest rows as (population index, the loan ledger's own
/// net line), taken rows, repaid rows).
#[allow(clippy::type_complexity)]
fn compute_loan_rows<'a>(
    pop: &[&'a Voucher],
    loan_ledger: &str,
    interest_ledger: Option<&str>,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
) -> (
    Vec<(usize, i128)>,
    Vec<PrincipalRow<'a>>,
    Vec<PrincipalRow<'a>>,
) {
    let mut interest = Vec::new();
    let mut taken = Vec::new();
    let mut repaid = Vec::new();
    for (at, v) in pop.iter().copied().enumerate() {
        if v.base_type == "Contra" || !v.lines.iter().any(|l| l.ledger == loan_ledger) {
            continue;
        }
        let loan_amt = net(v, loan_ledger);
        if loan_amt == 0 {
            continue;
        }
        let others = others_than(v, loan_ledger);
        if interest_ledger.is_some_and(|il| only(&others, il)) {
            interest.push((at, loan_amt));
            continue;
        }
        let m = mode(&others, &v.base_type, cash, bank);
        if loan_amt < 0 {
            taken.push(PrincipalRow {
                voucher: v,
                amount: -loan_amt,
                mode: m,
            });
        } else {
            repaid.push(PrincipalRow {
                voucher: v,
                amount: loan_amt,
                mode: m,
            });
        }
    }
    (interest, taken, repaid)
}

/// One walked taken/repaid row, with both running balances.
struct WalkedRow<'a> {
    voucher: &'a Voucher,
    amount: i128,
    mode: &'static str,
    direction: &'static str,
    prior_outstanding: i128,
    after_outstanding: i128,
    prior_breach: i128,
}

/// The reference's `compute_running_balance_rows`: taken, repaid and interest rows walked together
/// in (date, GUID) order from the clipped opening.
fn compute_running_balance_rows<'a>(
    pop: &[&'a Voucher],
    taken: &[PrincipalRow<'a>],
    repaid: &[PrincipalRow<'a>],
    interest: &[(usize, i128)],
    opening_outstanding: i128,
) -> Vec<WalkedRow<'a>> {
    // (voucher, amount, mode, direction, is_interest), built in the reference's order before its
    // stable sort.
    let mut combined: Vec<(&Voucher, i128, &'static str, &'static str, bool)> = Vec::new();
    combined.extend(
        taken
            .iter()
            .map(|r| (r.voucher, r.amount, r.mode, "taken", false)),
    );
    combined.extend(
        repaid
            .iter()
            .map(|r| (r.voucher, r.amount, r.mode, "repaid", false)),
    );
    combined.extend(interest.iter().map(|&(at, amt)| {
        (
            pop[at],
            amt.abs(),
            "",
            if amt < 0 { "taken" } else { "repaid" },
            true,
        )
    }));
    combined.sort_by(|a, b| (&a.0.date, &a.0.guid).cmp(&(&b.0.date, &b.0.guid)));
    let mut principal = opening_outstanding.max(0);
    let mut breach = principal;
    let mut out = Vec::new();
    for (voucher, amount, m, direction, is_interest) in combined {
        let (principal_prior, breach_prior) = (principal, breach);
        let sign = if direction == "taken" { 1 } else { -1 };
        breach = breach_prior + sign * amount;
        if is_interest {
            continue;
        }
        principal = principal_prior + sign * amount;
        out.push(WalkedRow {
            voucher,
            amount,
            mode: m,
            direction,
            prior_outstanding: principal_prior,
            after_outstanding: principal,
            prior_breach: breach_prior,
        });
    }
    out
}

/// Run the test with the rule in force ([`NET_REVERSALS`]).
#[allow(clippy::too_many_arguments)]
pub fn run(
    book: &Book,
    rules: &Rules,
    entity_type: &str,
    loans: &BTreeMap<String, LoanConfig>,
    previous_year_turnover_paise: Option<i64>,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    shared_interest_ledgers: &BTreeSet<String>,
) -> Result<TestResult> {
    run_with(
        book,
        rules,
        entity_type,
        loans,
        previous_year_turnover_paise,
        cash,
        bank,
        shared_interest_ledgers,
        NET_REVERSALS,
    )
}

/// [`run`], with the shared-ledger rule given.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn run_with(
    book: &Book,
    rules: &Rules,
    entity_type: &str,
    loans: &BTreeMap<String, LoanConfig>,
    previous_year_turnover_paise: Option<i64>,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    shared_interest_ledgers: &BTreeSet<String>,
    net_reversals: bool,
) -> Result<TestResult> {
    let missing = |table: &str| AuditError::Config(format!("{TEST_ID} needs rules [{table}]"));
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let pop = book.population()?;
    r.population_note = "Books population (optional, cancelled and post-dated vouchers excluded); \
Contra excluded throughout. A loan ledger's interest journals are vouchers whose only other ledger \
line is its configured interest ledger (loan_config); every other voucher touching the loan ledger \
is a taken (credit) or repaid (debit) transaction, never classified by ledger name."
        .to_string();

    let s194a = rules.s194a.as_ref().ok_or_else(|| missing("s194a"))?;
    let exempt_194a: BTreeSet<&str> = s194a
        .exempt_lender_types
        .iter()
        .map(String::as_str)
        .collect();
    let threshold_194a = i128::from(s194a.threshold_other_than_securities_paise);
    let limit_269 = i128::from(rules.s269ss_269t_limit_paise);
    let exempt_269: BTreeSet<&str> = match &rules.s269ss_269t_exempt_lender_types {
        Some(v) => v.iter().map(String::as_str).collect(),
        None => DEFAULT_269_EXEMPT_LENDER_TYPES.into_iter().collect(),
    };
    let reporting_exempt_269: BTreeSet<&str> =
        match &rules.s269ss_269t_reporting_exempt_lender_types {
            Some(v) => v.iter().map(String::as_str).collect(),
            None => DEFAULT_269_REPORTING_EXEMPT_LENDER_TYPES
                .into_iter()
                .collect(),
        };

    // ---------------------------------------------------------------- deductor status (reused)
    let deductor_threshold = rules
        .deductor_individual_huf_prev_year_turnover_paise
        .ok_or_else(|| missing("deductor"))?;
    let status = deductor_status(
        entity_type,
        deductor_threshold,
        previous_year_turnover_paise,
    );
    let f_status = r.fig(
        "deductor_status",
        Value::Text(status.to_string()),
        Unit::Text,
        "Whether the assessee must deduct TDS under s.194A for the year -- same rule and figure as \
tds_payees.deductor_status, reused verbatim: firm/LLP/company always; individual/HUF only if \
previous-year business turnover exceeded rules.deductor.individual_huf_prev_year_turnover_paise.",
        Vec::new(),
    );
    if status == "unknown" {
        // i64 -> f64 is exact below 2^53 and the division correctly rounded, as Python's is.
        #[allow(clippy::cast_precision_loss)]
        let crore = deductor_threshold as f64 / 1_000_000_000_f64;
        r.findings.push(Finding {
            id: format!("{TEST_ID}/deductor_status"),
            clauses: vec!["3CD-21(b)".to_string(), "3CD-34(a)".to_string()],
            title: "Deductor status for s.194A depends on previous-year turnover".to_string(),
            facts: vec![("deductor_status".to_string(), f_status)],
            evidence: Vec::new(),
            confidence: Confidence::JudgementRequired,
            limits: vec![format!(
                "An individual/HUF is a s.194A deductor only if the immediately preceding year's \
business turnover exceeded ₹{} crore; the current year's books alone cannot establish this.",
                py_format_g(crore)
            )],
            ask_client: vec!["Confirm previous-year business turnover.".to_string()],
        });
    }

    let mut taken_reportable_total: i128 = 0;
    let mut repaid_reportable_total: i128 = 0;
    let mut flag_count: usize = 0;
    let mut tds_over_threshold_count: usize = 0;

    // The population positions each interest ledger's paired loans counted as interest journals:
    // by identity, never by GUID (a GUID can be blank or repeated -- POP-5).
    let mut interest_vouchers: HashMap<&str, HashSet<usize>> = HashMap::new();

    for (loan_ledger, cfg) in loans {
        let lender = cfg.lender.as_str();
        let lender_type = cfg.lender_type.as_str();
        let interest_ledger = cfg.interest_ledger.as_deref();
        let h = stable_ledger_tag(book, loan_ledger)?;

        let (interest_rows, taken_rows, repaid_rows) =
            compute_loan_rows(&pop, loan_ledger, interest_ledger, cash, bank);
        if let Some(il) = interest_ledger.filter(|il| !il.is_empty()) {
            interest_vouchers
                .entry(il)
                .or_default()
                .extend(interest_rows.iter().map(|&(at, _)| at));
        }
        let interest_total: i128 = -interest_rows.iter().map(|&(_, amt)| amt).sum::<i128>();
        let taken_total: i128 = taken_rows.iter().map(|x| x.amount).sum();
        let repaid_total: i128 = repaid_rows.iter().map(|x| x.amount).sum();

        let f_lender_type = r.fig(
            &format!("lender_type_{h}"),
            Value::Text(lender_type.to_string()),
            Unit::Text,
            &format!(
                "Lender type for loan ledger (tag {h}) from client config (loans.loan_ledgers), \
classified by the loan ledger this interest pairs with -- never by a word in the ledger name."
            ),
            Vec::new(),
        );
        let ev_interest = voucher_refs(interest_rows.iter().map(|&(at, _)| pop[at]));
        let f_int = r.fig(
            &format!("interest_total_{h}"),
            Value::Int(to_i64(interest_total)?),
            Unit::Paise,
            &format!(
                "Interest paid/credited on loan ledger (tag {h}): sum of every voucher whose only \
other line is its configured interest ledger."
            ),
            ev_interest.clone(),
        );
        r.fig(
            &format!("interest_ledger_{h}"),
            Value::Text(interest_ledger.unwrap_or("").to_string()),
            Unit::Text,
            &format!(
                "The configured interest ledger name for loan ledger (tag {h}), or '' if the loan \
is interest-free -- the companion figure LOAN-2 and LOAN-3 read back to check each voucher on this \
loan and that ledger independently of this test's own bucketing."
            ),
            Vec::new(),
        );
        r.fig(
            &format!("clause31_taken_total_{h}"),
            Value::Int(to_i64(taken_total)?),
            Unit::Paise,
            &format!(
                "Credits to loan ledger (tag {h}) other than its interest journals -- amount \
taken/accepted in the year, before any Clause 31/s.269SS lender-exemption or mode filter."
            ),
            Vec::new(),
        );
        r.fig(
            &format!("clause31_repaid_total_{h}"),
            Value::Int(to_i64(repaid_total)?),
            Unit::Paise,
            &format!(
                "Debits to loan ledger (tag {h}) other than its interest journals -- amount repaid \
in the year, before any Clause 31/s.269T lender-exemption or mode filter."
            ),
            Vec::new(),
        );

        let opening_outstanding: i128 = book
            .tb
            .get(loan_ledger)
            .map_or(0, |t| -i128::from(t.opening_paise));
        let opening_clipped = opening_outstanding < 0;
        let running = compute_running_balance_rows(
            &pop,
            &taken_rows,
            &repaid_rows,
            &interest_rows,
            opening_outstanding,
        );
        let max_outstanding = running
            .iter()
            .map(|row| row.after_outstanding)
            .fold(opening_outstanding.max(0), i128::max);
        r.fig(
            &format!("max_outstanding_paise_{h}"),
            Value::Int(to_i64(max_outstanding)?),
            Unit::Paise,
            &format!(
                "Running maximum of the outstanding balance owed to the lender on loan ledger (tag \
{h}) during the year (opening balance, then after every taken/repaid entry in date order) -- the \
utility's MaxAmtOsAccPy column.{}",
                if opening_clipped {
                    " The TB opening on this ledger is a debit balance; walked from 0, not a \
negative outstanding -- confirm the opening figure with the client."
                } else {
                    ""
                }
            ),
            Vec::new(),
        );

        // ---------------------------------------------------------------- s.194A TDS
        if !exempt_194a.contains(lender_type)
            && interest_total > threshold_194a
            && status == "deductor"
        {
            tds_over_threshold_count += 1;
            let mut evidence = ev_interest.clone();
            evidence.push(EvidenceRef::new("ledger", loan_ledger));
            r.findings.push(Finding {
                id: format!("{TEST_ID}/s194a/{h}"),
                clauses: ["s.194A", "3CD-21(b)", "3CD-34(a)", "3CD-34(c)"]
                    .map(str::to_string)
                    .to_vec(),
                title: "Interest to a non-exempt lender over the s.194A threshold, no TDS ledger \
evidence"
                    .to_string(),
                facts: vec![
                    ("interest".to_string(), f_int.clone()),
                    ("lender_type".to_string(), f_lender_type.clone()),
                ],
                evidence,
                confidence: Confidence::NeedsDocument,
                limits: vec![
                    format!(
                        "Books only: lender type ({}) comes from client config, not from a \
notification lookup; confirm the lender is not itself a body notified as exempt under \
s.194A(3)(iii) beyond the classes already in rules.s194a.exempt_lender_types.",
                        py_repr_str(lender_type)
                    ),
                    "s.40(a)(ia) disallows 30% of the interest on TDS default; the second proviso \
removes this if the lender's Form 26A (Rule 31ACB) shows the interest was returned as income -- \
s.201(1A) interest still runs either way."
                        .to_string(),
                ],
                ask_client: vec![
                    format!(
                        "Confirm whether TDS under s.194A was deducted on interest to {lender}."
                    ),
                    "If not deducted, confirm whether Form 26A is available for this lender."
                        .to_string(),
                ],
            });
        }

        // ---------------------------------------------------------------- Clause 31(a)/(c)
        if reporting_exempt_269.contains(lender_type) {
            continue; // outside Clause 31 form reporting entirely (31-13)
        }
        for row in &running {
            let direction = row.direction;
            let clause = if direction == "taken" {
                "3CD-31(a)"
            } else {
                "3CD-31(c)"
            };
            let (v, amt, m) = (row.voucher, row.amount, row.mode);
            let (prior, after) = (row.prior_outstanding, row.after_outstanding);
            let crosses_reporting_window = if direction == "taken" {
                after >= limit_269
            } else {
                prior.max(amt) >= limit_269
            };
            let crosses_breach_balance = if direction == "taken" {
                after >= limit_269
            } else {
                row.prior_breach.max(amt) >= limit_269
            };
            if !(crosses_reporting_window || crosses_breach_balance) {
                continue;
            }
            if direction == "taken" {
                taken_reportable_total += amt;
            } else {
                repaid_reportable_total += amt;
            }
            let vh = hash8(&v.guid);
            let rid = format!("{direction}_{h}_{vh}");
            let amount_name = format!("clause31_row_amount_{rid}");
            if r.figures
                .iter()
                .any(|f| f.id == format!("{TEST_ID}.{amount_name}"))
            {
                // The reference raises on the repeated figure id; this refuses rather than panics.
                return Err(AuditError::Config(format!(
                    "{TEST_ID}: two vouchers share the figure id {TEST_ID}.{amount_name}"
                )));
            }
            let f_amt = r.fig(
                &amount_name,
                Value::Int(to_i64(amt)?),
                Unit::Paise,
                &format!("Loan {direction} on voucher (tag {vh}) against loan ledger (tag {h})."),
                vec![voucher_ref(v)],
            );
            let f_mode = r.fig(
                &format!("clause31_row_mode_{rid}"),
                Value::Text(m.to_string()),
                Unit::Text,
                &format!(
                    "Mode of this {direction} transaction, read from its counter-line ledger \
group(s): cash/bank if any counter-line is under that group, else journal if the voucher's own type \
is Journal, else other."
                ),
                Vec::new(),
            );
            let code = mode_code(m, direction);
            r.fig(
                &format!("clause31_row_mode_code_{rid}"),
                Value::Text(code.to_string()),
                Unit::Text,
                &format!(
                    "Form 3CD utility Note 1 code for this {direction} entry (module docstring's \
mode_code bullet): derived from mode+direction, not read off a specimen utility export -- confirm."
                ),
                Vec::new(),
            );
            r.fig(
                &format!("clause31_row_outstanding_after_{rid}"),
                Value::Int(to_i64(after)?),
                Unit::Paise,
                &format!(
                    "Outstanding balance owed to the lender on loan ledger (tag {h}) immediately \
after this entry (prior balance {prior}p {} this entry's amount) -- the GN 55.8/57.2 running-balance \
walk.",
                    if direction == "taken" { "plus" } else { "minus" }
                ),
                Vec::new(),
            );
            let flagged = NON_ACCOUNT_PAYEE_MODES.contains(&m)
                && crosses_breach_balance
                && !exempt_269.contains(lender_type);
            let mut clauses = vec![clause.to_string()];
            if flagged {
                flag_count += 1;
                clauses.push(
                    if direction == "taken" {
                        "s.269SS"
                    } else {
                        "s.269T"
                    }
                    .to_string(),
                );
            }

            let mut limits = vec![
                "Books only: confirm the lender's identity (name, address, PAN) for Form 3CD \
Clause 31, and whether this lender is excepted from s.269SS/269T beyond \
rules.s269ss_269t.exempt_lender_types (Government, a notified corporation, or another body notified \
under the Explanation)."
                    .to_string(),
                "The mode-and-direction code follows the Form 3CD utility's own Note 1 list (A/B \
cash payment/receipt, I/J journal entry debit/credit, K/L any other mode debit/credit); this pack's \
own mapping from mode and direction to that code is a derivation from the GN's text, not verified \
against a specimen utility export -- confirm."
                    .to_string(),
            ];
            let mut ask_client =
                vec!["Confirm lender identity (name, address, PAN) for Clause 31.".to_string()];
            if lender_type == "insurer" {
                limits.push(
                    "lender_type is 'insurer': exempt from TDS deduction under s.194A(3)(iii), but \
that is a different exemption from the one in the Explanation to s.269SS/269T -- this lender is NOT \
treated as exempt from Clause 31/s.269SS/s.269T here; confirm."
                        .to_string(),
                );
            }
            if m == MODE_CASH {
                limits.push(format!(
                    "Mode is read from the counter-line ledger's group only (Cash-in-Hand), never \
from the narration: this voucher's narration is {}. A narration naming an electronic instrument \
(e.g. UPI/PhonePe) booked through a cash ledger takes this outside s.269SS/269T even though the \
ledger group says 'cash' -- confirm from the narration and the bank statement before treating this \
as a cash breach.",
                    py_repr_str(&v.narration)
                ));
                ask_client.push(
                    "Confirm from narration/bank statement whether this was genuinely cash or an \
electronic transfer narrated against a cash ledger."
                        .to_string(),
                );
            }
            if flagged && amt < limit_269 {
                limits.push(format!(
                    "This entry's own amount ({amt}p) is below the s.269SS/269T limit on its own; \
it is flagged only because the running balance with this lender (GN 55.8/57.2) is at or over the \
limit -- report every entry from where the running balance first reaches the limit until it falls \
back below, not only the large ones."
                ));
            }
            if flagged {
                limits.push(
                    "s.273B reasonable cause (e.g. banking facilities not available in the area) is \
a CA judgement call on the facts, not a books fact."
                        .to_string(),
                );
            }
            if direction == "repaid" && crosses_breach_balance && !crosses_reporting_window {
                limits.push(
                    "This repayment's own principal balance never reaches the ordinary GN 55.8/57.2 \
reporting window on its own; it is reportable here only because the balance held with this lender, \
together with interest credited and not yet paid off, reaches the s.269T limit (GN 57.1/57.4: report \
repayments even below ₹20,000 where the loan plus interest is ₹20,000 or more)."
                        .to_string(),
                );
            }

            r.findings.push(Finding {
                id: format!("{TEST_ID}/clause31/{rid}"),
                clauses,
                title: format!(
                    "Loan {direction} against {lender} ({m} mode, code {}){}",
                    if code.is_empty() { "n/a" } else { code },
                    if flagged {
                        " -- at or over the s.269SS/269T running-balance limit"
                    } else {
                        ""
                    }
                ),
                facts: vec![("amount".to_string(), f_amt), ("mode".to_string(), f_mode)],
                evidence: vec![voucher_ref(v), EvidenceRef::new("ledger", loan_ledger)],
                confidence: Confidence::NeedsDocument,
                limits,
                ask_client,
            });
        }
    }

    r.fig(
        "clause31_taken_reportable_total",
        Value::Int(to_i64(taken_reportable_total)?),
        Unit::Paise,
        "Sum of clause31_taken_total_<tag> across every non-exempt-lender loan ledger (bank/\
co-operative-bank lenders excluded entirely).",
        Vec::new(),
    );
    r.fig(
        "clause31_repaid_reportable_total",
        Value::Int(to_i64(repaid_reportable_total)?),
        Unit::Paise,
        "Sum of clause31_repaid_total_<tag> across every non-exempt-lender loan ledger.",
        Vec::new(),
    );
    r.fig(
        "s269ss_269t_flag_count",
        count(TEST_ID, flag_count)?,
        Unit::Count,
        &format!(
            "Reportable taken/repaid rows in cash/journal/other mode at or over the s.269SS/269T \
limit ({limit_269} paise)."
        ),
        Vec::new(),
    );
    r.fig(
        "s194a_tds_over_threshold_lender_count",
        count(TEST_ID, tds_over_threshold_count)?,
        Unit::Count,
        "Loan ledgers with a non-exempt lender whose interest this year exceeds the s.194A \
threshold and for which the assessee is a deductor.",
        Vec::new(),
    );

    // Interest ledgers the client config declares shared: the unattributed part is a named figure
    // with its vouchers (the reference's owner decisions of 2026-09-22 and 2026-09-23).
    for il in shared_interest_ledgers {
        let empty = HashSet::new();
        let cited = interest_vouchers.get(il.as_str()).unwrap_or(&empty);
        // (key = (date, population position, line), population position, amount, other ledgers)
        let mut debits: Vec<InterestLine> = Vec::new();
        let mut credits: Vec<InterestLine> = Vec::new();
        for (at, v) in pop.iter().copied().enumerate() {
            if cited.contains(&at) || v.base_type == "Contra" {
                continue;
            }
            let others = others_than(v, il);
            for (i, l) in v.lines.iter().enumerate() {
                if l.ledger != *il || l.amount_paise == 0 {
                    continue;
                }
                let line = (
                    (&v.date, at, i),
                    at,
                    i128::from(l.amount_paise).abs(),
                    others.clone(),
                );
                if l.amount_paise > 0 {
                    debits.push(line);
                } else {
                    credits.push(line);
                }
            }
        }
        debits.sort_by(|a, b| a.0.cmp(&b.0));
        credits.sort_by(|a, b| a.0.cmp(&b.0));
        let mut taken: HashSet<usize> = HashSet::new();
        let mut reversed_pairs: Vec<(usize, usize)> = Vec::new();
        let mut unmatched: Vec<usize> = Vec::new();
        let (mut reversed_total, mut unmatched_total): (i128, i128) = (0, 0);
        for (ckey, cat, amount, others) in &credits {
            let found = debits
                .iter()
                .enumerate()
                .position(|(n, (dkey, dat, damount, dothers))| {
                    !taken.contains(&n)
                        && net_reversals
                        && dat != cat
                        && !is_receipt(&pop[*cat].base_type)
                        && damount == amount
                        && dothers == others
                        && dkey.0 <= ckey.0
                });
            match found {
                None => {
                    unmatched.push(*cat);
                    unmatched_total += amount;
                }
                Some(n) => {
                    taken.insert(n);
                    reversed_pairs.push((*cat, debits[n].1));
                    reversed_total += amount;
                }
            }
        }
        let unpaired_debits: i128 = debits.iter().map(|d| d.2).sum();
        let debit_on_vouchers: i128 = pop
            .iter()
            .flat_map(|v| v.lines.iter())
            .filter(|l| l.ledger == *il && l.amount_paise > 0)
            .map(|l| i128::from(l.amount_paise))
            .sum();
        let lh = stable_ledger_tag(book, il)?;

        let ev_debits = voucher_refs(debits.iter().map(|d| pop[d.1]));
        let pair_ev: Vec<EvidenceRef> = reversed_pairs
            .iter()
            .map(|&(c, d)| {
                (
                    pop[c].guid.clone(),
                    format!(
                        "{}, reversing {}",
                        voucher_label(pop[c]),
                        voucher_label(pop[d])
                    ),
                )
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(id, label)| EvidenceRef::with_label("voucher", &id, &label))
            .collect();
        let f_debits = r.fig(
            &format!("shared_interest_unpaired_debits_{lh}"),
            Value::Int(to_i64(unpaired_debits)?),
            Unit::Paise,
            &format!(
                "Debits to shared interest ledger (tag {lh}) on population vouchers (Contra \
excluded) that no paired loan's interest total counts."
            ),
            ev_debits.clone(),
        );
        let f_reversed = r.fig(
            &format!("shared_interest_reversed_credits_{lh}"),
            Value::Int(to_i64(reversed_total)?),
            Unit::Paise,
            &if net_reversals {
                format!(
                    "Credits to shared interest ledger (tag {lh}), on the same vouchers, each \
matched as the reversal of a specific earlier debit on another voucher counted above (each pair \
cited); netted against the debits."
                )
            } else {
                format!(
                    "Credits to shared interest ledger (tag {lh}) netted against the debits: none, \
since no credit reduces the figure."
                )
            },
            pair_ev.clone(),
        );
        let f_unmatched = r.fig(
            &format!("shared_interest_unmatched_credits_{lh}"),
            Value::Int(to_i64(unmatched_total)?),
            Unit::Paise,
            &if net_reversals {
                format!(
                    "Every other credit to shared interest ledger (tag {lh}) on the same vouchers: \
shown, not netted."
                )
            } else {
                format!(
                    "Every credit to shared interest ledger (tag {lh}) on the same vouchers: \
shown, never netted."
                )
            },
            voucher_refs(unmatched.iter().map(|&at| pop[at])),
        );
        let unattributed = unpaired_debits - reversed_total;
        let un_evidence: Vec<EvidenceRef> = ev_debits
            .iter()
            .chain(pair_ev.iter())
            .map(|e| (e.id.clone(), e.label.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|(id, label)| EvidenceRef::with_label("voucher", &id, &label))
            .collect();
        let f_un = r.fig(
            &format!("shared_interest_unattributed_{lh}"),
            Value::Int(to_i64(unattributed)?),
            Unit::Paise,
            &format!(
                "Interest on shared interest ledger (tag {lh}) not attributed to any configured \
loan: the debits on population vouchers (Contra excluded) that no paired loan's interest total \
counts{}{}",
                if net_reversals {
                    ", less the credits matched to them as reversals. "
                } else {
                    ". "
                },
                shared_netting_note(net_reversals)
            ),
            un_evidence.clone(),
        );
        r.fig(
            &format!("shared_interest_debits_{lh}"),
            Value::Int(to_i64(debit_on_vouchers)?),
            Unit::Paise,
            &format!("All population debits to shared interest ledger (tag {lh})."),
            Vec::new(),
        );
        if unattributed > 0 {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/shared_interest/{lh}"),
                clauses: vec!["s.194A".to_string()],
                title: "Interest on a shared interest ledger that is not attributed to any \
configured loan"
                    .to_string(),
                facts: vec![
                    ("unattributed".to_string(), f_un),
                    ("unpaired_debits".to_string(), f_debits),
                    ("reversed_credits".to_string(), f_reversed),
                    ("unmatched_credits".to_string(), f_unmatched),
                ],
                evidence: un_evidence,
                confidence: Confidence::NeedsDocument,
                limits: vec!["For a voucher that names no loan, the books do not say which lender \
this interest was paid to. Whether s.194A applies depends on the lender: interest paid to a bank, a \
co-operative bank, an insurer or another body s.194A(3)(iii) names is exempt; other lenders may not \
be. A voucher that also posts to a configured loan (an instalment paying principal and interest \
together, or interest net of TDS) names that loan; it is counted here because the loan's interest \
total does not include it."
                    .to_string()],
                ask_client: vec![
                    "Bank or lender statements identifying the lender for these interest debits."
                        .to_string(),
                ],
            });
        }
    }

    Ok(r)
}

/// LOAN-1, LOAN-2 and LOAN-3 with the rule in force ([`NET_REVERSALS`]); the reference's
/// `check_invariants`, whose docstring describes each check and its accepted limits.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    check_invariants_with(book, result, NET_REVERSALS)
}

/// [`check_invariants`], with the shared-ledger rule given.
#[allow(clippy::too_many_lines)]
pub fn check_invariants_with(
    book: &Book,
    result: &TestResult,
    net_reversals: bool,
) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let prefix = format!("{}.", result.test_id);
    let tol: i128 = 100;
    let hash_to_name = ledgers_by_tag(book)?;
    let figures: BTreeMap<&str, &crate::findings::Figure> =
        result.figures.iter().map(|f| (f.id.as_str(), f)).collect();
    let int_of = |f: &crate::findings::Figure| -> i128 {
        match f.value {
            Value::Int(n) => i128::from(n),
            _ => 0,
        }
    };

    let marker = format!("{prefix}interest_ledger_");
    let mut interest_ledger_by_tag: BTreeMap<String, String> = BTreeMap::new();
    for (fid, fig) in &figures {
        if let (Some(tag), Value::Text(text)) = (fid.strip_prefix(&marker), &fig.value) {
            if !text.is_empty() {
                interest_ledger_by_tag.insert(tag.to_string(), text.clone());
            }
        }
    }

    // LOAN-1
    let marker = format!("{prefix}interest_total_");
    for (fid, fig) in &figures {
        let Some(h) = fid.strip_prefix(&marker) else {
            continue;
        };
        let Some(name) = hash_to_name.get(h) else {
            out.push(format!(
                "LOAN-1: cannot resolve a loan ledger for figure {fid} (tag {h})"
            ));
            continue;
        };
        let Some(tb_row) = book.tb.get(name.as_str()) else {
            out.push(format!(
                "LOAN-1: {name} (tag {h}) has no Trial Balance row to reconcile against"
            ));
            continue;
        };
        let total = |kind: &str| {
            figures
                .get(format!("{prefix}clause31_{kind}_total_{h}").as_str())
                .map_or(0, |f| int_of(f))
        };
        let (taken_total, repaid_total, interest) = (total("taken"), total("repaid"), int_of(fig));
        let expected_movement = -taken_total + repaid_total - interest;
        let movement = i128::from(tb_row.closing_paise) - i128::from(tb_row.opening_paise);
        let diff = expected_movement - movement;
        if diff.abs() > tol {
            out.push(format!(
                "LOAN-1: {name} (tag {h}) expected FY movement {expected_movement}p (= -taken \
{taken_total}p + repaid {repaid_total}p - interest {interest}p) does not tie the TB movement \
{movement}p (opening {}p, closing {}p); difference {diff}p -- a voucher on this loan ledger was \
likely dropped from or wrongly added to the population.",
                tb_row.opening_paise, tb_row.closing_paise
            ));
        }
    }

    let pop = book.population()?;
    let mut pop_by_guid: HashMap<&str, Vec<&Voucher>> = HashMap::new();
    for v in &pop {
        pop_by_guid.entry(v.guid.as_str()).or_default().push(v);
    }

    // LOAN-2
    for (h, interest_ledger) in &interest_ledger_by_tag {
        let fid = format!("{prefix}interest_total_{h}");
        let Some(loan) = hash_to_name.get(h.as_str()) else {
            out.push(format!(
                "LOAN-2: cannot resolve a loan ledger for figure {prefix}interest_ledger_{h} (tag {h})"
            ));
            continue;
        };
        let Some(fig) = figures.get(fid.as_str()) else {
            out.push(format!(
                "LOAN-2: loan ledger {} (tag {h}) has an interest ledger ({}) but no \
interest_total_{h} figure, so its interest is checked nowhere",
                py_repr_str(loan),
                py_repr_str(interest_ledger)
            ));
            continue;
        };
        let cited: BTreeSet<&str> = fig
            .evidence
            .iter()
            .filter(|e| e.kind == "voucher")
            .map(|e| e.id.as_str())
            .collect();
        let on_loan: Vec<&Voucher> = pop
            .iter()
            .copied()
            .filter(|v| v.lines.iter().any(|l| l.ledger == **loan))
            .collect();
        let duplicated: Vec<String> = on_loan
            .iter()
            .filter(|v| pop_by_guid[v.guid.as_str()].len() > 1)
            .map(|v| v.guid.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !duplicated.is_empty() {
            out.push(format!(
                "LOAN-2: vouchers posting to loan ledger {} (tag {h}) share a GUID with another \
books-population voucher, so they cannot be matched to interest_total_{h}'s evidence: {}",
                py_repr_str(loan),
                py_repr_list(&duplicated)
            ));
        }
        for v in &on_loan {
            if cited.contains(v.guid.as_str()) || duplicated.contains(&v.guid) {
                continue;
            }
            let on_interest = net(v, interest_ledger);
            if on_interest != 0 {
                out.push(format!(
                    "LOAN-2: voucher {} (guid {}) posts {on_interest}p to interest ledger {} and a \
line to loan ledger {} (tag {h}), but that interest is in no interest_total_{h}: the builder \
counted the voucher as taken/repaid or skipped it (see the interest-journal limitation in this \
module's docstring).",
                    voucher_label(v),
                    v.guid,
                    py_repr_str(interest_ledger),
                    py_repr_str(loan)
                ));
            }
        }
        let missing: Vec<String> = cited
            .iter()
            .filter(|g| !pop_by_guid.contains_key(**g))
            .map(|g| (*g).to_string())
            .collect();
        if !missing.is_empty() {
            out.push(format!(
                "LOAN-2: interest_total_{h} cites vouchers outside the books population: {}",
                py_repr_list(&missing)
            ));
        }
        let cited_vouchers: Vec<&Voucher> = cited
            .iter()
            .filter(|g| !duplicated.iter().any(|d| d == **g))
            .flat_map(|g| pop_by_guid.get(*g).into_iter().flatten().copied())
            .collect();
        let on_cited: i128 = -cited_vouchers.iter().map(|v| net(v, loan)).sum::<i128>();
        let value = int_of(fig);
        if (on_cited - value).abs() > tol {
            out.push(format!(
                "LOAN-2: interest_total_{h} is {value}p but the loan ledger {}'s own lines on the \
vouchers it cites net {}p (interest {on_cited}p); difference {}p.",
                py_repr_str(loan),
                -on_cited,
                value - on_cited
            ));
        }
        for v in &cited_vouchers {
            let (on_l, on_i) = (net(v, loan), net(v, interest_ledger));
            let residue = on_l + on_i;
            if residue != 0 {
                out.push(format!(
                    "LOAN-2: interest_total_{h} counts voucher {} (guid {}), whose loan and \
interest-ledger lines do not cancel (loan {on_l}p, interest {on_i}p): {residue}p of it moves to or \
from another ledger, so part of what is counted as interest is principal or something else.",
                    voucher_label(v),
                    v.guid
                ));
            }
        }
    }

    // LOAN-3
    let mut by_interest_ledger: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (h, led) in &interest_ledger_by_tag {
        by_interest_ledger
            .entry(led.clone())
            .or_default()
            .push(h.clone());
    }
    let shared_marker = format!("{prefix}shared_interest_debits_");
    for fid in figures.keys() {
        let Some(sh) = fid.strip_prefix(&shared_marker) else {
            continue;
        };
        match hash_to_name.get(sh) {
            None => out.push(format!(
                "LOAN-3: cannot resolve a shared interest ledger for figure {fid} (tag {sh})"
            )),
            Some(name) => {
                by_interest_ledger.entry((*name).clone()).or_default();
            }
        }
    }
    for (interest_ledger, tags) in &by_interest_ledger {
        let lh = stable_ledger_tag(book, interest_ledger)?;
        let shared_fid = format!("{prefix}shared_interest_debits_{lh}");
        let paired: BTreeSet<&str> = tags
            .iter()
            .filter_map(|h| hash_to_name.get(h.as_str()).map(|n| n.as_str()))
            .collect();
        if let Some(shared_fig) = figures.get(shared_fid.as_str()) {
            let debit_pop: i128 = pop
                .iter()
                .flat_map(|v| v.lines.iter())
                .filter(|l| l.ledger == *interest_ledger && l.amount_paise > 0)
                .map(|l| i128::from(l.amount_paise))
                .sum();
            match book.tb.get(interest_ledger) {
                None => out.push(format!(
                    "LOAN-3: shared interest ledger {} has no Trial Balance row to reconcile against",
                    py_repr_str(interest_ledger)
                )),
                Some(tb_row) if (debit_pop - i128::from(tb_row.debit_paise)).abs() > tol => {
                    out.push(format!(
                        "LOAN-3: shared interest ledger {} population debits {debit_pop}p do not \
tie the TB period debit {}p",
                        py_repr_str(interest_ledger),
                        tb_row.debit_paise
                    ));
                }
                Some(_) => {}
            }
            if int_of(shared_fig) != debit_pop {
                out.push(format!(
                    "LOAN-3: shared_interest_debits_{lh} is {}p but the population's debits to {} \
are {debit_pop}p",
                    int_of(shared_fig),
                    py_repr_str(interest_ledger)
                ));
            }
            // Recomputed from the vouchers, never from the builder's figures or evidence.
            let journals: HashSet<usize> = pop
                .iter()
                .enumerate()
                .filter(|(_, v)| v.base_type != "Contra")
                .filter(|(_, v)| {
                    paired.iter().any(|loan| {
                        v.lines.iter().any(|l| l.ledger == *loan)
                            && only(&others_than(v, loan), interest_ledger)
                            && net(v, loan) != 0
                    })
                })
                .map(|(n, _)| n)
                .collect();
            let mut lines: Vec<InterestLine> = Vec::new();
            for (n, v) in pop.iter().enumerate() {
                if journals.contains(&n) || v.base_type == "Contra" {
                    continue;
                }
                for (i, l) in v.lines.iter().enumerate() {
                    if l.ledger == *interest_ledger && l.amount_paise != 0 {
                        lines.push((
                            (&v.date, n, i),
                            n,
                            i128::from(l.amount_paise),
                            others_than(v, interest_ledger),
                        ));
                    }
                }
            }
            let debit_lines: Vec<_> = lines.iter().filter(|x| x.2 > 0).collect();
            let mut credit_lines: Vec<_> = lines.iter().filter(|x| x.2 < 0).collect();
            credit_lines.sort_by(|a, b| a.0.cmp(&b.0));
            let mut used = BTreeSet::new();
            let (mut reversed_total, mut unmatched_total): (i128, i128) = (0, 0);
            for (key, n, amount, others) in credit_lines {
                let candidates: Vec<_> = if !net_reversals || is_receipt(&pop[*n].base_type) {
                    Vec::new()
                } else {
                    debit_lines
                        .iter()
                        .filter(|d| {
                            !used.contains(&d.0)
                                && d.1 != *n
                                && d.2 == -amount
                                && d.3 == *others
                                && d.0 .0 <= key.0
                        })
                        .collect()
                };
                if let Some(first) = candidates.iter().map(|d| d.0).min() {
                    used.insert(first);
                    reversed_total -= amount;
                } else {
                    unmatched_total -= amount;
                }
            }
            let unpaired_debits: i128 = debit_lines.iter().map(|d| d.2).sum();
            let expected = [
                ("unpaired_debits", unpaired_debits),
                ("reversed_credits", reversed_total),
                ("unmatched_credits", unmatched_total),
                ("unattributed", unpaired_debits - reversed_total),
            ];
            for (name, value) in expected {
                let fid = format!("{prefix}shared_interest_{name}_{lh}");
                match figures.get(fid.as_str()) {
                    None => out.push(format!(
                        "LOAN-3: shared interest ledger {} has no shared_interest_{name}_{lh} figure",
                        py_repr_str(interest_ledger)
                    )),
                    Some(f) if int_of(f) != value => out.push(format!(
                        "LOAN-3: shared_interest_{name}_{lh} is {}p but the vouchers no paired \
loan's interest total cites give {value}p",
                        int_of(f)
                    )),
                    Some(_) => {}
                }
            }
            continue;
        }
        let outside: Vec<(&Voucher, i128)> = pop
            .iter()
            .copied()
            .filter(|v| !v.lines.iter().any(|l| paired.contains(l.ledger.as_str())))
            .map(|v| (v, net(v, interest_ledger)))
            .filter(|&(_, a)| a != 0)
            .collect();
        let outside_net: i128 = outside.iter().map(|&(_, a)| a).sum();
        if outside_net > tol {
            let debits: Vec<&(&Voucher, i128)> = outside.iter().filter(|&&(_, a)| a > 0).collect();
            let shown: Vec<String> = debits
                .iter()
                .take(5)
                .map(|&&(v, amt)| format!("{} ({amt}p)", voucher_label(v)))
                .collect();
            out.push(format!(
                "LOAN-3: interest ledger {} (not declared shared) nets {outside_net}p of debits on \
vouchers that post to none of its paired loan ledgers, so that interest is in no loan's \
interest_total; the debits: {}{} -- declare the ledger shared ([loans].shared_interest_ledgers) or \
book the interest against its loan.",
                py_repr_str(interest_ledger),
                shown.join(", "),
                if debits.len() > 5 { " ..." } else { "" }
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::{Ledger, LedgerLine, VoucherStatus};
    use bridge_tally_primitives::TallyDate;

    fn table(text: &str) -> BTreeMap<String, toml::Value> {
        let t: toml::Table = toml::from_str(text).unwrap();
        t.into_iter().collect()
    }

    #[test]
    fn a_loan_entry_is_typed_or_refused_naming_it() {
        let ok = loan_config(&table(
            "[\"Loan A\"]\nlender = \"x\"\nlender_type = \"nbfc\"\ninterest_ledger = \"Int\"\n\
             [\"Loan B\"]\nlender = \"y\"\nlender_type = \"person\"\n",
        ))
        .unwrap();
        assert_eq!(ok["Loan A"].interest_ledger.as_deref(), Some("Int"));
        assert_eq!(ok["Loan B"].interest_ledger, None);
        for (text, needle) in [
            ("\"Loan A\" = 5\n", "is not a table"),
            ("[\"Loan A\"]\nlender_type = \"nbfc\"\n", "has no lender"),
            ("[\"Loan A\"]\nlender = \"x\"\n", "has no lender_type"),
            (
                "[\"Loan A\"]\nlender = \"x\"\nlender_type = 3\n",
                "lender_type is not a string",
            ),
            (
                "[\"Loan A\"]\nlender = \"x\"\nlender_type = \"nbfc\"\ninterest_ledger = 1\n",
                "interest_ledger is not a string",
            ),
        ] {
            let err = loan_config(&table(text)).unwrap_err();
            assert!(format!("{err}").contains(needle), "{text}: {err}");
        }
    }

    fn book(vouchers: Vec<Voucher>) -> Book {
        let ledger = |name: &str, group: &str| Ledger {
            name: name.to_string(),
            parent: group.to_string(),
            chain: vec![group.to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: String::new(),
            masterid: None,
        };
        Book {
            company_name: "Invented".to_string(),
            company_guid: "invented".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: [
                ledger("Loan A", "Unsecured Loans"),
                ledger("Cash", "Cash-in-Hand"),
            ]
            .into_iter()
            .map(|l| (l.name.clone(), l))
            .collect(),
            vouchers,
            tb: BTreeMap::new(),
        }
    }

    fn taken(guid: &str, day: &str) -> Voucher {
        Voucher {
            guid: guid.to_string(),
            date: TallyDate::parse(day.to_string()).unwrap(),
            vtype: "Receipt".to_string(),
            base_type: "Receipt".to_string(),
            number: String::new(),
            status: VoucherStatus::Regular,
            lines: vec![
                LedgerLine {
                    ledger: "Loan A".to_string(),
                    amount_paise: -2_500_000,
                },
                LedgerLine {
                    ledger: "Cash".to_string(),
                    amount_paise: 2_500_000,
                },
            ],
            ..Default::default()
        }
    }

    fn run_on(b: &Book, rules: &Rules) -> Result<TestResult> {
        let loans = loan_config(&table(
            "[\"Loan A\"]\nlender = \"x\"\nlender_type = \"person\"\n",
        ))
        .unwrap();
        let cash: BTreeSet<String> = ["Cash".to_string()].into_iter().collect();
        run(
            b,
            rules,
            "firm",
            &loans,
            None,
            &cash,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
    }

    #[test]
    fn two_vouchers_sharing_a_row_figure_id_are_refused() {
        // Both are reportable cash receipts on one loan with one GUID: the reference raises on the
        // repeated figure id, and this refuses rather than panicking.
        let rules = Rules::vendored().unwrap();
        let dup = book(vec![taken("g1", "20250601"), taken("g1", "20250602")]);
        let err = run_on(&dup, &rules).unwrap_err();
        assert!(
            format!("{err}").contains("two vouchers share the figure id"),
            "{err}"
        );
        // The control: distinct GUIDs give two rows.
        let two = book(vec![taken("g1", "20250601"), taken("g2", "20250602")]);
        assert_eq!(run_on(&two, &rules).unwrap().findings.len(), 2);
    }

    #[test]
    fn rules_without_s194a_are_refused() {
        let mut rules = Rules::vendored().unwrap();
        rules.s194a = None;
        let err = run_on(&book(vec![]), &rules).unwrap_err();
        assert!(format!("{err}").contains("needs rules [s194a]"), "{err}");
    }
}
