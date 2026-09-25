// SPDX-License-Identifier: Apache-2.0
//! The reference implementation's `tds_tcs_26as` test: Form 26AS Part I (TDS) and Part VI (TCS)
//! against the books' TDS/TCS claims, the TCS-on-asset booking fact, and AIS/TIS figures beside
//! the books. A line-for-line port; the reference module's docstring is the design record.
//!
//! * 26AS rows aggregate per (part, TAN, section). Books claims are population vouchers of any
//!   type carrying a line on a configured TDS/TCS ledger and a named party (`party_field`); a
//!   voucher with no party is attributed only when its one other ledger is a configured alias
//!   target. A TAN matches a party only through `[tds_tcs_26as].deductor_aliases`.
//! * Rows sharing a (kind, party) are compared once against that party's claim. Categories are
//!   closed: matched / amount_differs, and a closed reason set for each unmatched side.
//! * AIS/TIS figures stand beside the books, never matched. The PAN-view scope limit is stated on
//!   every run.
//!
//! Divergences:
//! * a total that overflows i64 paise is refused where the reference would go on;
//! * the three ledger lists bind as sets, so when more than one name fails to bind, the refusal
//!   may name a different one first than the reference (which binds in config order).
//!
//! Refused as the reference raises: a figure id repeated by a hash collision or a repeated key (two
//! TANs under one section for one party, a TIS category twice, one voucher GUID twice).

use std::collections::{BTreeMap, BTreeSet};

use sha1::{Digest, Sha1};

use crate::book::{Book, Voucher};
use crate::documents::{AisRow, Form26asRow, TisRow};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::{iso, Window};
use crate::rules::Rules;
use crate::support::{count, overflow, py_repr_str, voucher_label};
use crate::Tds26asConfig;

pub const TEST_ID: &str = "tds_tcs_26as";
pub const VERSION: &str = "1";

const TOL_PAISE: i64 = 100;

const REASON_26AS_DIFFERENT_PERIOD: &str = "different_period";
const REASON_26AS_NO_ALIAS: &str = "no_books_ledger_alias";
const REASON_26AS_NOT_FOUND: &str = "not_found_in_books";
const REASON_26AS_UNCLASSIFIED: &str = "unclassified";
const REASONS_26AS: [&str; 4] = [
    REASON_26AS_DIFFERENT_PERIOD,
    REASON_26AS_NO_ALIAS,
    REASON_26AS_NOT_FOUND,
    REASON_26AS_UNCLASSIFIED,
];

const REASON_BOOKS_DIFFERENT_PERIOD: &str = "different_period";
const REASON_BOOKS_NOT_IN_26AS: &str = "not_found_in_26as";
const REASON_BOOKS_UNCLASSIFIED: &str = "unclassified";
const REASONS_BOOKS: [&str; 3] = [
    REASON_BOOKS_DIFFERENT_PERIOD,
    REASON_BOOKS_NOT_IN_26AS,
    REASON_BOOKS_UNCLASSIFIED,
];

/// The reference's `PART_KIND`.
fn part_kind(part: &str) -> Option<&'static str> {
    match part {
        "I" => Some("tds"),
        "VI" => Some("tcs"),
        _ => None,
    }
}

/// The reference's `_hash`: sha1 of the text, first 8 hex digits.
fn hash8(text: &str) -> String {
    crate::canonical::hex(&Sha1::digest(text.as_bytes()))[..8].to_string()
}

fn sum_paise(mut values: impl Iterator<Item = i64>) -> Result<i64> {
    values
        .try_fold(0_i64, |a, b| a.checked_add(b))
        .ok_or_else(|| overflow(TEST_ID))
}

fn voucher_ref(v: &Voucher) -> EvidenceRef {
    EvidenceRef::with_label("voucher", &v.guid, &voucher_label(v))
}

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

/// The reference's `Agg26AsRow`: one (part, TAN, section) bucket.
struct AggRow {
    tan: String,
    section: String,
    part: String,
    tax_paise: i64,
    min_date: String,
    max_date: String,
    evidence: Vec<EvidenceRef>,
}

fn aggregate_26as(rows: &[Form26asRow]) -> Result<Vec<AggRow>> {
    let mut buckets: BTreeMap<(&str, &str, &str), Vec<&Form26asRow>> = BTreeMap::new();
    for row in rows {
        buckets
            .entry((&row.part, &row.deductor_tan, &row.section))
            .or_default()
            .push(row);
    }
    buckets
        .into_iter()
        .map(|((part, tan, section), rs)| {
            let dates: Vec<&str> = rs.iter().map(|r| r.txn_date.as_str()).collect();
            Ok(AggRow {
                tan: tan.to_string(),
                section: section.to_string(),
                part: part.to_string(),
                tax_paise: sum_paise(rs.iter().map(|r| r.tax_paise))?,
                min_date: dates.iter().min().expect("a bucket has a row").to_string(),
                max_date: dates.iter().max().expect("a bucket has a row").to_string(),
                evidence: rs
                    .iter()
                    .map(|r| {
                        EvidenceRef::with_label(
                            "document_row",
                            &format!("{}#{}", r.doc, r.row),
                            &format!("{tan} {section} {}", iso(&r.txn_date)),
                        )
                    })
                    .collect(),
            })
        })
        .collect()
}

/// The reference's `BooksClaimRow`.
struct ClaimRow<'a> {
    kind: &'static str,
    party: String,
    amount_paise: i64,
    vouchers: Vec<&'a Voucher>,
}

/// The reference's `_books_claim_rows`, in first-seen order of (kind, party).
fn books_claim_rows<'a>(
    pop: &[&'a Voucher],
    tds_ledgers: &BTreeSet<String>,
    tcs_ledgers: &BTreeSet<String>,
    alias_targets: &BTreeSet<&str>,
) -> Result<Vec<ClaimRow<'a>>> {
    let mut order: Vec<(&'static str, String)> = Vec::new();
    let mut grouped: BTreeMap<(&'static str, String), Vec<&'a Voucher>> = BTreeMap::new();
    for &v in pop {
        let mut party = v.party_field.clone();
        if party.is_empty() {
            let others: BTreeSet<&str> = v
                .lines
                .iter()
                .filter(|l| {
                    !tds_ledgers.contains(&l.ledger)
                        && !tcs_ledgers.contains(&l.ledger)
                        && l.amount_paise != 0
                })
                .map(|l| l.ledger.as_str())
                .collect();
            match others.iter().next() {
                Some(only) if others.len() == 1 && alias_targets.contains(only) => {
                    party = (*only).to_string();
                }
                _ => continue,
            }
        }
        for (role, kind) in [(tds_ledgers, "tds"), (tcs_ledgers, "tcs")] {
            if v.lines
                .iter()
                .any(|l| role.contains(&l.ledger) && l.amount_paise != 0)
            {
                let key = (kind, party.clone());
                if !grouped.contains_key(&key) {
                    order.push(key.clone());
                }
                grouped.entry(key).or_default().push(v);
            }
        }
    }
    order
        .into_iter()
        .map(|(kind, party)| {
            let vs = grouped.remove(&(kind, party.clone())).expect("ordered key");
            let role = if kind == "tds" {
                tds_ledgers
            } else {
                tcs_ledgers
            };
            let amount_paise = sum_paise(
                vs.iter()
                    .flat_map(|v| v.lines.iter())
                    .filter(|l| role.contains(&l.ledger))
                    .map(|l| l.amount_paise),
            )?;
            Ok(ClaimRow {
                kind,
                party,
                amount_paise,
                vouchers: vs,
            })
        })
        .collect()
}

fn classify_26as_only(agg: &AggRow, alias: Option<&String>, period: &Window) -> &'static str {
    if agg.max_date.as_str() < period.from.as_str() || agg.min_date.as_str() > period.to.as_str() {
        return REASON_26AS_DIFFERENT_PERIOD;
    }
    if alias.is_none() {
        return REASON_26AS_NO_ALIAS;
    }
    REASON_26AS_NOT_FOUND
}

fn classify_books_only(
    row: &ClaimRow,
    matchable: &BTreeSet<(&str, &str)>,
    period: &Window,
) -> &'static str {
    let outside = |v: &&Voucher| {
        v.date.as_str() < period.from.as_str() || v.date.as_str() > period.to.as_str()
    };
    if row.vouchers.iter().all(outside) {
        return REASON_BOOKS_DIFFERENT_PERIOD;
    }
    if !matchable.contains(&(row.kind, row.party.as_str())) {
        return REASON_BOOKS_NOT_IN_26AS;
    }
    REASON_BOOKS_UNCLASSIFIED
}

fn joined_sorted<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ")
}

fn text_count(r: &TestResult, prefix: &str, value: &str) -> usize {
    r.figures
        .iter()
        .filter(|f| f.id.starts_with(prefix) && f.value == Value::Text(value.to_string()))
        .count()
}

/// Run the test. `period` is the engagement's audit period (the reference's `book.period`).
#[allow(clippy::too_many_arguments)]
pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    form26as: &[Form26asRow],
    ais_rows: &[AisRow],
    tis_rows: &[TisRow],
    cfg: &Tds26asConfig,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = "Form 26AS Part I (TDS) and Part VI (TCS) rows, aggregated per (TAN, \
section); matched to vouchers of any type carrying a line on the TDS/TCS ledger set for this client \
and a named party (each deductor's TAN is linked to its books ledger for this client, never by \
name). AIS/TIS GST-turnover/purchases and advance-tax figures are reported side by side, not \
matched."
        .to_string();

    for (role, ledgers) in [("tds", &cfg.tds_ledgers), ("tcs", &cfg.tcs_ledgers)] {
        for name in ledgers {
            fig(
                &mut r,
                &format!("{role}_ledger_{}", stable_ledger_tag(book, name)?),
                Value::Text(role.to_string()),
                Unit::Text,
                &format!(
                    "{}-role ledger (client configuration).",
                    role.to_uppercase()
                ),
                vec![EvidenceRef::new("ledger", name)],
            )?;
        }
    }
    for (tan, ledger) in &cfg.deductor_aliases {
        fig(
            &mut r,
            &format!("alias_{}", hash8(tan)),
            Value::Text(ledger.clone()),
            Unit::Text,
            "26AS TAN -> books party ledger (client configuration).",
            vec![EvidenceRef::new("config", tan)],
        )?;
    }

    let agg = aggregate_26as(form26as)?;
    let pop = book.population()?;
    let alias_targets: BTreeSet<&str> = cfg.deductor_aliases.values().map(String::as_str).collect();
    let claims = books_claim_rows(&pop, &cfg.tds_ledgers, &cfg.tcs_ledgers, &alias_targets)?;
    fig(
        &mut r,
        "twentysixas_agg_count",
        count(TEST_ID, agg.len())?,
        Unit::Count,
        "Distinct (TAN, section) rows in Form 26AS Part I/VI.",
        Vec::new(),
    )?;
    fig(
        &mut r,
        "books_claim_count",
        count(TEST_ID, claims.len())?,
        Unit::Count,
        "Distinct (kind, party) TDS/TCS claim rows in the books.",
        Vec::new(),
    )?;
    for (role, ledgers) in [("tds", &cfg.tds_ledgers), ("tcs", &cfg.tcs_ledgers)] {
        let movement = sum_paise(
            ledgers
                .iter()
                .filter_map(|n| book.tb.get(n))
                .map(|t| t.debit_paise),
        )?;
        fig(
            &mut r,
            &format!("books_{role}_ledger_movement_paise"),
            Value::Int(movement),
            Unit::Paise,
            &format!(
                "Period debit movement (Dr+) of the {}-role ledger(s), recomputed from the TB.",
                role.to_uppercase()
            ),
            Vec::new(),
        )?;
    }
    for (name, part, what) in [
        (
            "twentysixas_total_tds_tax_paise",
            "I",
            "Part I tax across every deductor",
        ),
        (
            "twentysixas_total_tcs_tax_paise",
            "VI",
            "Part VI tax across every collector",
        ),
    ] {
        let total = sum_paise(agg.iter().filter(|a| a.part == part).map(|a| a.tax_paise))?;
        fig(
            &mut r,
            name,
            Value::Int(total),
            Unit::Paise,
            &format!("Sum of Form 26AS {what}."),
            Vec::new(),
        )?;
    }

    // Rows sharing a (kind, party) form one group, compared once against that party's claim.
    let claim_index: BTreeMap<(&str, &str), usize> = claims
        .iter()
        .enumerate()
        .map(|(i, c)| ((c.kind, c.party.as_str()), i))
        .collect();
    let mut used_claims: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut cat_totals: BTreeMap<&str, [i64; 3]> =
        [("matched", [0; 3]), ("amount_differs", [0; 3])]
            .into_iter()
            .collect();
    let mut group_order: Vec<(&str, &str)> = Vec::new();
    let mut groups: BTreeMap<(&str, &str), Vec<&AggRow>> = BTreeMap::new();
    let mut folded: usize = 0;
    for a in &agg {
        let kind = part_kind(&a.part);
        let alias = cfg.deductor_aliases.get(&a.tan);
        let claim = match (kind, alias) {
            (Some(k), Some(al)) if !al.is_empty() => claim_index.get(&(k, al.as_str())),
            _ => None,
        };
        if claim.is_none() {
            let h = hash8(&format!("{}{}{}", a.tan, a.section, a.part));
            let reason = classify_26as_only(a, alias, period);
            fig(
                &mut r,
                &format!("twentysixas_only_row_{h}"),
                Value::Text(reason.to_string()),
                Unit::Text,
                &format!(
                    "Unmatched 26AS row (TAN {}, section {}, part {}).",
                    a.tan, a.section, a.part
                ),
                a.evidence.clone(),
            )?;
            continue;
        }
        let key = (kind.expect("matched"), alias.expect("matched").as_str());
        if !groups.contains_key(&key) {
            group_order.push(key);
        }
        groups.entry(key).or_default().push(a);
    }
    for key in &group_order {
        let (kind, alias) = *key;
        let rows = &groups[key];
        let claim = &claims[claim_index[key]];
        used_claims.insert(*key);
        folded += rows.len() - 1;
        let tax = sum_paise(rows.iter().map(|a| a.tax_paise))?;
        let sections = joined_sorted(rows.iter().map(|a| a.section.as_str()));
        let tans = joined_sorted(rows.iter().map(|a| a.tan.as_str()));
        let mut keys: Vec<String> = rows
            .iter()
            .map(|a| format!("{}{}{}", a.tan, a.section, a.part))
            .collect();
        keys.sort();
        let h = hash8(&keys.join("|"));
        let gap = claim
            .amount_paise
            .checked_sub(tax)
            .and_then(i64::checked_abs)
            .ok_or_else(|| overflow(TEST_ID))?;
        let cat = if gap <= TOL_PAISE {
            "matched"
        } else {
            "amount_differs"
        };
        let t = cat_totals.get_mut(cat).expect("a known category");
        t[0] += 1;
        t[1] = t[1].checked_add(tax).ok_or_else(|| overflow(TEST_ID))?;
        t[2] = t[2]
            .checked_add(claim.amount_paise)
            .ok_or_else(|| overflow(TEST_ID))?;
        let mut ev: Vec<EvidenceRef> = rows.iter().flat_map(|a| a.evidence.clone()).collect();
        ev.extend(claim.vouchers.iter().map(|v| voucher_ref(v)));
        let together = if rows.len() > 1 {
            format!(
                " ({} 26AS sections compared together with one books claim)",
                rows.len()
            )
        } else {
            String::new()
        };
        fig(
            &mut r,
            &format!("match_pair_{kind}_{h}"),
            Value::Text(cat.to_string()),
            Unit::Text,
            &format!(
                "Match outcome for TAN {tans} section(s) {sections} vs books party {}{together}.",
                py_repr_str(alias)
            ),
            ev,
        )?;
        if rows.len() > 1 {
            for a in rows {
                fig(
                    &mut r,
                    &format!(
                        "pair_section_tax_{kind}_{h}_{}",
                        hash8(&format!("{}{}", a.section, a.part))
                    ),
                    Value::Int(a.tax_paise),
                    Unit::Paise,
                    &format!(
                        "26AS tax under section {} (TAN {}) inside that combined comparison.",
                        a.section, a.tan
                    ),
                    a.evidence.clone(),
                )?;
            }
        }
    }
    fig(
        &mut r,
        "twentysixas_rows_folded_count",
        count(TEST_ID, folded)?,
        Unit::Count,
        "26AS (TAN, section) rows compared together with another row of the same deductor against \
one books claim, beyond the first row of each such group.",
        Vec::new(),
    )?;

    let matchable: BTreeSet<(&str, &str)> = agg
        .iter()
        .filter_map(|a| {
            let kind = part_kind(&a.part)?;
            let alias = cfg.deductor_aliases.get(&a.tan)?;
            Some((kind, alias.as_str()))
        })
        .collect();
    for row in claims
        .iter()
        .filter(|c| !used_claims.contains(&(c.kind, c.party.as_str())))
    {
        let reason = classify_books_only(row, &matchable, period);
        let h = hash8(&format!(
            "{}|{}",
            row.kind,
            stable_ledger_tag(book, &row.party)?
        ));
        fig(
            &mut r,
            &format!("books_only_row_{h}"),
            Value::Text(reason.to_string()),
            Unit::Text,
            &format!(
                "Books TDS/TCS claim with no 26AS counterpart (kind {}, party {}).",
                row.kind,
                py_repr_str(&row.party)
            ),
            row.vouchers.iter().map(|v| voucher_ref(v)).collect(),
        )?;
    }

    for cat in ["matched", "amount_differs"] {
        let [n, tax26, bamt] = cat_totals[cat];
        fig(
            &mut r,
            &format!("category_{cat}_count"),
            Value::Int(n),
            Unit::Count,
            &format!("(TAN, section) rows in category '{cat}'."),
            Vec::new(),
        )?;
        fig(
            &mut r,
            &format!("category_{cat}_26as_tax_paise"),
            Value::Int(tax26),
            Unit::Paise,
            &format!("Sum of 26AS tax, category '{cat}'."),
            Vec::new(),
        )?;
        fig(
            &mut r,
            &format!("category_{cat}_books_amount_paise"),
            Value::Int(bamt),
            Unit::Paise,
            &format!("Sum of the matching books claim, category '{cat}'."),
            Vec::new(),
        )?;
    }

    let only_prefix = format!("{TEST_ID}.twentysixas_only_row_");
    let n_26as = |r: &TestResult, reason: &str| text_count(r, &only_prefix, reason);
    for reason in REASONS_26AS {
        let n = n_26as(&r, reason);
        fig(
            &mut r,
            &format!("twentysixas_only_reason_{reason}_count"),
            count(TEST_ID, n)?,
            Unit::Count,
            &format!("26AS-only rows classified '{reason}'."),
            Vec::new(),
        )?;
    }
    let n = n_26as(&r, REASON_26AS_UNCLASSIFIED);
    fig(
        &mut r,
        "twentysixas_only_unclassified_count",
        count(TEST_ID, n)?,
        Unit::Count,
        "26AS-only rows with no reason from the closed set (must be zero -- TT-2).",
        Vec::new(),
    )?;
    let books_prefix = format!("{TEST_ID}.books_only_row_");
    let n_books = |r: &TestResult, reason: &str| text_count(r, &books_prefix, reason);
    for reason in REASONS_BOOKS {
        let n = n_books(&r, reason);
        fig(
            &mut r,
            &format!("books_only_reason_{reason}_count"),
            count(TEST_ID, n)?,
            Unit::Count,
            &format!("Books-only rows classified '{reason}'."),
            Vec::new(),
        )?;
    }
    let n = n_books(&r, REASON_BOOKS_UNCLASSIFIED);
    fig(
        &mut r,
        "books_only_unclassified_count",
        count(TEST_ID, n)?,
        Unit::Count,
        "Books-only rows with no reason from the closed set (must be zero -- TT-2).",
        Vec::new(),
    )?;
    let (n_not_found, n_no_alias) = (
        n_26as(&r, REASON_26AS_NOT_FOUND),
        n_26as(&r, REASON_26AS_NO_ALIAS),
    );
    let n_books_only = n_books(&r, REASON_BOOKS_NOT_IN_26AS);

    // ------------------------------------------------------------ TCS capitalisation
    let matched_tcs_parties: BTreeSet<&str> = used_claims
        .iter()
        .filter(|(kind, _)| *kind == "tcs")
        .map(|(_, party)| *party)
        .collect();
    let mut not_separate = 0_usize;
    let mut not_separate_ev = Vec::new();
    for &v in &pop {
        if v.base_type != "Purchase" || !matched_tcs_parties.contains(v.party_field.as_str()) {
            continue;
        }
        let fixed_asset = v.lines.iter().any(|l| {
            book.ledgers
                .get(&l.ledger)
                .is_some_and(|led| led.under("Fixed Assets"))
        });
        if !fixed_asset {
            continue;
        }
        let separate = v
            .lines
            .iter()
            .any(|l| cfg.tcs_ledgers.contains(&l.ledger) && l.amount_paise != 0);
        if !separate {
            not_separate += 1;
            not_separate_ev.push(voucher_ref(v));
        }
        fig(
            &mut r,
            &format!("tcs_capitalisation_{}", hash8(&v.guid)),
            Value::Text(
                if separate {
                    "booked_separately"
                } else {
                    "no_separate_tcs_line"
                }
                .to_string(),
            ),
            Unit::Text,
            "Whether TCS was booked on a ledger line distinct from the asset's cost lines on this \
Fixed-Assets voucher -- a booking fact, not a tax-treatment conclusion (see 'Vehicle trail').",
            vec![voucher_ref(v)],
        )?;
    }
    let f_not_separate = fig(
        &mut r,
        "tcs_capitalisation_no_separate_line_count",
        count(TEST_ID, not_separate)?,
        Unit::Count,
        "Fixed-Assets vouchers, tied to a matched TCS collector, with no distinct TCS ledger line.",
        Vec::new(),
    )?;

    // ------------------------------------------------------------ AIS/TIS/books figures
    let ais_sum = |category: &str| {
        sum_paise(
            ais_rows
                .iter()
                .filter(|a| a.category == category)
                .map(|a| a.amount_paise),
        )
    };
    let movement_under = |group: &str| {
        sum_paise(
            book.tb
                .iter()
                .filter(|(n, _)| book.ledgers.get(*n).is_some_and(|l| l.under(group)))
                .map(|(_, t)| t.movement_paise()),
        )
    };
    let books_sales = movement_under("Sales Accounts")?
        .checked_neg()
        .ok_or_else(|| overflow(TEST_ID))?;
    let books_purchases = movement_under("Purchase Accounts")?;
    for (name, value, definition) in [
        (
            "books_sales_paise",
            books_sales,
            "Sales Accounts period movement (closing-opening), negated -- recomputed from the TB.",
        ),
        (
            "books_purchases_paise",
            books_purchases,
            "Purchase Accounts period movement (closing-opening) -- recomputed from the TB.",
        ),
        (
            "ais_gst_turnover_paise",
            ais_sum("gst_turnover")?,
            "AIS category 'GST turnover', accepted-by-taxpayer value, summed across entries.",
        ),
        (
            "ais_gst_purchases_paise",
            ais_sum("gst_purchases")?,
            "AIS category 'GST purchases', accepted-by-taxpayer value, summed across entries.",
        ),
        (
            "ais_advance_tax_paise",
            ais_sum("advance_tax")?,
            "AIS Part B3 (tax payments/challans), 'total' column, summed.",
        ),
        (
            "ais_refund_paise",
            ais_sum("refund")?,
            "AIS Part B4 (refund), the refund amount column, summed. No books-side figure is \
attempted (limit).",
        ),
    ] {
        fig(
            &mut r,
            name,
            Value::Int(value),
            Unit::Paise,
            definition,
            Vec::new(),
        )?;
    }
    for t in tis_rows {
        fig(
            &mut r,
            &format!("tis_category_{}_accepted_paise", hash8(&t.category)),
            Value::Int(t.accepted_paise),
            Unit::Paise,
            &format!(
                "TIS category {}, accepted-by-taxpayer/confirmed-by-source total.",
                py_repr_str(&t.category)
            ),
            vec![EvidenceRef::with_label(
                "document_row",
                &format!("{}#{}", t.doc, t.row),
                &t.category,
            )],
        )?;
    }
    let books_advance_tax = sum_paise(
        cfg.advance_tax_ledgers
            .iter()
            .filter_map(|n| book.tb.get(n))
            .map(|t| t.debit_paise),
    )?;
    fig(
        &mut r,
        "books_advance_tax_paise",
        Value::Int(books_advance_tax),
        Unit::Paise,
        "Period debit movement of the caller-supplied advance-tax ledger(s) -- recomputed from the \
TB.",
        Vec::new(),
    )?;

    // ------------------------------------------------------------ findings
    let fid = |name: &str| format!("{TEST_ID}.{name}");
    if n_not_found > 0 || n_no_alias > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/twentysixas_only"),
            clauses: Vec::new(),
            title: "Form 26AS shows TDS/TCS credit with no matching claim identified in the books"
                .to_string(),
            facts: vec![
                (
                    "not_found_count".to_string(),
                    fid(&format!(
                        "twentysixas_only_reason_{REASON_26AS_NOT_FOUND}_count"
                    )),
                ),
                (
                    "no_alias_count".to_string(),
                    fid(&format!(
                        "twentysixas_only_reason_{REASON_26AS_NO_ALIAS}_count"
                    )),
                ),
            ],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "A deductor/collector not yet aliased to a books ledger cannot be \
matched by this test; the gap may be a mapping the CA still needs to supply, not a books omission."
                    .to_string(),
            ],
            ask_client: vec![
                "For each unmatched TAN/section: identify the books party ledger, \
or confirm the income/purchase was never recorded, and supply the alias."
                    .to_string(),
            ],
        });
    }
    if cat_totals["amount_differs"][0] > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/amount_differs"),
            clauses: Vec::new(),
            title: "A books TDS/TCS claim matched a Form 26AS row but the tax amount differs by \
more than a rupee"
                .to_string(),
            facts: vec![(
                "amount_differs_count".to_string(),
                fid("category_amount_differs_count"),
            )],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "26AS 'Date of Booking' can fall in the next FY relative to the \
transaction date; a small timing gap can look like an amount difference without being a books \
error."
                    .to_string(),
            ],
            ask_client: vec![
                "TDS/TCS certificate (Form 16A/27D) for each differing pair.".to_string(),
            ],
        });
    }
    if n_books_only > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/books_only"),
            clauses: Vec::new(),
            title: "Books claim a TDS/TCS credit that the uploaded Form 26AS does not show"
                .to_string(),
            facts: vec![(
                "books_only_count".to_string(),
                fid(&format!(
                    "books_only_reason_{REASON_BOOKS_NOT_IN_26AS}_count"
                )),
            )],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "26AS updates continuously as deductors file late; the credit may still \
appear in a later-dated 26AS pull even though this upload does not show it."
                    .to_string(),
            ],
            ask_client: vec![
                "A fresh 26AS pull nearer the filing date; the TDS/TCS certificate for \
each row."
                    .to_string(),
            ],
        });
    }
    if not_separate > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/tcs_capitalisation"),
            clauses: vec!["3CD-18".to_string(), "s.43(1)".to_string()],
            title: "Fixed-asset purchase from a TCS collector with no separate TCS line in the \
books: confirm whether TCS was charged on it and, if so, that it is not in the asset's cost"
                .to_string(),
            facts: vec![("no_separate_line_count".to_string(), f_not_separate)],
            evidence: not_separate_ev,
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "This states only that the books show no distinct TCS ledger line on the \
voucher; it does not by itself confirm the TCS amount was folded into the capitalised cost -- the \
purchase invoice's own price breakup must be seen to confirm that either way."
                    .to_string(),
            ],
            ask_client: vec![
                "The purchase invoice for each flagged asset, to confirm TCS is shown \
and recoverable separately from the asset's price."
                    .to_string(),
            ],
        });
    }
    r.findings.push(Finding {
        id: format!("{TEST_ID}/pan_view_scope_limit"),
        clauses: Vec::new(),
        title: "The uploaded Form 26AS/AIS/TIS are the assessee's own PAN-view record".to_string(),
        facts: Vec::new(),
        evidence: Vec::new(),
        confidence: Confidence::NeedsDocument,
        limits: vec![
            "A PAN-view 26AS/AIS/TIS shows tax deducted/collected FROM the assessee, never \
what the assessee itself deducted or collected as a deductor/collector."
                .to_string(),
        ],
        ask_client: vec![
            "If the entity holds a TAN (deducts u/s 192/194C/194I/194J/etc., or \
collects TCS), obtain its own TRACES TAN-view export (quarterly 24Q/26Q/27EQ statements or the \
Justification Report) -- this PAN-view document does not cover that."
                .to_string(),
        ],
    });
    Ok(r)
}

/// The reference's `check_invariants` (TT-1..TT-4), independent of `run`'s own matching: the
/// partition counts, each role ledger's movement recomputed from the Trial Balance, and every
/// match pair's 26AS evidence resolved to a row of the right part.
pub fn check_invariants(
    book: &Book,
    form26as: &[Form26asRow],
    result: &TestResult,
) -> Result<Vec<String>> {
    let prefix = format!("{}.", result.test_id);
    let get = |name: &str| {
        result
            .figures
            .iter()
            .find(|f| f.id == format!("{prefix}{name}"))
    };
    let int_of = |name: &str| -> Option<i64> {
        match get(name).map(|f| &f.value) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    };
    let count_prefix = |p: &str| -> i64 {
        let n = result
            .figures
            .iter()
            .filter(|f| f.id.starts_with(&format!("{prefix}{p}")))
            .count();
        i64::try_from(n).unwrap_or(i64::MAX)
    };
    let mut out = Vec::new();

    // TT-1: both sides partition.
    let matched = int_of("category_matched_count").unwrap_or(0);
    let differs = int_of("category_amount_differs_count").unwrap_or(0);
    let only_26as = count_prefix("twentysixas_only_row_");
    let only_books = count_prefix("books_only_row_");
    let folded = int_of("twentysixas_rows_folded_count").unwrap_or(0);
    if let Some(agg_total) = int_of("twentysixas_agg_count") {
        let got = matched + differs + only_26as + folded;
        if got != agg_total {
            out.push(format!(
                "TT-1: matched+amount_differs+twentysixas_only+folded ({got}) != \
twentysixas_agg_count ({agg_total})"
            ));
        }
    }
    if let Some(claim_total) = int_of("books_claim_count") {
        let got = matched + differs + only_books;
        if got != claim_total {
            out.push(format!(
                "TT-1: matched+amount_differs+books_only ({got}) != books_claim_count \
({claim_total})"
            ));
        }
    }

    // TT-2: the unclassified counts are zero.
    for name in [
        "twentysixas_only_unclassified_count",
        "books_only_unclassified_count",
    ] {
        if let Some(f) = get(name) {
            if f.value != Value::Int(0) {
                let shown = match &f.value {
                    Value::Int(n) => n.to_string(),
                    Value::Text(t) => t.clone(),
                    Value::Undefined => "None".to_string(),
                };
                out.push(format!("TT-2: {name} = {shown} (must be zero)"));
            }
        }
    }

    // TT-3: each role ledger's movement, recomputed from the TB.
    let by_tag: BTreeMap<String, &String> = book
        .ledgers
        .keys()
        .map(|n| Ok((stable_ledger_tag(book, n)?, n)))
        .collect::<Result<_>>()?;
    for (role, name) in [
        ("tds", "books_tds_ledger_movement_paise"),
        ("tcs", "books_tcs_ledger_movement_paise"),
    ] {
        let marker = format!("{prefix}{role}_ledger_");
        let ledgers: BTreeSet<&String> = result
            .figures
            .iter()
            .filter(|f| f.id.starts_with(&marker) && f.value == Value::Text(role.to_string()))
            .filter_map(|f| by_tag.get(&f.id[marker.len()..]).copied())
            .collect();
        let recomputed = sum_paise(
            ledgers
                .iter()
                .filter_map(|n| book.tb.get(*n))
                .map(|t| t.debit_paise),
        )?;
        if let Some(f) = get(name) {
            if f.value != Value::Int(recomputed) {
                let shown = match &f.value {
                    Value::Int(n) => n.to_string(),
                    Value::Text(t) => t.clone(),
                    Value::Undefined => "None".to_string(),
                };
                out.push(format!(
                    "TT-3: {name} = {shown} but recomputed from the TB = {recomputed}"
                ));
            }
        }
    }

    // TT-4: a match pair's 26AS evidence is of its own kind's part.
    let doc_index: BTreeMap<String, &Form26asRow> = form26as
        .iter()
        .map(|d| (format!("{}#{}", d.doc, d.row), d))
        .collect();
    for (kind, part) in [("tds", "I"), ("tcs", "VI")] {
        let marker = format!("{prefix}match_pair_{kind}_");
        for f in result.figures.iter().filter(|f| f.id.starts_with(&marker)) {
            for e in f.evidence.iter().filter(|e| e.kind == "document_row") {
                match doc_index.get(&e.id) {
                    None => out.push(format!(
                        "TT-4: {} evidence {} does not resolve to a 26AS row",
                        f.id, e.id
                    )),
                    Some(d) if d.part != part => out.push(format!(
                        "TT-4: {} (kind {kind}) matches a 26AS part-{} row ({})",
                        f.id, d.part, e.id
                    )),
                    Some(_) => {}
                }
            }
        }
    }
    Ok(out)
}
