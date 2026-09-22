// SPDX-License-Identifier: Apache-2.0
//! The reference implementation's `tds_payees` test: payees credited under a s.194C
//! (contractors), s.194-I (rent) or s.194J (professional/technical fees, royalty, s.28(va)) mapped
//! expense ledger, grouped by payee ENTITY and tested against each section's limits, plus the
//! assessee's deductor status. A line-for-line port; the reference module's docstring is the
//! design record, summarised here.
//!
//! * The client config maps expense ledgers to a nature (`nature_by_ledger`), merges payee ledgers
//!   into entities (`payee_aliases`), and, for 194J only, maps a ledger to its category
//!   (`s194j_category_by_ledger`). No ledger name is interpreted here.
//! * For each books-population, non-Contra voucher with a line on a mapped expense ledger, the
//!   payees are its CREDIT lines on ledgers that are neither mapped expense ledgers nor under
//!   `Duties & Taxes`, except that a voucher also carrying a `Purchase Accounts`/`Sales Accounts`
//!   line puts the mapped line's OWN amount into the goods-invoice bucket and inspects no credit
//!   line, and a credit on a cash or bank ledger goes to the payee-not-named bucket.
//! * 194C trips on the largest total credited within one voucher or on the year's aggregate;
//!   194-I on any calendar month; 194J on each category's aggregate separately. A 194J ledger with
//!   no known category is never threshold-tested: every nonzero payee is a judgement finding.
//! * An individual/HUF is a deductor only on a supplied previous-year turnover over the limit;
//!   without one, the status is `unknown` and a judgement finding says so. Never assumed.
//!
//! Divergences from the reference:
//! * a `[tds].nature_by_ledger` or `[tds].payee_aliases` value that is not a string, a
//!   `[tds].previous_year_turnover_paise` that is not an integer, and a `[tds_payees]` or
//!   `[tds_payees].s194j_category_by_ledger` that is not a table are refused when the engagement is
//!   read ([`crate::TdsConfig`]), which refuses every test on that engagement. The reference takes
//!   a truthy non-string nature as an unknown one and a falsy one (`false`, `0`) as no mapping,
//!   compares a float turnover, raises on a non-string alias
//!   only once that payee is over a limit, and raises on a non-table `[tds_payees]` in this test;
//! * `[tds_payees]` without `[tds]` is neither read nor bound here, where the reference binds its
//!   keys for every test: an unknown ledger named only there refuses the reference's whole pack and
//!   none of this port's tests;
//! * a missing `[client].entity_type` is refused here; the reference's engagement always has one;
//! * two over-limit entities sharing a figure id are refused, as the reference raises;
//! * a total that overflows i64 paise is refused, where Python's integers are unbounded.

use std::collections::{BTreeMap, BTreeSet};

use sha1::{Digest, Sha1};

use crate::book::{Book, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::rules::Rules;
use crate::support::{count, overflow, py_lower, voucher_label};
use crate::TdsConfig;

pub const TEST_ID: &str = "tds_payees";
pub const VERSION: &str = "1";

// Tally's own reserved/standard group names (not client data), matched through the whole chain.
const CASH_GROUP: &str = "Cash-in-Hand";
const BANK_GROUPS: [&str; 2] = ["Bank Accounts", "Bank OD A/c"];
const DUTIES_TAXES_GROUP: &str = "Duties & Taxes";
const PURCHASE_ACCOUNTS_GROUP: &str = "Purchase Accounts";
const SALES_ACCOUNTS_GROUP: &str = "Sales Accounts";

pub const WITHIN_GOODS_INVOICE: &str = "(within supplier goods invoices)";
pub const PAYEE_NOT_NAMED: &str = "(payee not named)";

const NATURES: [&str; 3] = ["194C", "194I", "194J"];

/// s.194J first proviso, clause (B): four categories, each with its own limit.
const CATEGORIES_194J: [&str; 4] = ["professional", "technical", "royalty", "28va"];
/// A 194J-mapped ledger with no entry, or an unrecognised one, in `s194j_category_by_ledger`.
pub const CATEGORY_UNMAPPED: &str = "unmapped";

/// The reference's `DEFAULT_S194J["aggregate_paise"]`, used only when the rules carry no
/// `[s194j]` table.
const DEFAULT_S194J_AGGREGATE_PAISE: i64 = 5_000_000;

/// The reference's `_hash`: sha1 of the text, first 8 hex digits.
fn hash8(text: &str) -> String {
    crate::canonical::hex(&Sha1::digest(text.as_bytes()))[..8].to_string()
}

fn month_key(v: &Voucher) -> String {
    let s = v.date.as_str();
    format!("{}-{}", &s[0..4], &s[4..6])
}

fn under_any(book: &Book, ledger: &str, groups: &[&str]) -> bool {
    book.ledgers
        .get(ledger)
        .is_some_and(|l| groups.iter().any(|g| l.under(g)))
}

/// `(nature, subcat)`: subcat is empty for 194C/194I and, for 194J, the ledger's own category or
/// [`CATEGORY_UNMAPPED`]. Decided per LEDGER, so one voucher with two 194J ledgers of different
/// categories lands in two rows.
fn nature_key(nature: &str, ledger: &str, cfg: &TdsConfig) -> (String, String) {
    if nature != "194J" {
        return (nature.to_string(), String::new());
    }
    let cat = match cfg.s194j_category_by_ledger.get(ledger) {
        Some(Some(cat)) if CATEGORIES_194J.contains(&cat.as_str()) => cat.clone(),
        _ => CATEGORY_UNMAPPED.to_string(),
    };
    (nature.to_string(), cat)
}

/// One (nature key, entity) row: what each voucher credited, and the vouchers by GUID.
#[derive(Default)]
struct Row<'a> {
    by_voucher: BTreeMap<&'a str, i64>,
    vouchers: BTreeMap<&'a str, &'a Voucher>,
}

impl<'a> Row<'a> {
    fn add(&mut self, v: &'a Voucher, amount: i64) -> Result<()> {
        let slot = self.by_voucher.entry(v.guid.as_str()).or_insert(0);
        *slot = slot.checked_add(amount).ok_or_else(|| overflow(TEST_ID))?;
        self.vouchers.insert(v.guid.as_str(), v);
        Ok(())
    }
}

type RowKey = ((String, String), String);

/// The reference's `compute_payee_rows`.
fn compute_payee_rows<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cfg: &TdsConfig,
) -> Result<BTreeMap<RowKey, Row<'a>>> {
    let mut rows: BTreeMap<RowKey, Row<'a>> = BTreeMap::new();
    for &v in pop {
        if v.base_type == "Contra" {
            continue;
        }
        let mut expense_lines: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
        let mut expense_ledgers_here: BTreeSet<&str> = BTreeSet::new();
        for l in &v.lines {
            // The reference maps a line only on a truthy nature: an empty one maps nothing.
            if let Some(nature) = cfg
                .nature_by_ledger
                .get(&l.ledger)
                .filter(|n| !n.is_empty())
            {
                expense_lines
                    .entry(nature_key(nature, &l.ledger, cfg))
                    .or_default()
                    .push(l.amount_paise);
                expense_ledgers_here.insert(l.ledger.as_str());
            }
        }
        if expense_lines.is_empty() {
            continue;
        }
        let has_goods_line = v.lines.iter().any(|l| {
            under_any(
                book,
                &l.ledger,
                &[PURCHASE_ACCOUNTS_GROUP, SALES_ACCOUNTS_GROUP],
            )
        });
        if has_goods_line {
            // The charge itself (the mapped line), never the supplier's full invoice credit.
            for (key, amounts) in &expense_lines {
                let amount = amounts
                    .iter()
                    .try_fold(0_i64, |a, b| a.checked_add(*b))
                    .ok_or_else(|| overflow(TEST_ID))?;
                rows.entry((key.clone(), WITHIN_GOODS_INVOICE.to_string()))
                    .or_default()
                    .add(v, amount)?;
            }
            continue;
        }
        let credit_lines: Vec<_> = v
            .lines
            .iter()
            .filter(|l| {
                l.amount_paise < 0
                    && !expense_ledgers_here.contains(l.ledger.as_str())
                    && !under_any(book, &l.ledger, &[DUTIES_TAXES_GROUP])
            })
            .collect();
        if credit_lines.is_empty() {
            continue;
        }
        // A voucher whose mapped lines span more than one nature key attributes the full credit
        // total to EACH one present.
        for key in expense_lines.keys() {
            for l in &credit_lines {
                let amount = l
                    .amount_paise
                    .checked_neg()
                    .ok_or_else(|| overflow(TEST_ID))?;
                let entity = if under_any(
                    book,
                    &l.ledger,
                    &[CASH_GROUP, BANK_GROUPS[0], BANK_GROUPS[1]],
                ) {
                    PAYEE_NOT_NAMED.to_string()
                } else {
                    cfg.payee_aliases
                        .get(&l.ledger)
                        .cloned()
                        .unwrap_or_else(|| l.ledger.clone())
                };
                rows.entry((key.clone(), entity))
                    .or_default()
                    .add(v, amount)?;
            }
        }
    }
    Ok(rows)
}

/// The reference's `summarise_row`.
struct Summary<'a> {
    credited: i64,
    max_single: i64,
    months: BTreeMap<String, i64>,
    vouchers: &'a BTreeMap<&'a str, &'a Voucher>,
}

fn summarise<'a>(row: &'a Row<'a>) -> Result<Summary<'a>> {
    let credited = row
        .by_voucher
        .values()
        .try_fold(0_i64, |a, b| a.checked_add(*b))
        .ok_or_else(|| overflow(TEST_ID))?;
    let max_single = row.by_voucher.values().copied().max().unwrap_or(0);
    let mut months: BTreeMap<String, i64> = BTreeMap::new();
    for (guid, amount) in &row.by_voucher {
        let slot = months.entry(month_key(row.vouchers[guid])).or_insert(0);
        *slot = slot.checked_add(*amount).ok_or_else(|| overflow(TEST_ID))?;
    }
    Ok(Summary {
        credited,
        max_single,
        months,
        vouchers: &row.vouchers,
    })
}

/// The reference's `deductor_status`: "deductor" | "not_deductor" | "unknown". An individual/HUF
/// is never assumed either way without a supplied previous-year turnover. The entity types match
/// exactly, as the reference compares them.
pub(crate) fn deductor_status(
    entity_type: &str,
    threshold: i64,
    turnover: Option<i64>,
) -> &'static str {
    if entity_type == "individual" || entity_type == "huf" {
        return match turnover {
            None => "unknown",
            Some(t) if t > threshold => "deductor",
            Some(_) => "not_deductor",
        };
    }
    "deductor"
}

/// Python's `format(x, "g")`: six significant digits, trailing zeros dropped, scientific notation
/// below 1e-4 or from 1e6. The reference formats the deductor turnover limit in crore this way.
pub(crate) fn py_format_g(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    let sci = format!("{x:.5e}");
    let (mantissa, exponent) = sci.split_once('e').expect("{:e} always has an exponent");
    let exponent: i32 = exponent.parse().expect("an integer exponent");
    let strip = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if (-4..6).contains(&exponent) {
        let decimals = usize::try_from(5 - exponent).expect("0..=9 decimals");
        strip(&format!("{x:.decimals$}"))
    } else {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), exponent.abs())
    }
}

/// Run the test. `entity_type` is `[client].entity_type`.
pub fn run(book: &Book, rules: &Rules, entity_type: &str, cfg: &TdsConfig) -> Result<TestResult> {
    let missing = |table: &str| AuditError::Config(format!("{TEST_ID} needs rules [{table}]"));
    let s194c = rules.s194c.ok_or_else(|| missing("s194c"))?;
    let per_month_limit = rules
        .s194i_per_month_per_payee_paise
        .ok_or_else(|| missing("s194i"))?;
    let deductor_threshold = rules
        .deductor_individual_huf_prev_year_turnover_paise
        .ok_or_else(|| missing("deductor"))?;
    let s194j_is_default = rules.s194j_aggregate_paise.is_none();
    let s194j_limit = rules
        .s194j_aggregate_paise
        .unwrap_or(DEFAULT_S194J_AGGREGATE_PAISE);

    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let pop = book.population()?;
    r.population_note = "Books population (optional, cancelled and post-dated vouchers \
excluded); Contra excluded throughout. A voucher whose mapped expense lines span more than one \
TDS nature (rare) attributes its full credit-line total to each nature present, not divided or \
netted."
        .to_string();

    // ---------------------------------------------------------------- deductor status
    let status = deductor_status(
        entity_type,
        deductor_threshold,
        cfg.previous_year_turnover_paise,
    );
    let f_status = r.fig(
        "deductor_status",
        Value::Text(status.to_string()),
        Unit::Text,
        "Whether the assessee must deduct TDS under s.194A/194C/194-I/194J for the year: firm/LLP/\
company always (rules.deductor.firm); individual/HUF only if previous-year business turnover \
exceeded rules.deductor.individual_huf_prev_year_turnover_paise -- never assumed from the \
current year's books alone.",
        Vec::new(),
    );
    if status == "unknown" {
        // i64 -> f64 is exact below 2^53 and the division correctly rounded, as Python's is.
        #[allow(clippy::cast_precision_loss)]
        let crore = deductor_threshold as f64 / 1_000_000_000_f64;
        r.findings.push(Finding {
            id: format!("{TEST_ID}/deductor_status"),
            clauses: vec!["3CD-21(b)".to_string(), "3CD-34(a)".to_string()],
            title: "Deductor status depends on previous-year turnover".to_string(),
            facts: vec![("deductor_status".to_string(), f_status)],
            evidence: Vec::new(),
            confidence: Confidence::JudgementRequired,
            limits: vec![format!(
                "An individual/HUF is a s.194A/194C/194-I/194J deductor only if the immediately \
preceding year's business turnover exceeded ₹{} crore; the current year's books alone cannot \
establish this.",
                py_format_g(crore)
            )],
            ask_client: vec!["Confirm previous-year business turnover.".to_string()],
        });
    }

    // ---------------------------------------------------------------- firm-level: TDS ledger figure
    let tds_ledgers: Vec<&String> = book
        .ledgers
        .iter()
        .filter(|(name, l)| l.under(DUTIES_TAXES_GROUP) && py_lower(name).contains("tds"))
        .map(|(name, _)| name)
        .collect();
    r.fig(
        "tds_ledger_under_duties_taxes_count",
        count(TEST_ID, tds_ledgers.len())?,
        Unit::Count,
        "Ledgers under 'Duties & Taxes' whose name contains 'tds' (case-insensitive) -- a books \
figure only; existence or absence of such a ledger is not itself a conclusion about TDS compliance.",
        tds_ledgers
            .iter()
            .map(|n| EvidenceRef::with_label("ledger", n, n))
            .collect(),
    );

    // ---------------------------------------------------------------- payee rows
    let rows = compute_payee_rows(&pop, book, cfg)?;
    let clauses_by_nature = |nature: &str| -> Vec<String> {
        let section = match nature {
            "194C" => "s.194C(5)",
            "194I" => "s.194-I",
            _ => "s.194J",
        };
        vec![
            section.to_string(),
            "3CD-21(b)".to_string(),
            "3CD-34(a)".to_string(),
        ]
    };

    // (nature, subcat, prefix): one row per 194J category plus unmapped, each tested separately.
    let mut nature_subcats: Vec<(&str, &str, String)> = Vec::new();
    for nature in NATURES {
        if nature == "194J" {
            for cat in CATEGORIES_194J.iter().copied().chain([CATEGORY_UNMAPPED]) {
                nature_subcats.push((nature, cat, format!("194J_{cat}")));
            }
        } else {
            nature_subcats.push((nature, "", nature.to_string()));
        }
    }

    for (nature, subcat, prefix) in &nature_subcats {
        let (nature, subcat) = (*nature, *subcat);
        let mut summaries: BTreeMap<&str, Summary> = BTreeMap::new();
        for (((row_nature, row_subcat), entity), row) in &rows {
            if row_nature == nature && row_subcat == subcat {
                summaries.insert(entity.as_str(), summarise(row)?);
            }
        }

        let goods_total = summaries
            .get(WITHIN_GOODS_INVOICE)
            .map_or(0, |s| s.credited);
        let cat_note = if subcat.is_empty() {
            String::new()
        } else {
            format!(", category '{subcat}'")
        };
        r.fig(
            &format!("{prefix}_within_supplier_goods_invoices_total"),
            Value::Int(goods_total),
            Unit::Paise,
            &format!(
                "Sum of the {nature}-mapped expense line(s) themselves (the charge -- e.g. freight \
-- not the supplier's full invoice credit) on vouchers that also carry a Purchase Accounts/Sales \
Accounts line{cat_note}: inside a goods invoice, not a separate contract with the payee; excluded \
from the per-payee tests below."
            ),
            Vec::new(),
        );

        let payee_entities: BTreeMap<&str, &Summary> = summaries
            .iter()
            .filter(|(e, _)| **e != WITHIN_GOODS_INVOICE)
            .map(|(e, s)| (*e, s))
            .collect();
        let credited_total = payee_entities
            .values()
            .try_fold(0_i64, |a, s| a.checked_add(s.credited))
            .ok_or_else(|| overflow(TEST_ID))?;
        r.fig(
            &format!("{prefix}_payee_entities_count"),
            count(TEST_ID, payee_entities.len())?,
            Unit::Count,
            &format!(
                "Distinct payee entities (payee_aliases-merged; 'payee not named' counted as one \
entity, 'within supplier goods invoices' excluded) credited on a voucher with a {nature}-mapped \
expense ledger line{cat_note}."
            ),
            Vec::new(),
        );
        r.fig(
            &format!("{prefix}_credited_total"),
            Value::Int(credited_total),
            Unit::Paise,
            &format!(
                "Sum credited to all {nature}{cat_note} payee entities above (excludes the \
goods-invoice bucket). Never summed with any other category's total before a threshold test."
            ),
            Vec::new(),
        );

        let is_unmapped_194j = nature == "194J" && subcat == CATEGORY_UNMAPPED;
        let over: BTreeMap<&str, &Summary> = payee_entities
            .iter()
            .filter(|(_, s)| {
                if is_unmapped_194j {
                    s.credited > 0
                } else if nature == "194C" {
                    s.credited > s194c.aggregate_paise || s.max_single > s194c.single_sum_paise
                } else if nature == "194I" {
                    s.months
                        .values()
                        .max()
                        .is_some_and(|m| *m > per_month_limit)
                } else {
                    s.credited > s194j_limit
                }
            })
            .map(|(e, s)| (*e, *s))
            .collect();
        r.fig(
            &format!("{prefix}_over_limit_payee_count"),
            count(TEST_ID, over.len())?,
            Unit::Count,
            &format!(
                "Payee entities above whose {nature}{cat_note} test trips (single sum/aggregate for \
194C, any month for 194-I, per-category aggregate for 194J; every nonzero-credit payee for category \
'unmapped', which is never threshold-tested)."
            ),
            Vec::new(),
        );

        let mut ordered: Vec<(&str, &Summary)> = over.into_iter().collect();
        ordered.sort_by_key(|(e, _)| hash8(e));
        for (entity, s) in ordered {
            let h = hash8(&format!("{prefix}:{}", stable_ledger_tag(book, entity)?));
            let rid = format!("{prefix}_{h}");
            let mut evidence: Vec<EvidenceRef> = s
                .vouchers
                .iter()
                .map(|(g, v)| EvidenceRef::with_label("voucher", g, &voucher_label(v)))
                .collect();
            evidence.sort_by(|a, b| a.id.cmp(&b.id));
            evidence.dedup();
            // Two over-limit entities with one tag (an alias spelled as another ledger's GUID, say)
            // would repeat a figure id: the reference raises there, so this refuses rather than
            // letting `TestResult::fig` panic.
            let row_figure = format!("{TEST_ID}.{prefix}_row_credited_{rid}");
            if r.figures.iter().any(|f| f.id == row_figure) {
                return Err(AuditError::Config(format!(
                    "{TEST_ID}: two payee entities share the figure id {row_figure}"
                )));
            }
            let f_credited = r.fig(
                &format!("{prefix}_row_credited_{rid}"),
                Value::Int(s.credited),
                Unit::Paise,
                &format!(
                    "Total credited to one payee entity (tag {h}) under {nature}{cat_note}, summed \
across every population voucher touching a mapped expense ledger."
                ),
                evidence.clone(),
            );
            let f_max_single = r.fig(
                &format!("{prefix}_row_max_single_{rid}"),
                Value::Int(s.max_single),
                Unit::Paise,
                &format!(
                    "Largest total credited to this payee entity (tag {h}) within one voucher, \
under {nature}{cat_note}."
                ),
                Vec::new(),
            );
            // An unmapped-category amount is never a computed TDS default: its fact key keeps it
            // out of the clause amount sums while the finding still counts toward the clauses.
            let mut facts = vec![
                (
                    if is_unmapped_194j {
                        "unmapped_category_credited"
                    } else {
                        "credited"
                    }
                    .to_string(),
                    f_credited,
                ),
                ("max_single".to_string(), f_max_single),
            ];
            if nature == "194I" {
                let max_month = s.months.values().copied().max().unwrap_or(0);
                let f_month = r.fig(
                    &format!("{prefix}_row_max_month_{rid}"),
                    Value::Int(max_month),
                    Unit::Paise,
                    &format!(
                        "Highest single calendar-month credit total to this payee entity (tag {h}) \
under 194-I."
                    ),
                    Vec::new(),
                );
                facts.push(("max_month".to_string(), f_month));
            }

            let (title, mut limits, ask_client): (String, Vec<String>, Vec<String>) =
                if is_unmapped_194j {
                    (
                        "Payee credited under a 194J-mapped ledger with no known s.194J category"
                            .to_string(),
                        vec![
                            "This ledger is treated as a s.194J payment, but its fee category \
(professional, technical, royalty, or s.28(va)) has not been recorded, so which of the first \
proviso's four separate ₹50,000 tests applies cannot be determined from the books alone."
                                .to_string(),
                            "This amount is NEVER summed with any known-category total for this \
payee before a threshold test -- doing so would silently re-create the over-flagging bug this \
correction fixes (gap register section 7)."
                                .to_string(),
                        ],
                        vec![
                            "Confirm which s.194J category (professional fees, technical fees, \
royalty, or s.28(va)) this ledger's payments belong to, then re-test against that category's own \
₹50,000 aggregate."
                                .to_string(),
                        ],
                    )
                } else if entity == PAYEE_NOT_NAMED {
                    (
                        format!(
                            "Cash/bank paid under a {nature}-mapped expense, payee not \
identified, over the limit"
                        ),
                        vec![
                            "The credit line is a cash or bank ledger, not a named payee; the \
actual payee must be identified from narration, vouchers or the bank statement before any TDS \
conclusion can be drawn for this amount."
                                .to_string(),
                            "s.40(a)(ia) disallowance and its Form 26A relief cannot be assessed \
until the payee is known."
                                .to_string(),
                        ],
                        vec![
                            "Identify the payee(s) behind this cash/bank credit from narration, \
vouchers or the bank statement, then re-test."
                                .to_string(),
                        ],
                    )
                } else {
                    (
                        format!("Payee over the {nature}{cat_note} limit"),
                        vec![
                            "Books only: the contract's nature, any lower/nil-deduction \
certificate (s.197) and, for 194C, a s.194C(6) declaration (own-account transporter, PAN \
furnished, ten or fewer goods carriages) are not visible from vouchers -- contract nature and any \
certificate/declaration are a CA judgement call, not a books fact."
                                .to_string(),
                            "s.40(a)(ia) disallows 30% of the sum on TDS default; the second \
proviso removes this if the payee's Form 26A (Rule 31ACB) shows the income was returned -- \
s.201(1A) interest still runs either way."
                                .to_string(),
                        ],
                        vec![
                            "Confirm the nature of the contract/service and whether any \
lower/nil-deduction certificate or s.194C(6) declaration applies."
                                .to_string(),
                            "If no TDS was deducted, confirm whether Form 26A is available for \
this payee."
                                .to_string(),
                        ],
                    )
                };
            if nature == "194J" && !is_unmapped_194j && s194j_is_default {
                limits.push(format!(
                    "The s.194J aggregate limit used here ({s194j_limit} paise) is a local \
prototype default (status=\"confirm\"), pending confirmation in rules/ay2026-27.toml -- not yet a \
verified rule."
                ));
            }

            r.findings.push(Finding {
                id: format!("{TEST_ID}/{rid}"),
                clauses: clauses_by_nature(nature),
                title,
                facts,
                evidence,
                confidence: if is_unmapped_194j {
                    Confidence::JudgementRequired
                } else {
                    Confidence::NeedsDocument
                },
                limits,
                ask_client,
            });
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python 3.13's `format(x, "g")`, case by case.
    #[test]
    fn py_format_g_matches_python() {
        for (x, want) in [
            (1.0, "1"),
            (0.5, "0.5"),
            (12.5, "12.5"),
            (100_000.0, "100000"),
            (1_000_000.0, "1e+06"),
            (1_234_567.0, "1.23457e+06"),
            (0.0001, "0.0001"),
            (0.000_01, "1e-05"),
            (2.5e-7, "2.5e-07"),
            (0.1 + 0.2, "0.3"),
            (123_456.5, "123456"),
            (99_999.95, "99999.9"),
            (0.0, "0"),
            (-1.5, "-1.5"),
        ] {
            assert_eq!(py_format_g(x), want, "{x}");
        }
    }

    /// An alias spelled as another over-limit payee's GUID gives both entities one tag, so one
    /// figure id twice: the reference raises, and this refuses instead of panicking.
    #[test]
    fn two_payees_sharing_a_figure_id_are_refused() {
        use crate::book::{Ledger, LedgerLine, VoucherStatus};
        use bridge_tally_primitives::TallyDate;
        const GUID_C: &str = "aaaaaaaa-0000-4000-8000-000000000001";
        let ledger = |name: &str, group: &str, guid: &str| Ledger {
            name: name.to_string(),
            parent: group.to_string(),
            chain: vec![group.to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: guid.to_string(),
            masterid: None,
        };
        let voucher = |guid: &str, payee: &str| Voucher {
            guid: guid.to_string(),
            date: TallyDate::parse("20250610".to_string()).unwrap(),
            vtype: "Journal".to_string(),
            base_type: "Journal".to_string(),
            number: guid.to_string(),
            status: VoucherStatus::Regular,
            lines: vec![
                LedgerLine {
                    ledger: "Freight".to_string(),
                    amount_paise: 20_000_000,
                },
                LedgerLine {
                    ledger: payee.to_string(),
                    amount_paise: -20_000_000,
                },
            ],
            narration: String::new(),
            ..Default::default()
        };
        let book = Book {
            company_name: "Invented".to_string(),
            company_guid: "invented".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: [
                ledger("Freight", "Direct Expenses", ""),
                ledger("Contractor C", "Sundry Creditors", GUID_C),
                ledger("Contractor E", "Sundry Creditors", ""),
            ]
            .into_iter()
            .map(|l| (l.name.clone(), l))
            .collect(),
            vouchers: vec![voucher("v1", "Contractor C"), voucher("v2", "Contractor E")],
            tb: BTreeMap::new(),
        };
        let mut cfg = TdsConfig::default();
        cfg.nature_by_ledger
            .insert("Freight".to_string(), "194C".to_string());
        cfg.payee_aliases
            .insert("Contractor E".to_string(), GUID_C.to_string());
        let rules = Rules::vendored().unwrap();
        let err = run(&book, &rules, "firm", &cfg).unwrap_err();
        assert!(
            format!("{err}").contains("two payee entities share the figure id"),
            "{err}"
        );
        // The control: without the colliding alias both payees are reported.
        cfg.payee_aliases.clear();
        let r = run(&book, &rules, "firm", &cfg).unwrap();
        assert_eq!(r.findings.len(), 2);
    }

    #[test]
    fn deductor_status_is_never_assumed_for_an_individual_or_huf() {
        for (entity, turnover, want) in [
            ("individual", None, "unknown"),
            ("huf", None, "unknown"),
            ("individual", Some(100), "not_deductor"),
            ("individual", Some(101), "deductor"),
            ("huf", Some(101), "deductor"),
            ("Individual", None, "deductor"),
            ("firm", None, "deductor"),
            ("company", Some(0), "deductor"),
        ] {
            assert_eq!(
                deductor_status(entity, 100, turnover),
                want,
                "{entity} {turnover:?}"
            );
        }
    }
}
