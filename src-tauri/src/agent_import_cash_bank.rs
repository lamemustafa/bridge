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
//!
//! # What this does not see
//!
//! A verdict here is easy to read for more than it covers, so the limits are
//! written beside it rather than inferred from the absence of a check.
//!
//! * **It is a group check, not a ledger-suitability check.** The question
//!   answered is "does this ledger's group ancestry reach a money identity",
//!   not "can Tally use this ledger in this voucher". A bill-wise party
//!   (§9.13), a foreign-currency bank account, a ledger requiring cost-centre
//!   allocation — all classify exactly the same as one that needs none of it.
//! * **It is true as of the read, and nothing re-checks it.** The build reads
//!   the masters twice and refuses if they moved, which proves stability
//!   *across the build* and says nothing about afterwards. The file is then
//!   imported by hand, and Bridge never observes that import. Regrouping a
//!   ledger is an ordinary Tally operation; do it between build and import and
//!   the verdict is stale, with no later gate to catch it. `verify_import`
//!   compares entries and would not notice a party that has since become a
//!   bank ledger.
//! * **An incomplete read cannot admit, only refuse.** A group missing from a
//!   truncated collection presents as unresolvable ancestry, which refuses. So
//!   the failure mode of a partial read is a wrongly rejected batch, never a
//!   wrongly accepted one.

use bridge_tally_protocol::group_ancestry::{AncestryGap, GroupIndex};
use bridge_tally_protocol::TallyNamedMaster;
use std::collections::BTreeMap;

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
const MONEY_RESERVED_GROUPS: &[(&str, Admission)] = &[
    ("Bank Accounts", Admission::Admitted),
    ("Cash-in-Hand", Admission::Admitted),
    ("Bank OD A/c", Admission::NoCapturedLedger),
    ("Bank OCC A/c", Admission::NoCapturedGroup),
];

/// Every reserved Tally group identity captured, across both committed
/// companies, that has been observed to hold no money.
///
/// This is the observed predefined set, not a guess at the rest of Tally's
/// group taxonomy: each entry is one of the 28 `RESERVEDNAME` values the two
/// captured `List of Groups` responses actually exhibit
/// (`group_snapshot_aarav.xml`, `group_snapshot_wr2.xml`), minus the three
/// money identities already in [`MONEY_RESERVED_GROUPS`]. An identity that
/// appears in neither table is not thereby non-money — it is unknown, and
/// unknown is exactly what a captured predefined-group domain cannot rule
/// out. `Bank OCC A/c` is the standing proof of that: it is a real Tally
/// predefined *money* group, documented by Tally itself, and it appears in
/// neither captured company. Nothing about the shape of a `RESERVEDNAME`
/// distinguishes a captured money identity from an uncaptured one, so an
/// identity's absence from this table is never read as evidence it belongs on
/// the money side either — it is read as no evidence at all.
///
/// That is why an unknown identity is refused rather than admitted as a
/// counterparty. The two mistakes this module can make are not symmetric: a
/// money ledger misjudged onto the money leg makes Tally reject the import
/// outright — loud, and caught at the point of failure. A money ledger
/// misjudged onto the counterparty leg is accepted, and what actually happens
/// is that the voucher's real shape — a Contra — gets filed into the Payment
/// or Receipt register instead, which Tally accepts without complaint and
/// which surfaces later, if at all, as a reconciliation problem with no
/// pointer back to this build. The silent failure earns the stricter rule:
/// an identity this table has not observed refuses rather than passes.
///
/// Held in Tally's own spelling (with a literal `&`, not `&amp;` — the
/// fixtures carry the XML-escaped form) and normalized at comparison time, so
/// the matched entry is directly reportable.
const NON_MONEY_RESERVED_GROUPS: &[&str] = &[
    "Branch / Divisions",
    "Capital Account",
    "Current Assets",
    "Current Liabilities",
    "Deposits (Asset)",
    "Direct Expenses",
    "Direct Incomes",
    "Duties & Taxes",
    "Fixed Assets",
    "Indirect Expenses",
    "Indirect Incomes",
    "Investments",
    "Loans & Advances (Asset)",
    "Loans (Liability)",
    "Misc. Expenses (ASSET)",
    "Provisions",
    "Purchase Accounts",
    "Reserves & Surplus",
    "Sales Accounts",
    "Secured Loans",
    "Stock-in-Hand",
    "Sundry Creditors",
    "Sundry Debtors",
    "Suspense A/c",
    "Unsecured Loans",
];

/// Why a money group is or is not admitted. The two refusals are not the same
/// gap, and an operator told the wrong one goes looking for a capture that
/// already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Admission {
    Admitted,
    /// The group was captured; no captured ledger sits under it.
    NoCapturedLedger,
    /// The identity has never appeared in a captured group set at all.
    NoCapturedGroup,
}

impl Admission {
    fn gap(self) -> &'static str {
        match self {
            Self::Admitted => "",
            Self::NoCapturedLedger => "that group is captured, but no captured ledger sits under it, and the ledger-to-parent edge is what this classification reads",
            Self::NoCapturedGroup => "that identity has never appeared in a captured group set",
        }
    }
}

/// A ledger's cash/bank standing, as established by observed masters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CashBankState {
    /// Ancestry reached a reserved money identity Bridge admits.
    Established { reserved_group: &'static str },
    /// Ancestry reached a reserved identity that holds money but that Bridge
    /// does not admit. Known money, and admitted on neither side; `gap` names
    /// the evidence actually missing, which differs per group.
    UnadmittedMoney {
        reserved_group: &'static str,
        gap: &'static str,
    },
    /// Ancestry reached a different predefined group identity. The ledger is
    /// established, and established as something other than cash or bank.
    ///
    /// The name here is safe to repeat back in a refusal without redaction,
    /// and only because of where it comes from: this variant is constructed
    /// solely from a *non-empty* `RESERVEDNAME`, which §8.2a establishes is
    /// Tally's own predefined identity. A user-created group carries an empty
    /// one and the walk keeps climbing, so a book's own naming never reaches
    /// here. Widen that construction and this becomes a leak.
    OtherReservedGroup { reserved_group: String },
    /// Ancestry ran out before any predefined identity was reached. This is a
    /// refusal, not a weaker acceptance.
    NotEstablished { reason: &'static str },
}

impl CashBankState {
    /// Whether this ledger holds money at all — a wider question than
    /// admission, and the one a counterparty leg must answer "no" to.
    fn is_known_money(&self) -> bool {
        matches!(
            self,
            Self::Established { .. } | Self::UnadmittedMoney { .. }
        )
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
            Self::UnadmittedMoney {
                reserved_group,
                gap,
            } => format!(
                "The ledger's group ancestry reaches the reserved {reserved_group} identity, which holds money — but {gap}. Bridge admits it on neither side of a voucher."
            ),
            Self::OtherReservedGroup { reserved_group } => format!(
                "The ledger's group ancestry reaches the reserved {reserved_group} identity, which holds no cash or bank balance."
            ),
            Self::NotEstablished { reason } => (*reason).to_string(),
        }
    }
}

/// What one constrained leg of a bank voucher must be.
///
/// This lives beside [`CashBankState`] because the two are one idea: the state
/// is what a ledger *is*, and this is what a side *needs*. Splitting them put
/// the asymmetry below in one module and the states it reasons about in
/// another, which is how it came to be got wrong twice.
///
/// The two requirements are deliberately not mirror images, because the facts
/// they need are not mirror images either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LegRequirement {
    /// Tally needs the company's own money on this side, so the ledger must be
    /// *established* as cash or bank. Anything else refuses, including "could
    /// not be established" — a positive fact is required and absent.
    Money,
    /// The counterparty side, which is also what `PARTYLEDGERNAME` names. It
    /// must be *established* as holding no money, so an unresolved ancestry
    /// refuses just as a money one does.
    ///
    /// This was once the looser of the two, on the reasoning that an
    /// unclassifiable counterparty is not evidence of a disguised Contra and
    /// refusing it would cost a legitimate build. Both halves were weaker than
    /// they sounded. An ordinary party never lands unclassified — one under
    /// `Sundry Debtors` resolves directly and one under a user-created group
    /// walks up to its reserved ancestor — so only anomalies reach that state.
    /// And the consequences are not symmetric: a misjudged money leg makes
    /// Tally reject the import, which is loud, while a misjudged counterparty
    /// files a Contra into the Payment register, which is silent and found
    /// later. The silent failure earns the stricter rule, not the looser one.
    Counterparty,
}

impl LegRequirement {
    /// Whether a ledger in this state may occupy this side. Both arms demand a
    /// positive fact and differ only in which one, so an unresolved ancestry
    /// refuses either way.
    pub(super) fn admits(self, state: &CashBankState) -> bool {
        match self {
            Self::Money => matches!(state, CashBankState::Established { .. }),
            Self::Counterparty => matches!(state, CashBankState::OtherReservedGroup { .. }),
        }
    }

    /// Why this leg was refused, or `None` if it was admitted.
    ///
    /// A counterparty fails two different ways and the fixes differ: money
    /// there means the voucher is really a Contra, while an unresolvable group
    /// means nobody has classified the ledger, and advising a Contra would be
    /// wrong about a ledger nobody has classified.
    pub(super) fn refusal(self, state: &CashBankState, voucher_type: &str) -> Option<String> {
        if self.admits(state) {
            return None;
        }
        Some(match self {
            Self::Money => state.detail(),
            Self::Counterparty if state.is_known_money() => format!(
                "{} Money on both sides of a {voucher_type} is a Contra; book it as one.",
                state.detail()
            ),
            Self::Counterparty => format!(
                "{} A {voucher_type} counterparty must be established as holding no money, and this one could not be classified either way.",
                state.detail()
            ),
        })
    }
}

/// The observed masters one classification needs, indexed once per batch so a
/// payload with many vouchers does not rescan the catalogue per leg.
pub(super) struct ObservedMasters {
    /// Exact catalogue ledger name to the immediate parent group Tally
    /// returned for it. Requested ledgers reach this map only after the exact
    /// spelling gate, so the key is compared exactly.
    ledger_parents: BTreeMap<String, Option<String>>,
    groups: GroupIndex,
}

impl ObservedMasters {
    pub(super) fn new<'a>(
        ledger_parents: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
        groups: impl IntoIterator<Item = TallyNamedMaster>,
    ) -> Self {
        Self {
            ledger_parents: ledger_parents
                .into_iter()
                .map(|(name, parent)| (name.to_string(), parent.map(str::to_string)))
                .collect(),
            groups: GroupIndex::build(groups),
        }
    }

    /// Walks one ledger's ancestry to the first predefined group identity, and
    /// asks only this module's question of the answer.
    ///
    /// The traversal itself is shared with the Schedule III classifier, which
    /// needs the same climb for a different verdict — see
    /// [`bridge_tally_protocol::group_ancestry`]. What stays here is the
    /// mapping from a reserved identity to whether it holds money, and from a
    /// refusal to a sentence this caller can act on.
    pub(super) fn classify(&self, ledger: &str) -> CashBankState {
        let Some(parent) = self.ledger_parents.get(ledger) else {
            return CashBankState::NotEstablished {
                reason: "The observed ledger catalogue does not carry this ledger.",
            };
        };
        let reserved = match self.groups.reserved_ancestor(parent.as_deref()) {
            Ok(reserved) => reserved,
            Err(gap) => {
                return CashBankState::NotEstablished {
                    reason: reason(gap),
                }
            }
        };
        let normalized = normalize(reserved);
        match MONEY_RESERVED_GROUPS
            .iter()
            .find(|(candidate, _)| normalize(candidate) == normalized)
        {
            // The reserved identity, not the book's spelling of it.
            Some((reserved_group, Admission::Admitted)) => {
                CashBankState::Established { reserved_group }
            }
            Some((reserved_group, admission)) => CashBankState::UnadmittedMoney {
                reserved_group,
                gap: admission.gap(),
            },
            None if NON_MONEY_RESERVED_GROUPS
                .iter()
                .any(|candidate| normalize(candidate) == normalized) =>
            {
                CashBankState::OtherReservedGroup {
                    reserved_group: reserved.to_string(),
                }
            }
            // Neither table exhibits this identity. Silence here is not
            // evidence of anything — see NON_MONEY_RESERVED_GROUPS's doc
            // comment — so this refuses instead of assuming the domain of
            // captured predefined groups is closed.
            None => CashBankState::NotEstablished {
                reason: "The group's reserved identity is one no captured response exhibits, so it establishes neither that the ledger holds money nor that it does not.",
            },
        }
    }
}

/// What each shared refusal means to a caller building a bank voucher.
///
/// The gaps are one enum so the traversal can stay verdict-free; the wording
/// is here because a Schedule III exclusion says something different about the
/// same fact.
fn reason(gap: AncestryGap) -> &'static str {
    match gap {
        AncestryGap::NoParent => "Tally returned no parent group for this ledger.",
        AncestryGap::ReachedRoot => {
            "The ledger's group ancestry reaches the account root without a predefined group identity."
        }
        AncestryGap::GroupAbsent => {
            "A group in the ledger's ancestry is absent from the observed group collection."
        }
        AncestryGap::GroupNameRepeated => {
            "The observed group collection repeated a group name in this ancestry."
        }
        AncestryGap::ReservedNameMissing => {
            "A group in the ledger's ancestry omitted RESERVEDNAME, so its identity survives no rename."
        }
        AncestryGap::Cycle => "The observed group ancestry contains a cycle.",
        AncestryGap::Exhausted => {
            "The ledger's group ancestry exceeded the observed group collection."
        }
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}
