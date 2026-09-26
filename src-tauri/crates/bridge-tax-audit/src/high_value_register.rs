// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `high_value_register`: one register of high-value receipts and
//! payments, in cash and by bank, at (party, day) and (party, voucher) grain, with the s.269ST
//! limbs the books can show, party-to-party journal transfers, and s.194N's informational
//! exposure from a supplied bank statement.
//!
//! * Cash rows are tested against s.269ST(a)'s person-per-day limit (`>=`, the Act's "or more");
//!   bank rows against the CA-set vouching threshold only (s.269ST is cash-only). Findings are at
//!   (party, day) grain; the voucher grain is a companion count and total.
//! * A voucher whose money leg has no identifiable party (only Sales/Purchase Accounts, Duties &
//!   Taxes or round-off lines) is one `UNIDENTIFIED_PARTY` row, cited as a `row`, never a ledger.
//! * A cash receipt against Loans (Liability) is left out entirely (a s.269SS transaction), and in
//!   cash mode so is a counterparty whose configured type is a bank, a co-operative bank or a
//!   Government company (the form's own parenthetical).
//! * Limb (b) groups cash rows by (party, `Voucher.reference`) across dates, a JUDGEMENT candidate;
//!   limb (c) is one fixed question the books cannot answer.
//! * s.194N reports the statement window's narration-matched cash withdrawals, informational, and
//!   states which threshold applies only when the recipient type is known.
//! * A row's amount is the party side of the voucher, not its money line, as the reference
//!   computes it (parity, pinned by the goldens). A voucher settling one party partly in cash and
//!   partly by bank (or against a discount) is a cash row, and a bank row, for the whole party
//!   amount (`hvr_paths`' h15). A voucher naming two or more parties gives each only its own line,
//!   and its tax, round-off and Sales/Purchase lines go to no party (`hvr_paths`' h11). So an
//!   s.269ST row can over-state, or under-state, the cash one party moved. Where a flagged row's
//!   vouchers carry a different money line, the row shows that line; where the line is under the
//!   threshold, the row is proven under it, keeps its tags, takes a title saying so and is
//!   counted apart from the at-or-over pairs (`parity/PORT-NOTE-HVR.md`).
//!
//! The reference reads `[high_value_register].ca_threshold_paise` and `[s194n]` when the rules
//! carry them and its own defaults otherwise. The vendored rules excerpt carries neither table, so
//! this port reads the defaults, whose numbers equal the reference's tables (a test pins them).
//!
//! The row walk generalises `cash_payments_40a3`'s s.269ST walk over a money set, a direction and
//! a grain. A figure id the reference would repeat (two ledgers sharing a tag, a two-line journal
//! on one ledger) is refused with an error, as the reference's `fig` raises, never a panic.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;

use crate::book::{Book, Voucher};
use crate::documents::{AisRow, BankStatementDoc};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::loans_interest::LoanConfig;
use crate::read::iso;
use crate::rules::Rules;
use crate::support::{count, hash8, overflow, py_upper, voucher_label};
use crate::tds_payees::py_format_g;

pub const TEST_ID: &str = "high_value_register";
pub const VERSION: &str = "1";

const SALES_ACCOUNTS_GROUP: &str = "Sales Accounts";
const PURCHASE_ACCOUNTS_GROUP: &str = "Purchase Accounts";
const PARTY_GROUPS: [&str; 2] = ["Sundry Debtors", "Sundry Creditors"];
const DUTIES_TAXES_GROUP: &str = "Duties & Taxes";
const LOANS_LIABILITY_GROUP: &str = "Loans (Liability)";

/// Not a ledger: the key a money leg with no identifiable counterparty is grouped under.
pub const UNIDENTIFIED_PARTY: &str = "‹cash leg with no identified party›";
/// How a finding cites that bucket: a `row`, never a `ledger` (EVID-1).
const UNIDENTIFIED_PARTY_ROW: &str = "high_value_register:unidentified_party";

/// Counterparty types the form excludes from Clause 31(ba)-(bd).
const COUNTERPARTY_TYPES_EXCLUDED_FROM_269ST: [&str; 3] =
    ["bank", "cooperative_bank", "government_company"];

pub const RECIPIENT_CO_OPERATIVE: &str = "co_operative_society";
pub const RECIPIENT_NOT_CO_OPERATIVE: &str = "not_co_operative_society";

/// The s.194N recipient: whether this assessee, withdrawing cash, is itself a co-operative
/// society. A type rather than text, so no value but these two reaches `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipient {
    CoOperative,
    NotCoOperative,
}

impl Recipient {
    /// The reference's text for this recipient type.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CoOperative => RECIPIENT_CO_OPERATIVE,
            Self::NotCoOperative => RECIPIENT_NOT_CO_OPERATIVE,
        }
    }
}

/// The reference's fallback defaults, equal to its rules tables' numbers.
pub const DEFAULT_CA_THRESHOLD_PAISE: i64 = 20_000_000; // Rs 2 lakh
pub const DEFAULT_S194N_THRESHOLD_PAISE: i64 = 1_000_000_000; // Rs 1 crore
pub const DEFAULT_S194N_THRESHOLD_CO_OPERATIVE_PAISE: i64 = 3_000_000_000; // Rs 3 crore
pub const DEFAULT_S194N_THRESHOLD_NON_FILER_PAISE: i64 = 200_000_000; // Rs 20 lakh

fn prefix_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b).ok_or_else(|| overflow(TEST_ID))
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

/// The optional `[roles].s194n_withdrawal_narration_terms`, as a set of strings; empty when absent.
///
/// Divergence, deliberate, and not parity (as `bank_reconciliation::charge_terms`): the reference
/// passes the value to `frozenset(...)` unchecked, so a single string becomes the set of its
/// characters. Here a non-list, a list holding a non-string, and a blank term (in every narration,
/// so every debit would count) refuse.
pub fn s194n_terms(raw: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    let Some(raw) = raw else {
        return Ok(BTreeSet::new());
    };
    let key = "[roles].s194n_withdrawal_narration_terms";
    raw.as_array()
        .ok_or_else(|| AuditError::Config(format!("{TEST_ID}: {key} is not a list")))?
        .iter()
        .map(|v| match v.as_str() {
            // A blank term is in every narration, or nearly (a space), so every debit would count.
            Some(s) if s.trim().is_empty() => Err(AuditError::Config(format!(
                "{TEST_ID}: {key} holds a blank term"
            ))),
            Some(s) => Ok(s.to_string()),
            None => Err(AuditError::Config(format!(
                "{TEST_ID}: {key} holds a non-string"
            ))),
        })
        .collect()
}

/// Each ledger's counterparty type, as pack.py builds it: every configured loan ledger's
/// `lender_type` first, then `[roles].counterparty_type_by_ledger` (keys already bound) over it.
///
/// Divergence, deliberate, and not parity: the reference keeps a non-string type as written, where
/// it can never match an excluded type. Here it refuses.
pub fn counterparty_types(
    loans: &BTreeMap<String, LoanConfig>,
    by_ledger: &BTreeMap<String, toml::Value>,
) -> Result<BTreeMap<String, String>> {
    let mut out: BTreeMap<String, String> = loans
        .iter()
        .map(|(ledger, cfg)| (ledger.clone(), cfg.lender_type.clone()))
        .collect();
    for (ledger, v) in by_ledger {
        let t = v.as_str().ok_or_else(|| {
            AuditError::Config(format!(
                "{TEST_ID}: [roles].counterparty_type_by_ledger.{ledger} is not a string"
            ))
        })?;
        out.insert(ledger.clone(), t.to_string());
    }
    Ok(out)
}

/// The recipient type pack.py derives from the engagement's own entity type.
pub fn s194n_recipient_type(entity_type: Option<&str>) -> Option<Recipient> {
    match entity_type {
        Some("individual" | "huf" | "firm" | "llp" | "company") => Some(Recipient::NotCoOperative),
        Some("cooperative_society") => Some(Recipient::CoOperative),
        _ => None,
    }
}

/// One row: its total and the vouchers behind it, by GUID (a repeated GUID keeps the last voucher,
/// as the reference's dict does), and per GUID the party's share of those vouchers against their
/// own money line in the row's mode and direction (see `parity/PORT-NOTE-HVR.md`).
#[derive(Default)]
pub struct Row<'a> {
    pub paise: i64,
    pub vouchers: BTreeMap<String, &'a Voucher>,
    /// GUID -> (share, line), each added per voucher as it is read, so two vouchers sharing a GUID
    /// add together and one voucher's line is never set against two vouchers' shares.
    pub lines: BTreeMap<String, (i64, i64)>,
}

impl<'a> Row<'a> {
    fn add(&mut self, v: &'a Voucher, share: i64, line: i64) -> Result<()> {
        self.paise = add(self.paise, share)?;
        self.vouchers.insert(v.guid.clone(), v);
        let slot = self.lines.entry(v.guid.clone()).or_insert((0, 0));
        *slot = (add(slot.0, share)?, add(slot.1, line)?);
        Ok(())
    }

    /// The row's vouchers' own money line, summed.
    fn line_total(&self) -> Result<i64> {
        self.lines
            .values()
            .try_fold(0, |t, (_, line)| add(t, *line))
    }

    /// At or over `threshold` on the party's side, and on the vouchers' own line too; a row over on
    /// the party's side but under on the line is proven under it (`parity/PORT-NOTE-HVR.md`).
    fn over(&self, threshold: i64) -> Result<Over> {
        Ok(if self.paise < threshold {
            Over::No
        } else if self.line_total()? < threshold {
            Over::PartySideOnly
        } else {
            Over::Yes
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Over {
    No,
    PartySideOnly,
    Yes,
}

type Rows<'a, K> = BTreeMap<(K, String), Row<'a>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Receipt,
    Payment,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Payment => "payment",
        }
    }
}

/// What a row walk leaves out, and how.
pub struct Exclusions<'c> {
    /// Never itself a party, but its amount stays in the voucher's total (Sales/Purchase Accounts).
    pub from_party_groups: &'c [&'c str],
    /// Dropped entirely, never counted (Loans (Liability) for a cash receipt).
    pub entirely_groups: &'c [&'c str],
    pub round_off_ledgers: &'c BTreeSet<String>,
    /// A ledger whose type is a bank, a co-operative bank or a Government company is dropped.
    pub counterparty_types: &'c BTreeMap<String, String>,
}

/// The reference's `compute_mode_rows`: `(key_fn(v), party ledger) -> row` over non-Contra
/// vouchers with a money leg in `direction` on `mode_set`; `other_money` is never a party.
pub fn mode_rows<'a, K: Ord>(
    pop: &[&'a Voucher],
    book: &Book,
    mode_set: &BTreeSet<String>,
    other_money: &BTreeSet<String>,
    direction: Direction,
    key_fn: impl Fn(&Voucher) -> K,
    x: &Exclusions<'_>,
) -> Result<Rows<'a, K>> {
    let mut rows: Rows<'a, K> = BTreeMap::new();
    for &v in pop {
        if v.base_type == "Contra" {
            continue;
        }
        let money_leg = v.lines.iter().any(|l| {
            mode_set.contains(&l.ledger)
                && match direction {
                    Direction::Receipt => l.amount_paise > 0,
                    Direction::Payment => l.amount_paise < 0,
                }
        });
        if !money_leg {
            continue;
        }
        // The voucher's own money line in this mode and direction: debits on a receipt, the
        // magnitude of credits on a payment.
        let mut line = 0_i64;
        for l in v.lines.iter().filter(|l| mode_set.contains(&l.ledger)) {
            match direction {
                Direction::Receipt if l.amount_paise > 0 => line = add(line, l.amount_paise)?,
                Direction::Payment if l.amount_paise < 0 => {
                    let m = l
                        .amount_paise
                        .checked_neg()
                        .ok_or_else(|| overflow(TEST_ID))?;
                    line = add(line, m)?;
                }
                _ => {}
            }
        }
        let mut party_amounts: BTreeMap<String, i64> = BTreeMap::new();
        let mut fallback_total = 0_i64;
        for l in &v.lines {
            if mode_set.contains(&l.ledger) || other_money.contains(&l.ledger) {
                continue;
            }
            let amt = match direction {
                Direction::Receipt if l.amount_paise < 0 => l
                    .amount_paise
                    .checked_neg()
                    .ok_or_else(|| overflow(TEST_ID))?,
                Direction::Payment if l.amount_paise > 0 => l.amount_paise,
                _ => continue,
            };
            let ledger = book.ledgers.get(&l.ledger);
            let under_any =
                |groups: &[&str]| ledger.is_some_and(|lg| groups.iter().any(|g| lg.under(g)));
            if under_any(x.entirely_groups) {
                continue; // (receipt only) Loans (Liability): out of scope entirely
            }
            if x.counterparty_types
                .get(&l.ledger)
                .is_some_and(|t| COUNTERPARTY_TYPES_EXCLUDED_FROM_269ST.contains(&t.as_str()))
            {
                continue; // a bank / co-operative bank / Government company counterparty
            }
            let excluded_from_party = under_any(x.from_party_groups);
            let tax_or_round_off = ledger.is_some_and(|lg| lg.under(DUTIES_TAXES_GROUP))
                || x.round_off_ledgers.contains(&l.ledger);
            fallback_total = add(fallback_total, amt)?;
            if excluded_from_party || tax_or_round_off {
                continue; // never itself a party; its amount stays in the total
            }
            let slot = party_amounts.entry(l.ledger.clone()).or_insert(0);
            *slot = add(*slot, amt)?;
        }
        if party_amounts.len() == 1 {
            // The whole transaction is grouped to its one real party.
            for amount in party_amounts.values_mut() {
                *amount = fallback_total;
            }
        }
        if !party_amounts.is_empty() {
            for (ledger, amt) in party_amounts {
                let row = rows.entry((key_fn(v), ledger)).or_default();
                row.add(v, amt, line)?;
            }
        } else if fallback_total > 0 {
            let row = rows
                .entry((key_fn(v), UNIDENTIFIED_PARTY.to_string()))
                .or_default();
            row.add(v, fallback_total, line)?;
        }
    }
    Ok(rows)
}

/// Limb (b): `(reference, party) -> row` for vouchers with a non-empty reference, and the count of
/// qualifying vouchers whose reference is blank.
fn bill_reference_rows<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    direction: Direction,
    x: &Exclusions<'_>,
) -> Result<(Rows<'a, String>, usize)> {
    let all = mode_rows(pop, book, cash, bank, direction, |v| v.reference.clone(), x)?;
    let rows: Rows<'a, String> = all
        .into_iter()
        .filter(|((reference, _), _)| !reference.is_empty())
        .collect();
    let with_money_leg = mode_rows(pop, book, cash, bank, direction, |v| v.guid.clone(), x)?;
    let with_ref: BTreeSet<&String> = rows.values().flat_map(|r| r.vouchers.keys()).collect();
    let all_guids: BTreeSet<&String> = with_money_leg
        .values()
        .flat_map(|r| r.vouchers.keys())
        .collect();
    let skipped = all_guids.difference(&with_ref).count();
    Ok((rows, skipped))
}

/// One party-to-party journal line.
struct JournalLine<'a> {
    voucher: &'a Voucher,
    ledger: String,
    amount_paise: i64,
    code: &'static str,
}

/// A two-line Journal with no cash or bank line and both lines on party ledgers: one row per line,
/// its absolute amount, and code "I" (debit) or "J" (credit).
fn journal_transfer_rows<'a>(
    pop: &[&'a Voucher],
    book: &Book,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
) -> Result<Vec<JournalLine<'a>>> {
    let mut out = Vec::new();
    for &v in pop {
        if v.base_type != "Journal" {
            continue;
        }
        if v.lines
            .iter()
            .any(|l| cash.contains(&l.ledger) || bank.contains(&l.ledger))
        {
            continue;
        }
        if v.lines.len() != 2 {
            continue;
        }
        let all_parties = v.lines.iter().all(|l| {
            book.ledgers
                .get(&l.ledger)
                .is_some_and(|lg| PARTY_GROUPS.iter().any(|g| lg.under(g)))
        });
        if !all_parties {
            continue;
        }
        for l in &v.lines {
            out.push(JournalLine {
                voucher: v,
                ledger: l.ledger.clone(),
                amount_paise: l
                    .amount_paise
                    .checked_abs()
                    .ok_or_else(|| overflow(TEST_ID))?,
                code: if l.amount_paise > 0 { "I" } else { "J" },
            });
        }
    }
    Ok(out)
}

fn voucher_evidence(vouchers: &BTreeMap<String, &Voucher>) -> Vec<EvidenceRef> {
    vouchers
        .iter()
        .map(|(g, v)| EvidenceRef::with_label("voucher", g, &voucher_label(v)))
        .collect()
}

/// The party a finding names: a `ledger` ref, or the `row` ref for the unidentified bucket.
fn party_ref(ledger: &str) -> EvidenceRef {
    if ledger == UNIDENTIFIED_PARTY {
        EvidenceRef::with_label("row", UNIDENTIFIED_PARTY_ROW, UNIDENTIFIED_PARTY)
    } else {
        EvidenceRef::new("ledger", ledger)
    }
}

/// The caller's inputs, as `tae/pack.py` passes them.
pub struct Inputs<'c> {
    pub cash: &'c BTreeSet<String>,
    pub bank: &'c BTreeSet<String>,
    /// `None`: `DEFAULT_CA_THRESHOLD_PAISE`, equal to the reference rules table's number.
    pub threshold_paise: Option<i64>,
    pub bank_statement: Option<&'c BankStatementDoc>,
    pub s194n_narration_terms: &'c BTreeSet<String>,
    pub ais_rows: &'c [AisRow],
    pub s194n_recipient_type: Option<Recipient>,
    pub round_off_ledgers: &'c BTreeSet<String>,
    pub counterparty_type_by_ledger: &'c BTreeMap<String, String>,
}

#[allow(clippy::too_many_lines)] // one section per limb, as the reference lays them out
pub fn run(book: &Book, rules: &Rules, i: &Inputs<'_>) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let pop = book.population()?;
    let limit_269st = rules.s269st_limit_per_person_per_day_paise;
    let threshold = i.threshold_paise.unwrap_or(DEFAULT_CA_THRESHOLD_PAISE);
    let no_types: BTreeMap<String, String> = BTreeMap::new();

    #[allow(clippy::cast_precision_loss)] // Python's float division, formatted with :g
    let lakh = py_format_g(threshold as f64 / 10_000_000.0); // paise per lakh
    r.population_note = format!(
        "Books population (optional, cancelled and post-dated vouchers excluded); Contra excluded \
         throughout. Vouching threshold set by the CA: ₹{lakh} lakh. Every row states its own mode \
         (cash, bank, journal); account-payee status of a cheque/draft/ECS instrument is NOT \
         visible in Tally and is never inferred from a ledger's group. s.269ST limb (c) (one event \
         or occasion) is never testable from the books alone, in either direction."
    );
    fig(
        &mut r,
        "ca_threshold_paise",
        Value::Int(threshold),
        Unit::Paise,
        "The CA-set vouching threshold used throughout this register.",
        vec![],
    )?;

    // ------------------------------------------------------------ (party, date) and (party, voucher)
    for (mode_name, mode_set, other_money) in [("cash", i.cash, i.bank), ("bank", i.bank, i.cash)] {
        for (direction, clause_31, clause_269) in [
            (Direction::Receipt, "3CD-31(ba)", "s.269ST(a)"),
            (Direction::Payment, "3CD-31(bc)", "s.269ST(a)"),
        ] {
            let dir = direction.as_str();
            let is_cash = mode_name == "cash";
            let row_threshold = if is_cash { limit_269st } else { threshold };
            let from_party: [&str; 1] = if direction == Direction::Receipt {
                [SALES_ACCOUNTS_GROUP]
            } else {
                [PURCHASE_ACCOUNTS_GROUP]
            };
            let entirely: &[&str] = if direction == Direction::Receipt && is_cash {
                &[LOANS_LIABILITY_GROUP]
            } else {
                &[]
            };
            let x = Exclusions {
                from_party_groups: &from_party,
                entirely_groups: entirely,
                round_off_ledgers: i.round_off_ledgers,
                counterparty_types: if is_cash {
                    i.counterparty_type_by_ledger
                } else {
                    &no_types
                },
            };
            for grain in ["day", "voucher"] {
                let prefix = format!("{mode_name}_{dir}_{grain}");
                // Both grains, one shape: the day grain keys by date, the voucher grain by GUID.
                let day_rows = if grain == "day" {
                    Some(mode_rows(
                        &pop,
                        book,
                        mode_set,
                        other_money,
                        direction,
                        |v| v.date.clone(),
                        &x,
                    )?)
                } else {
                    None
                };
                let voucher_rows = if grain == "voucher" {
                    Some(mode_rows(
                        &pop,
                        book,
                        mode_set,
                        other_money,
                        direction,
                        |v| v.guid.clone(),
                        &x,
                    )?)
                } else {
                    None
                };
                let (any_count, any_total, over_count, line_below_count, evidence) =
                    match (&day_rows, &voucher_rows) {
                        (Some(rows), _) => summarise(rows, row_threshold)?,
                        (_, Some(rows)) => summarise(rows, row_threshold)?,
                        _ => unreachable!("one grain is always computed"),
                    };
                fig(&mut r, &format!("{prefix}_any_amount_count"), count(TEST_ID, any_count)?, Unit::Count,
                    &format!("Distinct (party, {grain}) pairs with a {mode_name} {dir} on a population, \
                              non-Contra voucher."), vec![])?;
                fig(&mut r, &format!("{prefix}_any_amount_total"), Value::Int(any_total), Unit::Paise,
                    &format!("Sum of {mode_name} {dir}s across all (party, {grain}) pairs above, any amount."),
                    evidence)?;
                let over_what = if is_cash {
                    "s.269ST(a) limit"
                } else {
                    "CA-set vouching threshold"
                };
                let (verb, moved) = match direction {
                    Direction::Receipt => ("debited", "received from"),
                    Direction::Payment => ("credited", "paid to"),
                };
                fig(&mut r, &format!("{prefix}_at_or_over_threshold_count"), count(TEST_ID, over_count)?,
                    Unit::Count,
                    &format!("(party, {grain}) pairs at or over the {over_what} ({row_threshold} paise), \
                              counting a pair only when its vouchers' own {mode_name} {verb} is at or over \
                              it too."), vec![])?;
                fig(&mut r, &format!("{prefix}_party_side_over_line_below_count"),
                    count(TEST_ID, line_below_count)?, Unit::Count,
                    &format!("(party, {grain}) pairs whose party side is at or over the {over_what} \
                              ({row_threshold} paise) but whose vouchers' own {mode_name} {verb} is under \
                              it; not counted as at or over."), vec![])?;

                let Some(rows) = day_rows else {
                    continue; // row-level findings only at the (party, day) grain
                };
                let mut over: Vec<(&(TallyDate, String), &Row)> = rows
                    .iter()
                    .filter(|(_, d)| d.paise >= row_threshold)
                    .collect();
                let mut keyed = Vec::with_capacity(over.len());
                for (k, d) in over.drain(..) {
                    keyed.push(((k.0.clone(), stable_ledger_tag(book, &k.1)?), k, d));
                }
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                for ((date, h), (_, ledger), data) in keyed {
                    let day = iso(&date);
                    let rid = format!("{prefix}_{day}_{h}");
                    // Where a voucher's own money line differs from the party's share of it, the row
                    // says so and shows that line (parity/PORT-NOTE-HVR.md).
                    let differs = data.lines.values().any(|(share, line)| share != line);
                    let line_total = data.line_total()?;
                    let other_mode = if is_cash { "bank" } else { "cash" };
                    let mut amount_definition = format!(
                        "{} {dir} from/to one party ledger (tag {h}) on {day}, summed across every \
                         population voucher that day.",
                        capitalize(mode_name)
                    );
                    if differs {
                        amount_definition.push_str(&format!(
                            " This is the party's side of each voucher; on one or more of the row's \
                             vouchers it differs from the {mode_name} line, which is shown with it."
                        ));
                    }
                    let f_amt = fig(
                        &mut r,
                        &format!("{prefix}_row_amount_{rid}"),
                        Value::Int(data.paise),
                        Unit::Paise,
                        &amount_definition,
                        voucher_evidence(&data.vouchers),
                    )?;
                    let f_line = if differs {
                        Some(fig(&mut r, &format!("{prefix}_row_{mode_name}_line_{rid}"),
                            Value::Int(line_total), Unit::Paise,
                            &format!("The {mode_name} {verb} on the vouchers in this row, summed, whichever \
                                      parties it was for."),
                            voucher_evidence(&data.vouchers))?)
                    } else {
                        None
                    };
                    let (mut title, clauses, mut limits, mut ask) = if is_cash
                        && direction == Direction::Receipt
                    {
                        (format!("Cash received from one party at or over the s.269ST(a) limb (a) person-per-day \
                                  threshold on {day}"),
                         vec![clause_269.to_string(), clause_31.to_string()],
                         vec!["Books test covers limb (a) (aggregate per person per day) only. Limbs (b) (a \
                               single transaction) and (c) (receipts relating to one event or occasion) need \
                               bill-wise/event linkage the books do not carry (see the single-transaction and \
                               event views below).".to_string()],
                         vec!["Confirm whether this receipt is genuinely from one party.".to_string(),
                              "Confirm whether limbs (b)/(c) apply (documents needed).".to_string()])
                    } else if is_cash {
                        (format!("Cash paid to one party at or over the s.269ST(a) limb (a) person-per-day \
                                  threshold on {day} -- reportable in clause 31(bc)"),
                         vec![clause_269.to_string(), clause_31.to_string()],
                         vec!["s.269ST(a) is a duty on the receiver of the cash, not the payer ('No person \
                               shall receive...'); a payment by the assessee is not a books-testable exposure \
                               for the assessee under s.269ST -- it is a Form 3CD clause 31(bc) reporting item \
                               only (limb (a): aggregate per person per day).".to_string(),
                              "Books test covers limb (a) only. Limbs (b)/(c) need bill-wise/event linkage the \
                               books do not carry.".to_string()],
                         vec!["Confirm whether this payment is genuinely to one party.".to_string(),
                              "Confirm whether limbs (b)/(c) apply (documents needed).".to_string()])
                    } else {
                        (format!("Bank {dir} at or over the CA-set vouching threshold on {day}"),
                         Vec::new(),
                         vec!["Bank mode is not a s.269ST test (that section is cash-only); this row is the \
                               CA-set vouching-list threshold only, not a statutory limb.".to_string(),
                              "Account-payee status of the instrument behind this bank movement is not \
                               visible in Tally.".to_string()],
                         vec!["Vouch this entry to the bank statement and the underlying document.".to_string()])
                    };
                    if differs {
                        let mut limit = format!(
                            "The amount is this party's own side of each voucher, not the voucher's own \
                             {mode_name} line. On one or more of the row's vouchers the two differ, so such a \
                             voucher carries other lines as well (such as money moving by {other_mode}, a \
                             discount or deduction, a round-off, a loan, a counterparty this register leaves \
                             out, several parties sharing the line, or money moving the other way)."
                        );
                        if line_total < row_threshold {
                            limit.push_str(&format!(
                                " The {mode_name} {verb} on these vouchers is below the threshold, so the \
                                 {mode_name} {moved} this party on them is below it too."
                            ));
                        }
                        limit.push_str(&format!(
                            " The {mode_name} {verb} on these vouchers is shown with this row."
                        ));
                        limits.push(limit);
                    }
                    // Proven under the threshold on the line: a ledger-true title; the tags, limits,
                    // asks and facts stay (parity/PORT-NOTE-HVR.md).
                    if data.over(row_threshold)? == Over::PartySideOnly {
                        title = format!(
                            "{} {moved} one party on {day}: the party's side is at or over the {over_what}, \
                             but the {mode_name} {verb} on these vouchers is below it",
                            capitalize(mode_name)
                        );
                    }
                    if ledger.as_str() == UNIDENTIFIED_PARTY {
                        title = title.replace("one party", "one unidentified party");
                        limits.push("No party ledger is on this voucher -- the books cannot name the \
                                     counterparty; this is grouped as one transaction (limb (b) candidate) \
                                     rather than dropped or scattered across its tax/round-off lines.".to_string());
                        ask.push(
                            "Supply the counterparty's name and PAN for this transaction."
                                .to_string(),
                        );
                    }
                    let f_date = fig(
                        &mut r,
                        &format!("{prefix}_row_date_{rid}"),
                        Value::Text(day.clone()),
                        Unit::Text,
                        "Date of the party-day total above.",
                        vec![],
                    )?;
                    let mut evidence = voucher_evidence(&data.vouchers);
                    evidence.push(party_ref(ledger));
                    let mut facts =
                        vec![("amount".to_string(), f_amt), ("date".to_string(), f_date)];
                    if let Some(f_line) = f_line {
                        facts.push((format!("{mode_name}_line"), f_line));
                    }
                    r.findings.push(Finding {
                        id: format!("{TEST_ID}/{rid}"),
                        clauses,
                        title,
                        facts,
                        evidence,
                        confidence: Confidence::NeedsDocument,
                        limits,
                        ask_client: ask,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------ limb (b): single transaction (cash only)
    for (direction, clause) in [
        (Direction::Receipt, "3CD-31(ba)"),
        (Direction::Payment, "3CD-31(bc)"),
    ] {
        let dir = direction.as_str();
        let from_party: [&str; 1] = if direction == Direction::Receipt {
            [SALES_ACCOUNTS_GROUP]
        } else {
            [PURCHASE_ACCOUNTS_GROUP]
        };
        let entirely: &[&str] = if direction == Direction::Receipt {
            &[LOANS_LIABILITY_GROUP]
        } else {
            &[]
        };
        let x = Exclusions {
            from_party_groups: &from_party,
            entirely_groups: entirely,
            round_off_ledgers: i.round_off_ledgers,
            counterparty_types: i.counterparty_type_by_ledger,
        };
        let (rows, skipped) = bill_reference_rows(&pop, book, i.cash, i.bank, direction, &x)?;
        fig(&mut r, &format!("cash_{dir}_single_transaction_groups_count"), count(TEST_ID, rows.len())?, Unit::Count,
            &format!("Distinct (party ledger, voucher reference) groups across every date, cash {dir}s only, \
                      voucher.reference non-empty -- s.269ST limb (b)."), vec![])?;
        fig(&mut r, &format!("cash_{dir}_single_transaction_no_reference_vouchers_count"), count(TEST_ID, skipped)?,
            Unit::Count,
            &format!("Cash {dir} vouchers with a qualifying money leg whose own REFERENCE field is blank -- not \
                      linkable to any other voucher by this books-only signal; limb (b) coverage is partial, \
                      stated as this count, never silently assumed complete."), vec![])?;
        let mut keyed = Vec::new();
        for (k, d) in rows.iter().filter(|(_, d)| d.paise >= limit_269st) {
            keyed.push(((k.0.clone(), stable_ledger_tag(book, &k.1)?), &k.1, d));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        for ((reference, h), ledger, data) in keyed {
            let rh = hash8(&reference);
            let rid = format!("{dir}_{rh}_{h}");
            let f_amt = fig(&mut r, &format!("cash_{dir}_single_transaction_amount_{rid}"), Value::Int(data.paise),
                Unit::Paise,
                &format!("Cash {dir}s from/to one party ledger (tag {h}) sharing voucher reference (tag {rh}), \
                          summed across every date."), voucher_evidence(&data.vouchers))?;
            let mut evidence = voucher_evidence(&data.vouchers);
            evidence.push(party_ref(ledger));
            r.findings.push(Finding {
                id: format!("{TEST_ID}/single_transaction/{rid}"),
                clauses: vec![clause.to_string()],
                title: format!("Candidate: cash {dir}s sharing one bill/voucher reference may be a single \
                                s.269ST(b) transaction across dates -- CA determination needed"),
                facts: vec![("amount".to_string(), f_amt)],
                evidence,
                confidence: Confidence::JudgementRequired,
                limits: vec![
                    "'Single transaction' has no statutory or notified definition beyond the GN's own \
                     illustrative examples (GN 56.10: 'will depend on facts'); a shared voucher reference \
                     is evidence FOR, not proof of, one transaction -- this is a candidate for the CA to \
                     decide, never a computed breach or reportable fact.".to_string(),
                    "Grouped by the voucher's own REFERENCE field only, not full bill-wise allocation \
                     detail -- a reference reused for an unrelated bill, or a genuine single bill \
                     recorded across differently-worded references, would both be missed by this signal."
                        .to_string(),
                    "s.269ST limb (b) (one transaction) can span cash AND other modes together in law; \
                     this view is cash-only (the books-testable limb for the receiver/payer distinction)."
                        .to_string(),
                ],
                ask_client: vec!["Confirm whether this reference genuinely identifies one bill/transaction, \
                                  and whether limb (b) applies.".to_string()],
            });
        }
    }

    // ------------------------------------------------------------ limb (c): event or occasion (JUDG only)
    r.findings.push(Finding {
        id: format!("{TEST_ID}/event_occasion_question"),
        clauses: vec!["3CD-31(ba)".to_string(), "3CD-31(bc)".to_string()],
        title: "s.269ST limb (c) (transactions relating to one event or occasion) cannot be checked from books"
            .to_string(),
        facts: Vec::new(),
        evidence: Vec::new(),
        confidence: Confidence::JudgementRequired,
        limits: vec!["An 'event or occasion' (e.g. one wedding covered by separate catering and decoration \
                      contracts with the same party, GN 56.11) is a fact about the world, not a ledger entry; \
                      no books signal can group transactions by it, in either direction (receipts or payments)."
            .to_string()],
        ask_client: vec!["Ask whether any party in the party-day/single-transaction rows above relates to one \
                          event or occasion with other transactions, in cash, not otherwise captured."
            .to_string()],
    });

    // ------------------------------------------------------------ journal mode
    let journal = journal_transfer_rows(&pop, book, i.cash, i.bank)?;
    let over: Vec<&JournalLine> = journal
        .iter()
        .filter(|j| j.amount_paise >= threshold)
        .collect();
    fig(
        &mut r,
        "journal_transfer_lines_any_amount_count",
        count(TEST_ID, journal.len())?,
        Unit::Count,
        "Lines on a two-line Journal voucher, neither line cash/bank, both lines under Sundry \
         Debtors/Sundry Creditors -- a party-to-party transfer with no money-ledger movement.",
        vec![],
    )?;
    fig(
        &mut r,
        "journal_transfer_lines_at_or_over_threshold_count",
        count(TEST_ID, over.len())?,
        Unit::Count,
        &format!("Of those, at or over the CA-set vouching threshold ({threshold} paise)."),
        vec![],
    )?;
    let mut keyed = Vec::with_capacity(over.len());
    for j in over {
        keyed.push((
            (j.voucher.date.clone(), stable_ledger_tag(book, &j.ledger)?),
            j,
        ));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0)); // stable, as Python's sorted: ties keep line order
    for ((_, h), j) in keyed {
        let v = j.voucher;
        let vh = hash8(&v.guid);
        let rid = format!("{vh}_{h}");
        let v_ref = EvidenceRef::with_label("voucher", &v.guid, &voucher_label(v));
        let f_amt = fig(
            &mut r,
            &format!("journal_transfer_amount_{rid}"),
            Value::Int(j.amount_paise),
            Unit::Paise,
            &format!("Journal-entry line against party ledger (tag {h}) on voucher (tag {vh})."),
            vec![v_ref.clone()],
        )?;
        fig(&mut r, &format!("journal_transfer_code_{rid}"), Value::Text(j.code.to_string()), Unit::Text,
            "Form 3CD utility Note 1 journal-entry code: I (debit) / J (credit) -- never asserted as a \
             'receipt' or 'payment', which two arbitrary party ledgers do not settle (module docstring).",
            vec![])?;
        r.findings.push(Finding {
            id: format!("{TEST_ID}/journal_transfer/{rid}"),
            clauses: Vec::new(),
            title: format!("Journal transfer touching party ledger (tag {h}) at or over the vouching threshold \
                            (code {})", j.code),
            facts: vec![("amount".to_string(), f_amt)],
            evidence: vec![v_ref, EvidenceRef::new("ledger", &j.ledger)],
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "A journal entry between two party ledgers is reported by its own debit/credit code \
                 (GN 57.13's Note 1 list), never labelled a 'receipt' or 'payment' -- which side \
                 received value depends on facts (goods, services, a set-off) this line alone does \
                 not carry.".to_string(),
                "This is not a s.269ST test (that section is cash-only); a large journal transfer is \
                 a vouching-list item only unless it is also a Loans (Liability) posting, tested \
                 separately under Clause 31(a)/(c).".to_string(),
            ],
            ask_client: vec!["Confirm the nature of this transfer (set-off, write-off, asset/liability \
                              conversion) and the counterparty's identity.".to_string()],
        });
    }

    // ------------------------------------------------------------ s.194N (informational)
    fig(&mut r, "s194n_threshold_paise", Value::Int(DEFAULT_S194N_THRESHOLD_PAISE), Unit::Paise,
        "The s.194N first-proviso annual cash-withdrawal threshold for a recipient who is NOT a \
         co-operative society (rules.s194n.threshold_paise, or this module's own DEFAULT_S194N until \
         that key is added).", vec![])?;
    fig(&mut r, "s194n_threshold_co_operative_paise", Value::Int(DEFAULT_S194N_THRESHOLD_CO_OPERATIVE_PAISE),
        Unit::Paise,
        "The s.194N fourth-proviso threshold where the RECIPIENT (the withdrawer -- this assessee) is \
         itself a co-operative society (rules.s194n.threshold_co_operative_paise).", vec![])?;
    let f_recipient = fig(&mut r, "s194n_recipient_type",
        Value::Text(i.s194n_recipient_type.map_or("unknown", Recipient::as_str).to_string()), Unit::Text,
        "Whether this assessee (the RECIPIENT of a s.194N-deducting bank when it withdraws cash) is itself \
         a co-operative society -- derived from the engagement's own entity type; 'unknown' when not \
         supplied or not recognised.", vec![])?;
    match i.s194n_recipient_type {
        Some(Recipient::CoOperative) => {
            fig(&mut r, "s194n_applicable_threshold_paise", Value::Int(DEFAULT_S194N_THRESHOLD_CO_OPERATIVE_PAISE),
                Unit::Paise,
                "The threshold that applies to THIS recipient: the co-operative-society figure, because \
                 s194n_recipient_type says so.", vec![])?;
        }
        Some(Recipient::NotCoOperative) => {
            fig(&mut r, "s194n_applicable_threshold_paise", Value::Int(DEFAULT_S194N_THRESHOLD_PAISE), Unit::Paise,
                "The threshold that applies to THIS recipient: the ordinary (non-co-operative) figure, \
                 because s194n_recipient_type says so.", vec![])?;
        }
        None => {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/s194n_recipient_type_unknown"),
                clauses: Vec::new(),
                title: "s.194N: whether the co-operative-society (₹3 crore) or ordinary (₹1 crore) threshold \
                        applies to this recipient is not known".to_string(),
                facts: vec![
                    ("threshold".to_string(), format!("{TEST_ID}.s194n_threshold_paise")),
                    ("threshold_co_operative".to_string(), format!("{TEST_ID}.s194n_threshold_co_operative_paise")),
                    ("recipient_type".to_string(), f_recipient.clone()),
                ],
                evidence: Vec::new(),
                confidence: Confidence::JudgementRequired,
                limits: vec!["s194n_recipient_type was not supplied (or not recognised) for this engagement, so \
                              which of the two thresholds above applies to this assessee as RECIPIENT of its \
                              own cash withdrawals is stated as a limit, never assumed either way."
                    .to_string()],
                ask_client: vec!["Confirm whether the assessee is a co-operative society for s.194N purposes."
                    .to_string()],
            });
        }
    }
    let f_non_filer = fig(&mut r, "s194n_non_filer_threshold_paise", Value::Int(DEFAULT_S194N_THRESHOLD_NON_FILER_PAISE),
        Unit::Paise,
        "The lower threshold (rules.s194n.threshold_non_filer_paise) that applies INSTEAD of the figures \
         above when the recipient has filed no return for ALL THREE preceding assessment years (whose \
         s.139(1) time has expired): 2% from this figure up to s194n_threshold_paise, 5% above it.",
        vec![])?;
    r.findings.push(Finding {
        id: format!("{TEST_ID}/s194n_filer_status_unknown"),
        clauses: Vec::new(),
        title: "s.194N non-filer band: whether the assessee filed returns for the three preceding assessment \
                years is not a books fact".to_string(),
        facts: vec![("non_filer_threshold".to_string(), f_non_filer)],
        evidence: Vec::new(),
        confidence: Confidence::JudgementRequired,
        limits: vec!["If the assessee filed NO return for all three assessment years immediately preceding \
                      this one (whose s.139(1) filing time has expired), the lower non-filer band applies \
                      (2% from ₹20 lakh up to ₹1 crore, 5% above ₹1 crore) instead of the ordinary/co-operative \
                      threshold above -- return-filing history is not visible in these books and is never \
                      assumed either way.".to_string()],
        ask_client: vec!["Confirm return-filing status for the three assessment years immediately preceding \
                          this one.".to_string()],
    });

    match i.bank_statement {
        None => {
            fig(
                &mut r,
                "s194n_coverage",
                Value::Text("no bank statement supplied for this engagement".to_string()),
                Unit::Text,
                "s.194N reads bank-statement narration only; no statement is available for this \
                             client.",
                vec![],
            )?;
        }
        Some(statement) => {
            let terms: BTreeSet<String> = i
                .s194n_narration_terms
                .iter()
                .map(|t| py_upper(t))
                .collect();
            let mut total = 0_i64;
            let mut evidence = Vec::new();
            for row in &statement.rows {
                if row.debit_paise <= 0 {
                    continue;
                }
                let narration = py_upper(&row.narration);
                if terms.iter().any(|t| narration.contains(t.as_str())) {
                    total = add(total, row.debit_paise)?;
                    evidence.push(EvidenceRef::with_label(
                        "document_row",
                        &format!("{}#{}", row.doc, row.row),
                        &prefix_chars(&row.narration, 60),
                    ));
                }
            }
            fig(&mut r, "s194n_coverage",
                Value::Text(format!("{}..{} only -- s.194N is an annual test; no other month has a statement in \
                                     this engagement", iso(&statement.start), iso(&statement.end))),
                Unit::Text,
                "The calendar window this statement covers; s.194N's ₹1 crore (or ₹20 lakh for a non-filer) \
                 threshold is a FULL-YEAR aggregate this one window cannot establish or rule out.", vec![])?;
            fig(&mut r, "s194n_narration_matched_withdrawal_total_paise", Value::Int(total), Unit::Paise,
                "Sum of statement debit rows whose narration matches a caller-supplied withdrawal term, within \
                 the statement's own window only -- informational (the assessee's own exposure as the \
                 RECIPIENT of a s.194N-deducting bank, not a deduction computed here).", evidence)?;
            if terms.is_empty() {
                fig(&mut r, "s194n_narration_terms_note",
                    Value::Text("No cash-withdrawal narration forms were set for this client, so the withdrawal \
                                 total below is nil by construction, not a finding that no cash was withdrawn"
                        .to_string()),
                    Unit::Text, "Coverage note.", vec![])?;
            }
        }
    }

    // ------------------------------------------------------------ AIS (not SFT)
    fig(&mut r, "ais_sft_rows_available_count", Value::Int(0), Unit::Count,
        "AIS Part B2 (SFT) rows tied against the books -- always 0: the AIS reader used for this engagement \
         does not load Part B2 at all (by design, stated in its own documentation). This register cannot tie \
         AIS SFT entries against the books until that reader is extended.", vec![])?;
    fig(&mut r, "ais_other_rows_supplied_count", count(TEST_ID, i.ais_rows.len())?, Unit::Count,
        "AIS rows supplied to this run from parts the AIS reader DOES read (B7 GST turnover/purchases, B3 \
         advance tax, B4 refund) -- informational only; NOT SFT, never conflated with it.", vec![])?;
    if i.ais_rows.is_empty() {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/ais_sft_not_available"),
            clauses: Vec::new(),
            title: "AIS SFT rows are not available to tie against this high-value register".to_string(),
            facts: Vec::new(),
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec!["The AIS reader used for this engagement loads Parts B7/B3/B4 only, not B2 (SFT) -- by \
                          that reader's own design, not a gap in this test alone.".to_string()],
            ask_client: vec!["Supply the AIS Part B2 (SFT) detail export, or confirm the AIS reader should be \
                              extended to read it.".to_string()],
        });
    }
    Ok(r)
}

/// (pairs, total, pairs at or over `threshold` on the line too, pairs over on the party's side
/// only, evidence across every voucher of every pair -- one ref per distinct (GUID, label), as the
/// reference's set of refs).
fn summarise<K>(
    rows: &Rows<'_, K>,
    threshold: i64,
) -> Result<(usize, i64, usize, usize, Vec<EvidenceRef>)> {
    let mut total = 0_i64;
    let (mut over, mut line_below) = (0, 0);
    let mut refs: BTreeSet<(String, String)> = BTreeSet::new();
    for d in rows.values() {
        total = add(total, d.paise)?;
        match d.over(threshold)? {
            Over::Yes => over += 1,
            Over::PartySideOnly => line_below += 1,
            Over::No => {}
        }
        for (g, v) in &d.vouchers {
            refs.insert((g.clone(), voucher_label(v)));
        }
    }
    let evidence = refs
        .into_iter()
        .map(|(g, l)| EvidenceRef::with_label("voucher", &g, &l))
        .collect();
    Ok((rows.len(), total, over, line_below, evidence))
}

/// Python's `str.capitalize()` for the two ASCII mode names ("cash", "bank").
fn capitalize(word: &str) -> String {
    let mut c = word.chars();
    c.next()
        .map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_reference_tables_numbers() {
        // The reference's rules/ay2026-27.toml at 1038dc05: [high_value_register].ca_threshold_paise
        // and [s194n]'s three thresholds; run() prefers those tables, and they equal its defaults.
        assert_eq!(DEFAULT_CA_THRESHOLD_PAISE, 20_000_000);
        assert_eq!(DEFAULT_S194N_THRESHOLD_PAISE, 1_000_000_000);
        assert_eq!(DEFAULT_S194N_THRESHOLD_CO_OPERATIVE_PAISE, 3_000_000_000);
        assert_eq!(DEFAULT_S194N_THRESHOLD_NON_FILER_PAISE, 200_000_000);
    }

    #[test]
    fn the_recipient_type_follows_pack_py() {
        for e in ["individual", "huf", "firm", "llp", "company"] {
            assert_eq!(
                s194n_recipient_type(Some(e)),
                Some(Recipient::NotCoOperative),
                "{e}"
            );
        }
        assert_eq!(
            s194n_recipient_type(Some("cooperative_society")),
            Some(Recipient::CoOperative)
        );
        assert_eq!(s194n_recipient_type(Some("trust")), None);
        assert_eq!(s194n_recipient_type(None), None);
    }

    #[test]
    fn config_values_are_typed_or_refused() {
        let loan = |t: &str| LoanConfig {
            lender: "L".to_string(),
            lender_type: t.to_string(),
            interest_ledger: None,
        };
        let loans = BTreeMap::from([
            ("Loan A".to_string(), loan("bank")),
            ("Loan B".to_string(), loan("relative")),
        ]);
        // The roles map overrides a loan's own lender_type and adds ledgers of its own.
        let roles = BTreeMap::from([
            ("Loan A".to_string(), toml::Value::from("individual")),
            (
                "Gov Co".to_string(),
                toml::Value::from("government_company"),
            ),
        ]);
        let want: BTreeMap<String, String> = [
            ("Gov Co", "government_company"),
            ("Loan A", "individual"),
            ("Loan B", "relative"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(counterparty_types(&loans, &roles).unwrap(), want);
        let bad = BTreeMap::from([("Gov Co".to_string(), toml::Value::from(1))]);
        assert!(counterparty_types(&loans, &bad).is_err());

        assert!(s194n_terms(None).unwrap().is_empty());
        let terms = toml::Value::Array(vec!["ATW-".into(), "NWD-".into()]);
        assert_eq!(s194n_terms(Some(&terms)).unwrap().len(), 2);
        for v in [
            toml::Value::from("ATW-"),
            toml::Value::Array(vec![1.into()]),
            toml::Value::Array(vec!["".into()]),
            toml::Value::Array(vec![" ".into()]),
        ] {
            assert!(s194n_terms(Some(&v)).is_err(), "{v}");
        }
    }

    #[test]
    fn a_contra_voucher_is_never_a_row() {
        use crate::book::{LedgerLine, VoucherStatus};
        // A Contra carrying a party line, which Tally allows in an imported book; the same lines
        // as a Receipt are a row.
        let voucher = |guid: &str, base_type: &str| Voucher {
            guid: guid.to_string(),
            date: TallyDate::parse("20250601").unwrap(),
            base_type: base_type.to_string(),
            status: VoucherStatus::Regular,
            lines: vec![
                LedgerLine {
                    ledger: "Cash".to_string(),
                    amount_paise: 25_000_000,
                },
                LedgerLine {
                    ledger: "Customer A".to_string(),
                    amount_paise: -25_000_000,
                },
            ],
            ..Default::default()
        };
        let book = Book {
            company_name: "Synthetic".to_string(),
            company_guid: "test-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            vouchers: Vec::new(),
            tb: BTreeMap::new(),
        };
        let (cash, none, no_types) = (
            BTreeSet::from(["Cash".to_string()]),
            BTreeSet::new(),
            BTreeMap::new(),
        );
        let x = Exclusions {
            from_party_groups: &[],
            entirely_groups: &[],
            round_off_ledgers: &none,
            counterparty_types: &no_types,
        };
        let parties = |v: &Voucher| -> Vec<String> {
            let rows = mode_rows(
                &[v],
                &book,
                &cash,
                &none,
                Direction::Receipt,
                |v| v.date.clone(),
                &x,
            );
            rows.unwrap().into_keys().map(|(_, party)| party).collect()
        };
        assert!(parties(&voucher("c1", "Contra")).is_empty());
        assert_eq!(parties(&voucher("r1", "Receipt")), ["Customer A"]);
    }
    #[test]
    fn a_row_shows_the_money_line_summed_per_voucher_when_any_voucher_differs() {
        use crate::book::{LedgerLine, VoucherStatus};
        // Cash received from Customer A on one day: g1 (Rs 1.5 lakh, share = line), a second
        // voucher also filed as g1 whose cash is debited Rs 1 lakh and credited Rs 20,000 back (so
        // its share, Rs 80,000, differs from its line, the debit only), and g3 (Rs 50,000, share =
        // line). The row (Rs 2.8 lakh) is over the limit; its cash line is Rs 3 lakh, summed per
        // voucher, both g1 vouchers added together.
        let voucher = |guid: &str, on: &str, base_type: &str, lines: &[(&str, i64)]| Voucher {
            guid: guid.to_string(),
            date: TallyDate::parse(on).unwrap(),
            base_type: base_type.to_string(),
            status: VoucherStatus::Regular,
            lines: lines
                .iter()
                .map(|(ledger, amount_paise)| LedgerLine {
                    ledger: (*ledger).to_string(),
                    amount_paise: *amount_paise,
                })
                .collect(),
            ..Default::default()
        };
        let book = Book {
            company_name: "Synthetic".to_string(),
            company_guid: "test-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            vouchers: vec![
                voucher(
                    "g1",
                    "20250601",
                    "Receipt",
                    &[("Cash", 15_000_000), ("Customer A", -15_000_000)],
                ),
                voucher(
                    "g1",
                    "20250601",
                    "Receipt",
                    &[
                        ("Cash", 10_000_000),
                        ("Cash", -2_000_000),
                        ("Customer A", -8_000_000),
                    ],
                ),
                voucher(
                    "g3",
                    "20250601",
                    "Receipt",
                    &[("Cash", 5_000_000), ("Customer A", -5_000_000)],
                ),
                // Customer B: g4's line is Rs 20,000 over its share and g5's Rs 20,000 under, so
                // the row's totals agree, both exactly at the Rs 2 lakh limit, while each voucher
                // differs.
                voucher(
                    "g4",
                    "20250602",
                    "Receipt",
                    &[
                        ("Cash", 12_000_000),
                        ("Cash", -2_000_000),
                        ("Customer B", -10_000_000),
                    ],
                ),
                voucher(
                    "g5",
                    "20250602",
                    "Receipt",
                    &[
                        ("Cash", 8_000_000),
                        ("Bank", 2_000_000),
                        ("Customer B", -10_000_000),
                    ],
                ),
                // No party: Rs 1.5 lakh paid in cash (and Rs 10,000 of cash taken back) with Rs 1
                // lakh by bank, all against round-off, so the unidentified row's share (Rs 2.4
                // lakh) is over the limit while its cash line (Rs 1.5 lakh) is under it.
                voucher(
                    "g6",
                    "20250603",
                    "Payment",
                    &[
                        ("Cash", -15_000_000),
                        ("Cash", 1_000_000),
                        ("Bank", -10_000_000),
                        ("Round Off", 24_000_000),
                    ],
                ),
                // No party: Rs 2 lakh received, Rs 1.5 lakh by bank and Rs 50,000 in cash, against
                // round-off. Its bank row and its cash row each take the whole Rs 2 lakh, at the
                // threshold, while each line is under it.
                voucher(
                    "g7",
                    "20250604",
                    "Receipt",
                    &[
                        ("Bank", 15_000_000),
                        ("Cash", 5_000_000),
                        ("Round Off", -20_000_000),
                    ],
                ),
            ],
            tb: BTreeMap::new(),
        };
        let (cash, bank) = (
            BTreeSet::from(["Cash".to_string()]),
            BTreeSet::from(["Bank".to_string()]),
        );
        let (none, no_types) = (BTreeSet::new(), BTreeMap::new());
        let round_off = BTreeSet::from(["Round Off".to_string()]);
        let inputs = Inputs {
            cash: &cash,
            bank: &bank,
            threshold_paise: None,
            bank_statement: None,
            s194n_narration_terms: &none,
            ais_rows: &[],
            s194n_recipient_type: None,
            round_off_ledgers: &round_off,
            counterparty_type_by_ledger: &no_types,
        };
        let r = run(&book, &Rules::vendored().unwrap(), &inputs).unwrap();
        let figure = |prefix: &str| {
            let prefix = format!("{TEST_ID}.{prefix}");
            let f = r.figures.iter().find(|f| f.id.starts_with(&prefix));
            f.map(|f| f.value.clone())
        };
        assert_eq!(
            figure("cash_receipt_day_row_amount_"),
            Some(Value::Int(28_000_000))
        );
        assert_eq!(
            figure("cash_receipt_day_row_cash_line_"),
            Some(Value::Int(30_000_000))
        );
        let finding = r
            .findings
            .iter()
            .find(|f| f.id.contains("cash_receipt_day_2025-06-01"));
        let finding = finding.expect("the row is over the limit");
        assert!(finding.facts.iter().any(|(k, _)| k == "cash_line"));
        assert!(!finding
            .limits
            .iter()
            .any(|l| l.contains("is below the threshold")));

        let finding_on = |day: &str, what: &str| {
            let f = r
                .findings
                .iter()
                .find(|f| f.id.contains(&format!("{what}_day_{day}")));
            f.unwrap_or_else(|| panic!("no {what} finding on {day}"))
        };
        let b = finding_on("2025-06-02", "cash_receipt");
        assert!(
            b.facts.iter().any(|(k, _)| k == "cash_line"),
            "each voucher differs"
        );
        // At the limit on both sides is at or over it, not proven under it.
        assert_eq!(
            b.title,
            "Cash received from one party at or over the s.269ST(a) limb (a) person-per-day \
             threshold on 2025-06-02"
        );
        assert_eq!(
            figure("cash_receipt_day_at_or_over_threshold_count"),
            Some(Value::Int(2))
        );
        assert_eq!(
            figure("cash_receipt_day_party_side_over_line_below_count"),
            Some(Value::Int(1))
        );

        let u = finding_on("2025-06-03", "cash_payment");
        assert_eq!(
            figure("cash_payment_day_row_cash_line_"),
            Some(Value::Int(15_000_000))
        );
        assert!(u.facts.iter().any(|(k, _)| k == "cash_line"));
        let n = u.limits.len();
        assert_eq!(
            u.limits[n - 2],
            "The amount is this party's own side of each voucher, not the voucher's own cash \
             line. On one or more of the row's vouchers the two differ, so such a voucher carries \
             other lines as well (such as money moving by bank, a discount or deduction, a \
             round-off, a loan, a counterparty this register leaves out, several parties sharing \
             the line, or money moving the other way). The cash credited on these vouchers is \
             below the threshold, so the cash paid to this party on them is below it too. The \
             cash credited on these vouchers is shown with this row."
        );
        assert!(u.limits[n - 1].starts_with("No party ledger is on this voucher"));
        // Proven under the limit on the line: a true title, the tags kept, and counted apart at
        // both grains.
        assert_eq!(
            u.title,
            "Cash paid to one unidentified party on 2025-06-03: the party's side is at or over \
             the s.269ST(a) limit, but the cash credited on these vouchers is below it"
        );
        assert_eq!(u.clauses, ["s.269ST(a)", "3CD-31(bc)"]);
        for grain in ["day", "voucher"] {
            assert_eq!(
                figure(&format!("cash_payment_{grain}_at_or_over_threshold_count")),
                Some(Value::Int(0))
            );
            assert_eq!(
                figure(&format!(
                    "cash_payment_{grain}_party_side_over_line_below_count"
                )),
                Some(Value::Int(1))
            );
        }
        // The same for a receipt, in both modes, each against its own threshold.
        assert_eq!(
            finding_on("2025-06-04", "cash_receipt").title,
            "Cash received from one unidentified party on 2025-06-04: the party's side is at or \
             over the s.269ST(a) limit, but the cash debited on these vouchers is below it"
        );
        assert_eq!(
            finding_on("2025-06-04", "bank_receipt").title,
            "Bank received from one unidentified party on 2025-06-04: the party's side is at or \
             over the CA-set vouching threshold, but the bank debited on these vouchers is below it"
        );
        assert_eq!(
            figure("bank_receipt_day_at_or_over_threshold_count"),
            Some(Value::Int(0))
        );
        assert_eq!(
            figure("bank_receipt_day_party_side_over_line_below_count"),
            Some(Value::Int(1))
        );
    }
}
