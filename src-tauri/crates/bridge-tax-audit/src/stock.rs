// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `stock`: whether closing stock was typed in, the books figure
//! (two ways) against Tally's Stock Summary valuation (two dates), non-goods items, and negative
//! stock at year end and during the year.
//!
//! * "Typed in": population vouchers with a line on a Stock-in-Hand ledger; none means the closing
//!   figure was keyed onto the ledger directly.
//! * The books figure is read off the Trial Balance both ways (Tally's closing field, and opening +
//!   debit - credit); the Stock Summary figures are Tally's item valuation. Gaps are figures, never
//!   a conclusion about which side is right.
//! * A value-only item (BASEUNITS is Tally's reserved "Not Applicable") is left out of every
//!   quantity figure and counted separately when its value is negative.
//! * "Went negative during the year": each item's running quantity from its period-start quantity
//!   (the opening Stock Summary's, else the master's own opening), walked through the population's
//!   inventory lines by date. Same-day order is not in the export, so the walk runs twice (stock-in
//!   first, stock-out first) and each count is a range. A movement's direction is the stock
//!   journal's IN/OUT tag, else the sign of the line's amount; a line with no quantity or no amount
//!   sign moves nothing.
//!
//! Python's floating-point order is kept: quantities are f64 added in the reference's own order
//! (population order, inventory-line order, dates ascending, then a stable sort within a day).
//! The reference's per-item lowest quantities reach nothing `run` emits, so they are not tracked.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;

use crate::book::{Book, InventoryLine, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::read::iso;
use crate::rules::Rules;
use crate::stock_read::{StockInputs, StockItemMaster, StockSnapshot};
use crate::support::{count, voucher_label};

pub const TEST_ID: &str = "stock";
pub const VERSION: &str = "1";

/// Tally's own reserved group name, not client data.
pub const STOCK_IN_HAND_GROUP: &str = "Stock-in-Hand";

/// Float quantity tolerance, as the reference's `QTY_TOL`.
pub const QTY_TOL: f64 = 1e-6;

fn overflow() -> AuditError {
    crate::support::overflow(TEST_ID)
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b).ok_or_else(overflow)
}

fn sub(a: i64, b: i64) -> Result<i64> {
    a.checked_sub(b).ok_or_else(overflow)
}

fn item_evidence<'a>(names: impl IntoIterator<Item = &'a String>) -> Vec<EvidenceRef> {
    let sorted: BTreeSet<&String> = names.into_iter().collect();
    sorted
        .into_iter()
        .map(|n| EvidenceRef::with_label("stock_item", n, n))
        .collect()
}

/// +1.0 (in), -1.0 (out) or `None` (no quantity, or no native tag and no amount sign).
fn inventory_direction(il: &InventoryLine) -> Option<f64> {
    il.qty?;
    if let Some(d) = il.direction {
        return Some(f64::from(d));
    }
    match il.amount_paise {
        None | Some(0) => None,
        Some(a) if a > 0 => Some(1.0),
        Some(_) => Some(-1.0),
    }
}

/// A line's signed quantity movement, as the reference computes `mult * il.qty`.
fn movement(il: &InventoryLine) -> Option<f64> {
    let mult = inventory_direction(il)?;
    il.qty.map(|q| mult * q)
}

fn is_goods(items: &BTreeMap<String, StockItemMaster>, item: &str) -> bool {
    items.get(item).is_none_or(StockItemMaster::is_goods)
}

/// Each item's period-start quantity: the opening summary's own (a blank one is nil) for every
/// item it lists, else the master's own opening. Returns (quantities, from the summary, from a
/// master).
fn opening_quantities(
    items: &BTreeMap<String, StockItemMaster>,
    opening: &StockSnapshot,
) -> (BTreeMap<String, f64>, BTreeSet<String>, BTreeSet<String>) {
    let from_summary: BTreeSet<String> = opening.rows.keys().cloned().collect();
    let mut qty: BTreeMap<String, f64> = items
        .iter()
        .filter(|(n, _)| !from_summary.contains(*n))
        .map(|(n, it)| (n.clone(), it.opening_qty.unwrap_or(0.0)))
        .collect();
    let from_master: BTreeSet<String> = qty.keys().cloned().collect();
    for (n, row) in &opening.rows {
        qty.insert(n.clone(), row.qty.unwrap_or(0.0));
    }
    (qty, from_summary, from_master)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    InFirst,
    OutFirst,
}

/// One running-quantity walk: (items negative at some point, items negative at opening).
fn walk(
    pop: &[&Voucher],
    opening_qty: &BTreeMap<String, f64>,
    order: Order,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut running = opening_qty.clone();
    let at_opening: BTreeSet<String> = opening_qty
        .iter()
        .filter(|(_, q)| **q < -QTY_TOL)
        .map(|(n, _)| n.clone())
        .collect();
    let mut touched = at_opening.clone();
    let mut by_day: BTreeMap<TallyDate, Vec<(&str, f64)>> = BTreeMap::new();
    for v in pop {
        for il in &v.inventory {
            if il.item.is_empty() {
                continue;
            }
            if let Some(dq) = movement(il) {
                by_day
                    .entry(v.date.clone())
                    .or_default()
                    .push((il.item.as_str(), dq));
            }
        }
    }
    for moves in by_day.values_mut() {
        // Stable, as Python's list.sort: stock-in first sorts by -dq, stock-out first by dq. A
        // quantity is never NaN (it is parsed from digits), and -0.0 ties with 0.0 as in Python.
        let cmp = |x: f64, y: f64| x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal);
        match order {
            Order::InFirst => moves.sort_by(|a, b| cmp(b.1, a.1)),
            Order::OutFirst => moves.sort_by(|a, b| cmp(a.1, b.1)),
        }
        for (item, dq) in moves.iter() {
            let q = running.entry((*item).to_string()).or_insert(0.0);
            *q += dq;
            if *q < -QTY_TOL {
                touched.insert((*item).to_string());
            }
        }
    }
    (touched, at_opening)
}

#[allow(clippy::too_many_lines)] // one section per figure group, as the reference lays them out
pub fn run(book: &Book, rules: &Rules, inputs: &StockInputs) -> Result<TestResult> {
    let StockInputs {
        items,
        opening,
        closing,
        is_integrated,
    } = inputs;
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = "Books population (optional, cancelled and post-dated vouchers excluded). \
Non-goods stock items (BASEUNITS == \"Not Applicable\") excluded from every quantity-based figure."
        .to_string();
    let integrated = match is_integrated {
        None => "unknown",
        Some(true) => "Yes",
        Some(false) => "No",
    };
    r.fig(
        "is_integrated",
        Value::Text(integrated.to_string()),
        Unit::Text,
        "Company feature ISINTEGRATED (F11 'Integrate accounts with inventory'), read from the \
company object when available; 'unknown' if the tag was not found.",
        vec![],
    );

    // ---- quantity field presence ----
    let pop = book.population()?;
    let goods_lines: Vec<(&Voucher, &InventoryLine)> = pop
        .iter()
        .flat_map(|v| v.inventory.iter().map(move |il| (*v, il)))
        .filter(|(_, il)| !il.item.is_empty() && is_goods(items, &il.item))
        .collect();
    let no_field: Vec<&(&Voucher, &InventoryLine)> = goods_lines
        .iter()
        .filter(|(_, il)| !il.qty_field_present)
        .collect();
    if let Some((v0, il0)) = no_field.first() {
        return Err(AuditError::Config(format!(
            "stock: {} goods inventory line(s) in the population carry no quantity field \
(BILLEDQTY/ACTUALQTY) at all, first {:?} on {}; the read did not carry quantities, so no quantity \
figure can be computed from it",
            no_field.len(),
            il0.item,
            voucher_label(v0)
        )));
    }
    let no_qty: Vec<&(&Voucher, &InventoryLine)> = goods_lines
        .iter()
        .filter(|(_, il)| il.qty.is_none())
        .collect();
    let ev_no_qty: Vec<EvidenceRef> = no_qty
        .iter()
        .map(|(v, _)| (v.guid.clone(), voucher_label(v)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(g, l)| EvidenceRef::with_label("voucher", &g, &l))
        .collect();
    r.fig(
        "goods_lines_without_quantity_count",
        count(TEST_ID, no_qty.len())?,
        Unit::Count,
        "Inventory lines on goods stock items, in the books (optional, cancelled and post-dated \
vouchers excluded), whose quantity is empty (Tally's value-only line): they move value but no \
quantity, so the quantity reconstruction skips them. Evidence: the vouchers carrying them.",
        ev_no_qty,
    );
    let mut no_qty_value = 0_i64;
    for (_, il) in &no_qty {
        let a = il
            .amount_paise
            .unwrap_or(0)
            .checked_abs()
            .ok_or_else(overflow)?;
        no_qty_value = add(no_qty_value, a)?;
    }
    r.fig(
        "goods_lines_without_quantity_value_paise",
        Value::Int(no_qty_value),
        Unit::Paise,
        "Total value of those lines, each taken without its sign (in and out both counted); the \
vouchers are listed on the line count above.",
        vec![],
    );

    // ---- review: typed in, books two ways, the Stock Summary two dates ----
    let stock_ledgers = book.ledgers_under_any(&[STOCK_IN_HAND_GROUP.to_string()]);
    let typed_in = pop
        .iter()
        .filter(|v| v.lines.iter().any(|l| stock_ledgers.contains(&l.ledger)))
        .count();
    let without_tb: Vec<&String> = stock_ledgers
        .iter()
        .filter(|n| !book.tb.contains_key(*n))
        .collect();
    let (mut books_open, mut books_close_tb, mut books_close_mv) = (0_i64, 0_i64, 0_i64);
    for n in &stock_ledgers {
        if let Some(t) = book.tb.get(n) {
            books_open = add(books_open, t.opening_paise)?;
            books_close_tb = add(books_close_tb, t.closing_paise)?;
            let mv = sub(add(t.opening_paise, t.debit_paise)?, t.credit_paise)?;
            books_close_mv = add(books_close_mv, mv)?;
        }
    }
    let sum_open = opening.total_value_paise()?;
    let sum_close = closing.total_value_paise()?;
    let gap_open = sub(sum_open, books_open)?;
    let gap_close = sub(sum_close, books_close_mv)?;
    let goods_close_negative: BTreeSet<&String> = closing
        .rows
        .iter()
        .filter(|(n, row)| is_goods(items, n) && row.qty.unwrap_or(0.0) < -QTY_TOL)
        .map(|(n, _)| n)
        .collect();
    let nongoods_negative_value: BTreeSet<&String> = closing
        .rows
        .iter()
        .filter(|(n, row)| {
            items.get(*n).is_some_and(|m| !m.is_goods()) && row.value_paise.unwrap_or(0) < 0
        })
        .map(|(n, _)| n)
        .collect();

    let ev_stock_ledgers: Vec<EvidenceRef> = stock_ledgers
        .iter()
        .map(|n| EvidenceRef::new("ledger", n))
        .collect();
    let f_typed = r.fig(
        "stock_in_hand_voucher_count",
        count(TEST_ID, typed_in)?,
        Unit::Count,
        "Population vouchers with at least one line on a Stock-in-Hand ledger.",
        ev_stock_ledgers.clone(),
    );
    if !without_tb.is_empty() {
        r.fig(
            "stock_in_hand_ledgers_without_tb_row_count",
            count(TEST_ID, without_tb.len())?,
            Unit::Count,
            "Stock-in-Hand ledgers with no Trial Balance row at all (opening/closing treated as nil).",
            without_tb.iter().map(|n| EvidenceRef::new("ledger", n)).collect(),
        );
    }
    if typed_in == 0 && !stock_ledgers.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/typed_in"),
            clauses: vec!["3CD-14".to_string(), "3CD-35".to_string()],
            title: "No voucher in the books posts to any Stock-in-Hand ledger".to_string(),
            facts: vec![("voucher_count".to_string(), f_typed)],
            evidence: ev_stock_ledgers.clone(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "The closing-stock figure on the Stock-in-Hand ledger(s) was entered \
directly (Tally shows no voucher deriving it); how it was arrived at is a client fact the books \
cannot show."
                    .to_string(),
            ],
            ask_client: vec![
                "Basis of stock valuation (cost/net realisable value, method used).".to_string(),
                "Signed physical stock verification / certified stock statement.".to_string(),
                "Bank drawing-power statement, if any cash credit/OD is secured on stock."
                    .to_string(),
            ],
        });
    }

    let f_books_open = r.fig(
        "books_opening_paise",
        Value::Int(books_open),
        Unit::Paise,
        "Sum of Trial Balance OPENING balances of every Stock-in-Hand ledger.",
        ev_stock_ledgers.clone(),
    );
    r.fig(
        "books_closing_tb_field_paise",
        Value::Int(books_close_tb),
        Unit::Paise,
        "Sum of Trial Balance CLOSING balances of every Stock-in-Hand ledger, Tally's own closing \
field.",
        ev_stock_ledgers.clone(),
    );
    let f_books_close_mv = r.fig(
        "books_closing_movement_paise",
        Value::Int(books_close_mv),
        Unit::Paise,
        "Sum of (opening + period debit - period credit) of every Stock-in-Hand ledger, from the \
Trial Balance's own period-movement fields.",
        ev_stock_ledgers.clone(),
    );
    r.fig(
        "books_closing_tie_diff_paise",
        Value::Int(sub(books_close_tb, books_close_mv)?),
        Unit::Paise,
        "books_closing_tb_field_paise minus books_closing_movement_paise. Non-zero shows Tally's own \
closing field on a Stock-in-Hand ledger does not reflect the year's movement.",
        vec![],
    );
    let f_sum_open = r.fig(
        "stock_summary_opening_total_paise",
        Value::Int(sum_open),
        Unit::Paise,
        "Sum of every stock item's opening CLOSINGVALUE-as-of-period-start in the Stock Summary \
(Tally's item-level valuation, independent of the books figure above).",
        vec![],
    );
    let f_sum_close = r.fig(
        "stock_summary_closing_total_paise",
        Value::Int(sum_close),
        Unit::Paise,
        "Sum of every stock item's closing value in the Stock Summary.",
        vec![],
    );
    r.fig(
        "stock_summary_closing_positive_paise",
        Value::Int(closing.positive_value_paise()?),
        Unit::Paise,
        "Sum of the Stock Summary's positive-value items at year end.",
        vec![],
    );
    r.fig(
        "stock_summary_closing_negative_paise",
        Value::Int(closing.negative_value_paise()?),
        Unit::Paise,
        "Sum of the Stock Summary's negative-value items at year end.",
        vec![],
    );
    r.fig(
        "stock_summary_opening_item_count",
        count(TEST_ID, opening.rows.len())?,
        Unit::Count,
        "Stock items present in the opening Stock Summary read.",
        vec![],
    );
    r.fig(
        "stock_summary_closing_item_count",
        count(TEST_ID, closing.rows.len())?,
        Unit::Count,
        "Stock items present in the closing Stock Summary read.",
        vec![],
    );
    let f_gap_open = r.fig(
        "gap_opening_paise",
        Value::Int(gap_open),
        Unit::Paise,
        "stock_summary_opening_total_paise minus books_opening_paise. A figure, not a conclusion \
about which side is right.",
        vec![],
    );
    let f_gap_close = r.fig(
        "gap_closing_paise",
        Value::Int(gap_close),
        Unit::Paise,
        "stock_summary_closing_total_paise minus books_closing_movement_paise. A figure, not a \
conclusion about which side is right.",
        vec![],
    );
    if gap_open != 0 || gap_close != 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/two_stock_figures"),
            clauses: vec!["3CD-14".to_string(), "3CD-35".to_string()],
            title: "Books closing stock and the Tally Stock Summary valuation differ".to_string(),
            facts: vec![
                ("books_opening".to_string(), f_books_open),
                ("books_closing_movement".to_string(), f_books_close_mv),
                ("summary_opening".to_string(), f_sum_open),
                ("summary_closing".to_string(), f_sum_close),
                ("gap_opening".to_string(), f_gap_open),
                ("gap_closing".to_string(), f_gap_close),
            ],
            evidence: ev_stock_ledgers,
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "Both figures come from Tally; which one (if either) matches an actual \
physical count is not something the books can show. With 'Integrate accounts with inventory' off, \
the accounts use the ledger-entered figure, not the Stock Summary valuation."
                    .to_string(),
            ],
            ask_client: vec![
                "Signed physical stock verification / certified stock statement for both dates."
                    .to_string(),
                "Basis of stock valuation.".to_string(),
                "Bank drawing-power statement, if any cash credit/OD is secured on stock."
                    .to_string(),
            ],
        });
    }

    // ---- non-goods items ----
    r.fig(
        "non_goods_negative_value_item_count",
        count(TEST_ID, nongoods_negative_value.len())?,
        Unit::Count,
        "Stock items with BASEUNITS == 'Not Applicable' (Tally's own marker for a value-only, \
non-physical item) carrying a negative closing value at year end. Excluded from every \
quantity-based figure below; shown separately, not silently dropped.",
        item_evidence(nongoods_negative_value.iter().copied()),
    );

    // ---- negative at year end ----
    let ev_close_neg = item_evidence(goods_close_negative.iter().copied());
    let f_close_neg = r.fig(
        "negative_at_close_count",
        count(TEST_ID, goods_close_negative.len())?,
        Unit::Count,
        "Goods stock items with a negative closing quantity in the Stock Summary at year end.",
        ev_close_neg.clone(),
    );
    if !goods_close_negative.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/negative_at_close"),
            clauses: vec!["3CD-14".to_string(), "3CD-35".to_string()],
            title: "Stock items carry a negative closing quantity at year end".to_string(),
            facts: vec![("count".to_string(), f_close_neg)],
            evidence: ev_close_neg,
            confidence: Confidence::Computed,
            limits: vec![
                "Read directly off the Stock Summary's own closing quantity; a negative \
quantity is not physically possible, so the item's record is incomplete (an opening balance, a \
purchase, or a stock-journal entry was not captured in Tally)."
                    .to_string(),
            ],
            ask_client: vec![
                "Physical stock record for each item listed, or an explanation of the shortfall."
                    .to_string(),
            ],
        });
    }

    // ---- where the running quantity starts ----
    let (opening_qty, from_summary, from_master) = opening_quantities(items, opening);
    r.fig(
        "opening_seed_from_summary_count",
        count(TEST_ID, from_summary.len())?,
        Unit::Count,
        &format!(
            "Stock items whose running quantity starts from the opening Stock Summary's own quantity \
(as at {}; a listed item with no quantity starts at nil).",
            iso(&opening.as_of)
        ),
        vec![],
    );
    r.fig(
        "opening_seed_from_master_count",
        count(TEST_ID, from_master.len())?,
        Unit::Count,
        "Stock items the opening Stock Summary does not list, started instead from the item's own \
opening quantity (the quantity when it was created: the period-start quantity only if the books \
begin at the period start).",
        item_evidence(&from_master),
    );
    let no_master: BTreeSet<&String> = from_summary
        .iter()
        .filter(|n| !items.contains_key(*n))
        .collect();
    r.fig(
        "opening_summary_items_without_master_count",
        count(TEST_ID, no_master.len())?,
        Unit::Count,
        "Stock items the opening Stock Summary lists but the stock item masters do not: still \
started from the summary's quantity, which is the period-start fact, but with no master to say \
whether each is goods or what its unit is.",
        item_evidence(no_master.iter().copied()),
    );
    let differs: BTreeSet<&String> = from_summary
        .iter()
        .filter(|n| {
            items.get(*n).is_some_and(|m| {
                let summary_qty = opening.rows[*n].qty.unwrap_or(0.0);
                (summary_qty - m.opening_qty.unwrap_or(0.0)).abs() > QTY_TOL
            })
        })
        .collect();
    r.fig(
        "opening_summary_differs_from_master_count",
        count(TEST_ID, differs.len())?,
        Unit::Count,
        "Stock items whose opening Stock Summary quantity differs from their own opening quantity: \
the items a walk started from the item's own opening would have got wrong.",
        item_evidence(differs.iter().copied()),
    );

    // ---- (a) negative at any point, including opening; (b) became negative ----
    let (in_touched, at_opening) = walk(&pop, &opening_qty, Order::InFirst);
    let (out_touched, _) = walk(&pop, &opening_qty, Order::OutFirst);
    let any_point: BTreeSet<&String> = in_touched.union(&out_touched).collect();
    let (min_a, max_a) = (
        in_touched.len().min(out_touched.len()),
        in_touched.len().max(out_touched.len()),
    );
    r.fig(
        "negative_any_point_in_first_count",
        count(TEST_ID, in_touched.len())?,
        Unit::Count,
        "(a) Items negative at some point in the year, INCLUDING an item already negative at \
opening, same-day inventory lines ordered stock-in before stock-out.",
        item_evidence(&in_touched),
    );
    r.fig(
        "negative_any_point_out_first_count",
        count(TEST_ID, out_touched.len())?,
        Unit::Count,
        "(a) Items negative at some point in the year, INCLUDING an item already negative at \
opening, same-day inventory lines ordered stock-out before stock-in.",
        item_evidence(&out_touched),
    );
    let f_range_min = r.fig(
        "negative_any_point_range_min",
        count(TEST_ID, min_a)?,
        Unit::Count,
        "(a) min(negative_any_point_in_first_count, negative_any_point_out_first_count): same-day \
order is not recoverable from the export, so this is a range, never a single count.",
        vec![],
    );
    let f_range_max = r.fig(
        "negative_any_point_range_max",
        count(TEST_ID, max_a)?,
        Unit::Count,
        "(a) max(negative_any_point_in_first_count, negative_any_point_out_first_count).",
        vec![],
    );
    let became_in: BTreeSet<&String> = in_touched.difference(&at_opening).collect();
    let became_out: BTreeSet<&String> = out_touched.difference(&at_opening).collect();
    r.fig(
        "became_negative_in_first_count",
        count(TEST_ID, became_in.len())?,
        Unit::Count,
        "(b) Items NOT negative at opening that became negative at some point during the year, \
same-day inventory lines ordered stock-in before stock-out. A subset of (a).",
        item_evidence(became_in.iter().copied()),
    );
    r.fig(
        "became_negative_out_first_count",
        count(TEST_ID, became_out.len())?,
        Unit::Count,
        "(b) Items NOT negative at opening that became negative at some point during the year, \
same-day inventory lines ordered stock-out before stock-in. A subset of (a).",
        item_evidence(became_out.iter().copied()),
    );
    let f_became_min = r.fig(
        "became_negative_range_min",
        count(TEST_ID, became_in.len().min(became_out.len()))?,
        Unit::Count,
        "(b) min(became_negative_in_first_count, became_negative_out_first_count).",
        vec![],
    );
    let f_became_max = r.fig(
        "became_negative_range_max",
        count(TEST_ID, became_in.len().max(became_out.len()))?,
        Unit::Count,
        "(b) max(became_negative_in_first_count, became_negative_out_first_count).",
        vec![],
    );
    if max_a > 0 {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/went_negative_during_year"),
            clauses: vec!["3CD-14".to_string(), "3CD-35".to_string()],
            title: "Stock items were negative at some point during the year".to_string(),
            facts: vec![
                ("any_point_range_min".to_string(), f_range_min),
                ("any_point_range_max".to_string(), f_range_max),
                ("became_negative_range_min".to_string(), f_became_min),
                ("became_negative_range_max".to_string(), f_became_max),
            ],
            evidence: item_evidence(any_point.iter().copied()),
            confidence: Confidence::Indicative,
            limits: vec![
                "Two different counts are given, never merged into one: (a) negative at \
any point, including an item already negative at opening (the wider figure); (b) became negative \
during the year, excluding an item already negative at opening (the narrower, subset figure). Both \
are a reconstruction from each item's own opening quantity walked through every FY inventory \
voucher, direction read from the ledger-entry sign each allocation is nested under (see module \
docstring); same-day voucher order is not recoverable from the export, so each is a range across \
the two possible orderings, not exact."
                    .to_string(),
            ],
            ask_client: vec![
                "Physical stock record for the items listed, for the periods they show negative."
                    .to_string(),
            ],
        });
    }

    r.findings.push(Finding {
        id: format!("{TEST_ID}/clause_35_quantitative_details"),
        clauses: vec!["3CD-35".to_string()],
        title:
            "Clause 35 quantitative details depend on stock records that the books alone cannot \
confirm"
                .to_string(),
        facts: Vec::new(),
        evidence: Vec::new(),
        confidence: Confidence::NeedsDocument,
        limits: vec![
            "Clause 35 (quantitative details of principal items) needs opening stock, \
purchases, sales, yield and closing stock by item; this engine can show the Stock Summary and \
Trial Balance figures above but cannot certify completeness of the item records, particularly \
given the typed-in / negative-stock facts above."
                .to_string(),
        ],
        ask_client: vec![
            "Quantity reconciliation or a stock register maintained outside Tally, if any."
                .to_string(),
        ],
    });
    Ok(r)
}

/// STK-1, independent of `run`: each item's period-start quantity (every master's own opening,
/// then the opening summary's over it) walked through the population's inventory lines in
/// population order must end at the closing Stock Summary's own quantity, for every closing row
/// that carries one.
pub fn check_invariants(
    book: &Book,
    _result: &TestResult,
    inputs: &StockInputs,
) -> Result<Vec<String>> {
    let mut running: BTreeMap<String, f64> = inputs
        .items
        .iter()
        .map(|(n, it)| (n.clone(), it.opening_qty.unwrap_or(0.0)))
        .collect();
    for (n, row) in &inputs.opening.rows {
        running.insert(n.clone(), row.qty.unwrap_or(0.0));
    }
    for v in book.population()? {
        for il in &v.inventory {
            if il.item.is_empty() {
                continue;
            }
            if let Some(dq) = movement(il) {
                *running.entry(il.item.clone()).or_insert(0.0) += dq;
            }
        }
    }
    let mut out = Vec::new();
    for (name, row) in &inputs.closing.rows {
        let Some(rhs) = row.qty else {
            continue; // a non-goods (or otherwise quantity-less) item: nothing to tie
        };
        let lhs = running.get(name).copied().unwrap_or(0.0);
        if (lhs - rhs).abs() > QTY_TOL {
            out.push(format!(
                "STK-1: {name} quantity reconstructed from opening + population inventory lines \
({lhs:.3}) does not equal the closing Stock Summary's own quantity ({rhs:.3}); difference {:.3}",
                lhs - rhs
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// STK-1's message formats quantities as the reference's `f"{x:.3f}"` does: measured equal on
    /// exact binary ties and signed zeros (rustc 1.96, Python 3.13, 2026-09-25); pinned here so a
    /// toolchain change cannot move the dump's text silently.
    #[test]
    fn quantities_format_as_python_fixed_three() {
        let cases = [
            (0.0625, "0.062"),
            (0.0015, "0.002"),
            (-0.0001, "-0.000"),
            (-0.0, "-0.000"),
        ];
        for (x, want) in cases {
            assert_eq!(format!("{x:.3}"), want, "{x}");
        }
    }
}
