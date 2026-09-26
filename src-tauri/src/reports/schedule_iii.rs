//! A deliberately partial, traceable Schedule III view over an already read
//! party/ledger master source. No Tally I/O belongs in this module.
//!
//! A line is one of two kinds. A group subtotal is all a master-and-balance
//! read establishes: a built-in group identity and a balance side, never a
//! statutory head. A Schedule III head appears only through a CA's recorded
//! grouping decision (#737, ADR 0020). `build_schedule_iii_view` is the only
//! way to obtain a view, and it binds every decision it is given against this
//! read before placing a row: a decision that no longer fits the read is
//! reported and not applied. A decision moves a row between lines; it never
//! carries an amount. Every line total is summed from the read's own rows, and
//! every view is checked to place each ledger exactly once.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::group_ancestry::{AncestryGap, GroupIndex};
use bridge_tally_protocol::{is_tally_reserved_root, TallyNamedMaster};
use serde::{Deserialize, Serialize};

use super::party_ledger_master::{
    PartyLedgerMasterRow, PartyLedgerMasterSource, PartyLedgerMasterWorkbook,
};
use crate::tally::VerifiedCompanyIdentity;

const NO_HEAD_FROM_GROUPS: &str =
    "Group hierarchy does not determine a Schedule III head; client mapping decision required.";

/// Fields are private, so only `build_schedule_iii_view` makes or changes a
/// view, its lines and its exclusions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleIIIView {
    lines: Vec<ScheduleIIILine>,
    exclusions: Vec<ScheduleIIIExclusion>,
    /// One entry per decision given, in the order given.
    decisions: Vec<DecisionStatus>,
    decision_source: DecisionSource,
    finality: Finality,
    debit_total: ExactDecimal,
    credit_total: ExactDecimal,
    difference: ExactDecimal,
}

impl ScheduleIIIView {
    pub(crate) fn lines(&self) -> &[ScheduleIIILine] {
        &self.lines
    }

    pub(crate) fn exclusions(&self) -> &[ScheduleIIIExclusion] {
        &self.exclusions
    }

    /// One status per decision given, in the order the decisions were given.
    pub(crate) fn decisions(&self) -> &[DecisionStatus] {
        &self.decisions
    }

    pub(crate) fn decision_source(&self) -> DecisionSource {
        self.decision_source
    }

    pub(crate) fn finality(&self) -> Finality {
        self.finality
    }

    pub(crate) fn debit_total(&self) -> &ExactDecimal {
        &self.debit_total
    }

    pub(crate) fn credit_total(&self) -> &ExactDecimal {
        &self.credit_total
    }

    pub(crate) fn difference(&self) -> &ExactDecimal {
        &self.difference
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleIIILine {
    basis: LineBasis,
    section: &'static str,
    label: &'static str,
    total: ExactDecimal,
    /// Indices into `PartyLedgerMasterSource::rows`, so each subtotal keeps a
    /// direct link to the original named ledger row and exact source balance.
    row_indices: Vec<usize>,
}

impl ScheduleIIILine {
    pub(crate) fn basis(&self) -> LineBasis {
        self.basis
    }

    pub(crate) fn section(&self) -> &'static str {
        self.section
    }

    pub(crate) fn label(&self) -> &'static str {
        self.label
    }

    pub(crate) fn total(&self) -> &ExactDecimal {
        &self.total
    }

    pub(crate) fn row_indices(&self) -> &[usize] {
        &self.row_indices
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleIIIExclusion {
    row_index: usize,
    reason: String,
}

impl ScheduleIIIExclusion {
    pub(crate) fn row_index(&self) -> usize {
        self.row_index
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }
}

/// Why a line holds its rows: the read's own group evidence, or a CA's
/// recorded decision. Nothing else can place a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineBasis {
    GroupSubtotal(GroupSubtotalKind),
    Decided(ScheduleIIIHead),
}

/// A view is final only when its decisions could be read and every decision
/// for its period applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Finality {
    Final,
    NotFinal,
}

/// Whether the decisions for a view could be read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecisionSource {
    Read,
    Unavailable(DecisionsUnavailable),
}

/// Why no decision could be read. Either way the view is NOT FINAL: it never
/// claims that no decision exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecisionsUnavailable {
    /// The encrypted store could not be opened.
    StoreUnavailable,
    /// The store opened, but its decisions could not be read, for example
    /// because a newer build wrote a head this build does not know.
    Unreadable,
}

/// What a view is built with: one book's decisions, or the fact that they
/// could not be read.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DecisionInput<'a> {
    Read(&'a DecisionSet),
    Unavailable(DecisionsUnavailable),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ScheduleIIIError {
    #[error("Schedule III totals exceeded the exact-decimal range")]
    Arithmetic,
    #[error("an unestablished closing balance reached a Schedule III total")]
    UnestablishedBalance,
    #[error("two grouping decisions for this period name the same ledger")]
    DecisionRepeated,
    #[error("the grouping decisions belong to another book")]
    DecisionsForAnotherBook,
    #[error("the grouping decisions belong to another financial year")]
    DecisionsForAnotherYear,
    #[error("the Schedule III view did not place every ledger exactly once with its read balance")]
    NotConserved,
}

/// Tally signs a debit balance negative. A zero balance sits on either side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Debit,
    Credit,
}

impl Side {
    fn admits(self, balance: &ExactDecimal) -> bool {
        balance.is_zero()
            || match self {
                Self::Debit => balance.is_negative(),
                Self::Credit => !balance.is_negative(),
            }
    }
}

/// A built-in group whose identity and balance side the read establishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroupSubtotalKind {
    SundryDebtors,
    SundryCreditors,
    CashInHand,
    BankAccounts,
}

impl GroupSubtotalKind {
    const ALL: [Self; 4] = [
        Self::SundryDebtors,
        Self::SundryCreditors,
        Self::CashInHand,
        Self::BankAccounts,
    ];

    fn from_reserved_name(normalized: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.reserved_name() == normalized)
    }

    /// Tally's `RESERVEDNAME`, trimmed and lower-cased.
    fn reserved_name(self) -> &'static str {
        match self {
            Self::SundryDebtors => "sundry debtors",
            Self::SundryCreditors => "sundry creditors",
            Self::CashInHand => "cash-in-hand",
            Self::BankAccounts => "bank accounts",
        }
    }

    fn side(self) -> Side {
        match self {
            Self::SundryCreditors => Side::Credit,
            Self::SundryDebtors | Self::CashInHand | Self::BankAccounts => Side::Debit,
        }
    }

    fn section(self) -> &'static str {
        match self.side() {
            Side::Debit => "Debit-balance group subtotals",
            Side::Credit => "Credit-balance group subtotals",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SundryDebtors => "Sundry Debtors group subtotal",
            Self::SundryCreditors => "Sundry Creditors group subtotal",
            Self::CashInHand => "Cash-in-Hand group subtotal",
            Self::BankAccounts => "Bank Accounts group subtotal",
        }
    }

    fn opposite_side_reason(self) -> &'static str {
        match self {
            Self::SundryDebtors => "A credit-balance Sundry Debtors ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            Self::SundryCreditors => "A debit-balance Sundry Creditors ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            Self::CashInHand => "A credit-balance Cash-in-Hand ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            Self::BankAccounts => "A credit-balance Bank Accounts ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
        }
    }
}

/// A statutory head a CA can present a ledger under.
///
/// A provisional subset. Whether the first catalogue is Division I's or the
/// non-corporate Guidance Note's is not yet decided (#737), so only these
/// heads exist, with Division I captions. The full catalogue is transcribed
/// from the chosen Guidance Note before this reaches a CA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleIIIHead {
    TradeReceivables,
    CashAndCashEquivalents,
    ShortTermLoansAndAdvances,
    TradePayables,
    OtherCurrentLiabilities,
}

impl ScheduleIIIHead {
    const ALL: [Self; 5] = [
        Self::TradeReceivables,
        Self::CashAndCashEquivalents,
        Self::ShortTermLoansAndAdvances,
        Self::TradePayables,
        Self::OtherCurrentLiabilities,
    ];

    /// The code the store keeps. Stable: a stored code this build does not
    /// know makes the stored decisions unreadable, never silently dropped.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::TradeReceivables => "trade_receivables",
            Self::CashAndCashEquivalents => "cash_and_cash_equivalents",
            Self::ShortTermLoansAndAdvances => "short_term_loans_and_advances",
            Self::TradePayables => "trade_payables",
            Self::OtherCurrentLiabilities => "other_current_liabilities",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|head| head.code() == code)
    }

    fn side(self) -> Side {
        match self {
            Self::TradeReceivables
            | Self::CashAndCashEquivalents
            | Self::ShortTermLoansAndAdvances => Side::Debit,
            Self::TradePayables | Self::OtherCurrentLiabilities => Side::Credit,
        }
    }

    fn section(self) -> &'static str {
        match self.side() {
            Side::Debit => "Current assets",
            Side::Credit => "Current liabilities",
        }
    }

    pub(crate) fn caption(self) -> &'static str {
        match self {
            Self::TradeReceivables => "Trade receivables",
            Self::CashAndCashEquivalents => "Cash and cash equivalents",
            Self::ShortTermLoansAndAdvances => "Short-term loans and advances",
            Self::TradePayables => "Trade payables",
            Self::OtherCurrentLiabilities => "Other current liabilities",
        }
    }
}

/// What the read alone says about one ledger. A decision records the outcome
/// it was made against, and applies only while the ledger keeps the same
/// standing (see [`Standing`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DerivedOutcome {
    GroupSubtotal(GroupSubtotalKind),
    Undetermined(Undetermined),
}

/// Where the read places a ledger's group, independent of its balance side.
/// A decision applies only while its ledger keeps the standing it had when the
/// decision was made. A balance changing side, or settling to zero, within the
/// same group is not a move; whether the decided head still fits that balance
/// is checked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing<'a> {
    /// Under this predefined group: its trimmed, lower-cased `RESERVEDNAME`.
    PredefinedGroup(&'a str),
    /// Under this user-created group, which sits directly under the account
    /// root. A group name is mutable, so a rename also makes a decision stale.
    UserPrimaryGroup(&'a str),
    AccountRoot,
}

impl DerivedOutcome {
    /// `None` when the read cannot place the ledger at all, so no decision may
    /// rest on it.
    fn standing(&self) -> Option<Standing<'_>> {
        match self {
            Self::GroupSubtotal(kind) | Self::Undetermined(Undetermined::OppositeSide(kind)) => {
                Some(Standing::PredefinedGroup(kind.reserved_name()))
            }
            Self::Undetermined(Undetermined::UnmappedReservedGroup(name)) => {
                Some(Standing::PredefinedGroup(name))
            }
            Self::Undetermined(Undetermined::UserPrimaryGroup(name)) => {
                Some(Standing::UserPrimaryGroup(name))
            }
            Self::Undetermined(Undetermined::AccountRoot) => Some(Standing::AccountRoot),
            Self::Undetermined(
                Undetermined::NoParent
                | Undetermined::BalanceNotEstablished
                | Undetermined::ReadIncomplete(_),
            ) => None,
        }
    }

    /// Where the read now places a ledger, for someone holding a trial balance.
    pub(crate) fn description(&self) -> String {
        match self {
            Self::GroupSubtotal(kind) => kind.label().to_string(),
            Self::Undetermined(Undetermined::UnmappedReservedGroup(name)) => {
                format!("under the predefined group \"{name}\"")
            }
            Self::Undetermined(Undetermined::UserPrimaryGroup(name)) => {
                format!("under the user-created primary group \"{name}\"")
            }
            Self::Undetermined(Undetermined::AccountRoot) => {
                "directly under the account root".to_string()
            }
            Self::Undetermined(undetermined) => undetermined.reason().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Undetermined {
    /// A built-in group this view maps, with the balance on the other side.
    OppositeSide(GroupSubtotalKind),
    /// A predefined group this view does not map, named by its trimmed,
    /// lower-cased `RESERVEDNAME`, so a move between two such groups is seen.
    UnmappedReservedGroup(String),
    /// Only user-created groups up to this one, whose own parent Tally
    /// returned as the account root.
    UserPrimaryGroup(String),
    /// The ledger's own parent is the account root.
    AccountRoot,
    NoParent,
    BalanceNotEstablished,
    /// The group read cannot establish the chain; no decision may rest on it.
    ReadIncomplete(ReadGap),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReadGap {
    /// A user-created group whose parent Tally did not return.
    ///
    /// Unverified assumption: that Tally returns a user-created primary
    /// group's parent as the marked account root (`TALLY_PROTOCOL_REFERENCE.md`
    /// §1.1(d)), as every captured predefined primary group's is. No capture
    /// yet holds a user-created primary group. If the assumption is wrong, such
    /// a group lands here: its ledgers are excluded and no decision applies to
    /// them, so the view fails closed rather than placing a row wrongly.
    ParentNotObserved,
    GroupAbsent,
    GroupNameRepeated,
    ReservedNameMissing,
    Cycle,
    Exhausted,
}

impl Undetermined {
    /// What each outcome means to someone holding a trial balance.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::OppositeSide(kind) => kind.opposite_side_reason(),
            // Reaching the account root without a predefined identity is the
            // same outcome as a predefined group this view does not map:
            // nothing here determines a head, and a client decides.
            Self::UnmappedReservedGroup(_) | Self::UserPrimaryGroup(_) | Self::AccountRoot => {
                NO_HEAD_FROM_GROUPS
            }
            Self::NoParent => {
                "Ledger has no parent group; its Schedule III head is not determined."
            }
            Self::BalanceNotEstablished => {
                "Tally returned an empty CLOSINGBALANCE; the balance is not established and was excluded from Schedule III totals."
            }
            Self::ReadIncomplete(ReadGap::ParentNotObserved) => {
                "A group in the ledger's hierarchy has no parent in the read; classification withheld."
            }
            Self::ReadIncomplete(ReadGap::GroupAbsent) => {
                "Ledger parent is absent from the captured group hierarchy; classification withheld."
            }
            Self::ReadIncomplete(ReadGap::GroupNameRepeated) => {
                "Captured group hierarchy repeated a group name; classification withheld."
            }
            Self::ReadIncomplete(ReadGap::ReservedNameMissing) => {
                "Group omitted Tally RESERVEDNAME; immutable classification evidence is unavailable."
            }
            Self::ReadIncomplete(ReadGap::Cycle) => {
                "Group hierarchy contains a cycle; classification withheld."
            }
            Self::ReadIncomplete(ReadGap::Exhausted) => {
                "Group hierarchy exceeded its captured length; classification withheld."
            }
        }
    }
}

/// A ledger GUID, ASCII case-folded as the party/ledger master compares them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LedgerGuid(String);

impl LedgerGuid {
    pub(crate) fn new(value: &str) -> Option<Self> {
        (!value.trim().is_empty()).then(|| Self(value.to_ascii_lowercase()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// An Indian financial year, 1 April to 31 March, named by its first year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FinancialYear {
    first_year: u16,
}

impl FinancialYear {
    pub(crate) const fn beginning_in(first_year: u16) -> Self {
        Self { first_year }
    }

    /// The year a balance date falls in.
    pub(crate) fn containing(date: &TallyDate) -> Option<Self> {
        let text = date.as_str();
        let year = text[..4].parse::<u16>().ok()?;
        let first_year = if &text[4..] >= "0401" {
            year
        } else {
            year.checked_sub(1)?
        };
        Some(Self { first_year })
    }

    pub(crate) fn first_year(self) -> u16 {
        self.first_year
    }

    fn previous(self) -> Option<Self> {
        self.first_year.checked_sub(1).map(Self::beginning_in)
    }

    /// Whether a balance date falls inside this year.
    fn contains(self, date: &TallyDate) -> bool {
        let text = date.as_str();
        let (Ok(year), month_day) = (text[..4].parse::<u16>(), &text[4..]) else {
            return false;
        };
        (year == self.first_year && month_day >= "0401")
            || (Some(year) == self.first_year.checked_add(1) && month_day <= "0331")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DecisionId(pub(crate) u64);

/// What the read says of one ledger: the outcome, and the names of the
/// groups above it, nearest first. A decision is made against this. Its
/// serialized form is what the store keeps, so its codes are stable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Derivation {
    pub(crate) outcome: DerivedOutcome,
    /// Up to and including the nearest predefined group, which is as far as
    /// the ledger's standing rests on. Groups above that are not recorded, so
    /// how much of the upper hierarchy a read happened to capture cannot look
    /// like the ledger moving.
    pub(crate) ancestry: Vec<String>,
}

/// A CA's grouping decision, as this view needs it: which ledger, what the
/// read said of it when the decision was made, the head chosen, and the year
/// it is for. Who made it, when and why are recorded with it where it is
/// stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) id: DecisionId,
    pub(crate) ledger: LedgerGuid,
    pub(crate) ledger_name_when_made: String,
    pub(crate) made_against: Derivation,
    pub(crate) head: ScheduleIIIHead,
    pub(crate) year: FinancialYear,
}

/// The book a set of decisions belongs to (ADR 0020 item 3): the observed
/// company tuple without its display name, so a company rename keeps its
/// decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BookKey {
    company_guid: String,
    company_number: String,
    books_from_yyyymmdd: String,
}

impl BookKey {
    pub(crate) fn of(identity: &VerifiedCompanyIdentity) -> Self {
        Self {
            company_guid: identity.company_guid().to_ascii_lowercase(),
            company_number: identity.company_number().to_string(),
            books_from_yyyymmdd: identity.books_from_yyyymmdd().to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(company_guid: &str, company_number: &str, books_from: &str) -> Self {
        Self {
            company_guid: company_guid.to_ascii_lowercase(),
            company_number: company_number.to_string(),
            books_from_yyyymmdd: books_from.to_string(),
        }
    }

    pub(crate) fn company_guid(&self) -> &str {
        &self.company_guid
    }

    pub(crate) fn company_number(&self) -> &str {
        &self.company_number
    }

    pub(crate) fn books_from_yyyymmdd(&self) -> &str {
        &self.books_from_yyyymmdd
    }
}

/// One book's decisions for one year, plus the previous year's, which are
/// offered and never applied. At most one decision per ledger per year: a
/// view can only be built from a set that holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionSet {
    book: BookKey,
    year: FinancialYear,
    decisions: Vec<Decision>,
}

impl DecisionSet {
    pub(crate) fn new(
        book: BookKey,
        year: FinancialYear,
        decisions: Vec<Decision>,
    ) -> Result<Self, ScheduleIIIError> {
        let mut seen = BTreeSet::new();
        for decision in &decisions {
            if decision.year != year && Some(decision.year) != year.previous() {
                return Err(ScheduleIIIError::DecisionsForAnotherYear);
            }
            if !seen.insert((decision.year, &decision.ledger)) {
                return Err(ScheduleIIIError::DecisionRepeated);
            }
        }
        Ok(Self {
            book,
            year,
            decisions,
        })
    }

    pub(crate) fn decisions(&self) -> &[Decision] {
        &self.decisions
    }
}

#[cfg(test)]
impl DecisionSet {
    /// A set for a synthetic workbook: its GUID, a fixed number and
    /// books-from, and the year of its balance date. Built through `new`, so
    /// the same validation holds.
    pub(crate) fn for_tests(
        workbook: &PartyLedgerMasterWorkbook,
        decisions: Vec<Decision>,
    ) -> Result<Self, ScheduleIIIError> {
        let source = workbook.source();
        Self::new(
            BookKey::for_tests(&source.company_guid, "1", "20250401"),
            FinancialYear::containing(&source.to).expect("a valid balance date"),
            decisions,
        )
    }
}

/// A ledger's derivation, confirmed against a fresh read to be what the CA
/// saw. Only [`confirm_seen`] makes one, so a decision is never recorded
/// against a state the CA did not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfirmedDerivation {
    ledger: LedgerGuid,
    ledger_name: String,
    derivation: Derivation,
}

impl ConfirmedDerivation {
    pub(crate) fn ledger(&self) -> &LedgerGuid {
        &self.ledger
    }

    pub(crate) fn ledger_name(&self) -> &str {
        &self.ledger_name
    }

    pub(crate) fn derivation(&self) -> &Derivation {
        &self.derivation
    }
}

/// A ledger whose read changed between what the CA saw and the save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeenMismatch {
    LedgerMissing {
        ledger: LedgerGuid,
    },
    Moved {
        ledger: LedgerGuid,
        ledger_name: String,
        seen: Derivation,
        now: Derivation,
    },
}

/// Confirms, ledger by ledger, that a fresh read still says what the CA saw.
/// Any difference refuses the whole save, listing every ledger that moved.
pub(crate) fn confirm_seen(
    workbook: &PartyLedgerMasterWorkbook,
    seen: &[(LedgerGuid, Derivation)],
) -> Result<Vec<ConfirmedDerivation>, Vec<SeenMismatch>> {
    let source = workbook.source();
    let derived = derive(source);
    let rows_by_guid: BTreeMap<LedgerGuid, usize> = source
        .rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| Some((LedgerGuid::new(&row.guid)?, index)))
        .collect();
    let mut confirmed = Vec::with_capacity(seen.len());
    let mut mismatches = Vec::new();
    for (ledger, saw) in seen {
        let Some(&row_index) = rows_by_guid.get(ledger) else {
            mismatches.push(SeenMismatch::LedgerMissing {
                ledger: ledger.clone(),
            });
            continue;
        };
        let now = &derived[row_index];
        if now == saw {
            confirmed.push(ConfirmedDerivation {
                ledger: ledger.clone(),
                ledger_name: source.rows[row_index].name.clone(),
                derivation: now.clone(),
            });
        } else {
            mismatches.push(SeenMismatch::Moved {
                ledger: ledger.clone(),
                ledger_name: source.rows[row_index].name.clone(),
                seen: saw.clone(),
                now: now.clone(),
            });
        }
    }
    if mismatches.is_empty() {
        Ok(confirmed)
    } else {
        Err(mismatches)
    }
}

/// The groups above a ledger when a decision was made and now, nearest first.
/// Reported whenever they differ, even though the decision still applies: the
/// CA's reason may have rested on a group the ledger has since left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AncestryChange {
    pub(crate) was: Vec<String>,
    pub(crate) now: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecisionStatus {
    Applied {
        id: DecisionId,
        row_index: usize,
        /// The name the ledger had when the decision was made, when Tally now
        /// shows another. The GUID still proves which ledger it is.
        renamed_from: Option<String>,
        /// Set when the groups above the ledger changed without changing its
        /// standing, such as a move between two subgroups of one group.
        ancestry_changed: Option<AncestryChange>,
    },
    NotApplied {
        id: DecisionId,
        reason: NotApplied,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotApplied {
    /// For another financial year: offered, not stale.
    OtherYear,
    LedgerMissing,
    BalanceNotEstablished,
    ReadIncomplete,
    /// The ledger no longer has the standing the decision was made against,
    /// for example because it was moved to another group in Tally.
    GroupChanged {
        now: DerivedOutcome,
    },
    /// The balance is on the other side from the chosen head.
    OppositeSide,
}

impl NotApplied {
    fn needs_attention(&self) -> bool {
        !matches!(self, Self::OtherYear)
    }
}

/// What the read alone says about each row, in row order. A decision is made
/// against this, so whoever records one reads it from here.
// Called by decision authoring (#737, H3) to show the CA what a save must
// match; until then only tests call it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn derivations(workbook: &PartyLedgerMasterWorkbook) -> Vec<Derivation> {
    derive(workbook.source())
}

fn derive(source: &PartyLedgerMasterSource) -> Vec<Derivation> {
    let groups = GroupIndex::build(source.groups.iter().cloned());
    source
        .rows
        .iter()
        .map(|row| Derivation {
            outcome: match row.closing_balance.as_ref() {
                None => DerivedOutcome::Undetermined(Undetermined::BalanceNotEstablished),
                Some(closing_balance) => classify(row, closing_balance, &groups, &source.groups),
            },
            ancestry: ancestry_to_standing(&groups, row.parent.nonempty_returned_text()),
        })
        .collect()
}

/// Builds the view: the read's own group evidence, with each decision for the
/// read's period applied only when it still fits the read. Every ledger with a
/// decided head or an evidence-backed group subtotal is on a line; every other
/// ledger is listed loudly as an exclusion.
///
/// It takes the validated workbook, whose ledger GUIDs are non-blank and
/// unique, so a decision binds to at most one row.
pub(crate) fn build_schedule_iii_view(
    workbook: &PartyLedgerMasterWorkbook,
    decisions: DecisionInput<'_>,
) -> Result<ScheduleIIIView, ScheduleIIIError> {
    let source = workbook.source();
    let (decision_source, given) = match decisions {
        DecisionInput::Read(set) => {
            if set.book.company_guid != source.company_guid.to_ascii_lowercase() {
                return Err(ScheduleIIIError::DecisionsForAnotherBook);
            }
            if !set.year.contains(&source.to) {
                return Err(ScheduleIIIError::DecisionsForAnotherYear);
            }
            (DecisionSource::Read, set.decisions.as_slice())
        }
        DecisionInput::Unavailable(why) => (DecisionSource::Unavailable(why), &[][..]),
    };
    let derived = derive(source);
    let (decisions, decided) = bind(source, &derived, given);

    let mut debit_total = ExactDecimal::zero();
    let mut credit_total = ExactDecimal::zero();
    let mut line_rows =
        BTreeMap::<(u8, &'static str, &'static str), (LineBasis, Vec<usize>)>::new();
    let mut exclusions = Vec::new();
    for (index, (row, Derivation { outcome, .. })) in source.rows.iter().zip(&derived).enumerate() {
        if let Some(closing_balance) = row.closing_balance.as_ref() {
            if closing_balance.is_negative() {
                debit_total = debit_total
                    .checked_add(
                        &closing_balance
                            .abs()
                            .map_err(|_| ScheduleIIIError::Arithmetic)?,
                    )
                    .map_err(|_| ScheduleIIIError::Arithmetic)?;
            } else {
                credit_total = credit_total
                    .checked_add(closing_balance)
                    .map_err(|_| ScheduleIIIError::Arithmetic)?;
            }
        }
        // Group subtotals sort before decided heads, and each by its text.
        let (key, basis) = match (decided.get(&index), outcome) {
            (Some(head), _) => (
                (1, head.section(), head.caption()),
                LineBasis::Decided(*head),
            ),
            (None, DerivedOutcome::GroupSubtotal(kind)) => (
                (0, kind.section(), kind.label()),
                LineBasis::GroupSubtotal(*kind),
            ),
            (None, DerivedOutcome::Undetermined(undetermined)) => {
                exclusions.push(ScheduleIIIExclusion {
                    row_index: index,
                    reason: undetermined.reason().to_string(),
                });
                continue;
            }
        };
        line_rows
            .entry(key)
            .or_insert_with(|| (basis, Vec::new()))
            .1
            .push(index);
    }

    let mut lines = Vec::with_capacity(line_rows.len());
    for ((_, section, label), (basis, row_indices)) in line_rows {
        let total = sum_rows(source, &row_indices)?;
        lines.push(ScheduleIIILine {
            basis,
            section,
            label,
            total,
            row_indices,
        });
    }
    check_conservation(source, &lines, &exclusions)?;
    let difference = credit_total
        .checked_subtract(&debit_total)
        .map_err(|_| ScheduleIIIError::Arithmetic)?;
    let finality = if decision_source != DecisionSource::Read
        || decisions.iter().any(|status| {
            matches!(status, DecisionStatus::NotApplied { reason, .. } if reason.needs_attention())
        })
    {
        Finality::NotFinal
    } else {
        Finality::Final
    };
    Ok(ScheduleIIIView {
        lines,
        exclusions,
        decisions,
        decision_source,
        finality,
        debit_total,
        credit_total,
        difference,
    })
}

/// Sorts each decision into applied or not applied against this read, and
/// returns the head for each row a decision applies to.
fn bind(
    source: &PartyLedgerMasterSource,
    derived: &[Derivation],
    decisions: &[Decision],
) -> (Vec<DecisionStatus>, BTreeMap<usize, ScheduleIIIHead>) {
    let rows_by_guid: BTreeMap<LedgerGuid, usize> = source
        .rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| Some((LedgerGuid::new(&row.guid)?, index)))
        .collect();

    let mut decided = BTreeMap::new();
    let mut statuses = Vec::with_capacity(decisions.len());
    for decision in decisions {
        let not_applied = |reason| DecisionStatus::NotApplied {
            id: decision.id,
            reason,
        };
        if !decision.year.contains(&source.to) {
            statuses.push(not_applied(NotApplied::OtherYear));
            continue;
        }
        let Some(&row_index) = rows_by_guid.get(&decision.ledger) else {
            statuses.push(not_applied(NotApplied::LedgerMissing));
            continue;
        };
        let row = &source.rows[row_index];
        let Derivation {
            outcome: now,
            ancestry,
        } = &derived[row_index];
        let status = match (now, row.closing_balance.as_ref()) {
            (DerivedOutcome::Undetermined(Undetermined::BalanceNotEstablished), _) | (_, None) => {
                not_applied(NotApplied::BalanceNotEstablished)
            }
            _ if now.standing().is_none() => not_applied(NotApplied::ReadIncomplete),
            _ if now.standing() != decision.made_against.outcome.standing() => {
                not_applied(NotApplied::GroupChanged { now: now.clone() })
            }
            (_, Some(closing_balance)) if !decision.head.side().admits(closing_balance) => {
                not_applied(NotApplied::OppositeSide)
            }
            _ => {
                decided.insert(row_index, decision.head);
                DecisionStatus::Applied {
                    id: decision.id,
                    row_index,
                    renamed_from: (row.name != decision.ledger_name_when_made)
                        .then(|| decision.ledger_name_when_made.clone()),
                    ancestry_changed: (*ancestry != decision.made_against.ancestry).then(|| {
                        AncestryChange {
                            was: decision.made_against.ancestry.clone(),
                            now: ancestry.clone(),
                        }
                    }),
                }
            }
        };
        statuses.push(status);
    }
    (statuses, decided)
}

/// Every row is placed exactly once, checked against the read's own row count
/// rather than trusted from how the view was assembled. The net of the lines
/// plus the excluded rows is also re-added against the read's net; since line
/// totals are summed from the same rows, that half guards the arithmetic, not
/// the placement.
fn check_conservation(
    source: &PartyLedgerMasterSource,
    lines: &[ScheduleIIILine],
    exclusions: &[ScheduleIIIExclusion],
) -> Result<(), ScheduleIIIError> {
    let mut placed = vec![false; source.rows.len()];
    for index in lines
        .iter()
        .flat_map(|line| line.row_indices.iter())
        .chain(exclusions.iter().map(|exclusion| &exclusion.row_index))
    {
        let slot = placed
            .get_mut(*index)
            .ok_or(ScheduleIIIError::NotConserved)?;
        if std::mem::replace(slot, true) {
            return Err(ScheduleIIIError::NotConserved);
        }
    }
    if placed.contains(&false) {
        return Err(ScheduleIIIError::NotConserved);
    }

    let add = |total: ExactDecimal, amount: &ExactDecimal| {
        total
            .checked_add(amount)
            .map_err(|_| ScheduleIIIError::Arithmetic)
    };
    let read_total = source
        .rows
        .iter()
        .filter_map(|row| row.closing_balance.as_ref())
        .try_fold(ExactDecimal::zero(), add)?;
    let presented_total = exclusions
        .iter()
        .filter_map(|exclusion| source.rows[exclusion.row_index].closing_balance.as_ref())
        .chain(lines.iter().map(|line| &line.total))
        .try_fold(ExactDecimal::zero(), add)?;
    if read_total.numeric_eq(&presented_total) {
        Ok(())
    } else {
        Err(ScheduleIIIError::NotConserved)
    }
}

/// Derives what the read alone says about a ledger from its predefined group
/// ancestry and its balance side.
///
/// The climb itself is shared with the bank-voucher classifier, which needs
/// the same traversal for a different verdict — see
/// [`bridge_tally_protocol::group_ancestry`]. What is Schedule III's own is
/// which reserved identities give a group subtotal, the side each requires,
/// and how each refusal reads to someone holding a trial balance.
fn classify(
    row: &PartyLedgerMasterRow,
    closing_balance: &ExactDecimal,
    groups: &GroupIndex,
    masters: &[TallyNamedMaster],
) -> DerivedOutcome {
    let parent = row.parent.nonempty_returned_text();
    let reserved = match groups.reserved_ancestor(parent) {
        Ok(reserved) => normalize(reserved),
        Err(AncestryGap::ReachedRoot) => {
            return DerivedOutcome::Undetermined(below_the_root(parent, groups, masters))
        }
        Err(gap) => return DerivedOutcome::Undetermined(undetermined(gap)),
    };
    match GroupSubtotalKind::from_reserved_name(&reserved) {
        Some(kind) if kind.side().admits(closing_balance) => DerivedOutcome::GroupSubtotal(kind),
        Some(kind) => DerivedOutcome::Undetermined(Undetermined::OppositeSide(kind)),
        // A predefined identity Schedule III does not map. The ancestry is
        // established; it simply does not name a group subtotal.
        None => DerivedOutcome::Undetermined(Undetermined::UnmappedReservedGroup(reserved)),
    }
}

/// The shared walk reports reaching the account root both when a user-created
/// group's parent is the root and when that parent was not returned. Only the
/// first places a ledger; the second is a gap in the read.
fn below_the_root(
    parent: Option<&str>,
    groups: &GroupIndex,
    masters: &[TallyNamedMaster],
) -> Undetermined {
    let chain = groups.ancestry_chain(parent);
    let Some(top) = chain.hops.last() else {
        return Undetermined::AccountRoot;
    };
    let parent_is_root = masters
        .iter()
        .find(|group| group.name == top.name)
        .and_then(|group| group.parent.nonempty_returned_text())
        .is_some_and(is_tally_reserved_root);
    if parent_is_root {
        Undetermined::UserPrimaryGroup(top.name.clone())
    } else {
        Undetermined::ReadIncomplete(ReadGap::ParentNotObserved)
    }
}

fn ancestry_to_standing(groups: &GroupIndex, parent: Option<&str>) -> Vec<String> {
    let mut ancestry = Vec::new();
    for hop in groups.ancestry_chain(parent).hops {
        let predefined = !hop.reserved_name.trim().is_empty();
        ancestry.push(hop.name);
        if predefined {
            break;
        }
    }
    ancestry
}

fn undetermined(gap: AncestryGap) -> Undetermined {
    match gap {
        AncestryGap::NoParent => Undetermined::NoParent,
        // `classify` resolves reaching the root through `below_the_root`, so
        // this arm only keeps the match total.
        AncestryGap::ReachedRoot => Undetermined::ReadIncomplete(ReadGap::ParentNotObserved),
        AncestryGap::GroupAbsent => Undetermined::ReadIncomplete(ReadGap::GroupAbsent),
        AncestryGap::GroupNameRepeated => Undetermined::ReadIncomplete(ReadGap::GroupNameRepeated),
        AncestryGap::ReservedNameMissing => {
            Undetermined::ReadIncomplete(ReadGap::ReservedNameMissing)
        }
        AncestryGap::Cycle => Undetermined::ReadIncomplete(ReadGap::Cycle),
        AncestryGap::Exhausted => Undetermined::ReadIncomplete(ReadGap::Exhausted),
    }
}

fn sum_rows(
    source: &PartyLedgerMasterSource,
    indices: &[usize],
) -> Result<ExactDecimal, ScheduleIIIError> {
    indices
        .iter()
        .try_fold(ExactDecimal::zero(), |total, index| {
            total
                .checked_add(
                    source.rows[*index]
                        .closing_balance
                        .as_ref()
                        .ok_or(ScheduleIIIError::UnestablishedBalance)?,
                )
                .map_err(|_| ScheduleIIIError::Arithmetic)
        })
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
#[path = "schedule_iii_tests.rs"]
mod tests;
