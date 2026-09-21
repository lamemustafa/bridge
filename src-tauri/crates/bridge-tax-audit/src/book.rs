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
    pub opening_paise: i64,
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

#[derive(Debug, Clone)]
pub struct Voucher {
    pub guid: String,
    pub date: TallyDate,
    pub vtype: String,
    pub base_type: String,
    pub number: String,
    pub status: VoucherStatus,
    pub lines: Vec<LedgerLine>,
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
    let mut out = BTreeMap::new();
    for l in root.descendants_named("LEDGER") {
        let Some(name) = l.attr("NAME").filter(|n| !n.is_empty()) else {
            continue;
        };
        let parent = l.child_text("PARENT").to_string();
        let (chain, chain_complete) = chain(&parent, groups);
        let opening_paise = flip(l.child_text("OPENINGBALANCE"), part)?.unwrap_or(0);
        out.insert(
            name.to_string(),
            Ledger {
                name: name.to_string(),
                parent,
                chain,
                chain_complete,
                opening_paise,
                guid: l.child_text("GUID").to_string(),
                masterid: parse_masterid(l.child_text("MASTERID")),
            },
        );
    }
    Ok(out)
}

fn load_tb(root: &Element, part: &str) -> Result<BTreeMap<String, TbRow>> {
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
    if !guid.eq_ignore_ascii_case(&read.company_guid) {
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
