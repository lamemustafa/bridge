//! Bind an [`Engagement`]'s ledger and group names to the Book by Tally identity, or refuse.
//!
//! An engagement config names ledgers and groups by display text: `[roles].cash_groups`,
//! `[roles].round_off_ledgers`, `[tds].nature_by_ledger`'s and `[tds].payee_aliases`' keys,
//! `[tds_payees].s194j_category_by_ledger`'s keys, `[loans.loan_ledgers]`'s keys, each loan's
//! `interest_ledger` and `[loans].shared_interest_ledgers`,
//! `[depreciation].block_by_ledger`'s keys, `[depreciation].dep_expense_ledgers`,
//! `[partners.*].interest_ledger`, `[tds_tcs_26as]`'s three ledger lists and its
//! `deductor_aliases` values (the keys are TANs),
//! `[statutory_dues]`'s `salary_expense_ledgers` and `nature_by_ledger` keys,
//! `[creditor_ageing_43bh]`'s `supplier_classification` keys and `mse_interest_ledgers`, the ledger
//! names a legacy trade-creditor JSON source lists, and `[roles].creditor_groups` -- every location
//! this crate's [`Engagement`] reads. Staff rename ledgers between reads, and a name that stops matching used to drop out of
//! a role silently: the figures moved and nothing said why. [`bind`] is the one place a
//! configured name meets the Book; every one of the locations above is bound once, before any
//! test runs, mirroring the reference implementation's `tae/binding.py` (identifier before name,
//! never a score -- see `docs/tax-audit/config-identity-binding-v1.md`).
//!
//! * **Identity.** `[ledger_ids]` (and `[group_ids]`) map a LABEL -- the display name the rest of
//!   the config uses -- to the master's Tally GUID, `{ guid = "...", masterid = N }` when the
//!   MASTERID is also to be checked, or `{ masterid = N }` alone. Every occurrence of a bound
//!   label is rewritten to the master's CURRENT name in this Book.
//! * A bare name (no identity entry) resolves exactly as before -- byte-equal to a ledger (or
//!   group) name in the Book -- but a name that matches nothing is refused ([`BIND_NAME_UNKNOWN`]
//!   / [`BIND_GROUP_UNKNOWN`]), naming the configuration location.
//! * Nothing is ever case-folded, whitespace-folded or fuzzy-matched. The only thing that follows
//!   a rename is the GUID.
//! * A bound label whose master now carries a different name is reported as a [`LabelDrift`], not
//!   refused: the GUID already proves which master the configuration meant, so the figures are
//!   right, but a rename can also mean the ledger was repurposed, which only a person reading the
//!   drift can judge.
//!
//! **Scope.** This port's registry above is a strict subset of the reference implementation's
//! (which also binds names inside `gst_outward`, `related_parties`, and more): only the locations the ported tests actually read. A real client config's `[ledger_ids]`/
//! `[group_ids]` tables are written for the reference implementation's full pack and will
//! typically carry many labels this port never looks at; [`BIND_ID_UNUSED`] is checked only
//! against the locations this module reads, so this crate never refuses over a label some other,
//! unported test consumes. A caller feeding this crate a full production config where every label
//! not read by this port has already been removed from `[ledger_ids]`/`[group_ids]` -- e.g.
//! `examples/local_parity`, which narrows the tables it passes in for exactly this reason -- will
//! see [`BIND_ID_UNUSED`] fire on a genuinely stale entry the same way the reference does.

use std::collections::{BTreeMap, BTreeSet};

use crate::book::Book;
use crate::error::{AuditError, Result};
use crate::Engagement;

pub const BIND_NAME_UNKNOWN: &str = "BIND-NAME-UNKNOWN";
pub const BIND_GROUP_UNKNOWN: &str = "BIND-GROUP-UNKNOWN";
pub const BIND_GUID_UNKNOWN: &str = "BIND-GUID-UNKNOWN";
pub const BIND_MASTERID_UNKNOWN: &str = "BIND-MASTERID-UNKNOWN";
pub const BIND_MASTERID_MISMATCH: &str = "BIND-MASTERID-MISMATCH";
pub const BIND_NO_MASTERID: &str = "BIND-NO-MASTERID";
pub const BIND_BOOK_DUPLICATE_ID: &str = "BIND-BOOK-DUPLICATE-ID";
pub const BIND_ID_MALFORMED: &str = "BIND-ID-MALFORMED";
pub const BIND_ID_UNUSED: &str = "BIND-ID-UNUSED";
pub const BIND_COLLISION: &str = "BIND-COLLISION";

/// The location a legacy trade-creditor source's names are reported under, as the reference names it.
pub const LEGACY_PATH_LABEL: &str = "roles.trade_creditors_source(legacy_json)";

/// A configured identity: a Tally GUID, a MASTERID, or both. Never both absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub guid: Option<String>,
    pub masterid: Option<i64>,
}

/// A label bound by identity whose master now carries a different name in this Book. See the
/// module docs; the fields mirror the reference implementation's `LabelDrift`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDrift {
    pub kind: &'static str, // "ledger" | "group"
    pub label: String,
    /// The GUID the label was bound by, or `"masterid {N}"` when bound by MASTERID alone.
    pub identity: String,
    pub current_name: String,
    /// Every configuration location that used this label, in first-seen order.
    pub paths: Vec<String>,
    /// The GUID of a DIFFERENT master that now carries the label's own text, if one does.
    pub label_now_on: Option<String>,
}

/// What [`bind`] resolved, for a reader that wants to say what happened (a Review Register row,
/// a log line) rather than only using the bound [`Engagement`].
#[derive(Debug, Clone, Default)]
pub struct BindingReport {
    pub drifts: Vec<LabelDrift>,
    /// Distinct labels resolved through `[ledger_ids]`/`[group_ids]`.
    pub bound_by_id: usize,
    /// Distinct bare names resolved by exact name.
    pub bound_by_name: usize,
}

fn malformed(table: &str, label: &str, detail: &str) -> AuditError {
    AuditError::refused(BIND_ID_MALFORMED, format!("[{table}].{label:?}: {detail}"))
}

/// Parse `[ledger_ids]`/`[group_ids]`: label -> [`Identity`]. Absent tables bind nothing.
fn parse_identity_table(cfg: &toml::Table, table: &str) -> Result<BTreeMap<String, Identity>> {
    let Some(value) = cfg.get(table) else {
        return Ok(BTreeMap::new());
    };
    let raw = value.as_table().ok_or_else(|| {
        AuditError::refused(
            BIND_ID_MALFORMED,
            format!("[{table}] must be a table of label = GUID"),
        )
    })?;
    let mut out = BTreeMap::new();
    for (label, v) in raw {
        let (guid, masterid) = match v {
            toml::Value::String(s) => {
                let g = crate::support::py_strip(s).to_string();
                (if g.is_empty() { None } else { Some(g) }, None)
            }
            toml::Value::Table(t) => {
                if t.keys().any(|k| k != "guid" && k != "masterid") {
                    return Err(malformed(
                        table,
                        label,
                        "expected a GUID string or { guid = ..., masterid = ... }",
                    ));
                }
                let guid = match t.get("guid") {
                    None => None,
                    Some(toml::Value::String(s)) => {
                        let g = crate::support::py_strip(s).to_string();
                        (!g.is_empty()).then_some(g)
                    }
                    Some(_) => {
                        return Err(malformed(
                            table,
                            label,
                            "expected a GUID string or { guid = ..., masterid = ... }",
                        ))
                    }
                };
                let masterid = match t.get("masterid") {
                    None => None,
                    Some(toml::Value::Integer(n)) if *n > 0 => Some(*n),
                    Some(_) => {
                        return Err(malformed(
                            table,
                            label,
                            "masterid must be a positive integer",
                        ))
                    }
                };
                (guid, masterid)
            }
            _ => {
                return Err(malformed(
                    table,
                    label,
                    "expected a GUID string or { guid = ..., masterid = ... }",
                ))
            }
        };
        if guid.is_none() && masterid.is_none() {
            return Err(malformed(
                table,
                label,
                "names neither a GUID nor a MASTERID",
            ));
        }
        out.insert(label.clone(), Identity { guid, masterid });
    }
    Ok(out)
}

/// A master's Tally identity, name-keyed, for either ledgers or groups.
struct MasterInfo {
    guid: String, // "" when the master carries none
    masterid: Option<i64>,
}

fn ledger_masters(book: &Book) -> BTreeMap<String, MasterInfo> {
    book.ledgers
        .iter()
        .map(|(n, l)| {
            (
                n.clone(),
                MasterInfo {
                    guid: l.guid.clone(),
                    masterid: l.masterid,
                },
            )
        })
        .collect()
}

fn group_masters(book: &Book) -> BTreeMap<String, MasterInfo> {
    book.group_masters
        .iter()
        .map(|(n, g)| {
            (
                n.clone(),
                MasterInfo {
                    guid: g.guid.clone(),
                    masterid: g.masterid,
                },
            )
        })
        .collect()
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// label -> current name in this Book, by identity; refuses anything that does not bind to
/// exactly one master. The reference implementation's `_resolve_ids`.
fn resolve_ids(
    kind: &'static str,
    table: &str,
    ids: &BTreeMap<String, Identity>,
    masters: &BTreeMap<String, MasterInfo>,
) -> Result<BTreeMap<String, String>> {
    let mut by_guid: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut by_mid: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for (name, m) in masters {
        // The reference: `g = (m.guid or "").strip()`, then `if g:`, keyed by `g.lower()`.
        let g = crate::support::py_strip(&m.guid);
        if !g.is_empty() {
            by_guid
                .entry(crate::support::py_lower(g))
                .or_default()
                .push(name.clone());
        }
        if let Some(mid) = m.masterid {
            by_mid.entry(mid).or_default().push(name.clone());
        }
    }
    let any_mid = !by_mid.is_empty();
    let mut out = BTreeMap::new();
    for (label, ident) in ids {
        let mut name: Option<String> = None;
        if let Some(guid) = ident.guid.as_deref() {
            let hits = by_guid
                .get(&crate::support::py_lower(guid))
                .cloned()
                .unwrap_or_default();
            if hits.len() > 1 {
                return Err(AuditError::refused(
                    BIND_BOOK_DUPLICATE_ID,
                    format!(
                        "[{table}].{label:?}: GUID {guid} is carried by {} {kind}s in this read: {:?}",
                        hits.len(),
                        sorted(hits)
                    ),
                ));
            }
            if hits.is_empty() {
                let why = if by_guid.is_empty() {
                    format!(" (this read carries no {kind} GUIDs at all)")
                } else {
                    String::new()
                };
                return Err(AuditError::refused(
                    BIND_GUID_UNKNOWN,
                    format!(
                        "[{table}].{label:?}: no {kind} with GUID {guid} in this read{why}; it \
was deleted, or this is not the company the configuration was bound to"
                    ),
                ));
            }
            name = Some(hits[0].clone());
        }
        if let Some(mid) = ident.masterid {
            if !any_mid {
                return Err(AuditError::refused(
                    BIND_NO_MASTERID,
                    format!(
                        "[{table}].{label:?}: binds by MASTERID {mid} but this read carries no \
{kind} MASTERIDs"
                    ),
                ));
            }
            let mhits = by_mid.get(&mid).cloned().unwrap_or_default();
            if mhits.len() > 1 {
                return Err(AuditError::refused(
                    BIND_BOOK_DUPLICATE_ID,
                    format!(
                        "[{table}].{label:?}: MASTERID {mid} is carried by {} {kind}s: {:?}",
                        mhits.len(),
                        sorted(mhits)
                    ),
                ));
            }
            match &name {
                None => {
                    if mhits.is_empty() {
                        return Err(AuditError::refused(
                            BIND_MASTERID_UNKNOWN,
                            format!(
                                "[{table}].{label:?}: no {kind} with MASTERID {mid} in this read"
                            ),
                        ));
                    }
                    name = Some(mhits[0].clone());
                }
                Some(n) => {
                    if let Some(actual) = masters[n].masterid {
                        if actual != mid {
                            return Err(AuditError::refused(
                                BIND_MASTERID_MISMATCH,
                                format!(
                                    "[{table}].{label:?}: GUID {} is {kind} {n:?} with MASTERID \
{actual}, not {mid}",
                                    ident.guid.clone().unwrap_or_default()
                                ),
                            ));
                        }
                    }
                }
            }
        }
        out.insert(
            label.clone(),
            name.expect("an Identity always carries a GUID or a MASTERID"),
        );
    }
    Ok(out)
}

/// Binds names of one `kind` ("ledger" | "group") against `current` (identity-resolved labels)
/// and `masters` (every name in the Book), tracking which identity labels were used (for
/// [`BIND_ID_UNUSED`]) and which bare names were seen (for [`BindingReport::bound_by_name`]).
struct Binder<'a> {
    kind: &'static str,
    unknown_code: &'static str,
    table: &'static str,
    current: BTreeMap<String, String>,
    masters: &'a BTreeMap<String, MasterInfo>,
    used: BTreeMap<String, Vec<String>>,
    bare: BTreeSet<String>,
}

impl<'a> Binder<'a> {
    fn new(
        kind: &'static str,
        unknown_code: &'static str,
        table: &'static str,
        current: BTreeMap<String, String>,
        masters: &'a BTreeMap<String, MasterInfo>,
    ) -> Self {
        Self {
            kind,
            unknown_code,
            table,
            current,
            masters,
            used: BTreeMap::new(),
            bare: BTreeSet::new(),
        }
    }

    fn bind_one(&mut self, name: &str, location: &str) -> Result<String> {
        if let Some(bound) = self.current.get(name) {
            self.used
                .entry(name.to_string())
                .or_default()
                .push(location.to_string());
            return Ok(bound.clone());
        }
        if self.masters.contains_key(name) {
            self.bare.insert(name.to_string());
            return Ok(name.to_string());
        }
        Err(AuditError::refused(
            self.unknown_code,
            format!(
                "{location}: no {kind} named {name:?} in this read, and [{table}] does not bind \
it by identity (names are matched exactly; a renamed {kind} is followed only by its GUID -- \
rebind the client configuration's identity table against the read the names were taken from)",
                kind = self.kind,
                table = self.table,
            ),
        ))
    }

    fn bind_list(&mut self, names: &[String], location: &str) -> Result<Vec<String>> {
        names.iter().map(|n| self.bind_one(n, location)).collect()
    }

    /// Binds every key of a table keyed by name (`loans.loan_ledgers`,
    /// `depreciation.block_by_ledger`), and refuses [`BIND_COLLISION`] when two different
    /// configured keys bind to the same current name -- merging their values would be a guess.
    fn bind_keys(
        &mut self,
        keys: impl Iterator<Item = String>,
        location: &str,
    ) -> Result<Vec<(String, String)>> {
        let pairs: Vec<(String, String)> = keys
            .map(|k| {
                let bound = self.bind_one(&k, location)?;
                Ok((k, bound))
            })
            .collect::<Result<_>>()?;
        check_collision(self.kind, location, &pairs)?;
        Ok(pairs)
    }

    /// A table keyed by name, re-keyed by each key's bound name, values unchanged
    /// ([`Self::bind_keys`]).
    fn rebind_map<V: Clone>(
        &mut self,
        map: &BTreeMap<String, V>,
        location: &str,
    ) -> Result<BTreeMap<String, V>> {
        let pairs = self.bind_keys(map.keys().cloned(), location)?;
        Ok(pairs
            .into_iter()
            .map(|(orig, bound)| (bound, map[&orig].clone()))
            .collect())
    }

    fn check_unused(&self) -> Result<()> {
        let unused: Vec<String> = self
            .current
            .keys()
            .filter(|l| !self.used.contains_key(*l))
            .cloned()
            .collect();
        if !unused.is_empty() {
            let unused = sorted(unused);
            return Err(AuditError::refused(
                BIND_ID_UNUSED,
                format!(
                    "[{}] binds {unused:?} but no configuration location this port reads uses \
{noun}; remove the stale entries, or note they are used only by roles this port does not \
implement",
                    self.table,
                    noun = if unused.len() == 1 {
                        "that label"
                    } else {
                        "those labels"
                    },
                ),
            ));
        }
        Ok(())
    }
}

/// The value at a configuration location, or `None` when any level of it is absent or not a
/// table -- the reference's `_expand`, which skips such a location rather than refusing it.
fn raw_at<'a>(cfg: &'a toml::Table, path: &[&str]) -> Option<&'a toml::Value> {
    let (first, rest) = path.split_first()?;
    let mut node = cfg.get(*first)?;
    for key in rest {
        node = node.as_table()?.get(*key)?;
    }
    Some(node)
}

/// The names a list location holds (empty when absent), or `BIND-ID-MALFORMED` when it is not a
/// list of names -- the reference's `names_at(..., "list", ...)`.
fn list_at(cfg: &toml::Table, path: &[&str]) -> Result<Vec<String>> {
    let Some(v) = raw_at(cfg, path) else {
        return Ok(Vec::new());
    };
    v.as_array()
        .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
        .ok_or_else(|| {
            AuditError::refused(
                BIND_ID_MALFORMED,
                format!("{}: expected a list of names, got {v}", path.join(".")),
            )
        })
}

/// The table a keys location holds (`None` when absent), or `BIND-ID-MALFORMED` when it is not a
/// table -- the reference's `names_at(..., "keys", ...)`.
fn table_at<'a>(cfg: &'a toml::Table, path: &[&str]) -> Result<Option<&'a toml::Table>> {
    match raw_at(cfg, path) {
        None => Ok(None),
        Some(v) => v.as_table().map(Some).ok_or_else(|| {
            AuditError::refused(
                BIND_ID_MALFORMED,
                format!(
                    "{}: expected a table keyed by names, got {v}",
                    path.join(".")
                ),
            )
        }),
    }
}

/// Bind every key of a table location, keeping each value as written.
fn bind_table_keys(
    binder: &mut Binder,
    table: Option<&toml::Table>,
    location: &str,
) -> Result<BTreeMap<String, toml::Value>> {
    let Some(table) = table else {
        return Ok(BTreeMap::new());
    };
    let pairs = binder.bind_keys(table.keys().cloned(), location)?;
    Ok(pairs
        .into_iter()
        .map(|(orig, bound)| (bound, table[&orig].clone()))
        .collect())
}

fn check_collision(kind: &'static str, location: &str, pairs: &[(String, String)]) -> Result<()> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut dup: BTreeSet<String> = BTreeSet::new();
    for (_orig, bound) in pairs {
        if !seen.insert(bound.as_str()) {
            dup.insert(bound.clone());
        }
    }
    if !dup.is_empty() {
        return Err(AuditError::refused(
            BIND_COLLISION,
            format!(
                "{location}: two entries bind to the same {kind} {:?}; merging their values \
would be a guess",
                sorted(dup.into_iter().collect())
            ),
        ));
    }
    Ok(())
}

fn drift_rows(
    kind: &'static str,
    ids: &BTreeMap<String, Identity>,
    current: &BTreeMap<String, String>,
    used: &BTreeMap<String, Vec<String>>,
    masters: &BTreeMap<String, MasterInfo>,
) -> Vec<LabelDrift> {
    let mut drifts = Vec::new();
    for (label, name) in current {
        if name == label {
            continue;
        }
        let ident = &ids[label];
        let identity = ident
            .guid
            .clone()
            .unwrap_or_else(|| format!("masterid {}", ident.masterid.unwrap_or_default()));
        let mut seen = BTreeSet::new();
        let paths: Vec<String> = used
            .get(label)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| seen.insert(p.clone()))
            .collect();
        let label_now_on = masters
            .get(label)
            .map(|m| m.guid.clone())
            .filter(|g| !g.is_empty());
        drifts.push(LabelDrift {
            kind,
            label: label.clone(),
            identity,
            current_name: name.clone(),
            paths,
            label_now_on,
        });
    }
    drifts
}

/// Bind `engagement`'s ledger and group names against `book`, returning a copy of `engagement`
/// with every bound label rewritten to its master's current name, and a [`BindingReport`].
/// `engagement` itself is never modified. See the module docs for what is bound and refused.
pub fn bind(engagement: &Engagement, book: &Book) -> Result<(Engagement, BindingReport)> {
    // Mirrors the reference implementation's `bind_config`: one kind is parsed, resolved, walked
    // across every location it names and unused-checked -- completely -- before the other kind
    // even parses its identity table. The reference iterates `("ledger", ...)` before
    // `("group", ...)`, so a config with both an unused `[ledger_ids]` entry and an unrelated
    // problem in `[group_ids]` (an unknown GUID, say) refuses over the ledger side first: the
    // group table's identity is never even resolved. Resolving both kinds up front, as this used
    // to, would let a `[group_ids]` problem surface before an earlier, unused `[ledger_ids]`
    // entry ever gets its chance to refuse -- a different refusal than the reference gives for
    // the same config.
    let ledger_ids = parse_identity_table(&engagement.raw_cfg, "ledger_ids")?;
    let lmasters = ledger_masters(book);
    let ledger_current = resolve_ids("ledger", "ledger_ids", &ledger_ids, &lmasters)?;

    let mut lbinder = Binder::new(
        "ledger",
        BIND_NAME_UNKNOWN,
        "ledger_ids",
        ledger_current,
        &lmasters,
    );

    let round_off_ledgers =
        lbinder.bind_list(&engagement.round_off_ledgers, "roles.round_off_ledgers")?;

    // `[tds]` and `[tds_payees]` bind before `[loans]` and `[depreciation]`, as they come before
    // both in the reference's `LEDGER_PATHS` (only which refusal is reported first depends on it).
    // A payee alias's VALUE is a payee entity label, not a ledger, and is left as written.
    let mut tds = engagement.tds.clone();
    if let Some(t) = tds.as_mut() {
        t.nature_by_ledger = lbinder.rebind_map(&t.nature_by_ledger, "tds.nature_by_ledger")?;
        t.payee_aliases = lbinder.rebind_map(&t.payee_aliases, "tds.payee_aliases")?;
        t.s194j_category_by_ledger = lbinder.rebind_map(
            &t.s194j_category_by_ledger,
            "tds_payees.s194j_category_by_ledger",
        )?;
    }

    // `[tds_tcs_26as]` binds its three ledger lists and its alias VALUES (the keys are TANs), in
    // the reference's key order within the table (each list sorted, as a set, not in config
    // order); before `[loans]`, as in `LEDGER_PATHS`.
    let mut tds_tcs_26as = engagement.tds_tcs_26as.clone();
    if let Some(t) = tds_tcs_26as.as_mut() {
        for (set, location) in [
            (&mut t.tds_ledgers, "tds_tcs_26as.tds_ledgers"),
            (&mut t.tcs_ledgers, "tds_tcs_26as.tcs_ledgers"),
            (
                &mut t.advance_tax_ledgers,
                "tds_tcs_26as.advance_tax_ledgers",
            ),
        ] {
            let names: Vec<String> = set.iter().cloned().collect();
            *set = lbinder.bind_list(&names, location)?.into_iter().collect();
        }
        t.deductor_aliases = t
            .deductor_aliases
            .iter()
            .map(|(tan, ledger)| {
                Ok((
                    tan.clone(),
                    lbinder.bind_one(ledger, "tds_tcs_26as.deductor_aliases")?,
                ))
            })
            .collect::<Result<_>>()?;
    }

    // `[loans]`'s three name locations, in the reference's LEDGER_PATHS order: the loan ledgers
    // (the table's keys), each loan's `interest_ledger`, then `shared_interest_ledgers`. A loan
    // entry that is not a table has no `interest_ledger` location, as the reference's `_expand`
    // skips it; `loans_interest` refuses it when it runs. Each `interest_ledger` is named by the
    // loan's key as written, and lands on the entry under the loan's bound name, as the
    // reference's `bind_config` does since its commit 76310f60.
    let raw_loan_ledgers = table_at(&engagement.raw_cfg, &["loans", "loan_ledgers"])?;
    let loan_pairs = lbinder.bind_keys(
        raw_loan_ledgers.into_iter().flat_map(|t| t.keys().cloned()),
        "loans.loan_ledgers",
    )?;
    let mut loan_ledgers = BTreeMap::new();
    for (orig, bound) in &loan_pairs {
        let mut entry = raw_loan_ledgers.expect("a key came from the table")[orig].clone();
        if let Some(t) = entry.as_table_mut() {
            if let Some(v) = t.get("interest_ledger") {
                let location = format!("loans.loan_ledgers.{orig}.interest_ledger");
                let name = v.as_str().ok_or_else(|| {
                    AuditError::refused(
                        BIND_ID_MALFORMED,
                        format!("{location}: expected a name, got {v}"),
                    )
                })?;
                let name = lbinder.bind_one(name, &location)?;
                t.insert("interest_ledger".to_string(), toml::Value::from(name));
            }
        }
        loan_ledgers.insert(bound.clone(), entry);
    }
    let loans = crate::LoansConfig {
        not_a_table: engagement
            .raw_cfg
            .get("loans")
            .is_some_and(|v| !v.is_table()),
        loan_ledgers,
        shared_interest_ledgers: lbinder.bind_list(
            &list_at(&engagement.raw_cfg, &["loans", "shared_interest_ledgers"])?,
            "loans.shared_interest_ledgers",
        )?,
    };
    let loan_ledgers_configured: Vec<String> = loan_pairs.into_iter().map(|(_, b)| b).collect();

    let mut depreciation = engagement.depreciation.clone();
    if let Some(dep) = depreciation.as_mut() {
        let block_pairs = lbinder.bind_keys(
            dep.block_by_ledger.keys().cloned(),
            "depreciation.block_by_ledger",
        )?;
        let by_orig: BTreeMap<String, String> = block_pairs.into_iter().collect();
        dep.block_by_ledger = dep
            .block_by_ledger
            .iter()
            .map(|(orig, block)| (by_orig[orig].clone(), block.clone()))
            .collect();
        dep.dep_expense_ledgers = lbinder
            .bind_list(
                &dep.dep_expense_ledgers.iter().cloned().collect::<Vec<_>>(),
                "depreciation.dep_expense_ledgers",
            )?
            .into_iter()
            .collect();
    }

    let partner_interest_ledgers = engagement
        .partner_interest_ledgers
        .iter()
        .map(|(key, label)| {
            let bound = lbinder.bind_one(label, &format!("partners.{key}.interest_ledger"))?;
            Ok((key.clone(), bound))
        })
        .collect::<Result<BTreeMap<String, String>>>()?;

    // The two tables `statutory_dues_43b` and `creditor_ageing_43bh` read, in the reference's
    // LEDGER_PATHS order. Only the name locations' shapes are checked here; every value is kept as
    // written and typed when its own test runs (see `CreditorAgeingConfig`).
    let raw = &engagement.raw_cfg;
    let salary_expense_ledgers = lbinder.bind_list(
        &list_at(raw, &["statutory_dues", "salary_expense_ledgers"])?,
        "statutory_dues.salary_expense_ledgers",
    )?;
    let nature_by_ledger = bind_table_keys(
        &mut lbinder,
        table_at(raw, &["statutory_dues", "nature_by_ledger"])?,
        "statutory_dues.nature_by_ledger",
    )?;
    let statutory_dues = crate::StatutoryDuesConfig {
        not_a_table: raw.get("statutory_dues").is_some_and(|v| !v.is_table()),
        nature_by_ledger,
        salary_expense_ledgers,
    };
    let supplier_classification = bind_table_keys(
        &mut lbinder,
        table_at(raw, &["creditor_ageing_43bh", "supplier_classification"])?,
        "creditor_ageing_43bh.supplier_classification",
    )?;
    let mse_interest_ledgers = lbinder.bind_list(
        &list_at(raw, &["creditor_ageing_43bh", "mse_interest_ledgers"])?,
        "creditor_ageing_43bh.mse_interest_ledgers",
    )?;
    let creditor_ageing = crate::CreditorAgeingConfig {
        not_a_table: raw
            .get("creditor_ageing_43bh")
            .is_some_and(|v| !v.is_table()),
        acceptance_lag_days: raw_at(raw, &["creditor_ageing_43bh", "acceptance_lag_days"]).cloned(),
        supplier_classification,
        mse_interest_ledgers,
    };

    // As the reference does, after every other ledger location: a legacy trade-creditor source's
    // names are configuration too, read once here and replaced by the bound list.
    let mut trade_creditors_source = engagement.trade_creditors_source.clone();
    if let Some(names) = crate::legacy_trade_creditor_names(
        engagement.trade_creditors_source.as_ref(),
        &engagement.base_dir,
    )? {
        let bound = lbinder.bind_list(&names, LEGACY_PATH_LABEL)?;
        let mut t = toml::Table::new();
        t.insert("kind".to_string(), toml::Value::from("ledgers"));
        t.insert(
            "ledgers".to_string(),
            toml::Value::Array(bound.into_iter().map(toml::Value::from).collect()),
        );
        trade_creditors_source = Some(toml::Value::Table(t));
    }

    lbinder.check_unused()?;

    let group_ids = parse_identity_table(&engagement.raw_cfg, "group_ids")?;
    let gmasters = group_masters(book);
    let group_current = resolve_ids("group", "group_ids", &group_ids, &gmasters)?;

    let mut gbinder = Binder::new(
        "group",
        BIND_GROUP_UNKNOWN,
        "group_ids",
        group_current,
        &gmasters,
    );

    let cash_groups = gbinder.bind_list(&engagement.cash_groups, "roles.cash_groups")?;
    let bank_groups = gbinder.bind_list(&engagement.bank_groups, "roles.bank_groups")?;
    let creditor_groups = match raw_at(raw, &["roles", "creditor_groups"]) {
        Some(_) => Some(gbinder.bind_list(
            &list_at(raw, &["roles", "creditor_groups"])?,
            "roles.creditor_groups",
        )?),
        None => None,
    };

    gbinder.check_unused()?;

    let mut drifts = drift_rows(
        "ledger",
        &ledger_ids,
        &lbinder.current,
        &lbinder.used,
        &lmasters,
    );
    drifts.extend(drift_rows(
        "group",
        &group_ids,
        &gbinder.current,
        &gbinder.used,
        &gmasters,
    ));
    let report = BindingReport {
        bound_by_id: lbinder.current.len() + gbinder.current.len(),
        bound_by_name: lbinder.bare.len() + gbinder.bare.len(),
        drifts,
    };

    let bound = Engagement {
        cash_groups,
        bank_groups,
        round_off_ledgers,
        loan_ledgers_configured,
        loans,
        depreciation,
        partner_interest_ledgers,
        creditor_groups,
        trade_creditors_source,
        creditor_ageing,
        statutory_dues,
        tds,
        tds_tcs_26as,
        ..engagement.clone()
    };
    Ok((bound, report))
}

/// Review Register rows (filing aid) for each renamed master, plain-worded for a CA reader.
/// Mirrors the reference implementation's `review_register_rows`.
pub fn review_register_rows(drifts: &[LabelDrift]) -> Vec<(String, String)> {
    drifts
        .iter()
        .map(|d| {
            let mut why = format!(
                "The client's setup names the {kind} \"{label}\", but in these books that {kind} \
is now called \"{current}\". Figures follow the {kind} itself (its Tally identity), so they use \
\"{current}\".",
                kind = d.kind,
                label = d.label,
                current = d.current_name,
            );
            if let Some(other) = &d.label_now_on {
                why.push_str(&format!(
                    " A different {} now carries the name \"{}\" (GUID {other}); it was not used.",
                    d.kind, d.label
                ));
            }
            (format!("Client setup: {} \"{}\"", d.kind, d.label), why)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bridge_tally_primitives::TallyDate;

    use super::*;
    use crate::book;
    use crate::book::{LedgerLine, TbRow, Voucher, VoucherStatus};
    use crate::findings::{TestResult, Unit, Value};

    const G_CASH: &str = "11111111-1111-1111-1111-000000000001";
    const G_ROUNDOFF: &str = "11111111-1111-1111-1111-000000000002";
    const G_OTHER: &str = "11111111-1111-1111-1111-000000000003";

    fn base_toml(extra: &str) -> String {
        format!(
            "[client]\n\
             label = \"Test\"\n\
             assessment_year = \"2026-27\"\n\
             \n\
             [period]\n\
             start = \"2025-04-01\"\n\
             end = \"2026-03-31\"\n\
             \n\
             [snapshot]\n\
             format = \"tally-read-v1\"\n\
             path = \"unused\"\n\
             \n\
             [roles]\n\
             cash_groups = [\"Cash-in-Hand\"]\n\
             bank_groups = []\n\
             {extra}"
        )
    }

    fn engagement(extra: &str) -> Engagement {
        Engagement::from_toml(&base_toml(extra), Path::new(".")).unwrap()
    }

    fn engagement_err(extra: &str) -> AuditError {
        Engagement::from_toml(&base_toml(extra), Path::new(".")).unwrap_err()
    }

    fn ledger(name: &str, group: &str, guid: &str, masterid: Option<i64>) -> book::Ledger {
        book::Ledger {
            name: name.to_string(),
            parent: group.to_string(),
            chain: vec![group.to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: guid.to_string(),
            masterid,
        }
    }

    fn group_master(guid: &str, masterid: Option<i64>) -> book::GroupMaster {
        book::GroupMaster {
            guid: guid.to_string(),
            masterid,
        }
    }

    /// A minimal Book: one cash ledger under `cash_group` (with `cash_guid`/`cash_masterid`),
    /// one bank ledger, one sales ledger and one Receipt voucher moving money from Sales to
    /// Cash. Enough for `cash_44ab::run` to produce non-trivial figures.
    fn book(cash_group: &str, cash_guid: &str, cash_masterid: Option<i64>) -> book::Book {
        let ledgers = [
            ledger("Cash", cash_group, cash_guid, cash_masterid),
            ledger("Sales", "Sales Accounts", "", None),
        ]
        .into_iter()
        .map(|l| (l.name.clone(), l))
        .collect();
        book::Book {
            company_name: "Synthetic".to_string(),
            company_guid: "test-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::from([(
                cash_group.to_string(),
                group_master(cash_guid, None),
            )]),
            ledgers,
            vouchers: vec![Voucher {
                narration: String::new(),
                party_field: String::new(),
                guid: "v1".to_string(),
                date: TallyDate::parse("20250410").unwrap(),
                vtype: "Receipt".to_string(),
                base_type: "Receipt".to_string(),
                number: String::new(),
                status: VoucherStatus::Regular,
                lines: vec![
                    LedgerLine {
                        ledger: "Cash".to_string(),
                        amount_paise: 10_000,
                    },
                    LedgerLine {
                        ledger: "Sales".to_string(),
                        amount_paise: -10_000,
                    },
                ],
                ..Default::default()
            }],
            tb: BTreeMap::from([(
                "Cash".to_string(),
                TbRow {
                    opening_paise: 0,
                    debit_paise: 10_000,
                    credit_paise: 0,
                    closing_paise: 10_000,
                },
            )]),
        }
    }

    fn figures(r: &TestResult) -> Vec<(String, Value, Unit)> {
        let mut v: Vec<_> = r
            .figures
            .iter()
            .map(|f| (f.id.clone(), f.value.clone(), f.unit))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    fn run_cash_44ab(book: &book::Book, bound: &Engagement) -> TestResult {
        let rules = crate::rules::Rules::vendored().unwrap();
        let cash = book.ledgers_under_any(&bound.cash_groups);
        let bank = book.ledgers_under_any(&bound.bank_groups);
        crate::cash_44ab::run(book, &rules, &cash, &bank).unwrap()
    }

    // ---- bare names: no identity table at all ----

    #[test]
    fn a_bare_group_name_that_matches_binds_unchanged() {
        let e = engagement("");
        let b = book("Cash-in-Hand", "", None);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(bound.cash_groups, vec!["Cash-in-Hand".to_string()]);
        assert!(report.drifts.is_empty());
        assert_eq!(report.bound_by_name, 1); // just the group; no ledger location is used here
    }

    #[test]
    fn a_bare_group_name_that_matches_nothing_refuses_with_named_code() {
        let e = engagement("");
        let b = book("Cash In Hand", "", None); // renamed in the Book, no identity configured
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_GROUP_UNKNOWN));
        assert!(format!("{err}").contains("roles.cash_groups"));
        assert!(format!("{err}").contains("Cash-in-Hand"));
    }

    #[test]
    fn a_bare_ledger_name_that_matches_nothing_refuses_with_named_code() {
        let e = engagement("round_off_ledgers = [\"Round Off\"]\n");
        let b = book("Cash-in-Hand", "", None); // no "Round Off" ledger at all
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN));
        assert!(format!("{err}").contains("roles.round_off_ledgers"));
    }

    #[test]
    fn no_case_or_space_folding() {
        let b = book("Cash-in-Hand", "", None);
        for spelling in [
            "cash-in-hand",
            "CASH-IN-HAND",
            "Cash-in-Hand ",
            " Cash-in-Hand",
        ] {
            let e = Engagement::from_toml(
                &format!(
                    "[client]\nlabel=\"t\"\nassessment_year=\"2026-27\"\n[period]\nstart=\"2025-04-01\"\nend=\"2026-03-31\"\n[snapshot]\nformat=\"tally-read-v1\"\npath=\"unused\"\n[roles]\ncash_groups=[{spelling:?}]\nbank_groups=[]\n"
                ),
                Path::new("."),
            )
            .unwrap();
            let err = e.bind(&b).unwrap_err();
            assert_eq!(err.code(), Some(BIND_GROUP_UNKNOWN), "{spelling:?}");
        }
    }

    // ---- identity: GUID ----

    #[test]
    fn a_guid_follows_a_rename_and_reports_it() {
        let e = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_CASH:?}\n"));
        let renamed = book("Cash In Hand (New)", G_CASH, None);
        let (bound, report) = e.bind(&renamed).unwrap();
        assert_eq!(bound.cash_groups, vec!["Cash In Hand (New)".to_string()]);
        assert_eq!(report.bound_by_id, 1);
        let (d,) = (&report.drifts[0],);
        assert_eq!(d.kind, "group");
        assert_eq!(d.label, "Cash-in-Hand");
        assert_eq!(d.current_name, "Cash In Hand (New)");
        assert_eq!(d.identity, G_CASH);
        assert_eq!(d.paths, vec!["roles.cash_groups".to_string()]);
        assert_eq!(d.label_now_on, None);
    }

    /// The reference keys masters by `guid.strip().lower()` and looks a configured GUID up the same
    /// way, with Python's whitespace (U+001C..U+001F included) and full case mapping.
    #[test]
    fn a_guid_binds_whatever_its_case_and_python_whitespace() {
        // Letters in the GUID, so its case is actually exercised.
        let guid = "abcdef01-2345-4789-abcd-ef0123456789";
        let upper = guid.to_uppercase();
        assert_ne!(upper, guid);
        let e = engagement(&format!(
            "\n[group_ids]\n\"Cash-in-Hand\" = \"\u{a0}{upper}\u{2003}\"\n"
        ));
        let b = book("Cash In Hand (New)", &format!("\u{1c}{guid}\u{1f}"), None);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(bound.cash_groups, vec!["Cash In Hand (New)".to_string()]);
        assert_eq!(report.bound_by_id, 1);
    }

    /// The same, with Python-only whitespace (U+001D/U+001E) on the configured side and a non-ASCII
    /// letter, which an ASCII-only fold would not match.
    #[test]
    fn a_guid_binds_across_python_only_whitespace_and_non_ascii_case() {
        // TOML forbids raw control characters in a string, so the file carries them escaped.
        let e = engagement("\n[group_ids]\n\"Cash-in-Hand\" = \"\\u001D\u{c4}BC-GUID-1\\u001E\"\n");
        let b = book("Cash In Hand (New)", "\u{e4}bc-guid-1", None);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(bound.cash_groups, vec!["Cash In Hand (New)".to_string()]);
        assert_eq!(report.bound_by_id, 1);
    }

    #[test]
    fn the_same_read_reports_no_drift() {
        let e = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_CASH:?}\n"));
        let b = book("Cash-in-Hand", G_CASH, None);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(bound.cash_groups, vec!["Cash-in-Hand".to_string()]);
        assert!(report.drifts.is_empty());
    }

    #[test]
    fn identity_beats_a_new_master_that_took_the_old_name() {
        // Staff renamed "Cash-in-Hand" to "Cash In Hand (New)" and later created a brand new
        // group literally called "Cash-in-Hand": the configuration still means the first one
        // (by GUID), and the report says the name now belongs to someone else.
        let e = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_CASH:?}\n"));
        let mut b = book("Cash In Hand (New)", G_CASH, None);
        b.group_masters
            .insert("Cash-in-Hand".to_string(), group_master(G_OTHER, None));
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(bound.cash_groups, vec!["Cash In Hand (New)".to_string()]);
        assert_eq!(report.drifts[0].label_now_on.as_deref(), Some(G_OTHER));
    }

    #[test]
    fn an_unknown_guid_refuses() {
        let e = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_OTHER:?}\n"));
        let b = book("Cash-in-Hand", G_CASH, None);
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_GUID_UNKNOWN));
    }

    #[test]
    fn a_guid_carried_by_two_masters_refuses_as_a_book_duplicate() {
        let e = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_CASH:?}\n"));
        let mut b = book("Cash-in-Hand", G_CASH, None);
        b.group_masters
            .insert("Cash-in-Hand (dup)".to_string(), group_master(G_CASH, None));
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_BOOK_DUPLICATE_ID));
    }

    // ---- identity: MASTERID ----

    #[test]
    fn masterid_alone_binds_when_the_read_carries_them() {
        let e = engagement("round_off_ledgers = [\"Round Off\"]\n[ledger_ids]\n\"Round Off\" = { masterid = 177 }\n");
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Round Off (renamed)".to_string(),
            ledger("Round Off (renamed)", "Indirect Expenses", "", Some(177)),
        );
        let (bound, _report) = e.bind(&b).unwrap();
        assert_eq!(
            bound.round_off_ledgers,
            vec!["Round Off (renamed)".to_string()]
        );
    }

    #[test]
    fn masterid_alone_on_a_read_without_masterids_refuses() {
        let e = engagement("round_off_ledgers = [\"Round Off\"]\n[ledger_ids]\n\"Round Off\" = { masterid = 177 }\n");
        let b = book("Cash-in-Hand", "", None); // no ledger carries a MASTERID
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_NO_MASTERID));
    }

    #[test]
    fn an_unknown_masterid_refuses() {
        let e = engagement("round_off_ledgers = [\"Round Off\"]\n[ledger_ids]\n\"Round Off\" = { masterid = 177 }\n");
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Other".to_string(),
            ledger("Other", "Indirect Expenses", "", Some(9)),
        );
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_MASTERID_UNKNOWN));
    }

    #[test]
    fn guid_and_masterid_disagreeing_refuse() {
        let e = engagement(&format!(
            "round_off_ledgers = [\"Round Off\"]\n[ledger_ids]\n\"Round Off\" = {{ guid = {G_ROUNDOFF:?}, masterid = 178 }}\n"
        ));
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Round Off (renamed)".to_string(),
            ledger(
                "Round Off (renamed)",
                "Indirect Expenses",
                G_ROUNDOFF,
                Some(177),
            ),
        );
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_MASTERID_MISMATCH));
    }

    // ---- malformed [ledger_ids] / [group_ids] ----

    #[test]
    fn every_malformed_identity_shape_refuses() {
        let bad_values = [
            "42",
            "{ guid = \"\" }",
            "{ guid = \"x\", extra = 1 }",
            "{ masterid = -1 }",
            "{ masterid = 0 }",
        ];
        for bad in bad_values {
            let e = engagement(&format!(
                "\n[ledger_ids]\n\"Round Off\" = {bad}\nround_off_ledgers = [\"Round Off\"]\n"
            ));
            let b = book("Cash-in-Hand", "", None);
            let err = e.bind(&b).unwrap_err();
            assert_eq!(err.code(), Some(BIND_ID_MALFORMED), "{bad}");
        }
    }

    #[test]
    fn a_non_table_ledger_ids_value_refuses_at_parse() {
        // `ledger_ids` must be declared before any `[table]` header to land at the top level
        // rather than nesting inside one.
        let text = format!("ledger_ids = 5\n{}", base_toml(""));
        let e = Engagement::from_toml(&text, Path::new(".")).unwrap();
        let b = book("Cash-in-Hand", "", None);
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_ID_MALFORMED));
    }

    // ---- BIND-ID-UNUSED ----

    #[test]
    fn an_unused_identity_entry_refuses() {
        let e = engagement(&format!(
            "round_off_ledgers = [\"Round Off\"]\n[ledger_ids]\n\"Round Off\" = {G_ROUNDOFF:?}\n\"Never Used\" = {G_OTHER:?}\n"
        ));
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Round Off".to_string(),
            ledger("Round Off", "Indirect Expenses", G_ROUNDOFF, None),
        );
        // G_OTHER must resolve to a real master too, or the eager identity resolution refuses
        // BIND-GUID-UNKNOWN before this test ever reaches the unused check -- an unused entry
        // is one no configured location reads, not one whose GUID is also missing.
        b.ledgers.insert(
            "Someone Else's Ledger".to_string(),
            ledger("Someone Else's Ledger", "Indirect Expenses", G_OTHER, None),
        );
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_ID_UNUSED));
        assert!(format!("{err}").contains("Never Used"));
    }

    #[test]
    fn an_unused_ledger_entry_refuses_before_an_unrelated_group_problem_is_even_reached() {
        // Two independent problems in one config: [ledger_ids] binds a label no ledger location
        // reads, and [group_ids] names a GUID no group in the Book carries at all. The reference
        // implementation's `bind_config` processes one kind completely -- parse its identity
        // table, resolve it, walk every location of that kind, then run that kind's unused check
        // -- before the other kind's identity table is even parsed, in kind order (ledger, then
        // group). So here the ledger pass's unused check refuses first; the group pass, and its
        // unknown GUID, is never reached. Resolving every kind's identity table up front (the bug
        // this test guards against) would instead refuse BIND-GUID-UNKNOWN, because that
        // resolution ran before either kind's locations, or its unused check, were reached.
        let e = engagement(&format!(
            "round_off_ledgers = [\"Round Off\"]\n\
             [ledger_ids]\n\"Round Off\" = {G_ROUNDOFF:?}\n\"Never Used\" = {G_OTHER:?}\n\
             [group_ids]\n\"Some Group\" = \"11111111-1111-1111-1111-0000000000ff\"\n"
        ));
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Round Off".to_string(),
            ledger("Round Off", "Indirect Expenses", G_ROUNDOFF, None),
        );
        // As in the test above, G_OTHER must resolve to a real master, or the ledger pass itself
        // refuses BIND-GUID-UNKNOWN before ever reaching its own unused check.
        b.ledgers.insert(
            "Someone Else's Ledger".to_string(),
            ledger("Someone Else's Ledger", "Indirect Expenses", G_OTHER, None),
        );
        // No master anywhere carries "...-0000000000ff": if the group side were ever resolved,
        // this would refuse BIND-GUID-UNKNOWN instead.
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_ID_UNUSED));
        assert!(format!("{err}").contains("Never Used"));
    }

    // ---- BIND-COLLISION ----

    #[test]
    fn two_loan_labels_binding_to_the_same_ledger_collide() {
        let e = Engagement::from_toml(
            &base_toml(&format!(
                "\n[ledger_ids]\n\"Loan A\" = {G_ROUNDOFF:?}\n\"Loan B\" = {G_ROUNDOFF:?}\n\
                 \n[loans.loan_ledgers.\"Loan A\"]\nlender = \"x\"\n\
                 \n[loans.loan_ledgers.\"Loan B\"]\nlender = \"y\"\n"
            )),
            Path::new("."),
        )
        .unwrap();
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "The Only Loan".to_string(),
            ledger("The Only Loan", "Loans (Liability)", G_ROUNDOFF, None),
        );
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_COLLISION));
    }

    // ---- depreciation locations ----

    #[test]
    fn depreciation_block_by_ledger_keys_and_dep_expense_ledgers_are_bound() {
        let e = Engagement::from_toml(
            &base_toml(&format!(
                "\n[ledger_ids]\n\"Office Furniture\" = {G_ROUNDOFF:?}\n\
                 \n[depreciation]\ndep_expense_ledgers = [\"Depreciation A/c\"]\n\
                 \n[depreciation.block_by_ledger]\n\"Office Furniture\" = \"furniture_10\"\n\
                 \n[depreciation.opening_wdv_paise]\nfurniture_10 = 0\n"
            )),
            Path::new("."),
        )
        .unwrap();
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Furniture (renamed)".to_string(),
            ledger("Furniture (renamed)", "Fixed Assets", G_ROUNDOFF, None),
        );
        b.ledgers.insert(
            "Depreciation A/c".to_string(),
            ledger("Depreciation A/c", "Indirect Expenses", "", None),
        );
        let (bound, report) = e.bind(&b).unwrap();
        let dep = bound.depreciation.unwrap();
        assert_eq!(
            dep.block_by_ledger
                .get("Furniture (renamed)")
                .map(String::as_str),
            Some("furniture_10")
        );
        assert!(dep.dep_expense_ledgers.contains("Depreciation A/c"));
        assert_eq!(report.drifts[0].current_name, "Furniture (renamed)");
    }

    // ---- [creditor_ageing_43bh], [statutory_dues], creditor groups and a legacy source ----

    #[test]
    fn creditor_ageing_and_statutory_dues_names_are_bound_by_identity() {
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Supplier A\" = {G_ROUNDOFF:?}\n\"PF Payable\" = {G_OTHER:?}\n\
             \"Wages\" = {G_CASH:?}\n\
             \n[creditor_ageing_43bh]\nmse_interest_ledgers = [\"MSME Interest\"]\n\
             \n[creditor_ageing_43bh.supplier_classification]\n\"Supplier A\" = \"micro\"\n\
             \n[statutory_dues]\nsalary_expense_ledgers = [\"Wages\"]\n\
             \n[statutory_dues.nature_by_ledger]\n\"PF Payable\" = \"pf_employee\"\n"
        ));
        let mut b = book("Cash-in-Hand", "", None);
        for (name, group, guid) in [
            ("Supplier (renamed)", "Sundry Creditors", G_ROUNDOFF),
            ("PF (renamed)", "Duties & Taxes", G_OTHER),
            ("Salaries (renamed)", "Indirect Expenses", G_CASH),
            ("MSME Interest", "Indirect Expenses", ""),
        ] {
            b.ledgers
                .insert(name.to_string(), ledger(name, group, guid, None));
        }
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(
            bound.creditor_ageing.supplier_classification,
            BTreeMap::from([("Supplier (renamed)".to_string(), toml::Value::from("micro"))])
        );
        assert_eq!(
            bound.creditor_ageing.mse_interest_ledgers,
            vec!["MSME Interest"]
        );
        assert_eq!(
            bound.statutory_dues.nature_by_ledger,
            BTreeMap::from([("PF (renamed)".to_string(), toml::Value::from("pf_employee"))])
        );
        assert_eq!(
            bound.statutory_dues.salary_expense_ledgers,
            vec!["Salaries (renamed)"]
        );
        assert_eq!(report.drifts.len(), 3);
    }

    /// A malformed VALUE in a test's own table fails that test only: binding (every test) still
    /// succeeds, as in the reference, where only the test that reads the value raises.
    #[test]
    fn a_malformed_value_in_a_tests_own_table_fails_only_that_test() {
        let rules = crate::rules::Rules::vendored().unwrap();
        let b = book("Cash-in-Hand", "", None);
        for extra in [
            "creditor_groups = [\"Cash-in-Hand\"]\ntrade_creditors_source = { kind = \"groups\" }\n\
             \n[creditor_ageing_43bh]\nacceptance_lag_days = \"three\"\n",
            "creditor_groups = [\"Cash-in-Hand\"]\ntrade_creditors_source = { kind = \"groups\" }\n\
             \n[[creditor_ageing_43bh]]\nacceptance_lag_days = 1\n",
        ] {
            let e = engagement(extra);
            assert!(e.bind(&b).is_ok(), "{extra}");
            assert!(crate::cash_44ab_on(&e, &b, &rules).is_ok(), "{extra}");
            assert!(crate::creditor_ageing_43bh_on(&e, &b, &rules).is_err(), "{extra}");
            assert!(crate::statutory_dues_43b_on(&e, &b, &rules).is_ok(), "{extra}");
        }
        for extra in [
            "\n[statutory_dues.nature_by_ledger]\n\"Cash\" = 5\n",
            "\n[[statutory_dues]]\nnature_by_ledger = {}\n",
        ] {
            let e = engagement(extra);
            assert!(crate::cash_44ab_on(&e, &b, &rules).is_ok(), "{extra}");
            assert!(
                crate::statutory_dues_43b_on(&e, &b, &rules).is_err(),
                "{extra}"
            );
        }
        // A value typed at run: a lag written as a float runs, truncated as `int()` truncates.
        let e = engagement(
            "creditor_groups = [\"Cash-in-Hand\"]\ntrade_creditors_source = { kind = \"groups\" }\n\
             \n[creditor_ageing_43bh]\nacceptance_lag_days = 3.7\n",
        );
        assert!(crate::creditor_ageing_43bh_on(&e, &b, &rules).is_ok());
    }

    /// A malformed SHAPE at a name location is a binding refusal for every test, as the
    /// reference's `names_at` refuses it.
    #[test]
    fn a_malformed_name_location_refuses_binding() {
        for extra in [
            "\n[creditor_ageing_43bh]\nmse_interest_ledgers = \"MSME Interest\"\n",
            "\n[creditor_ageing_43bh]\nsupplier_classification = [\"Cash\"]\n",
            "\n[statutory_dues]\nsalary_expense_ledgers = \"Wages\"\n",
            "\n[statutory_dues]\nnature_by_ledger = 5\n",
            "creditor_groups = \"Cash-in-Hand\"\n",
        ] {
            let err = engagement(extra)
                .bind(&book("Cash-in-Hand", "", None))
                .unwrap_err();
            assert_eq!(err.code(), Some(BIND_ID_MALFORMED), "{extra}");
        }
    }

    #[test]
    fn an_unknown_creditor_group_refuses_naming_its_location() {
        let e = engagement("creditor_groups = [\"No Such Group\"]\n");
        let err = e.bind(&book("Cash-in-Hand", "", None)).unwrap_err();
        assert_eq!(err.code(), Some(BIND_GROUP_UNKNOWN));
        assert!(format!("{err}").contains("roles.creditor_groups"));
    }

    #[test]
    fn a_legacy_trade_creditor_source_is_read_bound_and_replaced() {
        let dir =
            std::env::temp_dir().join(format!("bridge-tax-audit-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("legacy.json"),
            r#"{"derived": {"fs": {"trade_creditors": [{"ledger": "Supplier A"}]}}}"#,
        )
        .unwrap();
        let toml = base_toml(&format!(
            "trade_creditors_source = {{ kind = \"legacy_json\", path = \"legacy.json\" }}\n\
             \n[ledger_ids]\n\"Supplier A\" = {G_ROUNDOFF:?}\n"
        ));
        let e = Engagement::from_toml(&toml, &dir).unwrap();
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            "Supplier (renamed)".to_string(),
            ledger("Supplier (renamed)", "Sundry Creditors", G_ROUNDOFF, None),
        );
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(
            crate::trade_creditors(&bound, &b).unwrap(),
            BTreeSet::from(["Supplier (renamed)".to_string()])
        );
        assert_eq!(report.drifts[0].paths, vec![LEGACY_PATH_LABEL]);
        // A legacy name that matches nothing refuses, naming the legacy location.
        let bare = Engagement::from_toml(
            &base_toml(
                "trade_creditors_source = { kind = \"legacy_json\", path = \"legacy.json\" }\n",
            ),
            &dir,
        )
        .unwrap();
        let err = bare.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN));
        assert!(format!("{err}").contains(LEGACY_PATH_LABEL));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // ---- [tds] and [tds_payees] (tds_payees) ----

    const TDS_TABLES: &str =
        "\n[tds.nature_by_ledger]\n\"Freight\" = \"194C\"\n\"Fees\" = \"194J\"\n\
         \n[tds.payee_aliases]\n\"Carrier One\" = \"Carrier group\"\n\
         \n[tds_payees.s194j_category_by_ledger]\n\"Fees\" = \"professional\"\n\"Royalty\" = \"royalty\"\n";

    fn book_with_tds_ledgers(freight: &str, freight_guid: &str) -> book::Book {
        let mut b = book("Cash-in-Hand", "", None);
        for (name, group, guid) in [
            (freight, "Direct Expenses", freight_guid),
            ("Fees", "Indirect Expenses", ""),
            ("Royalty", "Indirect Expenses", ""),
            ("Carrier One", "Sundry Creditors", ""),
        ] {
            b.ledgers
                .insert(name.to_string(), ledger(name, group, guid, None));
        }
        b
    }

    /// Every `[tds]`/`[tds_payees]` key is bound, as the reference binds them: a key bound by
    /// identity follows a rename, and a payee alias's value -- an entity label, not a ledger -- is
    /// kept as written.
    #[test]
    fn tds_keys_are_bound_and_alias_values_are_left_alone() {
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Freight\" = {G_ROUNDOFF:?}\n{TDS_TABLES}"
        ));
        let (bound, report) = e
            .bind(&book_with_tds_ledgers("Freight (renamed)", G_ROUNDOFF))
            .unwrap();
        let tds = bound.tds.unwrap();
        assert_eq!(
            tds.nature_by_ledger.keys().collect::<Vec<_>>(),
            ["Fees", "Freight (renamed)"]
        );
        assert_eq!(tds.nature_by_ledger["Freight (renamed)"], "194C");
        assert_eq!(tds.payee_aliases["Carrier One"], "Carrier group");
        assert_eq!(
            tds.s194j_category_by_ledger["Fees"].as_deref(),
            Some("professional")
        );
        assert_eq!(report.drifts[0].current_name, "Freight (renamed)");
    }

    /// A payee ledger and a 194J category ledger renamed in Tally but bound by identity keep their
    /// alias and category under the new names: without this a renamed 194J ledger would keep its
    /// nature and lose its category, turning computed findings into unmapped ones.
    #[test]
    fn tds_alias_and_category_keys_follow_a_rename_by_identity() {
        const G_CARRIER: &str = "11111111-1111-1111-1111-000000000004";
        const G_ROYALTY: &str = "11111111-1111-1111-1111-000000000005";
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Carrier One\" = {G_CARRIER:?}\n\"Royalty\" = {G_ROYALTY:?}\n{TDS_TABLES}"
        ));
        let mut b = book_with_tds_ledgers("Freight", "");
        for (old, new, guid) in [
            ("Carrier One", "Carrier One (renamed)", G_CARRIER),
            ("Royalty", "Royalty (renamed)", G_ROYALTY),
        ] {
            let mut l = b.ledgers.remove(old).unwrap();
            l.name = new.to_string();
            l.guid = guid.to_string();
            b.ledgers.insert(new.to_string(), l);
        }
        let (bound, _report) = e.bind(&b).unwrap();
        let tds = bound.tds.unwrap();
        assert_eq!(
            tds.payee_aliases
                .get("Carrier One (renamed)")
                .map(String::as_str),
            Some("Carrier group")
        );
        assert!(!tds.payee_aliases.contains_key("Carrier One"));
        assert_eq!(
            tds.s194j_category_by_ledger
                .get("Royalty (renamed)")
                .cloned()
                .flatten()
                .as_deref(),
            Some("royalty")
        );
        assert!(!tds.s194j_category_by_ledger.contains_key("Royalty"));
    }

    #[test]
    fn each_tds_location_refuses_a_name_that_matches_nothing() {
        for (missing, location) in [
            ("Freight", "tds.nature_by_ledger"),
            ("Carrier One", "tds.payee_aliases"),
            ("Royalty", "tds_payees.s194j_category_by_ledger"),
        ] {
            let e = engagement(TDS_TABLES);
            let mut b = book_with_tds_ledgers("Freight", "");
            b.ledgers.remove(missing);
            let err = e.bind(&b).unwrap_err();
            assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN), "{missing}");
            assert!(format!("{err}").contains(location), "{err}");
        }
    }

    /// `[tds]` is read as the reference's `tds_config` reads it: both maps required once the table
    /// exists, the turnover optional, and a `s194j_category_by_ledger` value that is not a string
    /// kept as no category. The refusals are this port's stated divergences.
    #[test]
    fn the_tds_tables_are_read_strictly() {
        assert!(engagement("").tds.is_none());
        let full = engagement(&format!(
            "\n[tds]\nprevious_year_turnover_paise = 1_000_000_001\n{TDS_TABLES}\
             \"Other\" = 7\n"
        ))
        .tds
        .unwrap();
        assert_eq!(full.previous_year_turnover_paise, Some(1_000_000_001));
        assert_eq!(full.s194j_category_by_ledger["Other"], None);
        assert_eq!(
            engagement(TDS_TABLES)
                .tds
                .unwrap()
                .previous_year_turnover_paise,
            None
        );
        // A top-level `[tds_payees]` that is not a table: the reference raises in this test.
        let not_a_table = format!(
            "tds_payees = \"x\"\n{}",
            base_toml("\n[tds.nature_by_ledger]\n[tds.payee_aliases]\n")
        );
        let err = Engagement::from_toml(&not_a_table, Path::new(".")).unwrap_err();
        assert!(
            format!("{err}").contains("[tds_payees] is not a table"),
            "{err}"
        );
        for (extra, needle) in [
            (
                "\n[tds.nature_by_ledger]\n\"Freight\" = \"194C\"\n",
                "[tds].payee_aliases",
            ),
            (
                "\n[tds.payee_aliases]\n\"A\" = \"B\"\n",
                "[tds].nature_by_ledger",
            ),
            (
                "\n[tds.nature_by_ledger]\n\"Freight\" = 1\n[tds.payee_aliases]\n",
                "[tds].nature_by_ledger.Freight is not a string",
            ),
            (
                "\n[tds.nature_by_ledger]\n[tds.payee_aliases]\n\"A\" = true\n",
                "[tds].payee_aliases.A is not a string",
            ),
            (
                "\n[tds]\nprevious_year_turnover_paise = 1.5e9\n[tds.nature_by_ledger]\n\
                 [tds.payee_aliases]\n",
                "previous_year_turnover_paise is not an integer",
            ),
        ] {
            let err = engagement_err(extra);
            assert!(format!("{err}").contains(needle), "{extra}: {err}");
        }
    }

    // ---- [tds_tcs_26as] (tds_tcs_26as, twentysixas_receipts) ----

    const T26_TABLE: &str = "\n[tds_tcs_26as]\ntds_ledgers = [\"TDS Receivable\"]\n\
         tcs_ledgers = [\"TCS Receivable\"]\nadvance_tax_ledgers = [\"Advance Tax\"]\n\
         \n[tds_tcs_26as.deductor_aliases]\n\"TAN-EDGE-A\" = \"Customer A\"\n";

    fn book_with_26as_ledgers(customer: &str, customer_guid: &str) -> book::Book {
        let mut b = book("Cash-in-Hand", "", None);
        for (name, group, guid) in [
            ("TDS Receivable", "Loans & Advances (Asset)", ""),
            ("TCS Receivable", "Loans & Advances (Asset)", ""),
            ("Advance Tax", "Loans & Advances (Asset)", ""),
            (customer, "Sundry Debtors", customer_guid),
        ] {
            b.ledgers
                .insert(name.to_string(), ledger(name, group, guid, None));
        }
        b
    }

    /// The three ledger lists and the alias VALUES are bound (the keys are TANs, left as written),
    /// and an alias value bound by identity follows a rename.
    #[test]
    fn tds_26as_ledgers_and_alias_values_are_bound() {
        const G_CUSTOMER: &str = "11111111-1111-1111-1111-000000000006";
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Customer A\" = {G_CUSTOMER:?}\n{T26_TABLE}"
        ));
        let (bound, _report) = e
            .bind(&book_with_26as_ledgers("Customer A (renamed)", G_CUSTOMER))
            .unwrap();
        let t = bound.tds_tcs_26as.unwrap();
        assert_eq!(
            t.deductor_aliases.get("TAN-EDGE-A").map(String::as_str),
            Some("Customer A (renamed)")
        );
        assert!(t.tds_ledgers.contains("TDS Receivable"));
        assert!(t.tcs_ledgers.contains("TCS Receivable"));
        assert!(t.advance_tax_ledgers.contains("Advance Tax"));
    }

    #[test]
    fn each_tds_26as_location_refuses_a_name_that_matches_nothing() {
        for (missing, location) in [
            ("TDS Receivable", "tds_tcs_26as.tds_ledgers"),
            ("TCS Receivable", "tds_tcs_26as.tcs_ledgers"),
            ("Advance Tax", "tds_tcs_26as.advance_tax_ledgers"),
            ("Customer A", "tds_tcs_26as.deductor_aliases"),
        ] {
            let mut b = book_with_26as_ledgers("Customer A", "");
            b.ledgers.remove(missing);
            let err = engagement(T26_TABLE).bind(&b).unwrap_err();
            assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN), "{missing}");
            assert!(format!("{err}").contains(location), "{err}");
        }
    }

    /// `[tds_tcs_26as]` is read as the reference's `tds_tcs_26as_config` reads it: all four keys
    /// required once the table exists. A non-string value is refused (this port's divergence).
    #[test]
    fn the_tds_26as_table_is_read_strictly() {
        assert!(engagement("").tds_tcs_26as.is_none());
        assert!(engagement(T26_TABLE).tds_tcs_26as.is_some());
        let without = |key: &str| {
            T26_TABLE
                .lines()
                .filter(|l| !l.starts_with(key))
                .collect::<Vec<_>>()
                .join("\n")
        };
        // A missing key reads the engagement (other tests run, as in the reference) and refuses
        // only the 26AS tests, naming the first key missing in the reference's order.
        for (key, extra) in [
            ("tds_ledgers", without("tds_ledgers")),
            ("tcs_ledgers", without("tcs_ledgers")),
            ("advance_tax_ledgers", without("advance_tax_ledgers")),
            (
                "deductor_aliases",
                "\n[tds_tcs_26as]\ntds_ledgers = []\ntcs_ledgers = []\nadvance_tax_ledgers = []\n"
                    .to_string(),
            ),
            (
                "tds_ledgers",
                "\n[tds_tcs_26as]\ntcs_ledgers = []\n".to_string(),
            ),
        ] {
            let e = engagement(&extra);
            assert!(e.tds_tcs_26as.is_some(), "{extra}");
            let err = crate::tds_26as_config(&e, "tds_tcs_26as").unwrap_err();
            assert_eq!(
                format!("{err}"),
                format!(
                    "config: tds_tcs_26as needs [tds_tcs_26as].{key}: the client config does not \
set it"
                ),
                "{extra}"
            );
        }
        assert!(crate::tds_26as_config(&engagement(T26_TABLE), "tds_tcs_26as").is_ok());
        for (extra, needle) in [
            (
                T26_TABLE.replace("tds_ledgers = [\"TDS Receivable\"]", "tds_ledgers = \"x\""),
                "[tds_tcs_26as].tds_ledgers is not a list",
            ),
            (
                T26_TABLE.replace("tds_ledgers = [\"TDS Receivable\"]", "tds_ledgers = [1]"),
                "[tds_tcs_26as].tds_ledgers holds a non-string",
            ),
            (
                T26_TABLE.replace("\"TAN-EDGE-A\" = \"Customer A\"", "\"TAN-EDGE-A\" = 7"),
                "[tds_tcs_26as].deductor_aliases.TAN-EDGE-A is not a string",
            ),
            (
                "\n[tds_tcs_26as]\ntds_ledgers = []\ntcs_ledgers = []\nadvance_tax_ledgers = []\n\
deductor_aliases = 5\n"
                    .to_string(),
                "[tds_tcs_26as].deductor_aliases is not a table",
            ),
        ] {
            let err = engagement_err(&extra);
            assert!(format!("{err}").contains(needle), "{extra}: {err}");
        }
    }

    // ---- [partners.*].interest_ledger (financial_statements) ----

    fn book_with_interest_ledger(name: &str, guid: &str, masterid: Option<i64>) -> book::Book {
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            name.to_string(),
            ledger(name, "Indirect Expenses", guid, masterid),
        );
        b
    }

    #[test]
    fn a_partner_interest_ledger_bound_by_identity_follows_a_rename() {
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Interest to Partners\" = {G_ROUNDOFF:?}\n\
             \n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n"
        ));
        let b = book_with_interest_ledger("Partners' Interest", G_ROUNDOFF, None);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(
            bound.partner_interest_ledgers.get("a").map(String::as_str),
            Some("Partners' Interest")
        );
        assert_eq!(report.drifts.len(), 1);
        assert_eq!(report.drifts[0].current_name, "Partners' Interest");
    }

    #[test]
    fn a_bare_partner_interest_ledger_that_matches_binds_unchanged() {
        let e = engagement("\n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n");
        let b = book_with_interest_ledger("Interest to Partners", "", None);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(
            bound.partner_interest_ledgers.get("a").map(String::as_str),
            Some("Interest to Partners")
        );
        assert!(report.drifts.is_empty());
    }

    #[test]
    fn an_unknown_partner_interest_ledger_refuses_naming_its_location() {
        let e = engagement("\n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n");
        let b = book_with_interest_ledger("Interest To Partners", "", None); // case differs
        let err = e.bind(&b).unwrap_err();
        assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN));
        assert!(format!("{err}").contains("partners.a.interest_ledger"));
    }

    #[test]
    fn a_partner_interest_identity_with_an_unknown_guid_refuses() {
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Interest to Partners\" = {G_OTHER:?}\n\
             \n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n"
        ));
        let b = book_with_interest_ledger("Interest to Partners", G_ROUNDOFF, None);
        assert_eq!(e.bind(&b).unwrap_err().code(), Some(BIND_GUID_UNKNOWN));
    }

    #[test]
    fn a_partner_interest_identity_with_the_wrong_masterid_refuses() {
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Interest to Partners\" = {{ guid = {G_ROUNDOFF:?}, masterid = 9 }}\n\
             \n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n"
        ));
        let b = book_with_interest_ledger("Interest to Partners", G_ROUNDOFF, Some(8));
        assert_eq!(e.bind(&b).unwrap_err().code(), Some(BIND_MASTERID_MISMATCH));
    }

    #[test]
    fn an_identity_used_only_by_a_partner_interest_ledger_is_not_unused() {
        // Before [partners.*].interest_ledger was a bound location, this label would have been
        // refused BIND-ID-UNUSED; a genuinely stale second label still is.
        let used = engagement(&format!(
            "\n[ledger_ids]\n\"Interest to Partners\" = {G_ROUNDOFF:?}\n\
             \n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n"
        ));
        let b = book_with_interest_ledger("Interest to Partners", G_ROUNDOFF, None);
        assert!(used.bind(&b).is_ok());
        let stale = engagement(&format!(
            "\n[ledger_ids]\n\"Interest to Partners\" = {G_ROUNDOFF:?}\n\"Old Label\" = {G_ROUNDOFF:?}\n\
             \n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n"
        ));
        assert_eq!(stale.bind(&b).unwrap_err().code(), Some(BIND_ID_UNUSED));
    }

    #[test]
    fn two_partners_sharing_one_interest_ledger_bind_to_it_once_each() {
        let e = engagement(
            "\n[partners.a]\ninterest_ledger = \"Interest to Partners\"\n\
             \n[partners.b]\ninterest_ledger = \"Interest to Partners\"\n",
        );
        let b = book_with_interest_ledger("Interest to Partners", "", None);
        let (bound, _) = e.bind(&b).unwrap();
        assert_eq!(bound.partner_interest_ledgers.len(), 2);
        let distinct: BTreeSet<&String> = bound.partner_interest_ledgers.values().collect();
        assert_eq!(distinct.len(), 1);
    }

    #[test]
    fn the_partners_table_is_read_as_the_reference_reads_it() {
        // `deed` is not a partner; an empty interest_ledger is not configured; a partner with no
        // interest_ledger contributes nothing.
        let e = engagement(
            "\n[partners.deed]\nreceived = false\n\
             \n[partners.a]\ninterest_ledger = \"\"\n\
             \n[partners.b]\ncapital_ledgers = [\"B Capital\"]\n",
        );
        assert!(e.partner_interest_ledgers.is_empty());
        let err = engagement_err("\n[partners.a]\ninterest_ledger = 5\n");
        assert!(matches!(err, AuditError::Config(_)));
        assert!(format!("{err}").contains("[partners].a.interest_ledger"));
    }

    // ---- the end-to-end proof: a rename bound by identity reproduces the un-renamed figures ----

    #[test]
    fn a_renamed_group_bound_by_identity_reproduces_the_unrenamed_figures() {
        let e_old = engagement("");
        let old = book("Cash-in-Hand", "", None);
        let (bound_old, report_old) = e_old.bind(&old).unwrap();
        assert!(report_old.drifts.is_empty());

        let e_renamed = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_CASH:?}\n"));
        let renamed = book("Cash In Hand (New)", G_CASH, None);
        let (bound_renamed, report_renamed) = e_renamed.bind(&renamed).unwrap();
        assert_eq!(report_renamed.drifts.len(), 1);

        let figs_old = figures(&run_cash_44ab(&old, &bound_old));
        let figs_renamed = figures(&run_cash_44ab(&renamed, &bound_renamed));
        assert!(!figs_old.is_empty());
        assert_eq!(figs_old, figs_renamed);
    }

    // ---- review register wording ----

    #[test]
    fn review_register_rows_name_both_the_label_and_the_current_name() {
        let e = engagement(&format!("\n[group_ids]\n\"Cash-in-Hand\" = {G_CASH:?}\n"));
        let renamed = book("Cash In Hand (New)", G_CASH, None);
        let (_bound, report) = e.bind(&renamed).unwrap();
        let rows = review_register_rows(&report.drifts);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].1.contains("Cash-in-Hand"));
        assert!(rows[0].1.contains("Cash In Hand (New)"));
    }

    #[test]
    fn no_drift_no_review_rows() {
        assert!(review_register_rows(&[]).is_empty());
    }

    // ---- [loans]: loan_ledgers keys, each loan's interest_ledger, shared_interest_ledgers ----

    fn book_with_loan(
        loan: &str,
        loan_guid: &str,
        interest: &str,
        interest_guid: &str,
    ) -> book::Book {
        let mut b = book("Cash-in-Hand", "", None);
        b.ledgers.insert(
            loan.to_string(),
            ledger(loan, "Unsecured Loans", loan_guid, None),
        );
        b.ledgers.insert(
            interest.to_string(),
            ledger(interest, "Indirect Expenses", interest_guid, None),
        );
        b
    }

    fn loan_entry(bound: &Engagement, loan: &str) -> toml::Table {
        bound.loans.loan_ledgers[loan].as_table().unwrap().clone()
    }

    #[test]
    fn a_renamed_loan_ledger_keeps_its_interest_ledger() {
        // The reference raised KeyError here before 76310f60; it now binds the entry whole.
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Old Loan\" = {G_ROUNDOFF:?}\n\
             \n[loans.loan_ledgers.\"Old Loan\"]\nlender = \"x\"\nlender_type = \"nbfc\"\n\
             interest_ledger = \"Loan Interest\"\n"
        ));
        let b = book_with_loan("New Loan", G_ROUNDOFF, "Loan Interest", G_OTHER);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(
            bound.loans.loan_ledgers.keys().collect::<Vec<_>>(),
            ["New Loan"]
        );
        let entry = loan_entry(&bound, "New Loan");
        assert_eq!(entry["interest_ledger"].as_str(), Some("Loan Interest"));
        assert_eq!(entry["lender"].as_str(), Some("x"));
        assert_eq!(bound.loan_ledgers_configured, ["New Loan"]);
        assert_eq!(report.drifts.len(), 1);
        assert_eq!(report.drifts[0].paths, ["loans.loan_ledgers"]);
    }

    #[test]
    fn a_renamed_interest_ledger_is_bound_and_its_label_is_used() {
        // A [ledger_ids] label used only by an interest_ledger (or the shared list) is a used label,
        // as in the reference; before these locations were bound it was refused BIND-ID-UNUSED.
        let e = engagement(&format!(
            "\n[ledger_ids]\n\"Old Interest\" = {G_OTHER:?}\n\
             \n[loans]\nshared_interest_ledgers = [\"Old Interest\"]\n\
             \n[loans.loan_ledgers.\"Loan A\"]\nlender = \"x\"\nlender_type = \"nbfc\"\n\
             interest_ledger = \"Old Interest\"\n"
        ));
        let b = book_with_loan("Loan A", "", "New Interest", G_OTHER);
        let (bound, report) = e.bind(&b).unwrap();
        assert_eq!(
            loan_entry(&bound, "Loan A")["interest_ledger"].as_str(),
            Some("New Interest")
        );
        assert_eq!(bound.loans.shared_interest_ledgers, ["New Interest"]);
        assert_eq!(
            report.drifts[0].paths,
            [
                "loans.loan_ledgers.Loan A.interest_ledger",
                "loans.shared_interest_ledgers"
            ]
        );
        // The unbound engagement is untouched.
        assert!(e.loans.loan_ledgers.is_empty());
    }

    #[test]
    fn an_unknown_interest_or_shared_ledger_refuses_naming_its_location() {
        let b = book_with_loan("Loan A", "", "Loan Interest", "");
        let err = engagement(
            "\n[loans.loan_ledgers.\"Loan A\"]\nlender = \"x\"\nlender_type = \"nbfc\"\n\
             interest_ledger = \"Loan interest\"\n",
        )
        .bind(&b)
        .unwrap_err();
        assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN));
        assert!(format!("{err}").contains("loans.loan_ledgers.Loan A.interest_ledger"));
        let err = engagement("\n[loans]\nshared_interest_ledgers = [\"Loan interest\"]\n")
            .bind(&b)
            .unwrap_err();
        assert_eq!(err.code(), Some(BIND_NAME_UNKNOWN));
        assert!(format!("{err}").contains("loans.shared_interest_ledgers"));
    }

    #[test]
    fn a_malformed_loans_location_refuses() {
        let b = book_with_loan("Loan A", "", "Loan Interest", "");
        for extra in [
            "\n[loans]\nloan_ledgers = [\"Loan A\"]\n",
            "\n[loans.loan_ledgers.\"Loan A\"]\nlender = \"x\"\ninterest_ledger = 5\n",
            "\n[loans]\nshared_interest_ledgers = \"Loan Interest\"\n",
        ] {
            let err = engagement(extra).bind(&b).unwrap_err();
            assert_eq!(err.code(), Some(BIND_ID_MALFORMED), "{extra}");
        }
        // A loan entry that is not a table has no interest_ledger location: it binds its key and
        // is kept as written, for loans_interest to refuse when it runs.
        let (bound, _) = engagement("\n[loans.loan_ledgers]\n\"Loan A\" = 5\n")
            .bind(&b)
            .unwrap();
        assert_eq!(bound.loans.loan_ledgers["Loan A"], toml::Value::Integer(5));
    }

    #[test]
    fn a_malformed_loans_value_fails_only_loans_interest() {
        let rules = crate::rules::Rules::vendored().unwrap();
        let b = book_with_loan("Loan A", "", "Loan Interest", "");
        let with = |extra: &str| {
            let mut e = engagement(extra);
            e.entity_type = Some("firm".to_string());
            e
        };
        // The control: a well-formed loan runs.
        let ok = with(
            "\n[loans.loan_ledgers.\"Loan A\"]\nlender = \"x\"\nlender_type = \"nbfc\"\n\
             interest_ledger = \"Loan Interest\"\n",
        );
        assert!(crate::loans_interest_on(&ok, &b, &rules).is_ok());
        for extra in [
            "\n[[loans]]\nloan_ledgers = {}\n",
            "\n[loans.loan_ledgers]\n\"Loan A\" = 5\n",
            "\n[loans.loan_ledgers.\"Loan A\"]\nlender_type = \"nbfc\"\n",
            "\n[loans.loan_ledgers.\"Loan A\"]\nlender = \"x\"\nlender_type = 3\n",
        ] {
            let e = with(extra);
            assert!(e.bind(&b).is_ok(), "{extra}");
            assert!(crate::cash_44ab_on(&e, &b, &rules).is_ok(), "{extra}");
            assert!(
                crate::cash_payments_40a3_on(&e, &b, &rules).is_ok(),
                "{extra}"
            );
            assert!(crate::loans_interest_on(&e, &b, &rules).is_err(), "{extra}");
        }
    }

    // ---- config parse errors surface through Engagement::from_toml, not bind ----

    #[test]
    fn from_toml_still_rejects_a_config_with_no_roles_table() {
        let err = engagement_err_no_roles();
        assert!(matches!(err, AuditError::Config(_)));
    }

    fn engagement_err_no_roles() -> AuditError {
        Engagement::from_toml(
            "[client]\nlabel=\"t\"\nassessment_year=\"2026-27\"\n[period]\nstart=\"2025-04-01\"\nend=\"2026-03-31\"\n[snapshot]\nformat=\"tally-read-v1\"\npath=\"unused\"\n",
            Path::new("."),
        )
        .unwrap_err()
    }

    // keep `engagement_err` reachable even if a future edit stops otherwise using it directly
    #[test]
    fn helper_engagement_err_reports_config_errors() {
        let err = engagement_err("cash_groups = 5\n"); // shadows the valid list with a bad type
        assert!(matches!(err, AuditError::Config(_)));
    }
}
