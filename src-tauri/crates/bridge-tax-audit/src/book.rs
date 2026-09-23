//! The audit book: only the fields `cash_44ab` and the book invariants read, built from a
//! verified [`Read`] by the reference Python implementation's rules (its Tally XML adapter and
//! its `tally-read-v1` loader, `load_book`).
//!
//! Conventions, as in the reference model: money is integer paise, debit positive (Tally
//! writes debit negative; the adapter flips once, here); identity is the company GUID; a
//! voucher's status is explicit and an unknown status makes [`Book::population`] refuse.
//!
//! Where the reference implementation keys a dict by name (groups, ledgers, TB rows), a later
//! entry with the same name replaces an earlier one; the maps here do the same.
//!
//! Foreign-currency amounts are refused, as the reference refuses them (FX-1): the ledgers, trial
//! balance and voucher parts are each scanned before any amount in them is read. One divergence,
//! in the refusal's text only: the part is labelled by its id, as every other error here is,
//! where the reference names its file.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::{ExactDecimal, TallyDate};
use serde_json::Value;

use crate::error::{AuditError, Result};
use crate::read::{iso, window_of, Part, Read, Window};
use crate::xml::{self, Element};

/// Group names Tally reserves as primary (the reference engine's `PRIMARY_GROUPS`).
const PRIMARY_GROUPS: [&str; 15] = [
    "Branch / Divisions",
    "Capital Account",
    "Current Assets",
    "Current Liabilities",
    "Direct Expenses",
    "Direct Incomes",
    "Fixed Assets",
    "Indirect Expenses",
    "Indirect Incomes",
    "Investments",
    "Loans (Liability)",
    "Misc. Expenses (ASSET)",
    "Purchase Accounts",
    "Sales Accounts",
    "Suspense A/c",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoucherStatus {
    Regular,
    Optional,
    Cancelled,
    Postdated,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Ledger {
    pub name: String,
    pub parent: String,
    /// Parent first, primary group last.
    pub chain: Vec<String>,
    /// False when the group masters could not resolve the chain to a primary group.
    pub chain_complete: bool,
    /// The ledger master's own `OPENINGBALANCE`, as the read returned it. Do not take it for the
    /// audit year's opening: that holds only when the `ledgers` part was requested with
    /// `SVFROMDATE` = period start, as `docs/tax-audit/read-format-v1.md` specifies, and nothing
    /// here checks that it was (`ledgers` is not a windowed kind). Requested without it, Tally
    /// answers for the company's current period, which on a book already carried into the next
    /// year is the audit year's closing. No test reads it; the audit year's opening is
    /// `TbRow::opening_paise` (`TBALOPENING` of a trial balance windowed to the period).
    pub master_opening_paise: i64,
    /// The Tally GUID (`crate::binding` matches an engagement config's `[ledger_ids]` entry
    /// against this), empty when the read's LEDGER element carried none.
    pub guid: String,
    /// The Tally MASTERID, when the digits parse as a positive integer.
    pub masterid: Option<i64>,
}

impl Ledger {
    pub fn under(&self, group: &str) -> bool {
        self.chain.iter().any(|g| g == group)
    }
}

/// A group master's Tally identity, alongside [`Book::groups`]' parent-chain map. Kept separate
/// from that map (rather than widening its value) because `chain()` walks it by name only and a
/// third of the crate's tests build one by hand; `crate::binding` is the one reader of this map.
#[derive(Debug, Clone, Default)]
pub struct GroupMaster {
    pub guid: String,
    pub masterid: Option<i64>,
}

/// Digits-only MASTERID text to a positive integer, as the reference implementation's `_masterid`
/// reads it (`getattr(obj, "masterid", "") or ""`, then `str.isdigit()`); anything else -- empty,
/// signed, non-numeric -- carries no MASTERID.
pub fn parse_masterid(text: &str) -> Option<i64> {
    let t = xml::py_strip(text);
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
        .then(|| t.parse().ok())
        .flatten()
}

#[derive(Debug, Clone)]
pub struct LedgerLine {
    pub ledger: String,
    pub amount_paise: i64,
}

/// One stock-item line on a voucher, as the reference model's `InventoryLine` holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct InventoryLine {
    /// STOCKITEMNAME, Python-stripped.
    pub item: String,
    /// The first number in the quantity text (`"12.5 Nos"` is 12.5), or `None` when the text
    /// carries none. Quantities are not money, so a float, as in the reference model.
    pub qty: Option<f64>,
    /// RATE's leading number (`"100.00/Nos"`) in paise; always `None` on a stock journal's
    /// IN/OUT line, which the reference reads without a rate.
    pub rate_paise: Option<i64>,
    /// AMOUNT in paise, debit positive; `None` when the text is empty.
    pub amount_paise: Option<i64>,
    /// A stock journal's direction: `Some(1)` from INVENTORYENTRIESIN, `Some(-1)` from
    /// INVENTORYENTRIESOUT, `None` otherwise.
    pub direction: Option<i8>,
    /// Whether the export carried a quantity element at all, empty or not: an empty one is
    /// Tally's value-only line (`qty` `None` is then a fact about the voucher); an absent one
    /// means the read did not ask for the field, and `qty` `None` says nothing.
    pub qty_field_present: bool,
}

#[derive(Debug, Clone)]
pub struct Voucher {
    pub guid: String,
    pub date: TallyDate,
    pub vtype: String,
    pub base_type: String,
    pub number: String,
    pub status: VoucherStatus,
    pub lines: Vec<LedgerLine>,
    /// NARRATION, Python-stripped as the reference's adapter reads it; empty when absent.
    pub narration: String,
    /// PARTYLEDGERNAME, Python-stripped as the reference's adapter reads it; empty when absent.
    /// Tally names only the first party here, so the reference's model does not attribute a
    /// voucher's lines by it in general: counterparties come from ledger lines. `tds_tcs_26as` is
    /// the stated exception, as in the reference: it attributes a TDS/TCS claim and a
    /// capitalisation fact to this party.
    pub party_field: String,
    /// MASTERID as text, Python-stripped, `None` when absent or empty -- the reference model's
    /// `str | None`. Never parsed here: a test that needs a number parses it by its own rule.
    pub masterid: Option<String>,
    /// Stock-item lines, chosen as the reference adapter chooses them (see `inventory_lines`).
    pub inventory: Vec<InventoryLine>,
}

/// Exists so a test or edge-book constructor can name only the fields it sets
/// (`..Voucher::default()`), and a new field does not touch every constructor. The status is
/// `Unknown`, so a voucher that never set one makes [`Book::population`] refuse rather than
/// count; the date is a fixed placeholder, 1 January 1900.
impl Default for Voucher {
    fn default() -> Self {
        Self {
            guid: String::new(),
            date: TallyDate::parse("19000101").expect("a valid fixed date"),
            vtype: String::new(),
            base_type: String::new(),
            number: String::new(),
            status: VoucherStatus::Unknown,
            lines: Vec::new(),
            narration: String::new(),
            party_field: String::new(),
            masterid: None,
            inventory: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TbRow {
    pub opening_paise: i64,
    pub debit_paise: i64,
    pub credit_paise: i64,
    pub closing_paise: i64,
}

impl TbRow {
    pub fn movement_paise(&self) -> i64 {
        self.closing_paise - self.opening_paise
    }
}

#[derive(Debug)]
pub struct Book {
    pub company_name: String,
    pub company_guid: String,
    pub read_at: String,
    pub groups: BTreeMap<String, Option<String>>,
    /// Every group's Tally identity, by name; see [`GroupMaster`].
    pub group_masters: BTreeMap<String, GroupMaster>,
    pub ledgers: BTreeMap<String, Ledger>,
    /// Every exported voucher, all statuses, in read order.
    pub vouchers: Vec<Voucher>,
    pub tb: BTreeMap<String, TbRow>,
}

impl Book {
    /// The books population: regular vouchers only. Refuses while any status is unknown.
    pub fn population(&self) -> Result<Vec<&Voucher>> {
        let unknown = self
            .vouchers
            .iter()
            .filter(|v| v.status == VoucherStatus::Unknown)
            .count();
        if unknown > 0 {
            return Err(AuditError::UnknownVoucherStatus(unknown));
        }
        Ok(self
            .vouchers
            .iter()
            .filter(|v| v.status == VoucherStatus::Regular)
            .collect())
    }

    pub fn excluded(&self) -> impl Iterator<Item = &Voucher> {
        self.vouchers
            .iter()
            .filter(|v| v.status != VoucherStatus::Regular)
    }

    /// The reference engine's `resolve_ledgers`: ledgers whose chain passes through any group.
    pub fn ledgers_under_any(&self, groups: &[String]) -> BTreeSet<String> {
        self.ledgers
            .values()
            .filter(|l| groups.iter().any(|g| l.under(g)))
            .map(|l| l.name.clone())
            .collect()
    }
}

/// Tally amount text to integer paise, as the reference engine's `model.paise` reads it:
/// commas dropped, leading signs stripped (a `-` anywhere among them makes it negative), and
/// the third decimal rounds half away from zero; further decimals are ignored. `None` for
/// empty text. The lexeme must then be one Bridge's [`ExactDecimal`] accepts, which is stricter
/// than the reference engine in one place: `.5` and `5.` are refused rather than read.
pub fn paise(text: &str, part: &str) -> Result<Option<i64>> {
    let s: String = xml::py_strip(text).chars().filter(|c| *c != ',').collect();
    if s.is_empty() {
        return Ok(None);
    }
    let negative = s.starts_with('-');
    let body = s.trim_start_matches(['-', '+']);
    let bad = || AuditError::parse(part, format!("not a plain decimal amount: {text:?}"));
    let lexeme = ExactDecimal::parse(body).map_err(|_| bad())?;
    let (whole, frac) = lexeme
        .as_str()
        .split_once('.')
        .unwrap_or((lexeme.as_str(), ""));
    let frac3: Vec<i64> = format!("{frac}000")
        .bytes()
        .take(3)
        .map(|b| i64::from(b - b'0'))
        .collect();
    let magnitude = whole
        .parse::<i64>()
        .ok()
        .and_then(|w| w.checked_mul(100))
        .and_then(|w| w.checked_add(frac3[0] * 10 + frac3[1] + i64::from(frac3[2] >= 5)))
        .ok_or_else(bad)?;
    Ok(Some(if negative { -magnitude } else { magnitude }))
}

/// Tally amount text (debit negative) to canonical paise (debit positive).
fn flip(text: &str, part: &str) -> Result<Option<i64>> {
    Ok(paise(text, part)?.map(|p| -p))
}

/// The inventory lists whose `AMOUNT` a foreign-currency line can carry, as the reference
/// engine's `_INVENTORY_LISTS` names them.
const INVENTORY_LISTS: [&str; 4] = [
    "ALLINVENTORYENTRIES.LIST",
    "INVENTORYALLOCATIONS.LIST",
    "INVENTORYENTRIESIN.LIST",
    "INVENTORYENTRIESOUT.LIST",
];

/// Whether stripped amount text is a foreign-currency amount, as the reference engine's
/// `FOREIGN_AMOUNT_RE.fullmatch` decides it: Tally writes the foreign amount, the rate and the
/// base amount as one string (`-$ 1100.00 @ I₹ 86/$  = -I₹ 94600.00`).
///
/// The pattern is `\s*-?\S+ -?[\d,]*\.?\d+ @ .+ = -?\S+ -?[\d,]*\.?\d+\s*`, with Python's `\s`
/// ([`xml::is_py_space`]), `\d` ([`support::py_is_decimal`]) and a `.` that is anything but a
/// newline. Its language needs no backtracking. Each end, `-?\S+ -?[\d,]*\.?\d+`, holds exactly
/// one whitespace char, a plain space, and neither begins nor ends with whitespace. So the text
/// matches exactly when:
/// - its second whitespace char starts ` @ `, and what precedes it is an end;
/// - its second-to-last whitespace char ends ` = `, and what follows it is an end;
/// - the text between ` @ ` and ` = ` is not empty and holds no newline.
///
/// The surrounding `\s*` matter only for unstripped text, which is stripped here first.
fn is_foreign_amount(text: &str) -> bool {
    let s: Vec<char> = xml::py_strip(text).chars().collect();
    let spaces: Vec<usize> = (0..s.len()).filter(|&i| xml::is_py_space(s[i])).collect();
    let [first, second, .., second_last, last] = spaces[..] else {
        return false;
    };
    let is_at = |at: usize, lit: &str| s[at..].iter().copied().take(3).eq(lit.chars());
    // Where ` = ` starts, if its closing space is the second-to-last whitespace char.
    let Some(equals) = second_last.checked_sub(2) else {
        return false;
    };
    is_end(&s[..second], first)
        && is_at(second, " @ ")
        && is_at(equals, " = ")
        && equals > second + 3
        && !s[second + 3..equals].contains(&'\n')
        && is_end(&s[second_last + 1..], last - second_last - 1)
}

/// FX-1: the part carries foreign-currency amounts. It is refused before any amount in it is
/// read, naming every ledger or stock item that carries one, sorted and once each, as the
/// reference engine's `ForeignCurrencyRefused` does. The part is labelled by its id, as every
/// other error here is, where the reference names the file.
fn refuse_foreign<'a>(part: &str, names: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let names: BTreeSet<&str> = names.into_iter().collect();
    if names.is_empty() {
        return Ok(());
    }
    Err(AuditError::refused(
        "FX-1",
        format!(
            "{part}: foreign-currency amounts on {} ledger(s) or stock item(s): {}. This engine \
             does not read foreign-currency amounts, so the book is refused rather than any \
             figure being built from them.",
            names.len(),
            names.into_iter().collect::<Vec<_>>().join(", ")
        ),
    ))
}

/// `-?\S+ -?[\d,]*\.?\d+` over text with no whitespace except the char at `space`: a non-empty
/// run before a plain space, then a number.
fn is_end(s: &[char], space: usize) -> bool {
    if space == 0 || s[space] != ' ' {
        return false;
    }
    let number = &s[space + 1..];
    let number = number.strip_prefix(&['-']).unwrap_or(number);
    let digit = |c: &char| crate::support::py_is_decimal(*c);
    let digit_or_comma = |c: &char| *c == ',' || digit(c);
    match number.iter().position(|c| *c == '.') {
        // `[\d,]*\.\d+`: after the one dot, digits only.
        Some(dot) => {
            number[..dot].iter().all(digit_or_comma)
                && dot + 1 < number.len()
                && number[dot + 1..].iter().all(digit)
        }
        // `[\d,]*\d+`: digits and commas, ending in a digit.
        None => number.iter().all(digit_or_comma) && number.last().is_some_and(digit),
    }
}

/// Where the pattern `-?[\d,]*\.?\d+` matches at char index `at`, the end of the match Python's
/// backtracking `re` returns there: the optional sign taken first, then the longest run of
/// digits and commas, shortened one char at a time, and at each length the optional dot tried
/// before its absence; the first way that leaves at least one digit wins, and `\d+` takes every
/// digit that follows. ASCII digits only; callers refuse text a Unicode `\d` could read
/// differently.
fn number_match_end(chars: &[char], at: usize) -> Option<usize> {
    let run = |from: usize, class: fn(&char) -> bool| {
        chars
            .get(from..)
            .map_or(0, |rest| rest.iter().take_while(|c| class(c)).count())
    };
    for sign in [true, false] {
        if sign && chars.get(at) != Some(&'-') {
            continue;
        }
        let body = at + usize::from(sign);
        let longest = run(body, |c| c.is_ascii_digit() || *c == ',');
        for len in (0..=longest).rev() {
            for dot in [true, false] {
                if dot && chars.get(body + len) != Some(&'.') {
                    continue;
                }
                let digits_from = body + len + usize::from(dot);
                let digits = run(digits_from, char::is_ascii_digit);
                if digits > 0 {
                    return Some(digits_from + digits);
                }
            }
        }
    }
    None
}

/// The refusal both number readers share: a non-ASCII decimal digit anywhere in the text. Python's
/// `\d` and `float()` read those digits as numbers; this port reads ASCII only, so such text is
/// refused rather than read as a different number. The set is exactly the one Python's `\d`
/// matches ([`crate::support::py_is_decimal`]), so numeric characters `\d` skips -- `²` in a unit
/// like `m²`, `½` -- are read by both sides the same way and never refused. A refusal fails the
/// whole read, as a malformed amount does.
fn refuse_non_ascii_digits(text: &str, field: &str, part: &str) -> Result<()> {
    if text
        .chars()
        .any(|c| !c.is_ascii() && crate::support::py_is_decimal(c))
    {
        return Err(AuditError::parse(
            part,
            format!("{field} has a non-ASCII digit: {text:?}"),
        ));
    }
    Ok(())
}

/// The reference adapter's `_qty`: the first match of `-?[\d,]*\.?\d+` anywhere in the text,
/// commas removed, read by `float()`; `None` when nothing matches.
fn quantity(text: &str, part: &str) -> Result<Option<f64>> {
    refuse_non_ascii_digits(text, "a quantity", part)?;
    let chars: Vec<char> = text.chars().collect();
    let Some((start, end)) =
        (0..chars.len()).find_map(|at| number_match_end(&chars, at).map(|end| (at, end)))
    else {
        return Ok(None);
    };
    let lexeme: String = chars[start..end].iter().filter(|c| **c != ',').collect();
    // Every lexeme the pattern admits is one Rust's `f64` parser reads, correctly rounded as
    // Python's `float()` is (a leading dot included).
    lexeme
        .parse::<f64>()
        .map(Some)
        .map_err(|_| AuditError::parse(part, format!("not a quantity: {text:?}")))
}

/// The reference adapter's `_rate_paise`: the same pattern, anchored after leading whitespace
/// (`re.match(r"\s*(...)")`), read by `paise`. `paise` here refuses a lexeme starting with a dot
/// where the reference reads it as `0.`, so a `0` is put in front first.
fn rate_paise(text: &str, part: &str) -> Result<Option<i64>> {
    refuse_non_ascii_digits(text, "a rate", part)?;
    let chars: Vec<char> = text.chars().collect();
    let start = chars
        .iter()
        .take_while(|c| crate::support::py_isspace(**c))
        .count();
    let Some(end) = number_match_end(&chars, start) else {
        return Ok(None);
    };
    let lexeme: String = chars[start..end].iter().filter(|c| **c != ',').collect();
    let (sign, body) = lexeme
        .strip_prefix('-')
        .map_or(("", lexeme.as_str()), |rest| ("-", rest));
    let zero = if body.starts_with('.') { "0" } else { "" };
    paise(&format!("{sign}{zero}{body}"), part)
}

/// A voucher's stock-item lines, chosen as the reference adapter chooses them. Inventory can sit
/// at the top level (ALLINVENTORYENTRIES) and also nested under a ledger line
/// (INVENTORYALLOCATIONS), often identically, so reading both would double every line: the
/// top-level entries are taken when any names an item, else the nested ones. A stock journal can
/// list its items there and again in the IN/OUT lists, which carry the direction; when an IN/OUT
/// list names an item, those lists replace both. An entry naming no item is skipped throughout.
fn inventory_lines(v: &Element, part: &str) -> Result<Vec<InventoryLine>> {
    let names_item = |ie: &&Element| !ie.child_text("STOCKITEMNAME").is_empty();
    let io_tags = [
        ("INVENTORYENTRIESIN.LIST", 1),
        ("INVENTORYENTRIESOUT.LIST", -1),
    ];
    let io_present = io_tags
        .iter()
        .any(|(tag, _)| v.children_named(tag).any(|ie| names_item(&ie)));
    let mut out = Vec::new();
    if !io_present {
        let top: Vec<&Element> = v
            .children_named("ALLINVENTORYENTRIES.LIST")
            .filter(names_item)
            .collect();
        let chosen = if top.is_empty() {
            v.children_named("ALLLEDGERENTRIES.LIST")
                .flat_map(|le| le.children_named("INVENTORYALLOCATIONS.LIST"))
                .filter(names_item)
                .collect()
        } else {
            top
        };
        for ie in chosen {
            let qty_text = match ie.child_text("BILLEDQTY") {
                "" => ie.child_text("ACTUALQTY"),
                billed => billed,
            };
            out.push(InventoryLine {
                item: ie.child_text("STOCKITEMNAME").to_string(),
                qty: quantity(qty_text, part)?,
                rate_paise: rate_paise(ie.child_text("RATE"), part)?,
                amount_paise: flip(ie.child_text("AMOUNT"), part)?,
                direction: None,
                qty_field_present: ie.child("BILLEDQTY").is_some()
                    || ie.child("ACTUALQTY").is_some(),
            });
        }
    }
    for (tag, direction) in io_tags {
        for ie in v.children_named(tag).filter(names_item) {
            out.push(InventoryLine {
                item: ie.child_text("STOCKITEMNAME").to_string(),
                qty: quantity(ie.child_text("ACTUALQTY"), part)?,
                rate_paise: None,
                amount_paise: flip(ie.child_text("AMOUNT"), part)?,
                direction: Some(direction),
                qty_field_present: ie.child("ACTUALQTY").is_some(),
            });
        }
    }
    Ok(out)
}

fn tally_date(text: &str, part: &str) -> Result<TallyDate> {
    TallyDate::parse(text)
        .map_err(|_| AuditError::parse(part, format!("not a YYYYMMDD date: {text:?}")))
}

/// Group name to parent group, `None` for a group directly under Tally's reserved root (or with
/// no parent, or one of [`PRIMARY_GROUPS`]). Only the reserved-root marker form ends a chain
/// ([`xml::is_reserved_root`]); a PARENT of `Primary` names a user group called that.
pub fn load_groups(root: &Element) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    for g in root.descendants_named("GROUP") {
        let Some(name) = g.attr("NAME").filter(|n| !n.is_empty()) else {
            continue;
        };
        let parent = g.child_text("PARENT");
        let primary =
            parent.is_empty() || xml::is_reserved_root(parent) || PRIMARY_GROUPS.contains(&name);
        out.insert(name.to_string(), (!primary).then(|| parent.to_string()));
    }
    out
}

/// Every group's Tally identity, by name -- a separate pass over the same element type as
/// [`load_groups`], read only by [`crate::binding`]. Kept as its own function (rather than
/// widening `load_groups`' return) so that function's existing callers, which read only the
/// parent-chain map, are undisturbed.
pub fn load_group_masters(root: &Element) -> BTreeMap<String, GroupMaster> {
    let mut out = BTreeMap::new();
    for g in root.descendants_named("GROUP") {
        let Some(name) = g.attr("NAME").filter(|n| !n.is_empty()) else {
            continue;
        };
        out.insert(
            name.to_string(),
            GroupMaster {
                guid: g.child_text("GUID").to_string(),
                masterid: parse_masterid(g.child_text("MASTERID")),
            },
        );
    }
    out
}

/// The reference engine's `_chain`, including its answers for a missing group (incomplete)
/// and for a cycle (complete). A ledger directly under the reserved root has the decoded
/// PARENT text itself (`"\u{fffd}#4; Primary"`) as its one-element chain.
pub fn chain(parent: &str, groups: &BTreeMap<String, Option<String>>) -> (Vec<String>, bool) {
    if parent.is_empty() || xml::is_reserved_root(parent) {
        let root = if parent.is_empty() { "Primary" } else { parent };
        return (vec![root.to_string()], true);
    }
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut g = Some(parent.to_string());
    while let Some(name) = g.clone().filter(|n| !n.is_empty() && !seen.contains(n)) {
        chain.push(name.clone());
        seen.insert(name.clone());
        if PRIMARY_GROUPS.contains(&name.as_str()) {
            return (chain, true);
        }
        match groups.get(&name) {
            None => return (chain, false),
            Some(next) => g.clone_from(next),
        }
    }
    let complete = g.is_none_or(|n| seen.contains(&n));
    (chain, complete)
}

fn load_ledgers(
    root: &Element,
    groups: &BTreeMap<String, Option<String>>,
    part: &str,
) -> Result<BTreeMap<String, Ledger>> {
    refuse_foreign(
        part,
        root.descendants_named("LEDGER")
            .into_iter()
            .filter(|l| is_foreign_amount(l.child_text("OPENINGBALANCE")))
            .filter_map(|l| l.attr("NAME").filter(|n| !n.is_empty())),
    )?;
    let mut out = BTreeMap::new();
    for l in root.descendants_named("LEDGER") {
        let Some(name) = l.attr("NAME").filter(|n| !n.is_empty()) else {
            continue;
        };
        let parent = l.child_text("PARENT").to_string();
        let (chain, chain_complete) = chain(&parent, groups);
        let master_opening_paise = flip(l.child_text("OPENINGBALANCE"), part)?.unwrap_or(0);
        out.insert(
            name.to_string(),
            Ledger {
                name: name.to_string(),
                parent,
                chain,
                chain_complete,
                master_opening_paise,
                guid: l.child_text("GUID").to_string(),
                masterid: parse_masterid(l.child_text("MASTERID")),
            },
        );
    }
    Ok(out)
}

fn load_tb(root: &Element, part: &str) -> Result<BTreeMap<String, TbRow>> {
    let fields = ["TBALOPENING", "TBALCLOSING", "DEBITTOTALS", "CREDITTOTALS"];
    refuse_foreign(
        part,
        root.descendants_named("LEDGER")
            .into_iter()
            .filter(|l| fields.iter().any(|f| is_foreign_amount(l.child_text(f))))
            .filter_map(|l| l.attr("NAME").filter(|n| !n.is_empty())),
    )?;
    let mut out = BTreeMap::new();
    for l in root.descendants_named("LEDGER") {
        let Some(name) = l.attr("NAME").filter(|n| !n.is_empty()) else {
            continue;
        };
        // TBAL*: debit negative. DEBITTOTALS is exported negative, CREDITTOTALS positive.
        let row = TbRow {
            opening_paise: flip(l.child_text("TBALOPENING"), part)?.unwrap_or(0),
            closing_paise: flip(l.child_text("TBALCLOSING"), part)?.unwrap_or(0),
            debit_paise: paise(l.child_text("DEBITTOTALS"), part)?.unwrap_or(0).abs(),
            credit_paise: paise(l.child_text("CREDITTOTALS"), part)?
                .unwrap_or(0)
                .abs(),
        };
        out.insert(name.to_string(), row);
    }
    Ok(out)
}

/// Voucher type name to its base type (Sales, Payment, Contra, ...), following PARENT until a
/// type is its own parent. A chain that repeats never reaches a base type, so its names are left
/// out, and a voucher of that type is refused rather than given a guessed base type.
fn base_types(root: &Element) -> BTreeMap<String, String> {
    let mut parents = BTreeMap::new();
    for vt in root.descendants_named("VOUCHERTYPE") {
        let Some(name) = vt.attr("NAME").filter(|n| !n.is_empty()) else {
            continue;
        };
        let parent = vt.child_text("PARENT");
        let parent = if parent.is_empty() { name } else { parent };
        parents.insert(name.to_string(), parent.to_string());
    }
    let mut out = BTreeMap::new();
    for name in parents.keys() {
        let mut seen = BTreeSet::new();
        let mut cur = name.clone();
        while parents.get(&cur).is_some_and(|p| *p != cur) && !seen.contains(&cur) {
            seen.insert(cur.clone());
            cur.clone_from(&parents[&cur]);
        }
        if parents.get(&cur) == Some(&cur) {
            out.insert(name.clone(), cur);
        }
    }
    out
}

/// Status side list: masterid to status, plus the date scope it exhaustively covers.
struct SideList {
    statuses: BTreeMap<String, VoucherStatus>,
    scope: Window,
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn load_side_list(part: &Part) -> Result<SideList> {
    let (scope, exhaustive) = part
        .scope
        .clone()
        .ok_or_else(|| AuditError::refused("C4-status-list", "voucher_status_list has no scope"))?;
    if !exhaustive {
        return Err(AuditError::refused(
            "C4-status-list",
            "voucher_status_list scope must be exhaustive to establish regular status",
        ));
    }
    let bad = |detail: &str| AuditError::parse(&part.id, detail.to_string());
    let data: Value = serde_json::from_slice(&part.content).map_err(|_| bad("not JSON"))?;
    let rows = data
        .get("vouchers")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no vouchers list"))?;
    let mut statuses = BTreeMap::new();
    for r in rows {
        let status = if truthy(r.get("optional")) {
            VoucherStatus::Optional
        } else if truthy(r.get("cancelled")) {
            VoucherStatus::Cancelled
        } else if truthy(r.get("postdated")) {
            VoucherStatus::Postdated
        } else if truthy(r.get("void")) {
            VoucherStatus::Cancelled
        } else {
            VoucherStatus::Unknown
        };
        let masterid = match r.get("masterid") {
            Some(Value::String(s)) => xml::py_strip(s).to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => return Err(bad("a row has no masterid")),
        };
        statuses.insert(masterid, status);
    }
    Ok(SideList { statuses, scope })
}

/// The reference engine's `_status`: each flag judged on its own; REGULAR needs positive
/// evidence (both flags present and No, or an exhaustive side list covering the date).
fn status(v: &Element, masterid: &str, date: &TallyDate, side: Option<&SideList>) -> VoucherStatus {
    if v.child_text("ISCANCELLED") == "Yes" {
        return VoucherStatus::Cancelled;
    }
    if v.child_text("ISOPTIONAL") == "Yes" {
        return VoucherStatus::Optional;
    }
    if v.child_text("ISPOSTDATED") == "Yes" {
        return VoucherStatus::Postdated;
    }
    if v.child("ISOPTIONAL").is_some() && v.child("ISPOSTDATED").is_some() {
        return VoucherStatus::Regular;
    }
    match side {
        Some(side) => match side.statuses.get(masterid) {
            Some(status) => *status,
            None if side.scope.from <= *date && *date <= side.scope.to => VoucherStatus::Regular,
            None => VoucherStatus::Unknown,
        },
        None => VoucherStatus::Unknown,
    }
}

struct PartVouchers {
    vouchers: Vec<Voucher>,
    rows: u64,
    alter_id_max: Option<u64>,
}

fn load_vouchers(
    root: &Element,
    part: &str,
    base: &BTreeMap<String, String>,
    side: Option<&SideList>,
) -> Result<PartVouchers> {
    // Every such list in the part, inside a voucher or not; a ledger line with no name is
    // named by its empty name, as the reference names it.
    let named_by = |list: &'static str, name: &'static str| {
        root.descendants_named(list)
            .into_iter()
            .filter(|e| is_foreign_amount(e.child_text("AMOUNT")))
            .map(move |e| e.child_text(name))
    };
    refuse_foreign(
        part,
        named_by("ALLLEDGERENTRIES.LIST", "LEDGERNAME").chain(
            INVENTORY_LISTS
                .into_iter()
                .flat_map(|list| named_by(list, "STOCKITEMNAME")),
        ),
    )?;
    let mut vouchers = Vec::new();
    let mut alter_id_max: Option<u64> = None;
    for v in root.descendants_named("VOUCHER") {
        if v.attr("REMOTEID").is_none_or(str::is_empty) && v.child("GUID").is_none() {
            continue; // the template placeholder row in each TDL export
        }
        let masterid = v.child_text("MASTERID");
        let date = tally_date(v.child_text("DATE"), part)?;
        let status = status(v, masterid, &date, side);
        let mut lines = Vec::new();
        for le in v.children_named("ALLLEDGERENTRIES.LIST") {
            let ledger = le.child_text("LEDGERNAME");
            // An entry with no ledger or no amount is skipped, as the reference engine skips it.
            if let (false, Some(amount)) = (ledger.is_empty(), flip(le.child_text("AMOUNT"), part)?)
            {
                lines.push(LedgerLine {
                    ledger: ledger.to_string(),
                    amount_paise: amount,
                });
            }
        }
        let inventory = inventory_lines(v, part)?;
        let alterid = v.child_text("ALTERID");
        if !alterid.is_empty() {
            let n: u64 = alterid.parse().map_err(|_| {
                AuditError::parse(part, format!("ALTERID is not an integer: {alterid:?}"))
            })?;
            alter_id_max = Some(alter_id_max.map_or(n, |m| m.max(n)));
        }
        let vtype = match v.child_text("VOUCHERTYPENAME") {
            "" => v.attr("VCHTYPE").unwrap_or_default(),
            name => name,
        };
        let guid = match v.child_text("GUID") {
            "" => v.attr("REMOTEID").unwrap_or_default(),
            guid => guid,
        };
        // A type the voucher_types part cannot resolve is refused, never read as its own base
        // type: a custom-named Contra type ("Bank Transfer") would otherwise count as a real
        // receipt or payment and move cash_44ab's shares with no warning.
        let Some(base_type) = base.get(vtype) else {
            return Err(AuditError::refused(
                "C4-vtype-unresolved",
                format!("{part}: voucher type {vtype:?} does not resolve to a base type"),
            ));
        };
        vouchers.push(Voucher {
            guid: guid.to_string(),
            date,
            vtype: vtype.to_string(),
            base_type: base_type.clone(),
            number: v.child_text("VOUCHERNUMBER").to_string(),
            status,
            lines,
            narration: v.child_text("NARRATION").to_string(),
            party_field: v.child_text("PARTYLEDGERNAME").to_string(),
            masterid: (!masterid.is_empty()).then(|| masterid.to_string()),
            inventory,
        });
    }
    Ok(PartVouchers {
        rows: vouchers.len() as u64,
        vouchers,
        alter_id_max,
    })
}

fn required<'a>(read: &'a Read, kind: &str) -> Result<&'a Part> {
    read.one(kind)
        .ok_or_else(|| AuditError::refused("C4-required", format!("no part of kind {kind:?}")))
}

/// Build the book from a verified read: C5 identity, C8 and C9 are checked here.
pub fn load_book(read: &Read, company_name: &str) -> Result<Book> {
    let gp = required(read, "groups")?;
    let gp_root = xml::read(&gp.content, &gp.id)?;
    let groups = load_groups(&gp_root);
    let group_masters = load_group_masters(&gp_root);
    let lp = required(read, "ledgers")?;
    let ledgers = load_ledgers(&xml::read(&lp.content, &lp.id)?, &groups, &lp.id)?;
    crate::ledger_ids::check_no_duplicate_ledger_guids(&ledgers)?;
    let tp = required(read, "trial_balance")?;
    let tb = load_tb(&xml::read(&tp.content, &tp.id)?, &tp.id)?;
    let vp = required(read, "voucher_types")?;
    let base = base_types(&xml::read(&vp.content, &vp.id)?);
    let side = read
        .one("voucher_status_list")
        .map(load_side_list)
        .transpose()?;

    let after_voucher_high_water = read.after.as_ref().and_then(|a| a.alter_voucher_id);
    let mut vouchers = Vec::new();
    for vp in read.voucher_parts() {
        let parsed = load_vouchers(
            &xml::read(&vp.content, &vp.id)?,
            &vp.id,
            &base,
            side.as_ref(),
        )?;
        let w = window_of(vp);
        let outside: Vec<&Voucher> = parsed
            .vouchers
            .iter()
            .filter(|v| v.date < w.from || v.date > w.to)
            .collect();
        if let Some(first) = outside.first() {
            return Err(AuditError::refused(
                "C8-window",
                format!(
                    "{}: {} voucher(s) dated outside {}..{}, first {}",
                    vp.id,
                    outside.len(),
                    iso(&w.from),
                    iso(&w.to),
                    iso(&first.date)
                ),
            ));
        }
        if Some(parsed.rows) != vp.rows || parsed.alter_id_max != vp.alter_id_max {
            return Err(AuditError::refused(
                "C9-rows",
                format!(
                    "{}: declared rows={:?} alter_id_max={:?}, parsed rows={} alter_id_max={:?}",
                    vp.id, vp.rows, vp.alter_id_max, parsed.rows, parsed.alter_id_max
                ),
            ));
        }
        if let (Some(hw), Some(max)) = (after_voucher_high_water, parsed.alter_id_max) {
            if max > hw {
                return Err(AuditError::refused(
                    "C9-high-water",
                    format!(
                        "{}: voucher ALTERID {max} above the closing high-water {hw}",
                        vp.id
                    ),
                ));
            }
        }
        if vp.stored_name != vp.name {
            return Err(AuditError::refused(
                "C2-name",
                format!(
                    "{}: stored file name {:?} != part name {:?}",
                    vp.id, vp.stored_name, vp.name
                ),
            ));
        }
        vouchers.extend(parsed.vouchers);
    }

    let cp = required(read, "company")?;
    let company = xml::read(&cp.content, &cp.id)?;
    let guid = company
        .descendants_named("COMPANY")
        .into_iter()
        .map(|c| c.child_text("GUID"))
        .find(|g| !g.is_empty())
        .ok_or_else(|| {
            AuditError::parse(
                &cp.id,
                "company GUID not established; identity is the GUID, not the name",
            )
        })?;
    if crate::support::py_lower(guid) != crate::support::py_lower(&read.company_guid) {
        return Err(AuditError::refused(
            "C5-identity",
            format!(
                "company part GUID {guid} != manifest company.guid {}",
                read.company_guid
            ),
        ));
    }
    // v1.1: BOOKSFROM is optional in the company part; when present it must agree with the
    // manifest's company.books_from (identity is (GUID, books_from), not the GUID alone).
    let part_books_from = company
        .descendants_named("COMPANY")
        .into_iter()
        .map(|c| c.child_text("BOOKSFROM"))
        .find(|b| !b.is_empty());
    if let Some(part_books_from) = part_books_from {
        let manifest_books_from = read.books_from.as_ref().map(|d| d.as_str());
        if manifest_books_from != Some(part_books_from.trim()) {
            return Err(AuditError::refused(
                "C5-identity",
                format!(
                    "company part BOOKSFROM {part_books_from} != manifest company.books_from {}",
                    read.books_from
                        .as_ref()
                        .map_or("unrecorded".to_string(), crate::read::iso)
                ),
            ));
        }
    }
    Ok(Book {
        company_name: company_name.to_string(),
        company_guid: guid.to_string(),
        read_at: read.read_at.clone(),
        groups,
        group_masters,
        ledgers,
        vouchers,
        tb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nine vouchers reaching every branch of the inventory choice and both number readers.
    /// The expected lines are the reference adapter's own output on the same text
    /// (`tally_xml.load_vouchers`), copied verbatim.
    const PROBE: &str = r"<ENVELOPE><BODY><DATA><COLLECTION>
<VOUCHER><GUID>g-top</GUID><MASTERID> 42 </MASTERID><DATE>20250601</DATE><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <ALLLEDGERENTRIES.LIST><LEDGERNAME>Cust</LEDGERNAME><AMOUNT>-1250.00</AMOUNT>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>Nested Only</STOCKITEMNAME><ACTUALQTY>9 Nos</ACTUALQTY><AMOUNT>900</AMOUNT></INVENTORYALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME> Widget </STOCKITEMNAME><BILLEDQTY>12.5 Nos</BILLEDQTY><ACTUALQTY>13 Nos</ACTUALQTY><RATE>100.00/Nos</RATE><AMOUNT>1250.00</AMOUNT></ALLINVENTORYENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME></STOCKITEMNAME><BILLEDQTY>1 Nos</BILLEDQTY></ALLINVENTORYENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Loose</STOCKITEMNAME><BILLEDQTY></BILLEDQTY><ACTUALQTY>5</ACTUALQTY><RATE>  .50/Nos</RATE><AMOUNT>-2.50</AMOUNT></ALLINVENTORYENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Valueonly</STOCKITEMNAME><BILLEDQTY></BILLEDQTY><RATE>/Nos 5</RATE><AMOUNT></AMOUNT></ALLINVENTORYENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Unasked</STOCKITEMNAME><RATE>-1,000.005/Nos</RATE><AMOUNT>10</AMOUNT></ALLINVENTORYENTRIES.LIST>
</VOUCHER>
<VOUCHER><GUID>g-nested</GUID><MASTERID></MASTERID><DATE>20250602</DATE><VOUCHERTYPENAME>Purchase</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME></STOCKITEMNAME></ALLINVENTORYENTRIES.LIST>
 <ALLLEDGERENTRIES.LIST><LEDGERNAME>Supp</LEDGERNAME><AMOUNT>300</AMOUNT>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>A</STOCKITEMNAME><ACTUALQTY>1,234.5 kg</ACTUALQTY><AMOUNT>-200</AMOUNT></INVENTORYALLOCATIONS.LIST>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>B</STOCKITEMNAME><BILLEDQTY>12,</BILLEDQTY><AMOUNT>-100</AMOUNT></INVENTORYALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST>
 <ALLLEDGERENTRIES.LIST><LEDGERNAME>Stock</LEDGERNAME><AMOUNT>-300</AMOUNT>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>C</STOCKITEMNAME><BILLEDQTY>x-.5 y</BILLEDQTY><AMOUNT>-1</AMOUNT></INVENTORYALLOCATIONS.LIST>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>D</STOCKITEMNAME><BILLEDQTY>1.5.3</BILLEDQTY><AMOUNT>-1</AMOUNT></INVENTORYALLOCATIONS.LIST>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>E</STOCKITEMNAME><BILLEDQTY>--3 5.</BILLEDQTY><AMOUNT>-1</AMOUNT></INVENTORYALLOCATIONS.LIST>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME> </STOCKITEMNAME><BILLEDQTY>3</BILLEDQTY><AMOUNT>-1</AMOUNT></INVENTORYALLOCATIONS.LIST>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>F</STOCKITEMNAME><BILLEDQTY> Nos</BILLEDQTY><AMOUNT>-1</AMOUNT></INVENTORYALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST>
</VOUCHER>
<VOUCHER><GUID>g-journal</GUID><DATE>20250603</DATE><VOUCHERTYPENAME>Stock Journal</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Raw</STOCKITEMNAME><BILLEDQTY>4</BILLEDQTY><RATE>1/Nos</RATE><AMOUNT>4</AMOUNT></ALLINVENTORYENTRIES.LIST>
 <INVENTORYENTRIESOUT.LIST><STOCKITEMNAME>Raw</STOCKITEMNAME><ACTUALQTY>4 Nos</ACTUALQTY><RATE>1/Nos</RATE><AMOUNT>4</AMOUNT></INVENTORYENTRIESOUT.LIST>
 <INVENTORYENTRIESIN.LIST><STOCKITEMNAME>Made</STOCKITEMNAME><BILLEDQTY>2</BILLEDQTY><AMOUNT>-4</AMOUNT></INVENTORYENTRIESIN.LIST>
 <INVENTORYENTRIESIN.LIST><STOCKITEMNAME></STOCKITEMNAME><ACTUALQTY>1</ACTUALQTY></INVENTORYENTRIESIN.LIST>
</VOUCHER>
<VOUCHER><GUID>g-emptyio</GUID><DATE>20250604</DATE><VOUCHERTYPENAME>Stock Journal</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <INVENTORYENTRIESIN.LIST><STOCKITEMNAME></STOCKITEMNAME><ACTUALQTY>1</ACTUALQTY></INVENTORYENTRIESIN.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Kept</STOCKITEMNAME><ACTUALQTY>7</ACTUALQTY><AMOUNT>-7</AMOUNT></ALLINVENTORYENTRIES.LIST>
</VOUCHER>
<VOUCHER><GUID>g-cancel</GUID><DATE>20250605</DATE><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME><ISCANCELLED>Yes</ISCANCELLED>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Void</STOCKITEMNAME><BILLEDQTY>0.1 Nos</BILLEDQTY><RATE>-.5/Nos</RATE><AMOUNT>1</AMOUNT></ALLINVENTORYENTRIES.LIST>
</VOUCHER>
<VOUCHER><GUID>g-outonly</GUID><DATE>20250606</DATE><VOUCHERTYPENAME>Stock Journal</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Both</STOCKITEMNAME><BILLEDQTY>3</BILLEDQTY><AMOUNT>3</AMOUNT></ALLINVENTORYENTRIES.LIST>
 <INVENTORYENTRIESOUT.LIST><STOCKITEMNAME>Gone</STOCKITEMNAME><ACTUALQTY>2</ACTUALQTY><AMOUNT>2</AMOUNT></INVENTORYENTRIESOUT.LIST>
</VOUCHER>
<VOUCHER><GUID>g-inonly</GUID><DATE>20250607</DATE><VOUCHERTYPENAME>Stock Journal</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Both</STOCKITEMNAME><BILLEDQTY>3</BILLEDQTY><AMOUNT>3</AMOUNT></ALLINVENTORYENTRIES.LIST>
 <INVENTORYENTRIESIN.LIST><STOCKITEMNAME>Come</STOCKITEMNAME><ACTUALQTY>1</ACTUALQTY><AMOUNT>-1</AMOUNT></INVENTORYENTRIESIN.LIST>
</VOUCHER>
<VOUCHER><GUID>g-deep</GUID><DATE>20250608</DATE><VOUCHERTYPENAME>Purchase</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <WRAP><ALLLEDGERENTRIES.LIST><LEDGERNAME>Hidden</LEDGERNAME><AMOUNT>5</AMOUNT>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>Deep</STOCKITEMNAME><ACTUALQTY>1</ACTUALQTY><AMOUNT>-5</AMOUNT></INVENTORYALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST></WRAP>
 <ALLLEDGERENTRIES.LIST><LEDGERNAME>Stock</LEDGERNAME><AMOUNT>-5</AMOUNT>
  <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>Direct</STOCKITEMNAME><ACTUALQTY>1</ACTUALQTY><AMOUNT>-5</AMOUNT></INVENTORYALLOCATIONS.LIST></ALLLEDGERENTRIES.LIST>
</VOUCHER>
<VOUCHER><GUID>g-units</GUID><DATE>20250609</DATE><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Cloth</STOCKITEMNAME><BILLEDQTY>5 m²</BILLEDQTY><RATE>2/m²</RATE><AMOUNT>10</AMOUNT></ALLINVENTORYENTRIES.LIST>
 <ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Half</STOCKITEMNAME><BILLEDQTY>1½ kg</BILLEDQTY><AMOUNT>1</AMOUNT></ALLINVENTORYENTRIES.LIST>
</VOUCHER>
</COLLECTION></DATA></BODY></ENVELOPE>";

    type Row<'a> = (
        &'a str,
        Option<f64>,
        Option<i64>,
        Option<i64>,
        Option<i8>,
        bool,
    );

    fn load(text: &str) -> Result<Vec<Voucher>> {
        let root = xml::parse(text, "probe")?;
        let base = ["Sales", "Purchase", "Stock Journal"]
            .into_iter()
            .map(|t| (t.to_string(), t.to_string()))
            .collect();
        Ok(load_vouchers(&root, "probe", &base, None)?.vouchers)
    }

    fn rows(v: &Voucher) -> Vec<Row<'_>> {
        v.inventory
            .iter()
            .map(|i| {
                let InventoryLine {
                    item,
                    qty,
                    rate_paise,
                    amount_paise,
                    direction,
                    qty_field_present,
                } = i;
                (
                    item.as_str(),
                    *qty,
                    *rate_paise,
                    *amount_paise,
                    *direction,
                    *qty_field_present,
                )
            })
            .collect()
    }

    #[test]
    fn inventory_lines_match_the_reference_adapter() {
        let vs = load(PROBE).unwrap();
        let masterids: Vec<(&str, Option<&str>)> = vs
            .iter()
            .map(|v| (v.guid.as_str(), v.masterid.as_deref()))
            .collect();
        assert_eq!(
            masterids,
            [
                ("g-top", Some("42")),
                ("g-nested", None),
                ("g-journal", None),
                ("g-emptyio", None),
                ("g-cancel", None),
                ("g-outonly", None),
                ("g-inonly", None),
                ("g-deep", None),
                ("g-units", None)
            ]
        );
        // Top-level entries win over the nested allocation; an entry naming no item is skipped.
        assert_eq!(
            rows(&vs[0]),
            [
                (
                    "Widget",
                    Some(12.5),
                    Some(10_000),
                    Some(-125_000),
                    None,
                    true
                ),
                ("Loose", Some(5.0), Some(50), Some(250), None, true),
                ("Valueonly", None, None, None, None, true),
                ("Unasked", None, Some(-100_001), Some(-1_000), None, false),
            ]
        );
        // No top-level entry names an item, so the nested allocations are read.
        assert_eq!(
            rows(&vs[1]),
            [
                ("A", Some(1234.5), None, Some(20_000), None, true),
                ("B", Some(12.0), None, Some(10_000), None, true),
                ("C", Some(-0.5), None, Some(100), None, true),
                ("D", Some(1.5), None, Some(100), None, true),
                ("E", Some(-3.0), None, Some(100), None, true),
                ("F", None, None, Some(100), None, true),
            ]
        );
        // IN/OUT lists replace the others: every IN, then every OUT, with no rate.
        assert_eq!(
            rows(&vs[2]),
            [
                ("Made", None, None, Some(400), Some(1), false),
                ("Raw", Some(4.0), None, Some(-400), Some(-1), true),
            ]
        );
        // An IN/OUT list naming no item does not displace the top-level entries.
        assert_eq!(
            rows(&vs[3]),
            [("Kept", Some(7.0), None, Some(700), None, true)]
        );
        // A cancelled voucher's lines are read too; 0.1 is not exact in f32; a negative
        // leading-dot rate.
        assert_eq!(
            rows(&vs[4]),
            [("Void", Some(0.1), Some(-50), Some(-100), None, true)]
        );
        // An OUT list alone, or an IN list alone, replaces the top-level entries.
        assert_eq!(
            rows(&vs[5]),
            [("Gone", Some(2.0), None, Some(-200), Some(-1), true)]
        );
        assert_eq!(
            rows(&vs[6]),
            [("Come", Some(1.0), None, Some(100), Some(1), true)]
        );
        // Only the voucher's own ledger entries carry nested allocations; a deeper one is not read.
        assert_eq!(
            rows(&vs[7]),
            [("Direct", Some(1.0), None, Some(500), None, true)]
        );
        // Numeric characters Python's `\d` does not match (a superscript, a fraction) are read as
        // the reference reads them, not refused.
        assert_eq!(
            rows(&vs[8]),
            [
                ("Cloth", Some(5.0), Some(200), Some(-1000), None, true),
                ("Half", Some(1.0), None, Some(-100), None, true),
            ]
        );
    }

    /// Python's `\d` reads non-ASCII decimal digits; this port refuses the text instead of
    /// reading it as a different number (the reference reads `"١٢ Nos"` as 12 and a fullwidth
    /// `"５ Nos"` as 5).
    #[test]
    fn a_non_ascii_digit_is_refused_not_read() {
        for (qty, rate) in [
            ("\u{0661}\u{0662} Nos", "1/Nos"),
            ("1 Nos", "\u{0661}/Nos"),
            ("\u{FF15} Nos", "1/Nos"),
            ("1 Nos", "\u{FF11}/Nos"),
        ] {
            let text = PROBE
                .replacen("12.5 Nos", qty, 1)
                .replacen("100.00/Nos", rate, 1);
            let err = load(&text).unwrap_err();
            assert!(err.to_string().contains("non-ASCII digit"), "{err}");
        }
    }

    /// A voucher built from `Default` and never given a status keeps the population closed.
    #[test]
    fn a_default_voucher_makes_the_population_refuse() {
        let book = Book {
            company_name: String::new(),
            company_guid: String::new(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            vouchers: vec![Voucher::default()],
            tb: BTreeMap::new(),
        };
        assert!(book.population().is_err());
    }

    // FX-1. Each expected answer below is the reference engine's own, on the same text: its
    // `FOREIGN_AMOUNT_RE.fullmatch` on the stripped string, and its loaders' refusal.

    const FX_CLOSING: &str = "-$ 1100.00 @ I₹ 86/$  = -I₹ 94600.00";
    const FX_OPENING: &str = "-$ 500.00 @ I₹ 84/$  = -I₹ 42000.00";
    const FX_LINE: &str = "$ 400.00 @ I₹ 86/$  = I₹ 34400.00";
    const FX_TAIL: &str = ". This engine does not read foreign-currency amounts, so the book is \
                           refused rather than any figure being built from them.";

    fn fx_refusal<T>(r: Result<T>) -> String {
        let Err(e) = r else { panic!("not refused") };
        assert_eq!(e.code(), Some("FX-1"), "{e}");
        e.to_string()
    }

    #[test]
    fn foreign_amount_matches_what_the_reference_pattern_matches() {
        let cases: [(&str, bool); 37] = [
            (FX_CLOSING, true),
            (FX_LINE, true),
            ("", false),
            ("1100.00", false),
            ("1234.50 Dr", false),
            ("$ 1 @ x = I 2", true),
            ("$ 1 @  = I 2", false),
            ("$ 1 @ a\nb = I 2", false),
            ("$ 1 @ a\rb = I 2", true),
            ("$\n1 @ x = I 2", false),
            ("$ 1 @ x = I\u{a0}2", false),
            ("$\u{a0}1 @ x = I 2", false),
            ("$\u{1c}1 @ x = I 2", false),
            ("\u{1c}$ 1 @ x = I 2\u{1f}", true),
            ("$ \u{663},\u{664} @ x = I \u{661}.\u{662}", true),
            ("$ \u{b2} @ x = I 2", false),
            ("$ 1 @ a @ b = c = I 2", true),
            ("$ .5 @ x = I 5.", false),
            ("$ 5. @ x = I 2", false),
            ("$ -,,5 @ x = I -.5", true),
            ("$ 1.5.3 @ x = I 2", false),
            ("$ --1 @ x = I 2", false),
            ("$ 1.2,3 @ x = I 2", false),
            ("$ , @ x = I 2", false),
            ("$ 1, @ x = I 2", false),
            (" $ 1 @ x = I 2 ", true),
            ("- 1 @ x = - 2", true),
            ("$ 1 @ x = I 2 3", false),
            ("$ 1 2 @ x = I 2", false),
            ("$ 1 @ x =  I 2", false),
            ("$  1 @ x = I 2", false),
            ("$ 1 @\tx = I 2", false),
            ("$ 1 @ \t = I 2", true),
            ("$ 1 @ x =  2", false),
            ("$ 1,000.50 @ x = I 2", true),
            ("$ 1 x x = I 2", false),
            ("$ 1 @ x x I 2", false),
        ];
        for (text, want) in cases {
            assert_eq!(is_foreign_amount(text), want, "{text:?}");
        }
    }

    fn fx_ledger(name: &str, opening: &str) -> String {
        format!("<LEDGER NAME='{name}'><PARENT>Sundry Debtors</PARENT><OPENINGBALANCE>{opening}</OPENINGBALANCE></LEDGER>")
    }

    fn fx_tb_row(name: &str, opening: &str, closing: &str, debit: &str, credit: &str) -> String {
        format!(
            "<LEDGER NAME='{name}'><TBALOPENING>{opening}</TBALOPENING><TBALCLOSING>{closing}</TBALCLOSING>\
             <DEBITTOTALS>{debit}</DEBITTOTALS><CREDITTOTALS>{credit}</CREDITTOTALS></LEDGER>"
        )
    }

    fn fx_voucher(guid: &str, lines: &[(&str, &str)], inventory: &str) -> String {
        let body: String = lines
            .iter()
            .map(|(n, a)| format!("<ALLLEDGERENTRIES.LIST><LEDGERNAME>{n}</LEDGERNAME><AMOUNT>{a}</AMOUNT></ALLLEDGERENTRIES.LIST>"))
            .collect();
        format!(
            "<VOUCHER REMOTEID='r-{guid}'><GUID>{guid}</GUID><DATE>20250902</DATE><VOUCHERTYPENAME>Receipt\
             </VOUCHERTYPENAME><ISOPTIONAL>No</ISOPTIONAL><ISPOSTDATED>No</ISPOSTDATED>{body}{inventory}</VOUCHER>"
        )
    }

    fn fx_ledgers(text: &str) -> Result<BTreeMap<String, Ledger>> {
        load_ledgers(&xml::parse(text, "probe")?, &BTreeMap::new(), "probe")
    }

    fn fx_tb(text: &str) -> Result<BTreeMap<String, TbRow>> {
        load_tb(&xml::parse(text, "probe")?, "probe")
    }

    fn fx_vouchers(text: &str) -> Result<PartVouchers> {
        let base = [("Receipt".to_string(), "Receipt".to_string())].into();
        load_vouchers(&xml::parse(text, "probe")?, "probe", &base, None)
    }

    #[test]
    fn ledger_masters_name_every_foreign_ledger() {
        let text = format!(
            "<ENVELOPE>{}{}{}</ENVELOPE>",
            fx_ledger("Invented FX Debtor B", FX_OPENING),
            fx_ledger("Invented Rupee Debtor", "-2000.00"),
            fx_ledger("Invented FX Debtor A", FX_OPENING)
        );
        assert_eq!(
            fx_refusal(fx_ledgers(&text)),
            format!("FX-1: probe: foreign-currency amounts on 2 ledger(s) or stock item(s): Invented FX Debtor A, Invented FX Debtor B{FX_TAIL}")
        );
    }

    #[test]
    fn a_foreign_ledger_with_no_name_is_skipped_unread() {
        // The reference names ledgers by a non-empty NAME only, and skips a nameless one
        // without reading its amount: nothing is refused and nothing fails to parse.
        let text = format!("<ENVELOPE>{}</ENVELOPE>", fx_ledger("", FX_OPENING));
        assert!(fx_ledgers(&text).is_ok_and(|l| l.is_empty()));
    }

    #[test]
    fn the_trial_balance_names_every_foreign_ledger_once() {
        let text = format!(
            "<ENVELOPE>{}{}{}</ENVELOPE>",
            fx_tb_row(
                "Invented FX Debtor A",
                FX_OPENING,
                FX_CLOSING,
                "0.00",
                "0.00"
            ),
            fx_tb_row("Invented FX Debtor C", "0.00", "0.00", "0.00", FX_LINE),
            fx_tb_row("Invented Rupee Debtor", "0.00", "-2000.00", "0.00", "0.00")
        );
        assert_eq!(
            fx_refusal(fx_tb(&text)),
            format!("FX-1: probe: foreign-currency amounts on 2 ledger(s) or stock item(s): Invented FX Debtor A, Invented FX Debtor C{FX_TAIL}")
        );
        // Each of the four fields is read on its own.
        for (i, field) in ["TBALOPENING", "TBALCLOSING", "DEBITTOTALS", "CREDITTOTALS"]
            .into_iter()
            .enumerate()
        {
            let mut values = ["0.00"; 4];
            values[i] = FX_LINE;
            let row = fx_tb_row("Only", values[0], values[1], values[2], values[3]);
            let got = fx_refusal(fx_tb(&format!("<ENVELOPE>{row}</ENVELOPE>")));
            assert!(
                got.contains("on 1 ledger(s) or stock item(s): Only."),
                "{field}: {got}"
            );
        }
    }

    #[test]
    fn voucher_lines_and_stock_lines_are_named() {
        let stock = format!(
            "<INVENTORYENTRIESIN.LIST><STOCKITEMNAME>Invented Item</STOCKITEMNAME><ACTUALQTY>1 Nos\
             </ACTUALQTY><AMOUNT>{FX_LINE}</AMOUNT></INVENTORYENTRIESIN.LIST>"
        );
        let text = format!(
            "<ENVELOPE>{}{}</ENVELOPE>",
            fx_voucher(
                "g1",
                &[("Cash", "-34400.00"), ("Invented FX Debtor A", FX_LINE)],
                ""
            ),
            fx_voucher(
                "g2",
                &[("Cash", "-100.00"), ("Invented Rupee Debtor", "100.00")],
                &stock
            )
        );
        assert_eq!(
            fx_refusal(fx_vouchers(&text)),
            format!("FX-1: probe: foreign-currency amounts on 2 ledger(s) or stock item(s): Invented FX Debtor A, Invented Item{FX_TAIL}")
        );
    }

    #[test]
    fn every_list_anywhere_in_a_voucher_part_is_scanned() {
        // Each inventory list; an allocation nested in a ledger line; a ledger line outside any
        // voucher, twice, named once; and a ledger line with no name, named by its empty name.
        let stock = format!(
            "<ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>All Item</STOCKITEMNAME><AMOUNT>{FX_LINE}</AMOUNT></ALLINVENTORYENTRIES.LIST>\
             <INVENTORYENTRIESOUT.LIST><STOCKITEMNAME>Out Item</STOCKITEMNAME><AMOUNT>{FX_LINE}</AMOUNT></INVENTORYENTRIESOUT.LIST>\
             <ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>1.00</AMOUNT>\
             <INVENTORYALLOCATIONS.LIST><STOCKITEMNAME>Alloc Item</STOCKITEMNAME><AMOUNT>{FX_LINE}</AMOUNT></INVENTORYALLOCATIONS.LIST>\
             </ALLLEDGERENTRIES.LIST>"
        );
        let stray = format!(
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Stray</LEDGERNAME><AMOUNT>{FX_LINE}</AMOUNT></ALLLEDGERENTRIES.LIST>"
        );
        let text = format!(
            "<ENVELOPE>{stray}{stray}{}</ENVELOPE>",
            fx_voucher("g1", &[("", FX_LINE)], &stock)
        );
        assert_eq!(
            fx_refusal(fx_vouchers(&text)),
            format!("FX-1: probe: foreign-currency amounts on 5 ledger(s) or stock item(s): , All Item, Alloc Item, Out Item, Stray{FX_TAIL}")
        );
    }

    #[test]
    fn a_plain_zero_on_a_foreign_ledger_still_loads() {
        let text = format!(
            "<ENVELOPE>{}</ENVELOPE>",
            fx_tb_row("Invented FX Debtor Z", "0.00", "0.00", "0.00", "0.00")
        );
        assert_eq!(
            fx_tb(&text).unwrap()["Invented FX Debtor Z"].closing_paise,
            0
        );
    }

    #[test]
    fn any_other_malformed_amount_keeps_the_generic_error() {
        let text = format!(
            "<ENVELOPE>{}</ENVELOPE>",
            fx_tb_row("Invented Debtor", "0.00", "1234.50 Dr", "0.00", "0.00")
        );
        let e = fx_tb(&text).expect_err("unreadable");
        assert!(matches!(e, AuditError::Parse { .. }), "{e}");
        assert!(e.to_string().contains("not a plain decimal amount"), "{e}");
    }
}
