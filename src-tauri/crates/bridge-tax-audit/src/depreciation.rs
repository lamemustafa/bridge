//! Depreciation base: Income-tax Act block WDV vs books. A port of the reference Python
//! implementation's `depreciation` test module, version 1.
//!
//! Definitions (mirrors the reference module's own docstring exactly):
//!   * A block is a rate bucket (s.32/Appendix I), not a books category: a motor car not used in
//!     a hiring business shares the SAME block as general Plant & Machinery (no separate
//!     "vehicles" block code) -- `block_by_ledger` therefore maps a Fixed Assets ledger straight
//!     to a rate-based block key (e.g. "plant_machinery_15"), never a books display category.
//!   * Additions are debit lines on a mapped asset ledger in population vouchers, dated the
//!     voucher date UNLESS `put_to_use_by_voucher` overrides it for that voucher's guid.
//!     ASSUMPTION, stated as a limit on every addition: put to use = purchase date (no separate
//!     commissioning/technical certificate is visible in Tally).
//!   * Deletions are credit lines on a mapped asset ledger, EXCLUDING the ledger's own credit
//!     lines inside a depreciation journal (a voucher is a depreciation journal iff it also
//!     carries a line on a ledger in `dep_expense_ledgers` -- identified by the sibling ledger,
//!     never by date/number heuristics).
//!   * GST input tax and TCS lines never enter an addition's cost: an addition is read off the
//!     asset ledger's OWN line only, and GST/TCS sit on separate ledger lines in the same voucher
//!     by construction; `gst_tcs_addition_lines_seen_count` confirms this is exercised on data,
//!     not merely assumed.
//!   * s.43(1), second proviso: an addition paid in cash over `rules.depreciation_cash_addition_
//!     limit_paise` is flagged (`Confidence::NeedsDocument`) and its amount is shown EXCLUDED
//!     from actual cost only as a named sensitivity figure alongside the normal (inclusive)
//!     total -- never silently subtracted from the reported block figures, because whether an
//!     exception applies is a CA judgement this engine cannot make from the books alone.
//!   * s.32(1), second proviso ("put to use ... for a period of less than one hundred and eighty
//!     days"): days used = (period end - max(put_to_use, period start)) + 1, inclusive of both
//!     ends. >= threshold -> full rate; < threshold -> half.
//!   * s.43(6) block mechanics: deletions first reduce the full-rate pool (opening + additions
//!     used >= threshold days), and only the excess ("overflow") reduces the half-rate pool
//!     (additions used < threshold days); neither pool is let go negative.
//!   * Book depreciation for the book-vs-Act finding is read from the Trial Balance movement of
//!     `dep_expense_ledgers` (never summed from vouchers) -- a separate tie figure cross-checks
//!     that figure against the sum of what was actually credited off the asset ledgers in the
//!     population's depreciation-journal vouchers, so a mismatch is visible rather than assumed
//!     away.
//!   * A Fixed Assets ledger (Tally primary group) with any TB closing balance, TB movement, or
//!     FY voucher activity that is not in `block_by_ledger` makes the whole book-vs-Act total
//!     unsafe: `run` fails loud (a `JudgementRequired` finding, no total figures) rather than
//!     silently omitting it.
//!   * Nothing mapped at all (no `block_by_ledger`, no `opening_wdv_paise`, no Fixed Assets ledger
//!     with a balance or movement) is not a computed nil: the book-vs-Act finding is
//!     `JudgementRequired`, titled "Depreciation not computed: ...", and cites only a non-zero book
//!     charge. Otherwise it is titled "Book vs Income-tax Act depreciation", never asserting a
//!     difference the figures may not show.
//!
//! `check_invariants` (DEP-1, DEP-2) is independent of [`compute_depreciation`]/[`run`]: it never
//! calls them, and DEP-1 re-derives each Fixed Assets ledger's own movement purely from the book
//! (population vouchers + `dep_expense_ledgers`, the latter read back from a figure's evidence,
//! never recomputed) and compares it to the Trial Balance's OWN closing balance, read directly --
//! the tautology guard: the right-hand side can never be the same computation as the left.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;
use sha2::Sha256;

use crate::book::{Book, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::{iso, Window};
use crate::rules::Rules;

pub const TEST_ID: &str = "depreciation";
pub const VERSION: &str = "1";

const FIXED_ASSETS_GROUP: &str = "Fixed Assets";
const CASH_GROUP: &str = "Cash-in-Hand";

const POPULATION: &str = "Books population (optional, cancelled and post-dated vouchers \
excluded). Put to use assumed = purchase/voucher date unless put_to_use_by_voucher overrides.";

fn overflow() -> AuditError {
    AuditError::Config("depreciation: a total overflowed i64 paise".to_string())
}

/// The reference implementation's `_hash` used inline for a voucher GUID at 12 hex characters
/// (same convention as `cash_payments_40a3::hash12_sha256`).
fn hash12_sha256(text: &str) -> String {
    use sha2::Digest;
    crate::canonical::hex(&Sha256::digest(text.as_bytes()))[..12].to_string()
}

fn guid_tail12(guid: &str) -> &str {
    let cut = guid.len().saturating_sub(12);
    &guid[cut..]
}

fn voucher_label(v: &Voucher) -> String {
    let num = if v.number.is_empty() {
        guid_tail12(&v.guid)
    } else {
        v.number.as_str()
    };
    format!("{} {} on {}", v.vtype, num, iso(&v.date))
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `word` occurs in `haystack_upper` as a whole "word" (flanked by non-word characters
/// or the string's edges) -- an ASCII approximation of the reference regex's `\bWORD\b`, matching
/// the same manual-scan convention `cash_payments_40a3::transport_name_match` uses instead of
/// adding a regex dependency.
fn contains_word(haystack_upper: &str, word: &str) -> bool {
    let chars: Vec<char> = haystack_upper.chars().collect();
    let wchars: Vec<char> = word.chars().collect();
    let (n, wn) = (chars.len(), wchars.len());
    if wn == 0 || wn > n {
        return false;
    }
    for start in 0..=(n - wn) {
        if chars[start..start + wn] == wchars[..] {
            let before_ok = start == 0 || !is_word_char(chars[start - 1]);
            let after_ok = start + wn == n || !is_word_char(chars[start + wn]);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// Heuristic-only, used solely to confirm on data (never to compute a figure) that GST/TCS lines
/// sit on a different ledger than the asset line they accompany: the reference engine's own regex
/// `\bCGST\b|\bSGST\b|\bIGST\b|\bGST\b|\bTCS\b`, case-insensitive.
fn gst_tcs_match(name: &str) -> bool {
    let upper = name.to_uppercase();
    ["CGST", "SGST", "IGST", "GST", "TCS"]
        .iter()
        .any(|w| contains_word(&upper, w))
}

/// Proleptic-Gregorian day number (days since 1970-01-01) for a [`TallyDate`], Howard Hinnant's
/// `days_from_civil` algorithm. `TallyDate::parse` already established the date is a valid
/// Gregorian calendar date, so the digit parses here cannot fail.
fn civil_day_number(date: &TallyDate) -> i64 {
    let s = date.as_str();
    let y: i64 = s[0..4].parse().unwrap_or(0);
    let m: i64 = s[4..6].parse().unwrap_or(1);
    let d: i64 = s[6..8].parse().unwrap_or(1);
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inclusive day-count from put-to-use (or period start if earlier) through period end. s.32(1)
/// second proviso: half rate applies when the asset was "put to use ... for a period of less than
/// one hundred and eighty days" in the previous year -- so a result of exactly the threshold (180
/// by the vendored rules) is NOT "less than" it and gets the FULL rate.
fn days_used(put_to_use: &TallyDate, period: &Window) -> i64 {
    let effective = if put_to_use < &period.from {
        &period.from
    } else {
        put_to_use
    };
    civil_day_number(&period.to) - civil_day_number(effective) + 1
}

/// `base_paise * rate_bp / denom_bp`, rounded half-up, floored at 0. `denom_bp` is 10_000 for the
/// full rate and 20_000 for the half rate (`rate_bp` already expresses the full-year rate).
fn rate_amount(base_paise: i64, rate_bp: i64, denom_bp: i64) -> i64 {
    if base_paise <= 0 || rate_bp <= 0 {
        return 0;
    }
    let num = i128::from(base_paise) * i128::from(rate_bp);
    let denom = i128::from(denom_bp);
    ((num + denom / 2) / denom) as i64
}

/// s.43(6): a deletion first reduces the full-rate pool; only the excess reduces the half-rate
/// pool, and neither pool is let go negative.
fn basis(opening: i64, add_ge: i64, add_lt: i64, deletions: i64) -> Result<(i64, i64)> {
    let full = i128::from(opening) + i128::from(add_ge) - i128::from(deletions);
    let full = full.max(0);
    let block_overflow = (i128::from(deletions) - i128::from(opening) - i128::from(add_ge)).max(0);
    let half = (i128::from(add_lt) - block_overflow).max(0);
    Ok((
        i64::try_from(full).map_err(|_| overflow())?,
        i64::try_from(half).map_err(|_| overflow())?,
    ))
}

struct Addition<'a> {
    voucher: &'a Voucher,
    put_to_use: TallyDate,
    amount_paise: i64,
    cash_paise: i64,
    cash_reason: Option<&'static str>,
}

// A deletion's voucher is never read after the walk (matches the reference module: its own
// `deletions` dict carries `voucher` too, but only `amount_paise` is ever consumed downstream),
// so a deletion is kept as a plain amount rather than a struct with a dead field.

pub struct CashRow<'a> {
    pub voucher: &'a Voucher,
    pub ledger: String,
    pub amount_paise: i64,
    pub cash_paise: i64,
    pub cash_reason: &'static str,
}

pub struct Sensitivity {
    pub full_basis_paise: i64,
    pub half_basis_paise: i64,
    pub dep_total_paise: i64,
}

pub struct BlockResult<'a> {
    pub rate_bp: i64,
    pub ledgers: Vec<String>,
    pub opening_paise: i64,
    pub additions_ge180_paise: i64,
    pub additions_lt180_paise: i64,
    pub deletions_paise: i64,
    pub full_basis_paise: i64,
    pub half_basis_paise: i64,
    pub dep_total_paise: i64,
    pub closing_paise: i64,
    pub book_dep_paise: i64,
    pub cash_rows: Vec<CashRow<'a>>,
    pub sensitivity_excl_cash: Option<Sensitivity>,
}

pub struct DepreciationData<'a> {
    pub blocks: BTreeMap<String, BlockResult<'a>>,
    pub unmapped: Vec<String>,
    pub gst_tcs_lines_seen: i64,
}

/// How much of one addition looks cash-paid, and why. Two routes, checked in order: (a) a cash
/// ledger is credited in the SAME voucher (the classic case: Purchase against Cash); (b) no cash
/// line here, but this voucher credits a counterparty ledger (e.g. Sundry Creditors) that some
/// OTHER population voucher on the SAME date both debits (reduces the payable) and pays out of a
/// cash ledger -- the supplier was in substance paid in cash that day. Route (b)'s total can
/// double-count if one same-day cash payment to that counterparty covers several invoices --
/// named as a limit on the Finding, never hidden, same convention as `cash_payments_40a3`'s
/// lump-entry caveat.
fn cash_exposure<'a>(
    v: &'a Voucher,
    asset_ledger: &str,
    cash: &BTreeSet<String>,
    pop_by_date: &BTreeMap<TallyDate, Vec<&'a Voucher>>,
) -> Result<(i64, Option<&'static str>)> {
    let mut same_voucher_cash: i64 = 0;
    for l in &v.lines {
        if cash.contains(&l.ledger) && l.amount_paise < 0 {
            same_voucher_cash = same_voucher_cash
                .checked_sub(l.amount_paise)
                .ok_or_else(overflow)?;
        }
    }
    if same_voucher_cash > 0 {
        return Ok((
            same_voucher_cash,
            Some("cash ledger credited in the same voucher"),
        ));
    }
    let counterparties: BTreeSet<&str> = v
        .lines
        .iter()
        .filter(|l| l.ledger != asset_ledger && l.amount_paise < 0 && !cash.contains(&l.ledger))
        .map(|l| l.ledger.as_str())
        .collect();
    if counterparties.is_empty() {
        return Ok((0, None));
    }
    let mut total: i64 = 0;
    if let Some(others) = pop_by_date.get(&v.date) {
        for &other in others {
            if other.guid == v.guid {
                continue;
            }
            let mut other_cash: i64 = 0;
            for l in &other.lines {
                if cash.contains(&l.ledger) && l.amount_paise < 0 {
                    other_cash = other_cash
                        .checked_sub(l.amount_paise)
                        .ok_or_else(overflow)?;
                }
            }
            if other_cash <= 0 {
                continue;
            }
            let other_debited: BTreeSet<&str> = other
                .lines
                .iter()
                .filter(|l| l.amount_paise > 0)
                .map(|l| l.ledger.as_str())
                .collect();
            if counterparties.iter().any(|c| other_debited.contains(c)) {
                total = total.checked_add(other_cash).ok_or_else(overflow)?;
            }
        }
    }
    if total > 0 {
        Ok((total, Some("same-day cash payment to the supplier ledger")))
    } else {
        Ok((0, None))
    }
}

/// The pure computation, kept separate and importable (like the reference module's own
/// `compute_depreciation`) so unit tests can exercise the block mechanics without going through
/// [`run`]'s figure/finding rendering.
#[allow(clippy::too_many_arguments)]
pub fn compute_depreciation<'a>(
    book: &'a Book,
    period: &Window,
    rules: &Rules,
    block_by_ledger: &BTreeMap<String, String>,
    opening_wdv_paise: &BTreeMap<String, i64>,
    dep_expense_ledgers: &BTreeSet<String>,
    put_to_use_by_voucher: &BTreeMap<String, TallyDate>,
) -> Result<DepreciationData<'a>> {
    let pop = book.population()?;
    let half_rate_days = rules.depreciation_half_rate_days_threshold;
    let cash_limit = rules.depreciation_cash_addition_limit_paise;
    let cash = book.ledgers_under_any(&[CASH_GROUP.to_string()]);
    let fa_ledgers = book.ledgers_under_any(&[FIXED_ASSETS_GROUP.to_string()]);

    let dep_journal_guids: BTreeSet<&str> = pop
        .iter()
        .filter(|v| {
            v.lines
                .iter()
                .any(|l| dep_expense_ledgers.contains(&l.ledger))
        })
        .map(|v| v.guid.as_str())
        .collect();

    let mut pop_by_date: BTreeMap<TallyDate, Vec<&Voucher>> = BTreeMap::new();
    for &v in &pop {
        pop_by_date.entry(v.date.clone()).or_default().push(v);
    }

    let mut additions: BTreeMap<String, Vec<Addition>> = BTreeMap::new();
    let mut deletions: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut dep_credited: BTreeMap<String, i64> = BTreeMap::new();
    let mut gst_tcs_lines_seen: i64 = 0;

    for &v in &pop {
        let is_dep_journal = dep_journal_guids.contains(v.guid.as_str());
        for l in &v.lines {
            if !block_by_ledger.contains_key(&l.ledger) {
                continue;
            }
            if l.amount_paise > 0 {
                let put_to_use = put_to_use_by_voucher
                    .get(&v.guid)
                    .cloned()
                    .unwrap_or_else(|| v.date.clone());
                let (cash_paise, cash_reason) = cash_exposure(v, &l.ledger, &cash, &pop_by_date)?;
                additions
                    .entry(l.ledger.clone())
                    .or_default()
                    .push(Addition {
                        voucher: v,
                        put_to_use,
                        amount_paise: l.amount_paise,
                        cash_paise,
                        cash_reason,
                    });
                let count = v
                    .lines
                    .iter()
                    .filter(|ol| ol.ledger != l.ledger && gst_tcs_match(&ol.ledger))
                    .count();
                gst_tcs_lines_seen = gst_tcs_lines_seen
                    .checked_add(count as i64)
                    .ok_or_else(overflow)?;
            } else if l.amount_paise < 0 {
                if is_dep_journal {
                    let entry = dep_credited.entry(l.ledger.clone()).or_insert(0);
                    *entry = entry.checked_sub(l.amount_paise).ok_or_else(overflow)?;
                } else {
                    deletions
                        .entry(l.ledger.clone())
                        .or_default()
                        .push(-l.amount_paise);
                }
            }
        }
    }

    let mut unmapped = Vec::new();
    for name in &fa_ledgers {
        if block_by_ledger.contains_key(name) {
            continue;
        }
        let tb_row = book.tb.get(name);
        let closing = tb_row.map_or(0, |r| r.closing_paise);
        let movement = tb_row.map_or(0, crate::book::TbRow::movement_paise);
        let has_activity = additions.contains_key(name)
            || deletions.contains_key(name)
            || dep_credited.get(name).is_some_and(|&p| p != 0);
        if closing != 0 || movement != 0 || has_activity {
            unmapped.push(name.clone());
        }
    }

    let mut blocks: BTreeMap<String, BlockResult> = BTreeMap::new();
    if unmapped.is_empty() {
        for (block_key, &opening) in opening_wdv_paise {
            let rate_bp = *rules
                .depreciation_block_rate_bp
                .get(block_key)
                .ok_or_else(|| {
                    AuditError::Config(format!(
                        "depreciation: opening_wdv_paise configures block {block_key:?} but \
rules.depreciation.blocks has no such block"
                    ))
                })?;
            let ledgers_in_block: Vec<String> = block_by_ledger
                .iter()
                .filter(|(name, b)| b.as_str() == block_key && fa_ledgers.contains(*name))
                .map(|(name, _)| name.clone())
                .collect();

            let (mut add_ge180, mut add_lt180) = (0i64, 0i64);
            let (mut cash_ge180, mut cash_lt180) = (0i64, 0i64);
            let mut block_deletions = 0i64;
            let mut cash_rows = Vec::new();

            for name in &ledgers_in_block {
                if let Some(adds) = additions.get(name) {
                    for a in adds {
                        let du = days_used(&a.put_to_use, period);
                        let ge180 = du >= half_rate_days;
                        if ge180 {
                            add_ge180 =
                                add_ge180.checked_add(a.amount_paise).ok_or_else(overflow)?;
                        } else {
                            add_lt180 =
                                add_lt180.checked_add(a.amount_paise).ok_or_else(overflow)?;
                        }
                        if a.cash_paise > cash_limit {
                            cash_rows.push(CashRow {
                                voucher: a.voucher,
                                ledger: name.clone(),
                                amount_paise: a.amount_paise,
                                cash_paise: a.cash_paise,
                                cash_reason: a.cash_reason.unwrap_or(""),
                            });
                            if ge180 {
                                cash_ge180 = cash_ge180
                                    .checked_add(a.amount_paise)
                                    .ok_or_else(overflow)?;
                            } else {
                                cash_lt180 = cash_lt180
                                    .checked_add(a.amount_paise)
                                    .ok_or_else(overflow)?;
                            }
                        }
                    }
                }
                if let Some(dels) = deletions.get(name) {
                    for &d in dels {
                        block_deletions = block_deletions.checked_add(d).ok_or_else(overflow)?;
                    }
                }
            }

            let (full_basis, half_basis) = basis(opening, add_ge180, add_lt180, block_deletions)?;
            let dep_full = rate_amount(full_basis, rate_bp, 10_000);
            let dep_half = rate_amount(half_basis, rate_bp, 20_000);
            let dep_total = dep_full.checked_add(dep_half).ok_or_else(overflow)?;
            let closing = full_basis
                .checked_add(half_basis)
                .and_then(|s| s.checked_sub(dep_total))
                .ok_or_else(overflow)?;

            let sensitivity = if cash_ge180 != 0 || cash_lt180 != 0 {
                let (s_full, s_half) = basis(
                    opening,
                    add_ge180.checked_sub(cash_ge180).ok_or_else(overflow)?,
                    add_lt180.checked_sub(cash_lt180).ok_or_else(overflow)?,
                    block_deletions,
                )?;
                let s_dep_full = rate_amount(s_full, rate_bp, 10_000);
                let s_dep_half = rate_amount(s_half, rate_bp, 20_000);
                let s_dep_total = s_dep_full.checked_add(s_dep_half).ok_or_else(overflow)?;
                Some(Sensitivity {
                    full_basis_paise: s_full,
                    half_basis_paise: s_half,
                    dep_total_paise: s_dep_total,
                })
            } else {
                None
            };

            let mut book_dep = 0i64;
            for name in &ledgers_in_block {
                book_dep = book_dep
                    .checked_add(dep_credited.get(name).copied().unwrap_or(0))
                    .ok_or_else(overflow)?;
            }

            blocks.insert(
                block_key.clone(),
                BlockResult {
                    rate_bp,
                    ledgers: ledgers_in_block,
                    opening_paise: opening,
                    additions_ge180_paise: add_ge180,
                    additions_lt180_paise: add_lt180,
                    deletions_paise: block_deletions,
                    full_basis_paise: full_basis,
                    half_basis_paise: half_basis,
                    dep_total_paise: dep_total,
                    closing_paise: closing,
                    book_dep_paise: book_dep,
                    cash_rows,
                    sensitivity_excl_cash: sensitivity,
                },
            );
        }
    }

    Ok(DepreciationData {
        blocks,
        unmapped,
        gst_tcs_lines_seen,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    block_by_ledger: &BTreeMap<String, String>,
    opening_wdv_paise: &BTreeMap<String, i64>,
    dep_expense_ledgers: &BTreeSet<String>,
    put_to_use_by_voucher: &BTreeMap<String, TallyDate>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = POPULATION.to_string();

    let data = compute_depreciation(
        book,
        period,
        rules,
        block_by_ledger,
        opening_wdv_paise,
        dep_expense_ledgers,
        put_to_use_by_voucher,
    )?;
    let fa_ledgers = book.ledgers_under_any(&[FIXED_ASSETS_GROUP.to_string()]);

    r.fig(
        "gst_tcs_addition_lines_seen_count",
        Value::Int(data.gst_tcs_lines_seen),
        Unit::Count,
        "GST input tax / TCS ledger lines observed alongside a fixed-asset addition line in the \
same voucher, confirmed to be separate ledger lines never summed into the asset ledger's own \
debit amount -- verified on data, not merely asserted.",
        Vec::new(),
    );

    for name in &fa_ledgers {
        let h = stable_ledger_tag(book, name)?;
        let block_val = block_by_ledger
            .get(name)
            .cloned()
            .unwrap_or_else(|| "unmapped".to_string());
        r.fig(
            &format!("ledger_block_{h}"),
            Value::Text(block_val),
            Unit::Text,
            &format!(
                "Depreciation block this Fixed Assets ledger (tag {h}) is mapped to, or 'unmapped'."
            ),
            vec![EvidenceRef::new("ledger", name)],
        );
    }

    r.fig(
        "dep_expense_ledgers_count",
        Value::Int(dep_expense_ledgers.len() as i64),
        Unit::Count,
        "Ledgers whose debit is the client's own book depreciation charge (the other side of the \
voucher that credits each asset ledger for its year's book depreciation).",
        dep_expense_ledgers
            .iter()
            .map(|n| EvidenceRef::new("ledger", n))
            .collect(),
    );

    if !data.unmapped.is_empty() {
        let ev: Vec<EvidenceRef> = data
            .unmapped
            .iter()
            .map(|n| EvidenceRef::new("ledger", n))
            .collect();
        let f_unmapped = r.fig(
            "unmapped_fixed_asset_ledger_count",
            Value::Int(data.unmapped.len() as i64),
            Unit::Count,
            "Fixed Assets ledgers with a non-zero Trial Balance closing balance, TB movement, or \
FY voucher activity that are not in block_by_ledger.",
            ev.clone(),
        );
        r.findings.push(Finding {
            id: format!("{TEST_ID}/unmapped"),
            clauses: vec!["s.32".to_string(), "3CD-18".to_string()],
            title: "Fixed Assets ledger(s) with account movement are not mapped to any \
depreciation block"
                .to_string(),
            facts: vec![("unmapped_count".to_string(), f_unmapped)],
            evidence: ev,
            confidence: Confidence::JudgementRequired,
            limits: vec![
                "Block-wise depreciation cannot be safely totalled while any Fixed Assets ledger \
with movement is unmapped; no book-vs-Act total is produced until every such ledger is \
classified into a block (see block_by_ledger)."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm which depreciation block each unmapped ledger belongs to.".to_string(),
            ],
        });
        return Ok(r); // fail loud: no totals while classification is incomplete
    }

    let mut book_dep_from_vouchers = 0i64;
    let mut act_dep_total = 0i64;
    let mut act_dep_total_excl_cash = 0i64;
    let mut book_dep_from_tb = 0i64;
    for n in dep_expense_ledgers {
        if let Some(row) = book.tb.get(n) {
            book_dep_from_tb = book_dep_from_tb
                .checked_add(row.movement_paise())
                .ok_or_else(overflow)?;
        }
    }
    let mut diff_facts: Vec<(String, String)> = Vec::new();

    for (block_key, b) in &data.blocks {
        let ev: Vec<EvidenceRef> = b
            .ledgers
            .iter()
            .map(|n| EvidenceRef::new("ledger", n))
            .collect();
        r.fig(
            &format!("opening_wdv_{block_key}"),
            Value::Int(b.opening_paise),
            Unit::Paise,
            &format!("Opening (1-4) Income-tax Act WDV for block '{block_key}'."),
            ev.clone(),
        );
        r.fig(
            &format!("additions_ge180_{block_key}"),
            Value::Int(b.additions_ge180_paise),
            Unit::Paise,
            &format!(
                "Additions to block '{block_key}' put to use >= {} days before period end (full \
rate).",
                rules.depreciation_half_rate_days_threshold
            ),
            ev.clone(),
        );
        r.fig(
            &format!("additions_lt180_{block_key}"),
            Value::Int(b.additions_lt180_paise),
            Unit::Paise,
            &format!(
                "Additions to block '{block_key}' put to use < {} days before period end (half \
rate, s.32(1) second proviso).",
                rules.depreciation_half_rate_days_threshold
            ),
            ev.clone(),
        );
        r.fig(
            &format!("deletions_{block_key}"),
            Value::Int(b.deletions_paise),
            Unit::Paise,
            &format!(
                "Deletions from block '{block_key}' (credit lines on its asset ledgers outside a \
depreciation journal)."
            ),
            ev.clone(),
        );
        let f_act = r.fig(
            &format!("dep_total_act_{block_key}"),
            Value::Int(b.dep_total_paise),
            Unit::Paise,
            &format!(
                "Income-tax Act depreciation for block '{block_key}': full rate on (opening + >= \
threshold additions - deletions, floor 0), half rate on < threshold additions net of any \
deletion spill (s.43(6))."
            ),
            ev.clone(),
        );
        r.fig(
            &format!("closing_wdv_{block_key}"),
            Value::Int(b.closing_paise),
            Unit::Paise,
            &format!("Closing Income-tax Act WDV for block '{block_key}'."),
            ev.clone(),
        );
        let f_book_dep = r.fig(
            &format!("book_dep_{block_key}"),
            Value::Int(b.book_dep_paise),
            Unit::Paise,
            &format!(
                "Book depreciation credited to block '{block_key}''s asset ledgers in \
depreciation-journal vouchers (voucher-level; cross-checked against the Trial Balance total by \
book_dep_tie_diff_paise below)."
            ),
            ev.clone(),
        );
        let diff_val = b
            .book_dep_paise
            .checked_sub(b.dep_total_paise)
            .ok_or_else(overflow)?;
        let f_diff = r.fig(
            &format!("book_vs_act_{block_key}"),
            Value::Int(diff_val),
            Unit::Paise,
            &format!(
                "Book depreciation minus Income-tax Act depreciation for block '{block_key}'."
            ),
            Vec::new(),
        );

        book_dep_from_vouchers = book_dep_from_vouchers
            .checked_add(b.book_dep_paise)
            .ok_or_else(overflow)?;
        act_dep_total = act_dep_total
            .checked_add(b.dep_total_paise)
            .ok_or_else(overflow)?;
        diff_facts.push((format!("book_{block_key}"), f_book_dep));
        diff_facts.push((format!("act_{block_key}"), f_act));
        diff_facts.push((format!("difference_{block_key}"), f_diff));

        match &b.sensitivity_excl_cash {
            Some(sens) => {
                act_dep_total_excl_cash = act_dep_total_excl_cash
                    .checked_add(sens.dep_total_paise)
                    .ok_or_else(overflow)?;
                r.fig(
                    &format!("dep_total_act_sensitivity_excl_cash_{block_key}"),
                    Value::Int(sens.dep_total_paise),
                    Unit::Paise,
                    &format!(
                        "Income-tax Act depreciation for block '{block_key}' applying the \
s.43(1) second proviso to every addition the books show as paid in cash (excluded from actual \
cost; the proviso is mandatory and has no Rule 6DD-style exception). This is the Act figure \
unless the payment mode is shown not to be cash."
                    ),
                    ev.clone(),
                );
            }
            None => {
                act_dep_total_excl_cash = act_dep_total_excl_cash
                    .checked_add(b.dep_total_paise)
                    .ok_or_else(overflow)?;
            }
        }

        for row in &b.cash_rows {
            let v = row.voucher;
            let h = stable_ledger_tag(book, &row.ledger)?;
            let rid = format!("{}_{h}", hash12_sha256(&v.guid));
            let f_amt = r.fig(
                &format!("cash_flagged_addition_{rid}"),
                Value::Int(row.amount_paise),
                Unit::Paise,
                &format!(
                    "Addition to a Fixed Assets ledger (tag {h}) on {} where the payment side \
shows cash of {} paise ({}), over rules_dep.cash_addition_limit_paise.",
                    iso(&v.date),
                    row.cash_paise,
                    row.cash_reason
                ),
                vec![EvidenceRef::with_label(
                    "voucher",
                    &v.guid,
                    &voucher_label(v),
                )],
            );
            r.findings.push(Finding {
                id: format!("{TEST_ID}/s43_1/{rid}"),
                clauses: vec!["s.43(1) second proviso".to_string(), "3CD-18".to_string()],
                title: "Fixed-asset addition paid in cash over the s.43(1) second proviso limit"
                    .to_string(),
                facts: vec![("amount".to_string(), f_amt)],
                evidence: vec![EvidenceRef::with_label(
                    "voucher",
                    &v.guid,
                    &voucher_label(v),
                )],
                confidence: Confidence::NeedsDocument,
                limits: vec![
                    "Books only: the second proviso excludes such payment from actual cost \
outright, with no Rule 6DD-style exception; confirm no part of the payment was by account \
payee cheque/draft or ECS before treating the whole addition as cash-paid."
                        .to_string(),
                    "If flagged only via a same-day cash payment to the supplier ledger (not a \
cash line on this voucher itself), that same-day total may cover more than this one invoice; \
confirm before excluding."
                        .to_string(),
                    "Both figures are shown: depreciation under the Act applying the proviso \
(the Act figure if the books' record of cash payment is right), and depreciation including this \
addition (the Act figure only if the payment is shown not to have been in cash)."
                        .to_string(),
                ],
                ask_client: vec![
                    "Confirm the mode of payment for this addition in full.".to_string(),
                    "Confirm whether any exception to the second proviso applies.".to_string(),
                ],
            });
        }
    }

    let f_book_total = r.fig(
        "book_dep_total",
        Value::Int(book_dep_from_tb),
        Unit::Paise,
        "Book depreciation for the year: Trial Balance movement of dep_expense_ledgers (never \
summed from vouchers).",
        dep_expense_ledgers
            .iter()
            .map(|n| EvidenceRef::new("ledger", n))
            .collect(),
    );
    let f_act_total = r.fig(
        "act_dep_total",
        Value::Int(act_dep_total),
        Unit::Paise,
        "Total Income-tax Act depreciation across every mapped block, INCLUDING additions the \
books show as paid in cash; the Act figure only if that payment mode is shown not to be cash.",
        Vec::new(),
    );
    r.fig(
        "act_dep_total_applying_s43_1_proviso",
        Value::Int(act_dep_total_excl_cash),
        Unit::Paise,
        "Total Income-tax Act depreciation applying the s.43(1) second proviso to every addition \
the books show as paid in cash: the Act figure on the books as recorded. Equal to act_dep_total \
when none is flagged.",
        Vec::new(),
    );
    let diff_total = book_dep_from_tb
        .checked_sub(act_dep_total)
        .ok_or_else(overflow)?;
    let f_diff_total = r.fig(
        "book_vs_act_difference_total",
        Value::Int(diff_total),
        Unit::Paise,
        "book_dep_total minus act_dep_total.",
        Vec::new(),
    );
    let tie_diff = book_dep_from_vouchers
        .checked_sub(book_dep_from_tb)
        .ok_or_else(overflow)?;
    r.fig(
        "book_dep_tie_diff_paise",
        Value::Int(tie_diff),
        Unit::Paise,
        "Tie check: sum of book depreciation credited to asset ledgers in depreciation-journal \
vouchers, minus the Trial Balance movement of dep_expense_ledgers. Non-zero means the \
depreciation journals do not fully explain the expense ledger's TB movement.",
        Vec::new(),
    );

    // Nothing to compute: no block is mapped, no opening WDV is configured, and no Fixed Assets
    // ledger carries a balance or movement. Act depreciation is then zero by construction, not a
    // computed nil -- a depreciable asset recorded outside the Fixed Assets group (or never
    // recorded) is invisible here -- so the finding is a question for the CA, never `Computed`.
    // Without this, the "every ledger mapped" check below passes vacuously on an empty set of Fixed
    // Assets ledgers. A configured opening WDV is a real block (its Act depreciation is computed and
    // filed in clause 18), so it never takes this branch.
    let any_fa_with_balance = fa_ledgers.iter().any(|n| {
        book.tb
            .get(n)
            .is_some_and(|row| row.closing_paise != 0 || row.movement_paise() != 0)
    });
    if block_by_ledger.is_empty() && opening_wdv_paise.is_empty() && !any_fa_with_balance {
        let charged = book_dep_from_tb != 0;
        let mut limits = vec![
            "No ledger under the Fixed Assets group carries a balance or movement, and no \
depreciation block or opening written-down value is configured, so Income-tax Act depreciation is \
not computed: an asset recorded outside the Fixed Assets group, or not recorded at all, is not \
tested."
                .to_string(),
        ];
        if charged {
            limits.push(
                "Depreciation is charged in the books although no asset ledger is mapped; the \
book total above has no asset behind it in this test."
                    .to_string(),
            );
        }
        r.findings.push(Finding {
            id: format!("{TEST_ID}/book_vs_act"),
            clauses: vec!["s.32".to_string(), "3CD-18".to_string()],
            title: "Depreciation not computed: no fixed-asset ledger is mapped to a depreciation \
block"
                .to_string(),
            facts: if charged {
                vec![("book_total".to_string(), f_book_total)]
            } else {
                Vec::new()
            },
            evidence: Vec::new(),
            confidence: Confidence::JudgementRequired,
            limits,
            ask_client: vec![
                "Confirm whether the business holds any depreciable asset, including one recorded \
outside the Fixed Assets group, and if so its block, opening written-down value, and additions and \
deletions in the year."
                    .to_string(),
            ],
        });
        return Ok(r);
    }

    let every_ledger_mapped = fa_ledgers.iter().all(|n| block_by_ledger.contains_key(n));
    let mut facts = vec![
        ("book_total".to_string(), f_book_total),
        ("act_total".to_string(), f_act_total),
        ("difference_total".to_string(), f_diff_total),
    ];
    facts.extend(diff_facts);
    r.findings.push(Finding {
        id: format!("{TEST_ID}/book_vs_act"),
        clauses: vec!["s.32".to_string(), "3CD-18".to_string()],
        title: "Book vs Income-tax Act depreciation".to_string(),
        facts,
        evidence: Vec::new(),
        confidence: if every_ledger_mapped {
            Confidence::Computed
        } else {
            Confidence::JudgementRequired
        },
        limits: vec![
            "Put-to-use date is assumed = purchase/voucher date for every addition not \
overridden in put_to_use_by_voucher; no separate commissioning/technical-person certificate is \
visible in Tally."
                .to_string(),
            "Opening WDV per block is a supplied constant (opening_wdv_paise), not re-derived \
from an AY 2025-26 tax computation the books do not carry; confirm it against the filed \
return/Form 3CD Clause 18(f) for the prior year."
                .to_string(),
        ],
        ask_client: Vec::new(),
    });
    Ok(r)
}

/// DEP-1 and DEP-2, independent of [`compute_depreciation`]/[`run`] (see the module docstring for
/// why DEP-1 reads the Trial Balance directly rather than re-deriving both sides the same way).
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let prefix = format!("{}.", result.test_id);

    let dep_fid = format!("{prefix}dep_expense_ledgers_count");
    let dep_expense_ledgers: BTreeSet<String> = result
        .figures
        .iter()
        .find(|f| f.id == dep_fid)
        .map(|f| f.evidence.iter().map(|e| e.id.clone()).collect())
        .unwrap_or_default();

    let fa_ledgers = book.ledgers_under_any(&[FIXED_ASSETS_GROUP.to_string()]);
    let pop = book.population()?;
    let dep_journal_guids: BTreeSet<&str> = pop
        .iter()
        .filter(|v| {
            v.lines
                .iter()
                .any(|l| dep_expense_ledgers.contains(&l.ledger))
        })
        .map(|v| v.guid.as_str())
        .collect();

    #[derive(Default, Clone, Copy)]
    struct Movement {
        additions: i64,
        deletions: i64,
        dep_credited: i64,
    }
    let mut movement: BTreeMap<&str, Movement> = fa_ledgers
        .iter()
        .map(|n| (n.as_str(), Movement::default()))
        .collect();

    for v in &pop {
        let is_dep_journal = dep_journal_guids.contains(v.guid.as_str());
        for l in &v.lines {
            let Some(m) = movement.get_mut(l.ledger.as_str()) else {
                continue;
            };
            if l.amount_paise > 0 {
                m.additions = m.additions.saturating_add(l.amount_paise);
            } else if l.amount_paise < 0 {
                if is_dep_journal {
                    m.dep_credited = m.dep_credited.saturating_sub(l.amount_paise);
                } else {
                    m.deletions = m.deletions.saturating_sub(l.amount_paise);
                }
            }
        }
    }

    for name in &fa_ledgers {
        let tb_row = book.tb.get(name);
        let opening = tb_row.map_or(0, |r| r.opening_paise);
        let closing_rhs = tb_row.map_or(0, |r| r.closing_paise);
        let m = movement[name.as_str()];
        let lhs = opening + m.additions - m.deletions - m.dep_credited;
        if lhs != closing_rhs {
            out.push(format!(
                "DEP-1: {name} opening + additions - deletions - depreciation credited ({lhs}p) \
does not equal the TB closing balance ({closing_rhs}p); difference {}p",
                lhs - closing_rhs
            ));
        }

        let h = stable_ledger_tag(book, name)?;
        let block_fid = format!("{prefix}ledger_block_{h}");
        let has_movement_for_dep2 = m.additions != 0 || m.deletions != 0 || m.dep_credited != 0;
        if closing_rhs != 0 || has_movement_for_dep2 {
            match result.figures.iter().find(|f| f.id == block_fid) {
                None => out.push(format!(
                    "DEP-2: {name} has TB closing {closing_rhs}p or FY movement but no \
ledger_block figure was published at all"
                )),
                Some(fig) => {
                    if matches!(&fig.value, Value::Text(s) if s == "unmapped") {
                        out.push(format!(
                            "DEP-2: {name} has TB closing {closing_rhs}p or FY movement but is \
not mapped to any depreciation block"
                        ));
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::{Ledger, LedgerLine, VoucherStatus};

    fn period() -> Window {
        Window {
            from: TallyDate::parse("20250401").unwrap(),
            to: TallyDate::parse("20260331").unwrap(),
        }
    }

    fn rules_dep(rate_bp: i64) -> Rules {
        let mut depreciation_block_rate_bp = BTreeMap::new();
        for k in [
            "block_a", "block_b", "block_c", "block_d", "block_e", "block_f",
        ] {
            depreciation_block_rate_bp.insert(k.to_string(), rate_bp);
        }
        Rules {
            version: "test".to_string(),
            turnover_threshold_paise: 0,
            turnover_threshold_low_cash_paise: 0,
            cash_share_limit_bp: 0,
            s40a3_limit_per_person_per_day_paise: 0,
            s40a3_goods_carriage_limit_paise: 0,
            s40a3_excluded_group_roles: Vec::new(),
            s269st_limit_per_person_per_day_paise: 0,
            s269ss_269t_limit_paise: 0,
            depreciation_half_rate_days_threshold: 180,
            depreciation_cash_addition_limit_paise: 1_000_000,
            depreciation_block_rate_bp,
            due_date_audit_report: String::new(),
            due_date_return_audit_case: String::new(),
            due_date_return_non_audit_firm: String::new(),
            due_dates_status: String::new(),
        }
    }

    fn asset(name: &str) -> Ledger {
        Ledger {
            name: name.to_string(),
            parent: "Fixed Assets".to_string(),
            chain: vec!["Fixed Assets".to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: format!("guid-{name}"),
            masterid: None,
        }
    }

    fn misc(name: &str, group: &str) -> Ledger {
        Ledger {
            name: name.to_string(),
            parent: group.to_string(),
            chain: vec![group.to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: format!("guid-{name}"),
            masterid: None,
        }
    }

    fn voucher(guid: &str, date: &str, lines: &[(&str, i64)]) -> Voucher {
        Voucher {
            guid: guid.to_string(),
            date: TallyDate::parse(date).unwrap(),
            vtype: "Journal".to_string(),
            base_type: "Journal".to_string(),
            number: String::new(),
            status: VoucherStatus::Regular,
            lines: lines
                .iter()
                .map(|(n, a)| LedgerLine {
                    ledger: (*n).to_string(),
                    amount_paise: *a,
                })
                .collect(),
        }
    }

    fn book(
        ledgers: Vec<Ledger>,
        vouchers: Vec<Voucher>,
        tb: Vec<(&str, i64, i64, i64, i64)>,
    ) -> Book {
        Book {
            company_name: "Synthetic".to_string(),
            company_guid: "test-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: ledgers.into_iter().map(|l| (l.name.clone(), l)).collect(),
            vouchers,
            tb: tb
                .into_iter()
                .map(|(n, opening, debit, credit, closing)| {
                    (
                        n.to_string(),
                        crate::book::TbRow {
                            opening_paise: opening,
                            debit_paise: debit,
                            credit_paise: credit,
                            closing_paise: closing,
                        },
                    )
                })
                .collect(),
        }
    }

    /// s.32(1) second proviso boundary: 180 days used is full rate, 179 is half rate.
    #[test]
    fn days_used_179_is_half_rate_180_is_full_rate() {
        let p = period();
        // period.to is 2026-03-31 (inclusive count: (end - put_to_use).days + 1).
        let put_180 = TallyDate::parse("20251003").unwrap(); // exactly 180 days used
        let put_179 = TallyDate::parse("20251004").unwrap(); // one day later -> 179 days used
        let du_180 = days_used(&put_180, &p);
        let du_179 = days_used(&put_179, &p);
        assert_eq!(du_180, 180, "expected exactly 180 days used");
        assert_eq!(du_179, 179, "expected exactly 179 days used");
        assert!(du_180 >= 180);
        assert!(du_179 < 180);
    }

    #[test]
    fn boundary_end_to_end_through_run() {
        let p = period();
        let put_180 = TallyDate::parse("20251003").unwrap();
        let put_179 = TallyDate::parse("20251004").unwrap();
        let ledgers = vec![
            asset("Asset GE180"),
            asset("Asset LT180"),
            misc("Suspense", "Suspense"),
        ];
        let v1 = voucher(
            "v1",
            put_180.as_str(),
            &[("Asset GE180", 100_000), ("Suspense", -100_000)],
        );
        let v2 = voucher(
            "v2",
            put_179.as_str(),
            &[("Asset LT180", 50_000), ("Suspense", -50_000)],
        );
        let tb = vec![
            ("Asset GE180", 0, 100_000, 0, 100_000),
            ("Asset LT180", 0, 50_000, 0, 50_000),
        ];
        let b = book(ledgers, vec![v1, v2], tb);
        let rules = rules_dep(1500);
        let mut block_by_ledger = BTreeMap::new();
        block_by_ledger.insert("Asset GE180".to_string(), "block_a".to_string());
        block_by_ledger.insert("Asset LT180".to_string(), "block_a".to_string());
        let mut opening = BTreeMap::new();
        opening.insert("block_a".to_string(), 0i64);
        let res = run(
            &b,
            &rules,
            &p,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let fig = |id: &str| {
            res.figures
                .iter()
                .find(|f| f.id == id)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(
            fig("depreciation.additions_ge180_block_a"),
            Value::Int(100_000)
        );
        assert_eq!(
            fig("depreciation.additions_lt180_block_a"),
            Value::Int(50_000)
        );
        // 15% full on 100000 = 15000; 15%/2 on 50000 = 3750
        assert_eq!(
            fig("depreciation.dep_total_act_block_a"),
            Value::Int(15_000 + 3_750)
        );
        assert_eq!(check_invariants(&b, &res).unwrap(), Vec::<String>::new());
    }

    /// s.43(6): a deletion exceeding the full-rate pool spills into the half-rate pool.
    #[test]
    fn deletion_exceeding_full_rate_pool_spills() {
        let p = period();
        let ledgers = vec![asset("Asset Del"), misc("Suspense", "Suspense")];
        let add_date = "20260321"; // well under 180 days used -> half rate
        let v_add = voucher(
            "v_add",
            add_date,
            &[("Asset Del", 50_000), ("Suspense", -50_000)],
        );
        let v_del = voucher(
            "v_del",
            "20250601",
            &[("Suspense", 120_000), ("Asset Del", -120_000)],
        );
        let tb = vec![("Asset Del", 100_000, 50_000, 120_000, 30_000)];
        let b = book(ledgers, vec![v_add, v_del], tb);
        let rules = rules_dep(1500);
        let mut block_by_ledger = BTreeMap::new();
        block_by_ledger.insert("Asset Del".to_string(), "block_b".to_string());
        let mut opening = BTreeMap::new();
        opening.insert("block_b".to_string(), 100_000i64);
        let data = compute_depreciation(
            &b,
            &p,
            &rules,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let blk = &data.blocks["block_b"];
        assert_eq!(blk.full_basis_paise, 0); // 100000 + 0 - 120000 -> floored at 0
        assert_eq!(blk.half_basis_paise, 30_000); // 50000 - overflow(20000)
        assert_eq!(blk.dep_total_paise, 2_250); // 15%/2 of 30000
        assert_eq!(blk.closing_paise, 30_000 - 2_250);
        let res = run(
            &b,
            &rules,
            &p,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(check_invariants(&b, &res).unwrap(), Vec::<String>::new());
    }

    /// A same-voucher cash line flags the addition but never silently excludes it from the base.
    #[test]
    fn same_voucher_cash_line_flagged_but_not_silently_excluded() {
        let p = period();
        let ledgers = vec![asset("Asset Cash"), misc("Cash", "Cash-in-Hand")];
        let v = voucher(
            "v1",
            p.from.as_str(),
            &[("Asset Cash", 2_000_000), ("Cash", -2_000_000)],
        );
        let tb = vec![("Asset Cash", 0, 2_000_000, 0, 2_000_000)];
        let b = book(ledgers, vec![v], tb);
        let rules = rules_dep(1500);
        let mut block_by_ledger = BTreeMap::new();
        block_by_ledger.insert("Asset Cash".to_string(), "block_c".to_string());
        let mut opening = BTreeMap::new();
        opening.insert("block_c".to_string(), 0i64);
        let data = compute_depreciation(
            &b,
            &p,
            &rules,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let blk = &data.blocks["block_c"];
        assert_eq!(blk.cash_rows.len(), 1);
        assert_eq!(blk.dep_total_paise, 300_000); // cost NOT excluded from the base figure
        let sens = blk.sensitivity_excl_cash.as_ref().unwrap();
        assert_eq!(sens.dep_total_paise, 0); // excluding it: 0 basis, 0 dep

        let res = run(
            &b,
            &rules,
            &p,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let s43: Vec<_> = res
            .findings
            .iter()
            .filter(|f| f.id.starts_with(&format!("{TEST_ID}/s43_1/")))
            .collect();
        assert_eq!(s43.len(), 1);
        assert_eq!(s43[0].confidence, Confidence::NeedsDocument);
    }

    /// Route (b): no cash line on the purchase voucher itself, but a same-day cash payment to the
    /// supplier ledger still flags the addition.
    #[test]
    fn same_day_cash_payment_to_supplier_flagged() {
        let p = period();
        let ledgers = vec![
            asset("Asset Sup"),
            misc("Cash", "Cash-in-Hand"),
            misc("Supplier X", "Sundry Creditors"),
        ];
        let d = p.from.as_str().to_string();
        let v_purchase = voucher(
            "v_p",
            &d,
            &[("Asset Sup", 1_500_000), ("Supplier X", -1_500_000)],
        );
        let v_payment = voucher(
            "v_pay",
            &d,
            &[("Supplier X", 1_500_000), ("Cash", -1_500_000)],
        );
        let tb = vec![("Asset Sup", 0, 1_500_000, 0, 1_500_000)];
        let b = book(ledgers, vec![v_purchase, v_payment], tb);
        let rules = rules_dep(1500);
        let mut block_by_ledger = BTreeMap::new();
        block_by_ledger.insert("Asset Sup".to_string(), "block_d".to_string());
        let mut opening = BTreeMap::new();
        opening.insert("block_d".to_string(), 0i64);
        let data = compute_depreciation(
            &b,
            &p,
            &rules,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let blk = &data.blocks["block_d"];
        assert_eq!(blk.cash_rows.len(), 1);
        assert_eq!(
            blk.cash_rows[0].cash_reason,
            "same-day cash payment to the supplier ledger"
        );
        assert_eq!(blk.dep_total_paise, rate_amount(1_500_000, 1500, 10_000)); // not excluded
    }

    /// An unmapped Fixed Assets ledger with movement blocks every total, fails loud, and DEP-2
    /// independently catches it.
    #[test]
    fn unmapped_asset_ledger_blocks_totals_and_dep2_catches_it() {
        let p = period();
        let ledgers = vec![asset("Mapped Asset"), asset("Unmapped Asset")];
        let tb = vec![
            ("Mapped Asset", 0, 0, 0, 0),
            ("Unmapped Asset", 500_000, 0, 0, 500_000),
        ];
        let b = book(ledgers, vec![], tb);
        let rules = rules_dep(1500);
        let mut block_by_ledger = BTreeMap::new();
        block_by_ledger.insert("Mapped Asset".to_string(), "block_e".to_string());
        let mut opening = BTreeMap::new();
        opening.insert("block_e".to_string(), 0i64);
        let res = run(
            &b,
            &rules,
            &p,
            &block_by_ledger,
            &opening,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        let fig = |id: &str| {
            res.figures
                .iter()
                .find(|f| f.id == id)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(
            fig("depreciation.unmapped_fixed_asset_ledger_count"),
            Value::Int(1)
        );
        assert!(!res
            .figures
            .iter()
            .any(|f| f.id == "depreciation.book_dep_total")); // no total: fail loud
        let unmapped_findings: Vec<_> = res
            .findings
            .iter()
            .filter(|f| f.id == format!("{TEST_ID}/unmapped"))
            .collect();
        assert_eq!(unmapped_findings.len(), 1);
        assert_eq!(
            unmapped_findings[0].confidence,
            Confidence::JudgementRequired
        );

        let violations = check_invariants(&b, &res).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("DEP-2") && v.contains("Unmapped Asset")),
            "{violations:?}"
        );
    }

    fn book_vs_act(res: &TestResult) -> &Finding {
        let found: Vec<_> = res
            .findings
            .iter()
            .filter(|f| f.id == format!("{TEST_ID}/book_vs_act"))
            .collect();
        assert_eq!(found.len(), 1);
        found[0]
    }

    fn run_with(
        b: &Book,
        block_by_ledger: &[(&str, &str)],
        opening: &[(&str, i64)],
        dep_expense: &[&str],
    ) -> TestResult {
        let bbl: BTreeMap<String, String> = block_by_ledger
            .iter()
            .map(|(l, k)| ((*l).to_string(), (*k).to_string()))
            .collect();
        let open: BTreeMap<String, i64> = opening
            .iter()
            .map(|(k, v)| ((*k).to_string(), *v))
            .collect();
        let dep: BTreeSet<String> = dep_expense.iter().map(|s| (*s).to_string()).collect();
        run(
            b,
            &rules_dep(1500),
            &period(),
            &bbl,
            &open,
            &dep,
            &BTreeMap::new(),
        )
        .unwrap()
    }

    /// No block mapped, no opening WDV and no Fixed Assets ledger with a balance: the zero totals
    /// are not a computed nil. Before this, `every_ledger_mapped` passed vacuously on an empty set
    /// of Fixed Assets ledgers and the finding read `Computed`, titled as a difference of zero.
    #[test]
    fn nothing_mapped_is_a_question_not_a_nil() {
        let b = book(vec![misc("Suspense", "Suspense")], vec![], vec![]);
        let f = book_vs_act(&run_with(&b, &[], &[], &[])).clone();
        assert_eq!(f.confidence, Confidence::JudgementRequired);
        assert!(f.title.contains("not computed"), "{}", f.title);
        assert!(f.facts.is_empty()); // no zero total cited as though it were a result
        assert!(!f.ask_client.is_empty());

        // A Fixed Assets ledger with no balance takes the same branch, not the older
        // every-ledger-mapped fallback (which would also say JudgementRequired).
        let b = book(
            vec![asset("Idle Asset")],
            vec![],
            vec![("Idle Asset", 0, 0, 0, 0)],
        );
        let f = book_vs_act(&run_with(&b, &[], &[], &[])).clone();
        assert!(f.title.contains("not computed"), "{}", f.title);
        assert!(f.facts.is_empty());
    }

    /// A configured opening WDV is a real block: its Act depreciation is computed (and filed in
    /// clause 18), so the finding must not call it "not computed".
    #[test]
    fn opening_wdv_without_a_mapped_ledger_is_real_depreciation() {
        let b = book(vec![misc("Suspense", "Suspense")], vec![], vec![]);
        let res = run_with(&b, &[], &[("block_a", 1_000_000)], &[]);
        let dep = res
            .figures
            .iter()
            .find(|f| f.id == "depreciation.dep_total_act_block_a")
            .unwrap();
        assert_eq!(dep.value, Value::Int(150_000));
        let f = book_vs_act(&res);
        assert!(!f.title.contains("not computed"), "{}", f.title);
        assert!(f.facts.iter().any(|(k, _)| k == "act_total"));
    }

    #[test]
    fn book_charge_with_nothing_mapped_is_cited() {
        let b = book(
            vec![misc("Depreciation", "Indirect Expenses")],
            vec![],
            vec![("Depreciation", 0, 500_000, 0, 500_000)],
        );
        let f = book_vs_act(&run_with(&b, &[], &[], &["Depreciation"])).clone();
        assert!(f.title.contains("not computed"), "{}", f.title);
        let keys: Vec<_> = f.facts.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["book_total"]);
        assert!(f.limits.iter().any(|l| l.contains("no asset behind it")));
    }

    /// The guard is "nothing mapped", not "all zero": a configured block with no movement is a real
    /// computed nil, and the title never asserts a difference.
    #[test]
    fn a_mapped_block_with_zero_figures_is_still_computed() {
        let b = book(
            vec![asset("Idle Asset")],
            vec![],
            vec![("Idle Asset", 0, 0, 0, 0)],
        );
        let f = book_vs_act(&run_with(
            &b,
            &[("Idle Asset", "block_a")],
            &[("block_a", 0)],
            &[],
        ))
        .clone();
        assert_eq!(f.confidence, Confidence::Computed);
        assert_eq!(f.title, "Book vs Income-tax Act depreciation");
        assert!(f.facts.iter().any(|(k, _)| k == "act_total"));
    }

    /// DEP-1: opening + additions - deletions - depreciation credited (from vouchers) must equal
    /// the Trial Balance closing balance (read directly, never derived) for every Fixed Assets
    /// ledger.
    #[test]
    fn dep1_correct_case_ties_and_mutation_is_caught() {
        let p = period();
        let make = |dep_credited_paise: i64| {
            let ledgers = vec![asset("Asset Mut")];
            let v = voucher(
                "v_dep",
                "20260331",
                &[
                    ("Depreciation A/c", dep_credited_paise),
                    ("Asset Mut", -dep_credited_paise),
                ],
            );
            // Ground truth (TB) always says 15000 of depreciation happened: opening 100000 ->
            // closing 85000, regardless of what the voucher above credited.
            let tb = vec![
                ("Asset Mut", 100_000, 0, dep_credited_paise, 85_000),
                ("Depreciation A/c", 0, 15_000, 0, 15_000),
            ];
            book(ledgers, vec![v], tb)
        };
        let rules = rules_dep(1500);
        let mut block_by_ledger = BTreeMap::new();
        block_by_ledger.insert("Asset Mut".to_string(), "block_f".to_string());
        let mut opening = BTreeMap::new();
        opening.insert("block_f".to_string(), 100_000i64);
        let mut dep_expense_ledgers = BTreeSet::new();
        dep_expense_ledgers.insert("Depreciation A/c".to_string());

        let b_ok = make(15_000);
        let res_ok = run(
            &b_ok,
            &rules,
            &p,
            &block_by_ledger,
            &opening,
            &dep_expense_ledgers,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            res_ok
                .figures
                .iter()
                .find(|f| f.id == "depreciation.book_dep_total")
                .unwrap()
                .value,
            Value::Int(15_000)
        );
        assert_eq!(
            check_invariants(&b_ok, &res_ok).unwrap(),
            Vec::<String>::new()
        );

        // The dep journal voucher credits 100 paise (Re 1) MORE off the asset ledger than the TB
        // reflects -- a bug in how depreciation was posted, not in the TB itself.
        let b_bad = make(15_100);
        let res_bad = run(
            &b_bad,
            &rules,
            &p,
            &block_by_ledger,
            &opening,
            &dep_expense_ledgers,
            &BTreeMap::new(),
        )
        .unwrap();
        let violations = check_invariants(&b_bad, &res_bad).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("DEP-1") && v.contains("Asset Mut")),
            "{violations:?}"
        );
    }
}
