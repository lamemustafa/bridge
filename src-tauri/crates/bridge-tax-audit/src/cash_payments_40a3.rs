//! Cash payments/receipts tests keyed off the books' cash ledgers, a port of the reference
//! Python implementation's `cash_payments_40a3` test module, version 1:
//!   (a) s.40A(3): cash paid to one payee in one day, over the per-person-per-day limit.
//!   (b) s.269ST limb (a) limb (i): cash received from one party in one day, at or over the
//!       limit. (Limbs (ii)/(iii) need bill-wise/event linkage the books do not carry; out of
//!       scope here, stated in the finding.)
//!   (b2) s.269ST limb (a), PAYMENT leg: cash paid to one party in one day, at or over the
//!       limit. s.269ST binds the receiver only; a cash payment by the assessee is never a
//!       contravention or a penalty exposure under s.269ST/s.271DA for the assessee -- it is a
//!       reporting duty under Form 3CD clause 31(bc) (limb (a) only). Every payment-side finding
//!       says exactly that, in words that never imply the assessee has breached anything.
//!   (c) s.269SS/269T candidates: a voucher with both a cash line and a Loans (Liability) line,
//!       at or over the limit; direction (loan accepted/repaid) from the cash sign.
//!
//! Double-count fix (mirrors the reference module's own 2026-09-17 note): (b)/(b2) above never
//! carry a Clause 31 tag -- a separate books-wide party-day scan (not yet ported) already tags
//! every cash receipt/payment 3CD-31(ba)/3CD-31(bc), so tagging it here too would double-count
//! the same voucher. (c) above keeps its Clause 31 tag only for a loan ledger NOT in the
//! client's `[loans]` configuration; a configured ledger is assumed already tagged by a
//! per-lender scan (not yet ported) and is an observation only here.
//!
//! Payee/party attribution comes from each non-cash ledger LINE on a voucher, never from a
//! single party field (a voucher paying two people in cash the same day must attribute to
//! both). Contra is excluded outright. s.40A(3) exclusion walks a payee ledger's full group
//! CHAIN, not just its immediate parent, so a ledger one level below "Loans (Liability)" is
//! still excluded.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;
use sha2::Sha256;

use crate::book::{Book, Ledger, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::iso;
use crate::rules::Rules;

pub const TEST_ID: &str = "cash_payments_40a3";
pub const VERSION: &str = "1";

const POPULATION: &str = "Books population (optional, cancelled and post-dated vouchers \
excluded); Contra excluded throughout.";

/// Not a real ledger name: the sentinel for a cash leg with a tax/round-off line but no
/// identified real party (module docstring).
const UNIDENTIFIED_PARTY: &str = "\u{2039}cash leg with no identified party\u{203a}";

/// Tally's own reserved/standard group names (not client data) that put a payee ledger out of
/// s.40A(3) "expenditure" scope, keyed by the rules-configured role name. A ledger's chain
/// contains at most one of these in practice (they sit under different primary groups).
const GROUP_BY_KIND: [(&str, &str); 5] = [
    ("capital", "Capital Account"),
    ("loans_liability", "Loans (Liability)"),
    ("loans_advances_asset", "Loans & Advances (Asset)"),
    ("fixed_assets", "Fixed Assets"),
    ("duties_taxes", "Duties & Taxes"),
];
const SALES_ACCOUNTS_GROUP: &str = "Sales Accounts";
const PURCHASE_ACCOUNTS_GROUP: &str = "Purchase Accounts";
const LOANS_LIABILITY_GROUP: &str = "Loans (Liability)";
const DUTIES_TAXES_GROUP: &str = "Duties & Taxes";

fn overflow() -> AuditError {
    AuditError::Config("cash_payments_40a3: a total overflowed i64 paise".to_string())
}

/// A generic transport-name match (heuristic only, stated as such in every finding that uses
/// it): the reference engine's own regex, searched with its `re.I` semantics
/// (`support::py_re_search`). Not a hardcoded list of staff or client names.
const TRANSPORT_NAME_RE: &str = "FREIGHT|TRANSPORT|ROAD\\s?LINES|CARRIER|LOGISTIC|CARGO|ROADWAYS";

fn transport_name_match(name: &str) -> bool {
    let alts = crate::support::re_alternatives(TRANSPORT_NAME_RE);
    let alts: Vec<&[crate::support::ReTok]> = alts.iter().map(Vec::as_slice).collect();
    crate::support::py_re_search(name, &alts)
}

/// The reference implementation's `_hash` used inline for a voucher GUID at 12 hex characters
/// (sha256, not sha1 -- the s.269SS/269T row id mixes both hashes deliberately, matching the
/// Python source exactly).
fn hash12_sha256(text: &str) -> String {
    use sha2::Digest;
    crate::canonical::hex(&Sha256::digest(text.as_bytes()))[..12].to_string()
}

/// Which s.40A(3) exclusion role (if any) a payee ledger falls under, by its full group chain
/// (not just the immediate parent).
fn classify_kind(ledger: Option<&Ledger>) -> &'static str {
    let Some(ledger) = ledger else {
        return "expenditure";
    };
    for (kind, group) in GROUP_BY_KIND {
        if ledger.under(group) {
            return kind;
        }
    }
    "expenditure"
}

fn group_for_kind(kind: &str) -> Result<&'static str> {
    GROUP_BY_KIND
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, g)| *g)
        .ok_or_else(|| {
            AuditError::Config(format!(
                "cash_payments_40a3: rules.s40a3.excluded_group_roles names unknown role {kind:?}"
            ))
        })
}

use crate::support::voucher_label;

/// One (date, ledger) row's aggregate: cash amount and the distinct vouchers that contributed.
struct RowAgg<'a> {
    paise: i64,
    vouchers: BTreeMap<String, &'a Voucher>,
}

impl<'a> RowAgg<'a> {
    fn new() -> Self {
        Self {
            paise: 0,
            vouchers: BTreeMap::new(),
        }
    }

    fn add(&mut self, amount: i64, v: &'a Voucher) -> Result<()> {
        self.paise = self.paise.checked_add(amount).ok_or_else(overflow)?;
        self.vouchers.insert(v.guid.clone(), v);
        Ok(())
    }
}

type RowMap<'a> = BTreeMap<(TallyDate, String), RowAgg<'a>>;

/// Evidence refs for one voucher map: sorted by guid, each carrying the fixed voucher label the
/// canonical serialiser compares.
fn evidence_for_vouchers(vouchers: &BTreeMap<String, &Voucher>) -> Vec<EvidenceRef> {
    vouchers
        .iter()
        .map(|(g, v)| EvidenceRef::with_label("voucher", g, &voucher_label(v)))
        .collect()
}

/// Evidence refs across every voucher touched by any row in `rows`, deduplicated by guid.
fn evidence_for_rows(rows: &RowMap<'_>) -> Vec<EvidenceRef> {
    let mut all: BTreeMap<String, &Voucher> = BTreeMap::new();
    for agg in rows.values() {
        for (g, v) in &agg.vouchers {
            all.insert(g.clone(), v);
        }
    }
    evidence_for_vouchers(&all)
}

/// (in_scope_rows, excluded_rows) keyed by (date, payee ledger name); `excluded_rows` has one
/// entry per role in `excluded_roles`, even a role with no rows.
fn compute_40a3_rows<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    excluded_roles: &[String],
) -> Result<(RowMap<'a>, BTreeMap<String, RowMap<'a>>)> {
    let mut in_scope: RowMap = BTreeMap::new();
    let mut excluded: BTreeMap<String, RowMap> = excluded_roles
        .iter()
        .map(|k| (k.clone(), BTreeMap::new()))
        .collect();
    for &v in pop {
        if v.base_type == "Contra" {
            continue;
        }
        let cash_paid_out = v
            .lines
            .iter()
            .any(|l| cash.contains(&l.ledger) && l.amount_paise < 0);
        if !cash_paid_out {
            continue; // no cash line credited (cash paid out) on this voucher
        }
        for l in &v.lines {
            if cash.contains(&l.ledger) || bank.contains(&l.ledger) || l.amount_paise <= 0 {
                continue; // only non-cash, non-bank DEBIT lines are payee candidates
            }
            let kind = classify_kind(book.ledgers.get(&l.ledger));
            let key = (v.date.clone(), l.ledger.clone());
            let bucket: &mut RowMap = match excluded.get_mut(kind) {
                Some(bucket) => bucket,
                None => &mut in_scope,
            };
            bucket
                .entry(key)
                .or_insert_with(RowAgg::new)
                .add(l.amount_paise, v)?;
        }
    }
    Ok((in_scope, excluded))
}

/// party_rows: (date, party ledger name) -> aggregate, for cash RECEIVED from a party.
fn compute_269st_rows<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    round_off_ledgers: &BTreeSet<String>,
) -> Result<RowMap<'a>> {
    let mut party_rows: RowMap = BTreeMap::new();
    for &v in pop {
        if v.base_type == "Contra" {
            continue;
        }
        let cash_received = v
            .lines
            .iter()
            .any(|l| cash.contains(&l.ledger) && l.amount_paise > 0);
        if !cash_received {
            continue; // no cash line debited (cash received) on this voucher
        }
        let mut party_amounts: BTreeMap<String, i64> = BTreeMap::new();
        let mut fallback_total: i64 = 0;
        for l in &v.lines {
            if cash.contains(&l.ledger) || bank.contains(&l.ledger) || l.amount_paise >= 0 {
                continue; // only non-cash, non-bank CREDIT lines are party candidates
            }
            let amt = -l.amount_paise;
            let ledger = book.ledgers.get(&l.ledger);
            let is_sales = ledger.is_some_and(|l2| l2.under(SALES_ACCOUNTS_GROUP));
            let is_tax_or_roundoff = ledger.is_some_and(|l2| l2.under(DUTIES_TAXES_GROUP))
                || round_off_ledgers.contains(&l.ledger);
            fallback_total = fallback_total.checked_add(amt).ok_or_else(overflow)?;
            if is_sales || is_tax_or_roundoff {
                continue; // never itself a party; its amount stays in fallback_total only
            }
            let entry = party_amounts.entry(l.ledger.clone()).or_insert(0);
            *entry = entry.checked_add(amt).ok_or_else(overflow)?;
        }
        if party_amounts.len() == 1 {
            let only = party_amounts.keys().next().unwrap().clone();
            party_amounts.insert(only, fallback_total);
        }
        if !party_amounts.is_empty() {
            for (ledger_name, amt) in party_amounts {
                let key = (v.date.clone(), ledger_name);
                party_rows
                    .entry(key)
                    .or_insert_with(RowAgg::new)
                    .add(amt, v)?;
            }
        } else if fallback_total > 0 {
            let key = (v.date.clone(), UNIDENTIFIED_PARTY.to_string());
            party_rows
                .entry(key)
                .or_insert_with(RowAgg::new)
                .add(fallback_total, v)?;
        }
    }
    Ok(party_rows)
}

/// Mirrors `compute_269st_rows` for the PAYMENT leg: cash paid BY the assessee to a party.
fn compute_269st_payment_rows<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    round_off_ledgers: &BTreeSet<String>,
) -> Result<RowMap<'a>> {
    let mut party_rows: RowMap = BTreeMap::new();
    for &v in pop {
        if v.base_type == "Contra" {
            continue;
        }
        let cash_paid_out = v
            .lines
            .iter()
            .any(|l| cash.contains(&l.ledger) && l.amount_paise < 0);
        if !cash_paid_out {
            continue;
        }
        let mut party_amounts: BTreeMap<String, i64> = BTreeMap::new();
        let mut fallback_total: i64 = 0;
        for l in &v.lines {
            if cash.contains(&l.ledger) || bank.contains(&l.ledger) || l.amount_paise <= 0 {
                continue;
            }
            let amt = l.amount_paise;
            let ledger = book.ledgers.get(&l.ledger);
            let is_purchase = ledger.is_some_and(|l2| l2.under(PURCHASE_ACCOUNTS_GROUP));
            let is_tax_or_roundoff = ledger.is_some_and(|l2| l2.under(DUTIES_TAXES_GROUP))
                || round_off_ledgers.contains(&l.ledger);
            fallback_total = fallback_total.checked_add(amt).ok_or_else(overflow)?;
            if is_purchase || is_tax_or_roundoff {
                continue;
            }
            let entry = party_amounts.entry(l.ledger.clone()).or_insert(0);
            *entry = entry.checked_add(amt).ok_or_else(overflow)?;
        }
        if party_amounts.len() == 1 {
            let only = party_amounts.keys().next().unwrap().clone();
            party_amounts.insert(only, fallback_total);
        }
        if !party_amounts.is_empty() {
            for (ledger_name, amt) in party_amounts {
                let key = (v.date.clone(), ledger_name);
                party_rows
                    .entry(key)
                    .or_insert_with(RowAgg::new)
                    .add(amt, v)?;
            }
        } else if fallback_total > 0 {
            let key = (v.date.clone(), UNIDENTIFIED_PARTY.to_string());
            party_rows
                .entry(key)
                .or_insert_with(RowAgg::new)
                .add(fallback_total, v)?;
        }
    }
    Ok(party_rows)
}

struct LoanCandidate<'a> {
    voucher: &'a Voucher,
    ledger: String,
    amount_paise: i64,
    direction: &'static str,
}

fn compute_269ss_269t_candidates<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cash: &BTreeSet<String>,
    limit_ss_t: i64,
) -> Result<Vec<LoanCandidate<'a>>> {
    let mut out = Vec::new();
    for &v in pop {
        if v.base_type == "Contra" {
            continue;
        }
        let mut cash_net: i64 = 0;
        let mut has_cash_line = false;
        for l in &v.lines {
            if cash.contains(&l.ledger) {
                has_cash_line = true;
                cash_net = cash_net.checked_add(l.amount_paise).ok_or_else(overflow)?;
            }
        }
        if !has_cash_line || cash_net == 0 {
            continue;
        }
        // cash Dr (positive) = received = loan accepted (269SS); cash Cr = repaid (269T).
        let direction = if cash_net > 0 { "accepted" } else { "repaid" };
        for l in &v.lines {
            let Some(ledger) = book.ledgers.get(&l.ledger) else {
                continue;
            };
            if !ledger.under(LOANS_LIABILITY_GROUP) {
                continue;
            }
            if l.amount_paise.abs() < limit_ss_t {
                continue;
            }
            out.push(LoanCandidate {
                voucher: v,
                ledger: l.ledger.clone(),
                amount_paise: l.amount_paise.abs(),
                direction,
            });
        }
    }
    Ok(out)
}

pub fn run(
    book: &Book,
    rules: &Rules,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    loan_ledgers_configured: &BTreeSet<String>,
    round_off_ledgers: &BTreeSet<String>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let pop = book.population()?;
    r.population_note = POPULATION.to_string();

    let limit_40a3 = rules.s40a3_limit_per_person_per_day_paise;
    let goods_limit = rules.s40a3_goods_carriage_limit_paise;
    let mut excluded_roles = rules.s40a3_excluded_group_roles.clone();
    excluded_roles.sort();

    let limit_269st = rules.s269st_limit_per_person_per_day_paise;
    let limit_ss_t = rules.s269ss_269t_limit_paise;

    // ---------------------------------------------------------------- (a) s.40A(3)
    let (in_scope_rows, excluded_rows) =
        compute_40a3_rows(&pop, book, cash, bank, &excluded_roles)?;
    let in_scope_count = in_scope_rows.len();
    let in_scope_total: i64 = in_scope_rows
        .values()
        .try_fold(0i64, |acc, d| acc.checked_add(d.paise).ok_or_else(overflow))?;
    let rows_over_limit: Vec<(&(TallyDate, String), &RowAgg)> = in_scope_rows
        .iter()
        .filter(|(_, d)| d.paise > limit_40a3)
        .collect();

    let in_scope_ev = evidence_for_rows(&in_scope_rows);
    r.fig(
        "s40a3_payee_days_any_amount_count",
        Value::Int(in_scope_count as i64),
        Unit::Count,
        "Distinct (date, payee ledger) pairs with a cash payment on a population, non-Contra \
voucher, payee classified as expenditure (not excluded by rules.s40a3.excluded_group_roles).",
        Vec::new(),
    );
    r.fig(
        "s40a3_payee_days_any_amount_total",
        Value::Int(in_scope_total),
        Unit::Paise,
        "Sum of cash paid across all in-scope (date, payee) pairs above, any amount.",
        in_scope_ev,
    );
    r.fig(
        "s40a3_over_limit_in_scope_count",
        Value::Int(rows_over_limit.len() as i64),
        Unit::Count,
        &format!(
            "In-scope (date, payee) pairs where the day's total exceeds the s.40A(3) limit \
({limit_40a3} paise per person per day)."
        ),
        Vec::new(),
    );

    let mut over_all_kinds = rows_over_limit.len();
    for kind in &excluded_roles {
        over_all_kinds += excluded_rows[kind]
            .values()
            .filter(|d| d.paise > limit_40a3)
            .count();
    }
    r.fig(
        "s40a3_over_limit_all_kinds_count",
        Value::Int(over_all_kinds as i64),
        Unit::Count,
        "(date, payee) pairs over the s.40A(3) limit before excluding capital, loans, fixed \
assets and duties and taxes; the in-scope count is a subset.",
        Vec::new(),
    );

    for kind in &excluded_roles {
        let rows = &excluded_rows[kind];
        let total: i64 = rows
            .values()
            .try_fold(0i64, |acc, d| acc.checked_add(d.paise).ok_or_else(overflow))?;
        let ev = evidence_for_rows(rows);
        let group = group_for_kind(kind)?;
        r.fig(
            &format!("s40a3_excluded_total_{kind}"),
            Value::Int(total),
            Unit::Paise,
            &format!(
                "Cash paid to ledgers under Tally group '{group}' (excluded from s.40A(3) \
scope by rules.s40a3.excluded_group_roles); {} (date, payee) pairs.",
                rows.len()
            ),
            ev,
        );
    }

    // One Finding per in-scope payee-day that is actually over the limit.
    for ((d, ledger_name), data) in rows_over_limit {
        let h = stable_ledger_tag(book, ledger_name)?;
        let rid = format!("{}_{h}", iso(d));
        let f_amt = r.fig(
            &format!("s40a3_row_amount_{rid}"),
            Value::Int(data.paise),
            Unit::Paise,
            &format!(
                "Cash paid to one payee ledger (tag {h}) on {}, summed across every \
population voucher that day.",
                iso(d)
            ),
            evidence_for_vouchers(&data.vouchers),
        );
        let goods_flag = data.paise <= goods_limit && transport_name_match(ledger_name);
        let f_goods = r.fig(
            &format!("s40a3_goods_carriage_candidate_{rid}"),
            Value::Text(if goods_flag { "yes" } else { "no" }.to_string()),
            Unit::Text,
            "Heuristic only: ledger name matches a generic transport-name pattern and the \
day's total is within the \u{20b9}35,000 goods-carriage limit (proviso to s.40A(3)).",
            Vec::new(),
        );
        let mut limits = vec![
            "Books only: Rule 6DD exceptions (e.g. bank/cooperative-bank closure, payments to \
government, payments where banking facilities are not available) are not visible from \
vouchers; ask the client whether any exception applies to this payment."
                .to_string(),
            "A single cash entry on this ledger and day may be a lump/year-end total covering \
several smaller payments to different people or on different days that the books do not \
itemise separately; confirm before treating it as one payee-day breach."
                .to_string(),
        ];
        if goods_flag {
            limits.push(
                "goods_carriage_candidate is a heuristic regex match on the ledger name only \
(freight/transport/carrier/logistics keywords), not a finding of fact that the payee operates \
a goods carriage; confirm with the client before relying on the higher \u{20b9}35,000 limit."
                    .to_string(),
            );
        }
        let mut evidence = evidence_for_vouchers(&data.vouchers);
        evidence.push(EvidenceRef::new("ledger", ledger_name));
        r.findings.push(Finding {
            id: format!("{TEST_ID}/s40a3/{rid}"),
            clauses: vec!["s.40A(3)".to_string(), "3CD-21(d)".to_string()],
            title: format!(
                "Cash paid to one payee over the s.40A(3) daily limit on {}",
                iso(d)
            ),
            facts: vec![
                ("amount".to_string(), f_amt),
                ("goods_carriage_candidate".to_string(), f_goods),
            ],
            evidence,
            confidence: Confidence::NeedsDocument,
            limits,
            ask_client: vec![
                "Confirm whether any Rule 6DD exception applies to this payment.".to_string(),
                "Confirm whether this ledger/day is a single payee or a lump entry covering \
several payments."
                    .to_string(),
            ],
        });
    }

    // ---------------------------------------------------------------- (b) s.269ST limb (i)
    let party_rows = compute_269st_rows(&pop, book, cash, bank, round_off_ledgers)?;
    let party_total: i64 = party_rows
        .values()
        .try_fold(0i64, |acc, d| acc.checked_add(d.paise).ok_or_else(overflow))?;
    let rows_269st: Vec<(&(TallyDate, String), &RowAgg)> = party_rows
        .iter()
        .filter(|(_, d)| d.paise >= limit_269st)
        .collect();
    let party_ev = evidence_for_rows(&party_rows);
    r.fig(
        "s269st_party_days_any_amount_count",
        Value::Int(party_rows.len() as i64),
        Unit::Count,
        "Distinct (date, party ledger) pairs with a cash receipt on a population, non-Contra \
voucher, party ledger not under 'Sales Accounts'.",
        Vec::new(),
    );
    r.fig(
        "s269st_party_days_any_amount_total",
        Value::Int(party_total),
        Unit::Paise,
        "Sum of cash received across all (date, party) pairs above, any amount.",
        party_ev,
    );
    r.fig(
        "s269st_at_or_over_limit_count",
        Value::Int(rows_269st.len() as i64),
        Unit::Count,
        &format!(
            "(date, party) pairs where the day's total is at or over the s.269ST(a) limb (i) \
limit ({limit_269st} paise per person per day)."
        ),
        Vec::new(),
    );

    for ((d, ledger_name), data) in rows_269st {
        let h = stable_ledger_tag(book, ledger_name)?;
        let rid = format!("{}_{h}", iso(d));
        let f_amt = r.fig(
            &format!("s269st_row_amount_{rid}"),
            Value::Int(data.paise),
            Unit::Paise,
            &format!(
                "Cash received from one party ledger (tag {h}) on {}, summed across every \
population voucher that day.",
                iso(d)
            ),
            evidence_for_vouchers(&data.vouchers),
        );
        let mut evidence = evidence_for_vouchers(&data.vouchers);
        evidence.push(EvidenceRef::new("ledger", ledger_name));
        r.findings.push(Finding {
            id: format!("{TEST_ID}/s269st/{rid}"),
            clauses: vec!["s.269ST(a)".to_string()],
            title: format!(
                "Cash received from one party at or over the s.269ST(a) limb (i) limit on {}",
                iso(d)
            ),
            facts: vec![("amount".to_string(), f_amt)],
            evidence,
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "Books test covers limb (i) (aggregate per person per day) only. Limbs (ii) (a \
single transaction) and (iii) (receipts relating to one event or occasion from a person) need \
bill-wise/event linkage the books do not carry; a same-party pattern across several days that \
never reaches this limit on one day is not tested here."
                    .to_string(),
                "No Clause 31 tag here (2026-09-17 double-count fix, gap register/orchestrator \
review): this is the same (party, date) cash-mode receipt high_value_register.py's own \
party-day scan already tags 3CD-31(ba) for every party, so this finding is an observation \
only, never counted a second time in the Clause 31 filing-aid total."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm whether this receipt is genuinely from one party.".to_string(),
                "Confirm whether limbs (ii)/(iii) are separately breached (documents needed)."
                    .to_string(),
            ],
        });
    }

    // ---------------------------------------------------------------- (b2) s.269ST payment leg
    let party_pay_rows = compute_269st_payment_rows(&pop, book, cash, bank, round_off_ledgers)?;
    let pay_total: i64 = party_pay_rows
        .values()
        .try_fold(0i64, |acc, d| acc.checked_add(d.paise).ok_or_else(overflow))?;
    let rows_269st_pay: Vec<(&(TallyDate, String), &RowAgg)> = party_pay_rows
        .iter()
        .filter(|(_, d)| d.paise >= limit_269st)
        .collect();
    let pay_ev = evidence_for_rows(&party_pay_rows);
    r.fig(
        "s269st_payment_party_days_any_amount_count",
        Value::Int(party_pay_rows.len() as i64),
        Unit::Count,
        "Distinct (date, party ledger) pairs with a cash payment on a population, non-Contra \
voucher, party ledger not under 'Purchase Accounts'.",
        Vec::new(),
    );
    r.fig(
        "s269st_payment_party_days_any_amount_total",
        Value::Int(pay_total),
        Unit::Paise,
        "Sum of cash paid across all (date, party) pairs above, any amount.",
        pay_ev,
    );
    r.fig(
        "s269st_payment_at_or_over_limit_count",
        Value::Int(rows_269st_pay.len() as i64),
        Unit::Count,
        &format!(
            "(date, party) pairs where the day's cash payment total is at or over the \
s.269ST(a) limb (a) threshold ({limit_269st} paise per person per day) -- reportable in clause \
31(bc), not a contravention by the assessee (s.269ST binds the receiver, not the payer)."
        ),
        Vec::new(),
    );

    for ((d, ledger_name), data) in rows_269st_pay {
        let h = stable_ledger_tag(book, ledger_name)?;
        let rid = format!("{}_{h}", iso(d));
        let f_amt = r.fig(
            &format!("s269st_payment_row_amount_{rid}"),
            Value::Int(data.paise),
            Unit::Paise,
            &format!(
                "Cash paid to one party ledger (tag {h}) on {}, summed across every population \
voucher that day.",
                iso(d)
            ),
            evidence_for_vouchers(&data.vouchers),
        );
        let mut evidence = evidence_for_vouchers(&data.vouchers);
        evidence.push(EvidenceRef::new("ledger", ledger_name));
        r.findings.push(Finding {
            id: format!("{TEST_ID}/s269st_payment/{rid}"),
            clauses: vec!["s.269ST(a)".to_string()],
            title: format!(
                "Cash paid to one party at or over the s.269ST person-per-day threshold on {} \
-- reportable in clause 31(bc)",
                iso(d)
            ),
            facts: vec![("amount".to_string(), f_amt)],
            evidence,
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "s.269ST(a) is a duty on the receiver of the cash, not the payer: the section \
reads 'No person shall receive...'. A payment made by the assessee is not itself a \
books-testable exposure for the assessee under s.269ST; it is a Form 3CD clause 31(bc) \
reporting item only (limb (a): aggregate per person per day)."
                    .to_string(),
                "Books test covers limb (a) (aggregate per person per day) only. Limbs (b) (a \
single transaction) and (c) (receipts relating to one event or occasion) need bill-wise/event \
linkage the books do not carry."
                    .to_string(),
                "This payment is counted once for clause 31(bc), in the high-value transactions \
list; it is shown here for the s.40A(3) review only and is not counted a second time."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm whether this payment is genuinely to one party.".to_string(),
                "Confirm whether limbs (b)/(c) apply from the party's own records (documents \
needed)."
                    .to_string(),
            ],
        });
    }

    // ---------------------------------------------------------------- (c) s.269SS/269T
    let candidates = compute_269ss_269t_candidates(&pop, book, cash, limit_ss_t)?;
    let mut loan_total: i64 = 0;
    let mut uncovered_count: i64 = 0;
    for c in &candidates {
        let v = c.voucher;
        loan_total = loan_total
            .checked_add(c.amount_paise)
            .ok_or_else(overflow)?;
        let h = stable_ledger_tag(book, &c.ledger)?;
        let rid = format!("{}_{h}", hash12_sha256(&v.guid));
        let clause = if c.direction == "accepted" {
            "s.269SS"
        } else {
            "s.269T"
        };
        let covered = loan_ledgers_configured.contains(&c.ledger);
        let coverage_tag = if covered { "covered" } else { "uncovered" };
        let f_amt = r.fig(
            &format!("s269ss269t_amount_{coverage_tag}_{rid}"),
            Value::Int(c.amount_paise),
            Unit::Paise,
            &format!(
                "Loan-ledger line (tag {h}) on voucher {}, cash {} in the same voucher ({} by \
the client's [loans] configuration).",
                crate::support::guid_tail12(&v.guid),
                c.direction,
                coverage_tag
            ),
            vec![EvidenceRef::with_label(
                "voucher",
                &v.guid,
                &voucher_label(v),
            )],
        );
        let common_limit = "Books only: confirm the counterparty is not government, a banking \
company, a co-operative bank, or another person/case excepted by s.269SS/269T, and that no \
other exception in the Act applies."
            .to_string();
        let (clauses, limits) = if covered {
            (
                vec![clause.to_string()],
                vec![
                    common_limit,
                    "No Clause 31 tag here: this loan ledger is in the client's [loans] \
configuration, so the loan-ledger scan already tags the matching entry 3CD-31(a)/3CD-31(c) -- \
this finding is an observation only, never counted a second time in the Clause 31 filing-aid \
total."
                        .to_string(),
                ],
            )
        } else {
            uncovered_count += 1;
            let clause_31 = if c.direction == "accepted" {
                "3CD-31(a)"
            } else {
                "3CD-31(c)"
            };
            (
                vec![clause.to_string(), clause_31.to_string()],
                vec![
                    common_limit,
                    "This loan ledger is NOT in the client's [loans] configuration \
(loan_ledgers_configured), so the loan-ledger scan never sees it -- this is the only place \
this entry is reported for Clause 31; add the ledger to that configuration to get the full \
lender-classification/running-balance test instead."
                        .to_string(),
                ],
            )
        };
        r.findings.push(Finding {
            id: format!("{TEST_ID}/s269ss269t/{rid}"),
            clauses,
            title: format!(
                "Cash {} against a loan ledger on {}, at or over the s.269SS/269T limit",
                c.direction,
                iso(&v.date)
            ),
            facts: vec![("amount".to_string(), f_amt)],
            evidence: vec![
                EvidenceRef::with_label("voucher", &v.guid, &voucher_label(v)),
                EvidenceRef::new("ledger", &c.ledger),
            ],
            confidence: Confidence::NeedsDocument,
            limits,
            ask_client: vec![
                "Confirm the lender/borrower's identity and relationship.".to_string(),
                "Confirm no s.269SS/269T exception applies.".to_string(),
            ],
        });
    }

    r.fig(
        "s269ss269t_candidate_uncovered_by_loans_interest_count",
        Value::Int(uncovered_count),
        Unit::Count,
        "Of the s.269SS/269T candidates above, how many are on a loan ledger NOT in the \
client's [loans] configuration -- these are the only ones this module tags for Clause 31; \
every other candidate is covered by the loan-ledger scan instead (2026-09-17 double-count \
fix).",
        Vec::new(),
    );
    r.fig(
        "s269ss269t_candidate_count",
        Value::Int(candidates.len() as i64),
        Unit::Count,
        &format!(
            "Population, non-Contra vouchers with a cash line and a Loans (Liability)-chain \
line of abs amount >= the s.269SS/269T limit ({limit_ss_t} paise)."
        ),
        Vec::new(),
    );
    r.fig(
        "s269ss269t_candidate_total",
        Value::Int(loan_total),
        Unit::Paise,
        "Sum of abs(loan-line amount) across all s.269SS/269T candidates above.",
        Vec::new(),
    );

    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pattern searched here is the reference's own, byte for byte (the probe file records it
    /// from the reference module).
    #[test]
    fn the_pattern_is_the_reference_s() {
        let v = crate::support::text_probe_tests::probes();
        assert_eq!(
            v["header"]["reference_regexes"]["transport"]
                .as_str()
                .unwrap(),
            TRANSPORT_NAME_RE
        );
    }

    /// The reference's transport-name regex is case-insensitive (`re.I`).
    #[test]
    fn the_transport_name_check_ignores_case() {
        for name in [
            "ABC LOGISTICS",
            "abc logistics",
            "Sharma Road Lines",
            "sharma roadlines",
            "Cargo Co",
        ] {
            assert!(transport_name_match(name), "{name}");
        }
        assert!(!transport_name_match("Sharma Traders"));
    }

    /// The s.269SS/269T figure definition names the voucher by `guid[-12:]` too, in the reference.
    #[test]
    fn a_loan_line_definition_names_the_voucher_by_the_last_12_characters_of_its_guid() {
        use crate::book::{LedgerLine, VoucherStatus};
        let ledger = |name: &str, chain: &[&str]| crate::book::Ledger {
            name: name.to_string(),
            parent: chain[0].to_string(),
            chain: chain.iter().map(|g| (*g).to_string()).collect(),
            chain_complete: true,
            master_opening_paise: 0,
            guid: format!("invented-{name}"),
            masterid: None,
        };
        let book = Book {
            company_name: "Invented".to_string(),
            company_guid: "invented-company".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: [
                ledger("Cash", &["Cash-in-Hand"]),
                ledger("Lender Loan", &["Loans (Liability)"]),
            ]
            .into_iter()
            .map(|l| (l.name.clone(), l))
            .collect(),
            vouchers: vec![Voucher {
                guid: "invented-guid-ééééééa".to_string(),
                date: TallyDate::parse("20250601").unwrap(),
                vtype: "Receipt".to_string(),
                base_type: "Receipt".to_string(),
                number: "R/1".to_string(),
                status: VoucherStatus::Regular,
                lines: vec![
                    LedgerLine {
                        ledger: "Cash".to_string(),
                        amount_paise: 3_000_000,
                    },
                    LedgerLine {
                        ledger: "Lender Loan".to_string(),
                        amount_paise: -3_000_000,
                    },
                ],
                narration: String::new(),
            }],
            tb: BTreeMap::new(),
        };
        let cash: BTreeSet<String> = ["Cash".to_string()].into_iter().collect();
        let r = run(
            &book,
            &Rules::vendored().unwrap(),
            &cash,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
        .unwrap();
        let def = r
            .figures
            .iter()
            .find(|f| f.id.contains("s269ss269t_amount_uncovered_"))
            .map(|f| f.definition.clone())
            .unwrap();
        assert!(
            def.contains("on voucher guid-ééééééa, cash accepted"),
            "{def}"
        );
    }

    /// The reference labels a voucher with no number by `guid[-12:]`: 12 characters. Here the
    /// 12-byte cut would land inside an 'é' (a panic when byte-sliced); the expected tail is the
    /// reference's own `"invented-guid-ééééééa"[-12:]`.
    #[test]
    fn a_voucher_without_a_number_is_labelled_by_the_last_12_characters_of_its_guid() {
        let v = Voucher {
            guid: "invented-guid-ééééééa".to_string(),
            date: TallyDate::parse("20250601").unwrap(),
            vtype: "Payment".to_string(),
            base_type: "Payment".to_string(),
            number: String::new(),
            status: crate::book::VoucherStatus::Regular,
            lines: Vec::new(),
            narration: String::new(),
        };
        assert_eq!(voucher_label(&v), "Payment guid-ééééééa on 2025-06-01");
    }
}
