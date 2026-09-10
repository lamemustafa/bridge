//! Establishing that a ledger holds a cash or bank balance, from observed
//! masters alone. No Tally I/O belongs in this module.
//!
//! Payment, Receipt and Contra each place a cash/bank ledger on a fixed side
//! (see `docs/tally/TALLY_PROTOCOL_REFERENCE.md` §9.13). Nothing in a ledger's
//! own name says which ledgers those are, so this walks the observed group
//! ancestry to a *reserved* group identity.
//!
//! Two observed facts drive the shape of that walk, both recorded in §8.2a and
//! measured on the captured Group collections in this tree:
//!
//! * a group's visible `NAME` is mutable and a predefined group can be renamed
//!   over XML, so classification reads `RESERVEDNAME` and never the name;
//! * a ledger exposes `PARENT` but no `PARENTSTRUCTURE`, so ancestry is walked
//!   one hop at a time through the Group collection rather than read off the
//!   ledger row.
//!
//! Every outcome that is not a reserved cash/bank identity is a refusal,
//! including "could not be established". An unclassifiable ledger is never
//! admitted onto a side that requires one.

use bridge_tally_protocol::TallyNamedMaster;
use std::collections::{BTreeMap, BTreeSet};

/// Every reserved Tally group identity that holds money, and whether Bridge
/// admits a ledger under it onto a leg that must hold money.
///
/// The two are not the same question, and one table answers both so they
/// cannot drift apart.
///
/// **Admission needs a captured ledger sitting under a captured group** — the
/// whole edge the gate walks, not just its far end. A group row alone proves
/// the identity exists; it does not show a ledger's `PARENT` resolving to it,
/// which is what classification actually reads. Both admitted entries below
/// have such a row in `ledgers_native_aarav.utf16le.xml`.
///
/// The unadmitted entries hold money all the same, and saying otherwise on the
/// counterparty side would wave through the bank-to-bank Payment that rule
/// exists to catch. So a voucher touching one is refused on either side: the
/// money leg for want of an observed edge, the counterparty leg because it is
/// money. Both refusals are the same ignorance pointed in the safe direction.
///
/// - `Bank OD A/c` — group captured in both companies, but no captured ledger
///   beneath it. One `List of Ledgers` read against a book with an overdraft
///   or cash-credit account promotes it.
/// - `Bank OCC A/c` — documented by Tally, in neither captured group set.
///
/// Held in Tally's own spelling and normalized at comparison time, so the
/// matched entry is directly reportable.
const MONEY_RESERVED_GROUPS: &[(&str, bool)] = &[
    ("Bank Accounts", true),
    ("Cash-in-Hand", true),
    ("Bank OD A/c", false),
    ("Bank OCC A/c", false),
];

/// A ledger's cash/bank standing, as established by observed masters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CashBankState {
    /// Ancestry reached a reserved money identity Bridge admits.
    Established { reserved_group: &'static str },
    /// Ancestry reached a reserved identity that holds money but that Bridge
    /// has never observed in a captured group set. Known money, and admitted
    /// on neither side.
    UnadmittedMoney { reserved_group: &'static str },
    /// Ancestry reached a different predefined group identity. The ledger is
    /// established, and established as something other than cash or bank.
    OtherReservedGroup { reserved_group: String },
    /// Ancestry ran out before any predefined identity was reached. This is a
    /// refusal, not a weaker acceptance.
    NotEstablished { reason: &'static str },
}

impl CashBankState {
    /// Whether a leg that must hold money may name this ledger.
    pub(super) fn is_established(&self) -> bool {
        matches!(self, Self::Established { .. })
    }

    /// Whether this ledger holds money at all — a wider question than
    /// admission, and the one a counterparty leg must answer "no" to.
    pub(super) fn is_known_money(&self) -> bool {
        matches!(
            self,
            Self::Established { .. } | Self::UnadmittedMoney { .. }
        )
    }

    /// Whether this ledger is *established* as holding no money. A counterparty
    /// leg needs this positively, not merely the absence of money evidence.
    ///
    /// The distinction is the whole of it: `NotEstablished` means the walk ran
    /// out, which is not the same as reaching a predefined identity that holds
    /// no cash or bank balance.
    pub(super) fn is_established_non_money(&self) -> bool {
        matches!(self, Self::OtherReservedGroup { .. })
    }

    /// A stable machine-readable label for the tool result.
    pub(super) fn state(&self) -> &'static str {
        match self {
            Self::Established { .. } => "cash_bank",
            Self::UnadmittedMoney { .. } => "cash_bank_unadmitted",
            Self::OtherReservedGroup { .. } => "not_cash_bank",
            Self::NotEstablished { .. } => "not_established",
        }
    }

    pub(super) fn detail(&self) -> String {
        match self {
            Self::Established { reserved_group } => format!(
                "The ledger's group ancestry reaches the reserved {reserved_group} identity."
            ),
            Self::UnadmittedMoney { reserved_group } => format!(
                "The ledger's group ancestry reaches the reserved {reserved_group} identity, which holds money but has never been observed in a captured group set. Bridge admits it on neither side of a voucher."
            ),
            Self::OtherReservedGroup { reserved_group } => format!(
                "The ledger's group ancestry reaches the reserved {reserved_group} identity, which holds no cash or bank balance."
            ),
            Self::NotEstablished { reason } => (*reason).to_string(),
        }
    }
}

/// The observed masters one classification needs, indexed once per batch so a
/// payload with many vouchers does not rescan the catalogue per leg.
pub(super) struct ObservedMasters {
    /// Exact catalogue ledger name to the immediate parent group Tally
    /// returned for it. Requested ledgers reach this map only after the exact
    /// spelling gate, so the key is compared exactly.
    ledger_parents: BTreeMap<String, Option<String>>,
    /// Normalized group name to its rows. A repeated normalized name keeps
    /// every row so an ambiguous hop can be refused rather than guessed.
    groups: BTreeMap<String, Vec<TallyNamedMaster>>,
}

impl ObservedMasters {
    pub(super) fn new<'a>(
        ledger_parents: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
        groups: impl IntoIterator<Item = TallyNamedMaster>,
    ) -> Self {
        let mut indexed_groups: BTreeMap<String, Vec<TallyNamedMaster>> = BTreeMap::new();
        for group in groups {
            let key = normalize(&group.name);
            if !key.is_empty() {
                indexed_groups.entry(key).or_default().push(group);
            }
        }
        Self {
            ledger_parents: ledger_parents
                .into_iter()
                .map(|(name, parent)| (name.to_string(), parent.map(str::to_string)))
                .collect(),
            groups: indexed_groups,
        }
    }

    /// Walks one ledger's ancestry to the first predefined group identity.
    pub(super) fn classify(&self, ledger: &str) -> CashBankState {
        let Some(parent) = self.ledger_parents.get(ledger) else {
            return CashBankState::NotEstablished {
                reason: "The observed ledger catalogue does not carry this ledger.",
            };
        };
        let Some(parent) = parent.as_deref() else {
            return CashBankState::NotEstablished {
                reason: "Tally returned no parent group for this ledger.",
            };
        };
        let mut current = normalize(parent);
        let mut visited = BTreeSet::new();
        // Each hop consumes one distinct group; the visited set bounds the walk
        // independently, so this only guards a pathological index.
        for _ in 0..=self.groups.len() {
            if current.is_empty() || is_reserved_root(&current) {
                return CashBankState::NotEstablished {
                    reason:
                        "The ledger's group ancestry reaches the account root without a predefined group identity.",
                };
            }
            if !visited.insert(current.clone()) {
                return CashBankState::NotEstablished {
                    reason: "The observed group ancestry contains a cycle.",
                };
            }
            let Some(rows) = self.groups.get(&current) else {
                return CashBankState::NotEstablished {
                    reason: "A group in the ledger's ancestry is absent from the observed group collection.",
                };
            };
            let [group] = rows.as_slice() else {
                return CashBankState::NotEstablished {
                    reason: "The observed group collection repeated a group name in this ancestry.",
                };
            };
            let Some(reserved_name) = group.reserved_name.as_deref() else {
                return CashBankState::NotEstablished {
                    reason: "A group in the ledger's ancestry omitted RESERVEDNAME, so its identity survives no rename.",
                };
            };
            if !reserved_name.is_empty() {
                let reserved = normalize(reserved_name);
                return match MONEY_RESERVED_GROUPS
                    .iter()
                    .find(|(candidate, _)| normalize(candidate) == reserved)
                {
                    // The reserved identity, not the book's spelling of it.
                    Some((reserved_group, true)) => CashBankState::Established { reserved_group },
                    Some((reserved_group, false)) => {
                        CashBankState::UnadmittedMoney { reserved_group }
                    }
                    None => CashBankState::OtherReservedGroup {
                        reserved_group: reserved_name.to_string(),
                    },
                };
            }
            // An empty RESERVEDNAME is Tally's own statement that the group is
            // user-created, so keep climbing towards a predefined ancestor.
            current = group
                .parent
                .nonempty_returned_text()
                .map(normalize)
                .unwrap_or_default();
        }
        CashBankState::NotEstablished {
            reason: "The ledger's group ancestry exceeded the observed group collection.",
        }
    }
}

/// Tally marks the reserved account root with `U+0004` before `" Primary"`
/// rather than the bare word (§1.1), so none of its spellings is a group row
/// and reaching one ends the walk.
///
/// The marker arrives as the character reference `&#4;`, which is illegal in
/// XML 1.0, so the tolerant reader replaces its `&` and the value reaches this
/// module as `U+FFFD` `#4; Primary` — that is the form the captured Group
/// collections actually produce, and it is the one a naive control-character
/// test misses. The raw `U+0004` form and the bare word are accepted too: a
/// report rendering of the same value drops the marker entirely (§12a.1).
fn is_reserved_root(normalized: &str) -> bool {
    const ROOT_MARKERS: &[&str] = &["\u{fffd}#4;", "\u{4}"];
    let bare = ROOT_MARKERS
        .iter()
        .find_map(|marker| normalized.strip_prefix(marker))
        .unwrap_or(normalized);
    bare.trim() == "primary"
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}
