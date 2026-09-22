//! Book-keeping quality: six tests on how a Tally book was actually kept, distinct from any tax
//! computation. A port of the reference Python implementation's `book_keeping_quality` test
//! module, version 1.
//!
//! Every voucher is taken from the books population (optional, cancelled and post-dated
//! excluded). Nothing here is a settled legal test, so every finding is indicative or
//! judgement-required: facts and a question for the CA, never a conclusion.
//!
//! 1. Entry order: walking the population in MASTERID (creation) order, the latest sales date
//!    created so far is a clock; every other voucher created once the clock has started lags it by
//!    `clock - date` days. Vouchers created after the last-created sale are reported separately.
//! 2. Consolidated payment-channel invoices: Sales on a configured payment-channel debtor, paired
//!    greedily (oldest invoice first) with a same-amount receipt 0-3 days later, and priced against
//!    each item's year-average purchase rate by debtor bucket.
//! 3. Stock-in-Hand ledgers no population voucher touches.
//! 4. Output-tax head balances and the configured GST payment ledgers, as TB figures.
//! 5. Narrations matching a configured re-issue term, and debtor write-off journals.
//! 6. Contra narrations whose claimed direction the Cash line's sign contradicts.
//!
//! Python semantics reproduced where they decide a figure: MASTERID is read as `int()` reads it
//! (its own whitespace set, a sign, single underscores; a well-formed MASTERID with a non-ASCII
//! digit, which `int()` reads, is refused rather than read differently, as is a value beyond i64;
//! see `masterid_int`); the purchase
//! rate and the cost are floats accumulated in population order, compared exactly with the integer
//! sale, and rounded half to even; the invoice, receipt and re-issue maps keep the last voucher per
//! GUID, as a dict does; a write-off is one row per voucher (by its position, not its GUID) and
//! debtor, that debtor's credit lines summed.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;
use sha2::{Digest, Sha256};

use crate::book::{Book, Voucher};
use crate::depreciation::civil_day_number;
use crate::error::{AuditError, Result};
use crate::findings::{pct_bp, Confidence, EvidenceRef, Figure, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::iso;
use crate::rules::Rules;
use crate::support;

pub const TEST_ID: &str = "book_keeping_quality";
pub const VERSION: &str = "1";

// Tally's own reserved names, not client data.
const SALES_BASE_TYPE: &str = "Sales";
const PURCHASE_BASE_TYPE: &str = "Purchase";
const CONTRA_BASE_TYPE: &str = "Contra";
const JOURNAL_BASE_TYPE: &str = "Journal";
const STOCK_GROUP: &str = "Stock-in-Hand";
const SUNDRY_DEBTORS_GROUP: &str = "Sundry Debtors";

// This test's own analytical definitions, not tax law.
const LAG_OVER_DAYS: i64 = 30;
const RECEIPT_MATCH_MIN_DAYS: i64 = 0;
const RECEIPT_MATCH_MAX_DAYS: i64 = 3;

/// A Contra narration's claimed direction and the Cash-line sign it implies, in the reference's
/// dict order: WITHDRAWAL (cash into hand, net debit) is tried before DEPOSIT (net credit).
const CONTRA_DIRECTION_KEYWORDS: [(&str, i64); 2] = [("WITHDRAWAL", 1), ("DEPOSIT", -1)];

/// The inputs this test takes from the client configuration, already bound and typed.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub payment_channel_debtors: BTreeSet<String>,
    /// Ledger -> GST head; a ledger listed under two heads keeps the last, as the reference's
    /// flattening dict does.
    pub tax_ledgers_by_head: BTreeMap<String, String>,
    pub gst_payment_ledgers: BTreeSet<String>,
    pub reissue_narration_terms: Vec<String>,
    pub writeoff_discount_ledgers: BTreeSet<String>,
}

fn overflow() -> AuditError {
    support::overflow(TEST_ID)
}

fn count(n: usize) -> Result<Value> {
    support::count(TEST_ID, n)
}

fn checked_sum(xs: impl Iterator<Item = i64>) -> Result<i64> {
    xs.into_iter()
        .try_fold(0i64, |acc, a| acc.checked_add(a).ok_or_else(overflow))
}

/// Tally's reserved voucher-type names go into figure ids literally, normalised the reference's
/// way: `re.sub(r"[^a-z0-9]+", "_", base_type.lower()).strip("_") or "unknown"`. The class is
/// ASCII (no `re.I`), so every other character, after Python's full lower-casing, becomes `_`.
fn slug(base_type: &str) -> String {
    let lowered = support::py_lower(base_type);
    let mut out = String::new();
    let mut in_run = false;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Voucher evidence from a map already ordered by GUID, as the reference sorts `items()`.
fn evidence<'a, V: Copy + 'a>(
    vouchers: impl IntoIterator<Item = (&'a &'a str, &'a V)>,
    voucher: impl Fn(V) -> &'a Voucher,
) -> Vec<EvidenceRef> {
    vouchers
        .into_iter()
        .map(|(g, v)| EvidenceRef::with_label("voucher", g, &support::voucher_label(voucher(*v))))
        .collect()
}

fn one_voucher(v: &Voucher) -> Vec<EvidenceRef> {
    vec![EvidenceRef::with_label(
        "voucher",
        &v.guid,
        &support::voucher_label(v),
    )]
}

/// Sum of a voucher's positive (debit-side) ledger lines.
fn gross_paise(v: &Voucher) -> Result<i64> {
    checked_sum(
        v.lines
            .iter()
            .filter(|l| l.amount_paise > 0)
            .map(|l| l.amount_paise),
    )
}

fn sum_gross<'a>(vs: impl Iterator<Item = &'a Voucher>) -> Result<i64> {
    vs.map(gross_paise)
        .try_fold(0i64, |acc, g| acc.checked_add(g?).ok_or_else(overflow))
}

fn guid_hash12(guid: &str) -> String {
    Sha256::digest(guid.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Python's default `sys.get_int_max_str_digits()`.
const PY_INT_MAX_STR_DIGITS: usize = 4300;

/// Python's `int(text)` for a MASTERID string: `Ok(None)` where `int()` raises `ValueError` (the
/// voucher is then "unparseable"), `Err` where `int()` reads a number this port does not.
///
/// `int()` strips exactly the characters Rust's `char::is_whitespace` holds (measured over every
/// code point, Python 3.13) -- not `str.strip()`'s set, which adds U+001C..U+001F -- then takes an
/// optional sign and decimal digits with single underscores between them. Its decimal digits are
/// Python's `\d` ([`support::py_is_decimal`]); a well-formed MASTERID holding a non-ASCII one is
/// refused rather than read, as is one beyond i64. Text `int()` rejects -- including more than
/// 4,300 digits -- is unparseable here too, non-ASCII digits or not.
///
/// Not the crate's `py_int`: that reads a config value and gives one error both where `int()`
/// raises and where it reads a number the port does not, while this test must count the first as
/// unparseable and refuse only the second; its string branch also strips `str.strip()`'s set,
/// refuses i64::MIN and has no 4,300-digit limit.
fn masterid_int(text: &str) -> Result<Option<i64>> {
    let t = text.trim();
    let (negative, digits) = match t.chars().next() {
        Some('-') => (true, &t[1..]),
        Some('+') => (false, &t[1..]),
        _ => (false, t),
    };
    let well_formed = !digits.is_empty()
        && digits
            .split('_')
            .all(|g| !g.is_empty() && g.chars().all(support::py_is_decimal));
    if !well_formed {
        return Ok(None);
    }
    // Python 3.13's `int()` raises past 4,300 digits (`sys.get_int_max_str_digits()`), counting
    // every digit, leading zeros and non-ASCII ones included, and no sign, whitespace or `_`.
    if digits.chars().filter(|c| *c != '_').count() > PY_INT_MAX_STR_DIGITS {
        return Ok(None);
    }
    if !digits.is_ascii() {
        return Err(AuditError::Config(format!(
            "{TEST_ID}: MASTERID {text:?} has a non-ASCII digit, which Python's int() reads"
        )));
    }
    // The sign is parsed with the digits, so i64::MIN is read, not refused.
    let signed = format!(
        "{}{}",
        if negative { "-" } else { "" },
        digits.replace('_', "")
    );
    signed
        .parse::<i64>()
        .map(Some)
        .map_err(|_| AuditError::Config(format!("{TEST_ID}: MASTERID {text:?} is beyond i64")))
}

/// `sale < cost` for an integer and a float, exactly, as Python compares them (the integer is
/// never rounded to a float first).
fn int_below_float(sale: i64, cost: f64) -> bool {
    if cost.is_nan() {
        return false;
    }
    if cost.is_infinite() {
        return cost > 0.0;
    }
    let floor = cost.floor();
    // A finite f64 beyond 2^63 in magnitude saturates here, which still orders correctly
    // against every i64.
    #[allow(clippy::cast_possible_truncation)]
    let f = floor as i128;
    if floor == cost {
        i128::from(sale) < f
    } else {
        i128::from(sale) <= f
    }
}

/// Python's `int(round(x))` for a float: half to even, then exact; refused beyond i64.
fn round_half_even(x: f64) -> Result<i64> {
    let r = x.round_ties_even();
    if !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&r) {
        return Err(overflow());
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(r as i64)
}

struct EntryOrder<'a> {
    /// GUID -> (lag days, voucher): the last in creation order per GUID, as the dict keeps it.
    lag: BTreeMap<&'a str, (i64, &'a Voucher)>,
    after_last_sale: BTreeMap<&'a str, &'a Voucher>,
    unparseable: usize,
}

fn entry_order<'a>(pop: &[&'a Voucher]) -> Result<EntryOrder<'a>> {
    let mut ordered: Vec<(i64, &Voucher)> = Vec::new();
    let mut unparseable = 0;
    for v in pop {
        match v
            .masterid
            .as_deref()
            .map(masterid_int)
            .transpose()?
            .flatten()
        {
            Some(mid) => ordered.push((mid, v)),
            None => unparseable += 1,
        }
    }
    ordered.sort_by_key(|(mid, _)| *mid); // stable, as Python's sort
    let last_sale = ordered
        .iter()
        .filter(|(_, v)| v.base_type == SALES_BASE_TYPE)
        .map(|(mid, _)| *mid)
        .max();
    let mut lag = BTreeMap::new();
    let mut clock: Option<&TallyDate> = None;
    for (_, v) in &ordered {
        if v.base_type == SALES_BASE_TYPE {
            clock = Some(match clock {
                Some(c) if *c >= v.date => c,
                _ => &v.date,
            });
            continue;
        }
        let Some(c) = clock else {
            continue; // no sale created yet at this point in the order
        };
        let days = civil_day_number(c) - civil_day_number(&v.date);
        lag.insert(v.guid.as_str(), (days, *v));
    }
    let after_last_sale = match last_sale {
        Some(last) => ordered
            .iter()
            .filter(|(mid, _)| *mid > last)
            .map(|(_, v)| (v.guid.as_str(), *v))
            .collect(),
        None => BTreeMap::new(),
    };
    Ok(EntryOrder {
        lag,
        after_last_sale,
        unparseable,
    })
}

/// Sales vouchers with a debit line on a payment-channel debtor: GUID -> (voucher, amount).
fn payment_channel_invoices<'a>(
    pop: &[&'a Voucher],
    debtors: &BTreeSet<String>,
) -> Result<BTreeMap<&'a str, (&'a Voucher, i64)>> {
    let mut out = BTreeMap::new();
    for v in pop {
        if v.base_type != SALES_BASE_TYPE {
            continue;
        }
        let amt = checked_sum(
            v.lines
                .iter()
                .filter(|l| debtors.contains(&l.ledger) && l.amount_paise > 0)
                .map(|l| l.amount_paise),
        )?;
        if amt != 0 {
            out.insert(v.guid.as_str(), (*v, amt));
        }
    }
    Ok(out)
}

/// Non-Sales vouchers crediting a payment-channel debtor: GUID -> (voucher, positive amount).
fn payment_channel_receipts<'a>(
    pop: &[&'a Voucher],
    debtors: &BTreeSet<String>,
) -> Result<BTreeMap<&'a str, (&'a Voucher, i64)>> {
    let mut out = BTreeMap::new();
    for v in pop {
        if v.base_type == SALES_BASE_TYPE {
            continue;
        }
        let credit = checked_sum(
            v.lines
                .iter()
                .filter(|l| debtors.contains(&l.ledger) && l.amount_paise < 0)
                .map(|l| l.amount_paise),
        )?;
        let amt = credit.checked_neg().ok_or_else(overflow)?;
        if amt != 0 {
            out.insert(v.guid.as_str(), (*v, amt));
        }
    }
    Ok(out)
}

/// Greedy match, invoices oldest first (date, then GUID): each takes the first unused receipt of
/// the same amount dated 0-3 days after it, receipts tried in (date, GUID) order. Invoice GUID ->
/// lag days, matched invoices only.
fn match_invoices_to_receipts<'a>(
    invoices: &BTreeMap<&'a str, (&'a Voucher, i64)>,
    receipts: &BTreeMap<&'a str, (&'a Voucher, i64)>,
) -> BTreeMap<&'a str, i64> {
    let mut by_amount: BTreeMap<i64, Vec<(&TallyDate, &str)>> = BTreeMap::new();
    for (guid, (v, amt)) in receipts {
        by_amount.entry(*amt).or_default().push((&v.date, guid));
    }
    for list in by_amount.values_mut() {
        list.sort_unstable();
    }
    let mut ordered: Vec<(&str, &Voucher, i64)> =
        invoices.iter().map(|(g, (v, a))| (*g, *v, *a)).collect();
    ordered.sort_by(|a, b| (&a.1.date, a.0).cmp(&(&b.1.date, b.0)));
    let mut used: BTreeSet<&str> = BTreeSet::new();
    let mut matches = BTreeMap::new();
    for (guid, v, amt) in ordered {
        let Some(candidates) = by_amount.get(&amt) else {
            continue;
        };
        for (rdate, rguid) in candidates {
            if used.contains(rguid) {
                continue;
            }
            let lag = civil_day_number(rdate) - civil_day_number(&v.date);
            if (RECEIPT_MATCH_MIN_DAYS..=RECEIPT_MATCH_MAX_DAYS).contains(&lag) {
                used.insert(rguid);
                matches.insert(guid, lag);
                break;
            }
        }
    }
    matches
}

/// Item -> paise per unit, from every Purchase voucher's inventory lines with a positive quantity
/// and an amount: `sum(abs(amount)) / sum(qty)`, both accumulated in population order.
fn average_purchase_rate(pop: &[&Voucher]) -> Result<BTreeMap<String, f64>> {
    let mut amt: BTreeMap<&str, i64> = BTreeMap::new();
    let mut qty: BTreeMap<&str, f64> = BTreeMap::new();
    for v in pop {
        if v.base_type != PURCHASE_BASE_TYPE {
            continue;
        }
        for i in &v.inventory {
            let (Some(q), Some(a)) = (i.qty, i.amount_paise) else {
                continue;
            };
            if q > 0.0 {
                let e = amt.entry(i.item.as_str()).or_insert(0);
                *e = e
                    .checked_add(a.checked_abs().ok_or_else(overflow)?)
                    .ok_or_else(overflow)?;
                *qty.entry(i.item.as_str()).or_insert(0.0) += q;
            }
        }
    }
    // Python's int / float rounds the int to the nearest float first, as `as f64` does.
    #[allow(clippy::cast_precision_loss)]
    Ok(amt
        .into_iter()
        .filter(|(item, _)| qty[item] != 0.0)
        .map(|(item, a)| (item.to_string(), a as f64 / qty[item]))
        .collect())
}

/// How a Sales voucher's own debit lines classify its debtor: 0 a payment-channel debtor line,
/// else 1 a cash line, else 2 a named customer. Never PARTYLEDGERNAME.
fn sale_bucket(v: &Voucher, debtors: &BTreeSet<String>, cash: &BTreeSet<String>) -> usize {
    let debits: BTreeSet<&String> = v
        .lines
        .iter()
        .filter(|l| l.amount_paise > 0)
        .map(|l| &l.ledger)
        .collect();
    if debits.iter().any(|l| debtors.contains(*l)) {
        0
    } else if debits.iter().any(|l| cash.contains(*l)) {
        1
    } else {
        2
    }
}

#[derive(Default, Clone, Copy)]
struct Bucket {
    lines: usize,
    below_cost: usize,
    sales_paise: i64,
    cost_paise: f64,
}

/// Payment-channel, cash-sale and named-customer buckets, in that order.
fn margin_buckets(
    pop: &[&Voucher],
    debtors: &BTreeSet<String>,
    cash: &BTreeSet<String>,
) -> Result<[Bucket; 3]> {
    let rate = average_purchase_rate(pop)?;
    let mut buckets = [Bucket::default(); 3];
    for v in pop {
        if v.base_type != SALES_BASE_TYPE {
            continue;
        }
        let b = &mut buckets[sale_bucket(v, debtors, cash)];
        for i in &v.inventory {
            let Some(&r) = rate.get(&i.item) else {
                continue;
            };
            let (Some(q), Some(a)) = (i.qty, i.amount_paise) else {
                continue;
            };
            if r == 0.0 || q <= 0.0 {
                continue;
            }
            let cost = r * q;
            let sale = a.checked_abs().ok_or_else(overflow)?;
            b.lines += 1;
            b.below_cost += usize::from(int_below_float(sale, cost));
            b.sales_paise = b.sales_paise.checked_add(sale).ok_or_else(overflow)?;
            b.cost_paise += cost;
        }
    }
    Ok(buckets)
}

fn margin_bp(b: Bucket) -> Result<Value> {
    let net = b
        .sales_paise
        .checked_sub(round_half_even(b.cost_paise)?)
        .ok_or_else(overflow)?;
    pct_bp(net, b.sales_paise).ok_or_else(overflow)
}

#[allow(clippy::too_many_lines)] // one section per reference section, in the reference's order
pub fn run(
    book: &Book,
    rules: &Rules,
    cash: &BTreeSet<String>,
    inputs: &Inputs,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let pop = book.population()?;
    r.population_note =
        "Books population (optional, cancelled and post-dated vouchers excluded).".to_string();

    // ------------------------------------------------------------------ 1. entry order
    let eo = entry_order(&pop)?;
    if eo.unparseable > 0 {
        r.fig(
            "entry_order_unparseable_masterid_count",
            count(eo.unparseable)?,
            Unit::Count,
            "Population vouchers with no usable creation-order number; excluded from every \
entry-order figure.",
            Vec::new(),
        );
    }
    let mut lag_by_type: BTreeMap<&str, BTreeMap<&str, (i64, &Voucher)>> = BTreeMap::new();
    for (guid, (lag, v)) in &eo.lag {
        lag_by_type
            .entry(v.base_type.as_str())
            .or_default()
            .insert(guid, (*lag, v));
    }
    for (base_type, rows) in &lag_by_type {
        let over: BTreeMap<&str, &Voucher> = rows
            .iter()
            .filter(|(_, (lag, _))| *lag > LAG_OVER_DAYS)
            .map(|(g, (_, v))| (*g, *v))
            .collect();
        let s = slug(base_type);
        r.fig(
            &format!("entry_order_lag_population_count_{s}"),
            count(rows.len())?,
            Unit::Count,
            &format!(
                "Population vouchers of base type '{base_type}' created (in Tally's creation \
order) after at least one sales voucher, with a lag against the latest sale date already created."
            ),
            Vec::new(),
        );
        let f_over = r.fig(
            &format!("entry_order_lag_over_30_count_{s}"),
            count(over.len())?,
            Unit::Count,
            &format!(
                "Population vouchers of base type '{base_type}' created (in Tally's creation \
order) after a sales voucher, whose own date is more than {LAG_OVER_DAYS} days before the latest \
sale date already created at that point (that sale date less the voucher's own date)."
            ),
            evidence(&over, |v| v),
        );
        if !over.is_empty() {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/entry_order/lag_over_30/{s}"),
                clauses: Vec::new(),
                title: format!(
                    "{} {base_type} vouchers were entered after a later-dated sale and are dated \
more than {LAG_OVER_DAYS} days before it",
                    over.len()
                ),
                facts: vec![("over_30_count".to_string(), f_over)],
                evidence: evidence(&over, |v| v),
                confidence: Confidence::Indicative,
                limits: vec![
                    "Tally's creation sequence approximates the order vouchers were created in \
the company; ALTERID moves on a later content edit but the creation order does not, so this lag \
does not show whether the voucher was later edited."
                        .to_string(),
                    "Gaps in Tally's creation sequence are not evidence of a deleted voucher (an \
unexported master also consumes an ID); this test draws no conclusion from any gap in the \
sequence."
                        .to_string(),
                ],
                ask_client: vec![
                    "Confirm whether these vouchers were created late because the underlying \
document reached the office late, or for some other reason."
                        .to_string(),
                ],
            });
        }
    }

    let after = &eo.after_last_sale;
    let f_after_count = r.fig(
        "entry_order_after_last_sale_count",
        count(after.len())?,
        Unit::Count,
        "Population vouchers (any base type) created after the sales voucher with the latest in \
Tally's creation order.",
        Vec::new(),
    );
    r.fig(
        "entry_order_after_last_sale_value_paise",
        Value::Int(sum_gross(after.values().copied())?),
        Unit::Paise,
        "Sum of the debit-side (gross) amount of every population voucher (any base type) created \
after the sales voucher with the latest in Tally's creation order.",
        evidence(after, |v| v),
    );
    let mut after_by_type: BTreeMap<&str, usize> = BTreeMap::new();
    for v in after.values() {
        *after_by_type.entry(v.base_type.as_str()).or_default() += 1;
    }
    let mut after_facts = vec![("after_last_sale_count".to_string(), f_after_count)];
    for (base_type, n) in &after_by_type {
        let s = slug(base_type);
        let fid = r.fig(
            &format!("entry_order_after_last_sale_count_{s}"),
            count(*n)?,
            Unit::Count,
            &format!(
                "Of the vouchers created after the last sale, those with base type \
'{base_type}'."
            ),
            Vec::new(),
        );
        after_facts.push((format!("after_last_sale_count_{s}"), fid));
    }
    if !after.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/entry_order/after_last_sale"),
            clauses: Vec::new(),
            title: format!(
                "{} vouchers were created after the last sale voucher already entered in the \
company for the year",
                after.len()
            ),
            facts: after_facts,
            evidence: evidence(after, |v| v),
            confidence: Confidence::Indicative,
            limits: vec![
                "A closing/adjustment journal created after the last sale is ordinary; the same \
is not true of a purchase or other transaction voucher, which raises a cut-off question about the \
period it belongs to."
                    .to_string(),
                "Tally's creation order does not reflect any later edit to a voucher's own \
content (see the lag findings above)."
                    .to_string(),
            ],
            ask_client: vec![
                "For each base type above other than closing/adjustment journals, confirm the \
period the underlying transaction relates to."
                    .to_string(),
            ],
        });
    }

    // ------------------------------------------------------------------ 2. payment-channel invoices
    let debtors = &inputs.payment_channel_debtors;
    r.fig(
        "pc_configured_debtor_count",
        count(debtors.len())?,
        Unit::Count,
        "Debtor ledgers configured as a consolidated payment-channel debtor; a books test with \
none configured does not apply and reports zero throughout this section.",
        Vec::new(),
    );
    if !debtors.is_empty() {
        let invoices = payment_channel_invoices(&pop, debtors)?;
        let receipts = payment_channel_receipts(&pop, debtors)?;
        let matches = match_invoices_to_receipts(&invoices, &receipts);
        let within3: BTreeMap<&str, (&Voucher, i64)> = invoices
            .iter()
            .filter(|(g, _)| matches.contains_key(*g))
            .map(|(g, d)| (*g, *d))
            .collect();
        let next_day = matches.values().filter(|lag| **lag == 1).count();
        let unmatched = invoices
            .keys()
            .filter(|g| !matches.contains_key(*g))
            .count();

        let f_inv_count = r.fig(
            "pc_invoice_count",
            count(invoices.len())?,
            Unit::Count,
            "Sales vouchers with a debit line on a configured payment-channel debtor ledger.",
            evidence(&invoices, |d| d.0),
        );
        r.fig(
            "pc_invoice_value_total_paise",
            Value::Int(checked_sum(invoices.values().map(|(_, a)| *a))?),
            Unit::Paise,
            "Sum of the payment-channel debtor line(s) across the sales vouchers with a debit \
line on a configured payment-channel debtor ledger.",
            Vec::new(),
        );
        let f_within3 = r.fig(
            "pc_matched_within_3_days_count",
            count(within3.len())?,
            Unit::Count,
            &format!(
                "Payment-channel invoices (sales vouchers with a debit line on a configured \
payment-channel debtor ledger) paired with a receipt of the same amount on the same ledger, dated \
{RECEIPT_MATCH_MIN_DAYS}-{RECEIPT_MATCH_MAX_DAYS} days later (greedy match, oldest invoice first)."
            ),
            evidence(&within3, |d| d.0),
        );
        r.fig(
            "pc_matched_next_day_count",
            count(next_day)?,
            Unit::Count,
            &format!(
                "Payment-channel invoices paired with a receipt of the same amount on the same \
ledger (dated {RECEIPT_MATCH_MIN_DAYS}-{RECEIPT_MATCH_MAX_DAYS} days later, greedy match, oldest \
invoice first) whose receipt is dated exactly one day later."
            ),
            Vec::new(),
        );
        r.fig(
            "pc_unmatched_count",
            count(unmatched)?,
            Unit::Count,
            &format!(
                "Payment-channel invoices with no receipt of the same amount on the same ledger \
within {RECEIPT_MATCH_MIN_DAYS}-{RECEIPT_MATCH_MAX_DAYS} days."
            ),
            Vec::new(),
        );
        let mut per_month: BTreeMap<&str, usize> = BTreeMap::new();
        for (v, _) in invoices.values() {
            *per_month.entry(&v.date.as_str()[..6]).or_default() += 1;
        }
        if let (Some(min), Some(max)) = (per_month.values().min(), per_month.values().max()) {
            r.fig(
                "pc_invoices_per_month_min",
                count(*min)?,
                Unit::Count,
                "Fewest payment-channel invoices in any calendar month touched by the population.",
                Vec::new(),
            );
            r.fig(
                "pc_invoices_per_month_max",
                count(*max)?,
                Unit::Count,
                "Most payment-channel invoices in any calendar month touched by the population.",
                Vec::new(),
            );
        }

        if !invoices.is_empty() {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/payment_channel/receipt_match"),
                clauses: Vec::new(),
                title: format!(
                    "{} of {} configured payment-channel debtor invoices were matched to a \
receipt of the same amount on the same ledger, {RECEIPT_MATCH_MIN_DAYS}-{RECEIPT_MATCH_MAX_DAYS} \
days later",
                    within3.len(),
                    invoices.len()
                ),
                facts: vec![
                    ("invoice_count".to_string(), f_inv_count),
                    ("matched_within_3_days_count".to_string(), f_within3),
                ],
                evidence: evidence(&invoices, |d| d.0),
                confidence: Confidence::Indicative,
                limits: vec![
                    "A same-amount, near-date match is not proof that the receipt discharges \
that specific invoice; only a bill-wise link recorded in Tally, or the bank's own narration, would \
confirm it."
                        .to_string(),
                ],
                ask_client: vec![
                    "Confirm whether these are genuinely separate daily invoices to this ledger, \
or a single invoice built to match each day's bank credit."
                        .to_string(),
                ],
            });
        }

        let [pc, _, named] = margin_buckets(&pop, debtors, cash)?;
        if pc.lines > 0 && pc.sales_paise != 0 {
            let f_lines = r.fig(
                "pc_margin_lines_count",
                count(pc.lines)?,
                Unit::Count,
                "Sales-voucher inventory lines on payment-channel debtor invoices priced against \
the item's own year-average purchase rate.",
                Vec::new(),
            );
            let f_below = r.fig(
                "pc_margin_below_cost_share_bp",
                pct_bp(
                    i64::try_from(pc.below_cost).map_err(|_| overflow())?,
                    i64::try_from(pc.lines).map_err(|_| overflow())?,
                )
                .ok_or_else(overflow)?,
                Unit::BasisPoints,
                "Share of the sales-voucher inventory lines on payment-channel debtor invoices \
where the sale value is less than the item's year-average purchase rate x quantity.",
                Vec::new(),
            );
            let f_margin_pc = r.fig(
                "pc_margin_bp",
                margin_bp(pc)?,
                Unit::BasisPoints,
                "Overall (sales - cost) / sales across payment-channel debtor lines, cost from \
the item's year-average purchase rate.",
                Vec::new(),
            );
            let mut facts = vec![
                ("lines".to_string(), f_lines),
                ("below_cost_share_bp".to_string(), f_below),
                ("margin_bp".to_string(), f_margin_pc),
            ];
            let comparison_available = named.lines > 0 && named.sales_paise != 0;
            if comparison_available {
                let f_named = r.fig(
                    "named_customer_margin_bp",
                    margin_bp(named)?,
                    Unit::BasisPoints,
                    "Same overall margin definition, computed on 'named customer' Sales lines \
(debtor line not a payment-channel debtor or a cash line), for comparison.",
                    Vec::new(),
                );
                facts.push(("named_customer_margin_bp".to_string(), f_named));
            }
            let mut limits = vec![
                "The purchase rate is a single year average per item, not FIFO or a specific \
lot; an item bought at varying rates during the year will show some lines 'below cost' against \
the average even when priced above the lot actually consumed."
                    .to_string(),
                "Rule 46 permits a single consolidated daily B2C invoice only where each \
underlying supply is below ₹200; the books do not show the underlying supply-level breakdown \
behind a consolidated invoice."
                    .to_string(),
            ];
            if !comparison_available {
                limits.push(
                    "No 'named customer' Sales lines were priced against a purchase rate on this \
book; no comparison margin is available."
                        .to_string(),
                );
            }
            r.findings.push(Finding {
                id: format!("{TEST_ID}/payment_channel/margin"),
                clauses: vec!["3CD-40".to_string()],
                title: "Payment-channel debtor sale lines priced against the item's \
year-average purchase rate show a share below cost, compared with named-customer lines"
                    .to_string(),
                facts,
                evidence: evidence(&invoices, |d| d.0),
                confidence: Confidence::JudgementRequired,
                limits,
                ask_client: vec![
                    "Confirm whether each supply represented on these invoices is individually \
below the ₹200 Rule 46 threshold for a consolidated daily B2C invoice."
                        .to_string(),
                    "Confirm the pricing basis for sales through this ledger against the item's \
cost."
                        .to_string(),
                ],
            });
        }
    }

    // ------------------------------------------------------------------ 3. stock ledgers untouched
    let stock_ledgers = book.ledgers_under_any(&[STOCK_GROUP.to_string()]);
    let mut touches: BTreeMap<&String, usize> = stock_ledgers.iter().map(|n| (n, 0)).collect();
    for v in &pop {
        let touched: BTreeSet<&String> = v
            .lines
            .iter()
            .map(|l| &l.ledger)
            .filter(|l| stock_ledgers.contains(*l))
            .collect();
        for name in touched {
            if let Some(n) = touches.get_mut(name) {
                *n += 1;
            }
        }
    }
    let untouched: Vec<&String> = touches
        .iter()
        .filter(|(_, c)| **c == 0)
        .map(|(n, _)| *n)
        .collect();
    let untouched_refs = || -> Vec<EvidenceRef> {
        untouched
            .iter()
            .map(|n| EvidenceRef::new("ledger", n))
            .collect()
    };
    r.fig(
        "stock_ledger_total_count",
        count(touches.len())?,
        Unit::Count,
        "Ledgers under Tally's 'Stock-in-Hand' group.",
        Vec::new(),
    );
    let f_untouched = r.fig(
        "stock_ledger_untouched_count",
        count(untouched.len())?,
        Unit::Count,
        "Ledgers under Tally's 'Stock-in-Hand' group with zero population vouchers touching them \
all year.",
        untouched_refs(),
    );
    for (name, n) in &touches {
        let tag = stable_ledger_tag(book, name)?;
        r.fig(
            &format!("stock_ledger_touch_count_{tag}"),
            count(*n)?,
            Unit::Count,
            &format!("Population vouchers touching one Stock-in-Hand ledger (tag {tag}) all year."),
            Vec::new(),
        );
    }
    if !untouched.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/stock_untouched"),
            clauses: vec!["3CD-35".to_string()],
            title: format!(
                "{} of {} Stock-in-Hand ledgers have zero vouchers touching them for the year",
                untouched.len(),
                touches.len()
            ),
            facts: vec![("untouched_count".to_string(), f_untouched)],
            evidence: untouched_refs(),
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "A zero-touch stock ledger is consistent with the closing figure having been \
keyed in directly rather than built up from purchase, sale or stock-journal vouchers; valuing the \
closing figure is a separate stock test's job, not this one."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm how the closing quantity and value on each of these ledgers was arrived \
at."
                .to_string(),
            ],
        });
    }

    // ------------------------------------------------------------------ 4. unjournalised GST set-off
    r.fig(
        "gst_setoff_configured_payment_ledger_count",
        count(inputs.gst_payment_ledgers.len())?,
        Unit::Count,
        "GST payment ledgers set for this client; a books test with none configured does not \
apply and reports only the tax ledgers' own balances, if any.",
        Vec::new(),
    );
    let mut by_head: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for (ledger, head) in &inputs.tax_ledgers_by_head {
        by_head.entry(head.as_str()).or_default().push(ledger);
    }
    let mut head_facts = Vec::new();
    for (head, ledgers) in &by_head {
        let closing = checked_sum(
            ledgers
                .iter()
                .map(|l| book.tb.get(*l).map_or(0, |t| t.closing_paise)),
        )?;
        let fid = r.fig(
            &format!("gst_setoff_head_closing_paise_{head}"),
            Value::Int(closing),
            Unit::Paise,
            &format!(
                "Sum of TB closing balances across every ledger configured under GST head \
'{head}' (Dr+/Cr-)."
            ),
            Vec::new(),
        );
        head_facts.push((format!("{head}_closing"), fid));
    }
    let mut payment_facts = Vec::new();
    for ledger in &inputs.gst_payment_ledgers {
        let tb = book.tb.get(ledger);
        let h = stable_ledger_tag(book, ledger)?;
        let fid = r.fig(
            &format!("gst_setoff_payment_ledger_closing_paise_{h}"),
            Value::Int(tb.map_or(0, |t| t.closing_paise)),
            Unit::Paise,
            &format!("TB closing balance of one configured GST payment ledger (tag {h})."),
            Vec::new(),
        );
        payment_facts.push((format!("payment_closing_{h}"), fid));
        let fid2 = r.fig(
            &format!("gst_setoff_payment_ledger_debit_movement_paise_{h}"),
            Value::Int(tb.map_or(0, |t| t.debit_paise)),
            Unit::Paise,
            &format!(
                "TB period debit total of one configured GST payment ledger (tag {h}) -- the \
payments posted to it during the year."
            ),
            Vec::new(),
        );
        payment_facts.push((format!("payment_debit_movement_{h}"), fid2));
    }
    if !head_facts.is_empty() && !payment_facts.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/gst_setoff"),
            clauses: Vec::new(),
            title: "Output GST head balances and the configured GST payment ledger, reported as \
plain Trial Balance figures"
                .to_string(),
            facts: head_facts.into_iter().chain(payment_facts).collect(),
            evidence: Vec::new(),
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "The books do not show whether a monthly journal actually set off input credit \
against output liability; a large, growing balance on an output-tax ledger alongside all payments \
running through one ledger is consistent with the set-off never having been journalised, so the \
balances are accumulated gross amounts, but that reading needs the CA's own judgement on this data."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm whether the monthly GST liability was set off against input credit by \
journal entry, and if so, ask for those entries."
                    .to_string(),
            ],
        });
    }

    // ------------------------------------------------------------------ 5. re-issued / written-off
    let terms_upper: Vec<String> = inputs
        .reissue_narration_terms
        .iter()
        .map(|t| support::py_upper(t))
        .collect();
    let mut reissue: BTreeMap<&str, &Voucher> = BTreeMap::new();
    for v in &pop {
        let narration = support::py_upper(&v.narration);
        if terms_upper.iter().any(|t| narration.contains(t.as_str())) {
            reissue.insert(v.guid.as_str(), v);
        }
    }
    let f_reissue = r.fig(
        "reissue_narration_match_count",
        count(reissue.len())?,
        Unit::Count,
        "Population vouchers whose narration matches a configured re-issue/amendment/transfer \
term.",
        evidence(&reissue, |v| v),
    );
    r.fig(
        "reissue_narration_match_value_paise",
        Value::Int(sum_gross(reissue.values().copied())?),
        Unit::Paise,
        "Sum of the debit-side (gross) amount of every population voucher whose narration \
matches a configured re-issue/amendment/transfer term.",
        Vec::new(),
    );
    if !reissue.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/reissue_narration"),
            clauses: Vec::new(),
            title: format!(
                "{} vouchers have a narration matching a configured re-issue/amendment/transfer \
term",
                reissue.len()
            ),
            facts: vec![("match_count".to_string(), f_reissue)],
            evidence: evidence(&reissue, |v| v),
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "A narration match is not proof that a sale was actually re-issued, amended or \
transferred; some other administrative meaning may explain the same word."
                    .to_string(),
            ],
            ask_client: vec![
                "For each of these, confirm whether an earlier invoice for the same supply was \
formally cancelled in Tally before this one was raised."
                    .to_string(),
                "Confirm whether any of these fall outside the s.34(2) credit-note window (30 \
November following the financial year)."
                    .to_string(),
            ],
        });
    }

    // (population position, debtor ledger) -> (voucher, amount): one row per voucher and debtor, the
    // amount the sum of that debtor's credit lines on it; keyed by position, never by GUID, which can
    // be blank or repeated.
    let mut writeoffs: BTreeMap<(usize, String), (&Voucher, i64)> = BTreeMap::new();
    for (n, v) in pop.iter().enumerate() {
        if v.base_type != JOURNAL_BASE_TYPE
            || !v
                .lines
                .iter()
                .any(|l| inputs.writeoff_discount_ledgers.contains(&l.ledger) && l.amount_paise > 0)
        {
            continue;
        }
        for l in &v.lines {
            if l.amount_paise >= 0 {
                continue;
            }
            if book
                .ledgers
                .get(&l.ledger)
                .is_some_and(|led| led.under(SUNDRY_DEBTORS_GROUP))
            {
                let amt = l.amount_paise.checked_neg().ok_or_else(overflow)?;
                let row = writeoffs.entry((n, l.ledger.clone())).or_insert((*v, 0));
                row.1 = row.1.checked_add(amt).ok_or_else(overflow)?;
            }
        }
    }
    // Each write-off cites its own voucher by GUID, the debtor named in the label: once per distinct
    // citation, in (GUID, label) order, as the reference's sorted set does.
    let writeoff_evidence = || -> Vec<EvidenceRef> {
        writeoffs
            .iter()
            .map(|((_, debtor), (v, _))| {
                (
                    v.guid.clone(),
                    format!("{}, {debtor}", support::voucher_label(v)),
                )
            })
            .collect::<BTreeSet<(String, String)>>()
            .into_iter()
            .map(|(guid, label)| EvidenceRef::with_label("voucher", &guid, &label))
            .collect()
    };
    let f_writeoff = r.fig(
        "debtor_writeoff_count",
        count(writeoffs.len())?,
        Unit::Count,
        "Journal vouchers debiting a configured discount/write-off ledger and crediting a Sundry \
Debtors ledger.",
        writeoff_evidence(),
    );
    r.fig(
        "debtor_writeoff_total_paise",
        Value::Int(checked_sum(writeoffs.values().map(|(_, a)| *a))?),
        Unit::Paise,
        "Sum of the debtor-ledger credit line(s) across the journal vouchers debiting a \
configured discount/write-off ledger and crediting a Sundry Debtors ledger.",
        Vec::new(),
    );
    if !writeoffs.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/debtor_writeoff"),
            clauses: Vec::new(),
            title: format!(
                "{} journal entries write a Sundry Debtors ledger off against a configured \
discount/write-off ledger",
                writeoffs.len()
            ),
            facts: vec![("writeoff_count".to_string(), f_writeoff)],
            evidence: writeoff_evidence(),
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "Whether each write-off is an actual bad debt, a negotiated discount, or a \
correction of a duplicate or erroneous invoice is not visible from the voucher alone."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm the reason recorded for each write-off.".to_string(),
                "Confirm whether the invoice being written off had output GST charged on it \
originally."
                    .to_string(),
            ],
        });
    }

    // ------------------------------------------------------------------ 6. contra narration vs direction
    let mut mismatches: BTreeMap<&str, (&Voucher, i64)> = BTreeMap::new();
    for v in &pop {
        if v.base_type != CONTRA_BASE_TYPE {
            continue;
        }
        let cash_net = checked_sum(
            v.lines
                .iter()
                .filter(|l| cash.contains(&l.ledger))
                .map(|l| l.amount_paise),
        )?;
        if cash_net == 0 {
            continue;
        }
        let narration = support::py_upper(&v.narration);
        for (keyword, expected_sign) in CONTRA_DIRECTION_KEYWORDS {
            if narration.contains(keyword) && (cash_net > 0) != (expected_sign > 0) {
                mismatches.insert(v.guid.as_str(), (v, cash_net));
                break;
            }
        }
    }
    r.fig(
        "contra_direction_mismatch_count",
        count(mismatches.len())?,
        Unit::Count,
        "Contra vouchers whose narration claims a direction that their own Cash line's sign \
contradicts.",
        evidence(&mismatches, |d| d.0),
    );
    let abs_total = mismatches
        .values()
        .map(|(_, a)| a.checked_abs().ok_or_else(overflow))
        .collect::<Result<Vec<i64>>>()?;
    r.fig(
        "contra_direction_mismatch_total_paise",
        Value::Int(checked_sum(abs_total.into_iter())?),
        Unit::Paise,
        "Sum of the Cash line amounts, each taken without its sign, across the Contra vouchers \
whose narration claims a direction that their own Cash line's sign contradicts.",
        Vec::new(),
    );
    let mut ordered: Vec<(&str, &Voucher, i64)> =
        mismatches.iter().map(|(g, (v, a))| (*g, *v, *a)).collect();
    ordered.sort_by(|a, b| (&a.1.date, a.0).cmp(&(&b.1.date, b.0)));
    for (_, v, cash_amount) in ordered {
        let h = guid_hash12(&v.guid);
        let f_amt = r.fig(
            &format!("contra_direction_cash_amount_paise_{h}"),
            Value::Int(cash_amount),
            Unit::Paise,
            "Net Cash-line amount (Dr+/Cr-) on this Contra voucher.",
            one_voucher(v),
        );
        r.findings.push(Finding {
            id: format!("{TEST_ID}/contra_direction/{h}"),
            clauses: Vec::new(),
            title: format!(
                "Contra narration and the voucher's own Cash-line direction disagree, on {}",
                iso(&v.date)
            ),
            facts: vec![("cash_amount".to_string(), f_amt)],
            evidence: one_voucher(v),
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "The narration, not the entry, may be the error: if a later bank statement ties \
to this amount and date, to the paisa, on the side actually booked (not the side the narration \
names), the entry is more likely correct and only its narration text is wrong."
                    .to_string(),
            ],
            ask_client: vec![
                "Does a later bank statement tie, to the paisa, to this amount on the side \
actually booked?"
                    .to_string(),
            ],
        });
    }

    Ok(r)
}

/// BKQ-1, independent of [`run`]: a Stock-in-Hand ledger reported untouched
/// (`stock_ledger_touch_count_<tag>` of 0) must show no Trial Balance movement beyond Re 1 -- a
/// movement means the touch count itself is wrong, not that the ledger was untouched.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    const TOL_PAISE: i64 = 100;
    let mut out = Vec::new();
    let tag_to_name = support::ledgers_by_tag(book)?;
    let marker = format!("{}.stock_ledger_touch_count_", result.test_id);
    let mut figures: Vec<&Figure> = result.figures.iter().collect();
    figures.sort_by(|a, b| a.id.cmp(&b.id));
    for fig in figures {
        let Some(tag) = fig.id.strip_prefix(marker.as_str()) else {
            continue;
        };
        if fig.value != Value::Int(0) {
            continue;
        }
        let Some(name) = tag_to_name.get(tag) else {
            out.push(format!(
                "BKQ-1: cannot resolve a stock ledger for figure {} (tag {tag})",
                fig.id
            ));
            continue;
        };
        let tb = book.tb.get(*name);
        let movement = match tb {
            Some(t) => t.movement_paise(),
            None => 0,
        };
        if movement.checked_abs().ok_or_else(overflow)? > TOL_PAISE {
            out.push(format!(
                "BKQ-1: {name} reported as untouched (0 vouchers) but TB movement is {movement}p \
(opening {}p -> closing {}p)",
                tb.map_or(0, |t| t.opening_paise),
                tb.map_or(0, |t| t.closing_paise)
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each expected value is Python 3.13's own `int()` on the same text.
    #[test]
    fn masterid_is_read_as_python_int_reads_it() {
        for (text, want) in [
            (" 12 ", Some(12)),
            ("+3", Some(3)),
            ("-2", Some(-2)),
            ("1_0", Some(10)),
            ("007", Some(7)),
            ("-0", Some(0)),
            ("\u{a0}12\u{2003}", Some(12)),
            ("x", None),
            ("1__0", None),
            ("_1", None),
            ("", None),
            ("1_", None),
            ("- 5", None),
            ("+", None),
            ("\u{1c}12", None),
            ("12\u{1f}", None),
            ("\u{85}12", Some(12)),
            ("12\u{3000}", Some(12)),
            ("\u{2028}-7\u{2029}", Some(-7)),
            ("\u{663}x", None),
        ] {
            assert_eq!(masterid_int(text).unwrap(), want, "{text:?}");
        }
        // int() reads these; the port refuses rather than read them differently.
        assert!(masterid_int("\u{0661}\u{0662}").is_err());
        assert!(masterid_int("1_\u{663}").is_err());
        assert!(masterid_int("99999999999999999999").is_err());
        // Python rejects more than 4,300 digits (zeros count; underscores do not), so those are
        // unparseable, even non-ASCII ones; exactly 4,300 zeros is 0.
        assert_eq!(masterid_int(&"1".repeat(4301)).unwrap(), None);
        assert_eq!(masterid_int(&"0".repeat(4301)).unwrap(), None);
        assert_eq!(masterid_int(&"\u{661}".repeat(4301)).unwrap(), None);
        let underscored = vec!["1"; 4301].join("_");
        assert_eq!(masterid_int(&underscored).unwrap(), None);
        assert_eq!(masterid_int(&"0".repeat(4300)).unwrap(), Some(0));
        // 4,300 digits with underscores between them is within the limit: int() reads it.
        assert!(masterid_int(&vec!["1"; 4300].join("_")).is_err());
        assert!(masterid_int(&"1".repeat(4300)).is_err()); // int() reads it; beyond i64
                                                           // The i64 bounds themselves are read; one past either is refused.
        assert_eq!(
            masterid_int("-9223372036854775808").unwrap(),
            Some(i64::MIN)
        );
        assert_eq!(masterid_int("9223372036854775807").unwrap(), Some(i64::MAX));
        assert!(masterid_int("-9223372036854775809").is_err());
        assert!(masterid_int("9223372036854775808").is_err());
    }

    /// Python's `int < float` is exact; each expectation is Python's own answer.
    #[test]
    fn a_sale_is_compared_with_its_cost_exactly() {
        assert!(int_below_float(10_000, 10_000.000_000_000_002));
        assert!(!int_below_float(5, 5.0));
        assert!(int_below_float(i64::MAX, 9.223_372_036_854_776e18));
        assert!(int_below_float(-3, -2.5));
        assert!(!int_below_float(-2, -2.5));
    }

    /// Python's `round()`: half to even.
    #[test]
    fn cost_rounds_half_to_even() {
        for (x, want) in [
            (0.5, 0),
            (1.5, 2),
            (2.5, 2),
            (-0.5, 0),
            (-1.5, -2),
            (12.5, 12),
        ] {
            assert_eq!(round_half_even(x).unwrap(), want, "{x}");
        }
        assert!(round_half_even(1e300).is_err());
    }

    /// `re.sub(r"[^a-z0-9]+", "_", base_type.lower()).strip("_") or "unknown"`, per Python.
    #[test]
    fn base_type_slugs_follow_the_reference() {
        for (base, want) in [
            ("Débit Note", "d_bit_note"),
            ("***", "unknown"),
            ("Credit Note", "credit_note"),
            ("_A_", "a"),
            ("\u{130} Note", "i_note"),
            ("Sales", "sales"),
        ] {
            assert_eq!(slug(base), want, "{base:?}");
        }
    }
}
