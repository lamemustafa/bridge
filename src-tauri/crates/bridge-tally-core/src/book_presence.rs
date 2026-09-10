//! Deterministic answer to "which of these proposed vouchers are already in
//! this company's book?"
//!
//! See `docs/adr/0017-voucher-presence-authority.md`. Tally has no idempotency
//! (`TALLY_PROTOCOL_REFERENCE.md` §9.3): re-sending a voucher creates a second
//! one, so this question stands between a generated batch and an import.
//!
//! Four rules carry the contract. Only an identity key — a `REMOTEID`, or a
//! voucher number on a voucher type declared `Manual` — can produce `Present`.
//! Nothing binds unless it is unique on both sides. `Absent` is only available
//! from a window proven complete and proven to cover the proposal. Everything
//! else is `PossiblyPresent`, which authorises nothing, carries no preferred
//! answer, and is handed to a person.
//!
//! Party matching is not reimplemented here: it is `master_binding`, whose
//! contract already owns "is this the same customer".
//!
//! This module performs no I/O, holds no company identity, and calls no model.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::exact_arithmetic::ExactDecimalAccumulator;
use crate::master_binding::{
    self, comparison_key, BindingStatus, MasterBindingError, MasterCatalog, MasterClass,
    SourceEntity, UnboundReason,
};
use crate::{ExactDecimal, TallyDate};

/// Most vouchers one observed window may carry. A window past this is refused
/// with a narrow-the-range error rather than silently compared in part.
pub const MAX_WINDOW_VOUCHERS: usize = 20_000;
/// Most vouchers one proposal set may carry.
pub const MAX_PROPOSED_VOUCHERS: usize = 5_000;
/// Most ledger entries one voucher may carry.
pub const MAX_ENTRIES_PER_VOUCHER: usize = 2_000;
/// Most candidates retained per undecided proposal.
pub const MAX_CANDIDATES_PER_PROPOSAL: usize = 25;
/// Most duplicate-number groups listed in the book observations.
pub const MAX_DUPLICATE_NUMBER_GROUPS: usize = 25;
/// Most book keys listed inside one duplicate-number group.
pub const MAX_KEYS_PER_DUPLICATE_GROUP: usize = 10;
/// Most unbalanced book vouchers listed in the book observations.
pub const MAX_UNBALANCED_LISTED: usize = 25;
/// Longest accepted text field, in characters. This bounds pathological input;
/// it is not a claim about what Tally accepts.
pub const MAX_TEXT_CHARS: usize = 16_384;

/// Presence refuses rather than degrades. Every variant is a boundary check on
/// input that was never observed, never complete, or already undecidable
/// before any comparison ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PresenceError {
    /// The window came from a read that was not complete. A window too dense
    /// to read, or one whose emptiness was only partly corroborated, is not
    /// "no match found" — and this is the confusion most likely to turn into a
    /// duplicated invoice, so it is a type error rather than a flag.
    #[error("book window was not read completely")]
    WindowIncomplete,
    #[error("book window range was invalid")]
    WindowRangeInvalid,
    #[error("book window exceeded its bound")]
    WindowTooLarge,
    #[error("book window carried a voucher dated outside its own range")]
    WindowVoucherOutsideRange,
    #[error("book window carried the same voucher key twice")]
    WindowDuplicateVoucherKey,
    /// A proposal dated outside the window would be judged against evidence
    /// that could not contain it.
    #[error("book window does not cover every proposed date")]
    WindowDoesNotCover,
    #[error("no vouchers were proposed")]
    ProposalsEmpty,
    #[error("proposed voucher list exceeded its bound")]
    TooManyProposals,
    #[error("voucher entry list exceeded its bound")]
    TooManyEntries,
    /// A voucher type whose numbering method nobody stated. Defaulting it
    /// would silently decide whether the only decisive key is usable.
    #[error("a proposed voucher type has no declared numbering method")]
    NumberingMethodUndeclared,
    #[error("a voucher type was declared twice with different numbering")]
    NumberingMethodConflict,
    #[error("text field was blank")]
    TextBlank,
    #[error("text field exceeded its bound")]
    TextTooLong,
    #[error("text field carried a control character")]
    TextUnsafe,
    #[error("date was not a valid Tally date")]
    DateInvalid,
    #[error("amount was not an exact decimal")]
    AmountInvalid,
    /// Presence compares party names against ledgers.
    #[error("master catalog was not a ledger catalog")]
    CatalogClassInvalid,
    #[error("party binding refused the input")]
    PartyBinding(MasterBindingError),
}

impl PresenceError {
    /// A stable code safe to surface to an operator or a tool result.
    pub fn safe_reason_code(&self) -> &'static str {
        match self {
            Self::WindowIncomplete => "presence_window_incomplete",
            Self::WindowRangeInvalid => "presence_window_range_invalid",
            Self::WindowTooLarge => "presence_window_too_large",
            Self::WindowVoucherOutsideRange => "presence_window_voucher_outside_range",
            Self::WindowDuplicateVoucherKey => "presence_window_duplicate_voucher_key",
            Self::WindowDoesNotCover => "presence_window_does_not_cover",
            Self::ProposalsEmpty => "presence_proposals_empty",
            Self::TooManyProposals => "presence_proposals_too_many",
            Self::TooManyEntries => "presence_entries_too_many",
            Self::NumberingMethodUndeclared => "presence_numbering_method_undeclared",
            Self::NumberingMethodConflict => "presence_numbering_method_conflict",
            Self::TextBlank => "presence_text_blank",
            Self::TextTooLong => "presence_text_too_long",
            Self::TextUnsafe => "presence_text_unsafe",
            Self::DateInvalid => "presence_date_invalid",
            Self::AmountInvalid => "presence_amount_invalid",
            Self::CatalogClassInvalid => "presence_catalog_class_invalid",
            Self::PartyBinding(error) => error.safe_reason_code(),
        }
    }
}

/// Whether the window's read gathered `REMOTEID` at all. A read profile that
/// does not fetch the field yields `NotRead`, which is a different fact from
/// "no voucher carried one" and must not be confused with it: a proposal whose
/// own `REMOTEID` was never compared cannot be reported `Absent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteIdEvidence {
    /// The read fetched `REMOTEID`; an absent value means the voucher has none.
    Observed,
    /// The read did not fetch `REMOTEID`; absence means nothing at all.
    NotRead,
}

/// How completely the window's source read observed its range. Only a complete
/// read may become a `BookWindow`; the other value exists so a caller must
/// state which it has rather than omit the question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowRead {
    Complete,
    Partial,
}

/// A voucher type's numbering method decides whether its voucher number is an
/// identity or a coincidence (§9.8). Under `Automatic`, Tally discards the
/// supplied number, so a number-based key is silently ineffective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumberingMethod {
    Manual,
    Automatic,
    /// Nobody has observed it. Legal, honest, and the common case; it demotes
    /// the number from identity to resemblance.
    Unknown,
}

/// Whether an observed voucher has accounting effect. A cancelled or optional
/// voucher still occupies its number, so it can be matched and must never be
/// reported as a posted duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostingState {
    Posted,
    Cancelled,
    Optional,
}

/// One ledger entry, from either side of the comparison. The same function
/// derives a magnitude from both, so the two sides cannot compute one fact
/// differently.
#[derive(Debug, Clone, Copy)]
pub struct ObservedEntry<'a> {
    pub ledger: &'a str,
    pub amount: &'a str,
}

/// One voucher as the book was observed to hold it.
#[derive(Debug, Clone, Copy)]
pub struct ObservedVoucher<'a> {
    /// An opaque caller-owned key for this voucher. It is echoed back in the
    /// report and never interpreted, so a caller chooses whatever it can join
    /// on without granting this crate any identity.
    pub key: &'a str,
    pub date: &'a str,
    pub voucher_type: &'a str,
    pub voucher_number: Option<&'a str>,
    pub remote_id: Option<&'a str>,
    /// `PARTYLEDGERNAME`, when the read carried one.
    pub party: Option<&'a str>,
    pub entries: &'a [ObservedEntry<'a>],
    pub cancelled: bool,
    pub optional: bool,
}

/// One voucher a source document proposes to import.
#[derive(Debug, Clone, Copy)]
pub struct ProposedVoucherInput<'a> {
    pub position: usize,
    pub date: &'a str,
    pub voucher_type: &'a str,
    pub voucher_number: Option<&'a str>,
    pub remote_id: Option<&'a str>,
    /// The party name exactly as the source document gives it. It is bound
    /// through `master_binding`, never compared raw.
    pub party: Option<&'a str>,
    pub entries: &'a [ObservedEntry<'a>],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookVoucher {
    key: String,
    date: TallyDate,
    voucher_type: String,
    voucher_number: Option<String>,
    remote_id: Option<String>,
    party: Option<String>,
    ledger_keys: BTreeSet<String>,
    magnitude: ExactDecimal,
    balanced: bool,
    posting: PostingState,
    type_key: String,
    number_key: Option<String>,
}

impl BookVoucher {
    pub fn observed(input: ObservedVoucher<'_>) -> Result<Self, PresenceError> {
        let key = validated_text(input.key)?;
        let date =
            TallyDate::parse(input.date.to_string()).map_err(|_| PresenceError::DateInvalid)?;
        let voucher_type = validated_text(input.voucher_type)?;
        let voucher_number = input.voucher_number.map(validated_text).transpose()?;
        let remote_id = input.remote_id.map(validated_text).transpose()?;
        let party = input.party.map(validated_text).transpose()?;
        let (magnitude, balanced, mut ledger_keys) = magnitude_of(input.entries)?;
        if let Some(party) = party.as_deref() {
            ledger_keys.insert(comparison_key(party));
        }
        let type_key = comparison_key(&voucher_type);
        let number_key = voucher_number.as_deref().map(comparison_key);
        Ok(Self {
            key,
            date,
            voucher_type,
            voucher_number,
            remote_id,
            party,
            ledger_keys,
            magnitude,
            balanced,
            // A cancelled voucher is cancelled whatever else it is.
            posting: match (input.cancelled, input.optional) {
                (true, _) => PostingState::Cancelled,
                (false, true) => PostingState::Optional,
                (false, false) => PostingState::Posted,
            },
            type_key,
            number_key,
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn date(&self) -> &str {
        self.date.as_str()
    }

    pub fn voucher_type(&self) -> &str {
        &self.voucher_type
    }

    pub fn voucher_number(&self) -> Option<&str> {
        self.voucher_number.as_deref()
    }

    pub fn party(&self) -> Option<&str> {
        self.party.as_deref()
    }

    pub fn magnitude(&self) -> &ExactDecimal {
        &self.magnitude
    }

    pub fn posting(&self) -> PostingState {
        self.posting
    }

    /// Whether the observed entries summed to zero. An unbalanced voucher is
    /// reported and still participates in every rule: excluding it would make
    /// `Absent` more likely, which is the wrong direction.
    pub fn balanced(&self) -> bool {
        self.balanced
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedVoucher {
    position: usize,
    date: TallyDate,
    voucher_type: String,
    voucher_number: Option<String>,
    remote_id: Option<String>,
    party: Option<String>,
    magnitude: ExactDecimal,
    balanced: bool,
    type_key: String,
    number_key: Option<String>,
}

impl ProposedVoucher {
    pub fn new(input: ProposedVoucherInput<'_>) -> Result<Self, PresenceError> {
        let date =
            TallyDate::parse(input.date.to_string()).map_err(|_| PresenceError::DateInvalid)?;
        let voucher_type = validated_text(input.voucher_type)?;
        let voucher_number = input.voucher_number.map(validated_text).transpose()?;
        let remote_id = input.remote_id.map(validated_text).transpose()?;
        let party = input.party.map(validated_text).transpose()?;
        let (magnitude, balanced, _) = magnitude_of(input.entries)?;
        let type_key = comparison_key(&voucher_type);
        let number_key = voucher_number.as_deref().map(comparison_key);
        Ok(Self {
            position: input.position,
            date,
            voucher_type,
            voucher_number,
            remote_id,
            party,
            magnitude,
            balanced,
            type_key,
            number_key,
        })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn date(&self) -> &str {
        self.date.as_str()
    }

    pub fn voucher_type(&self) -> &str {
        &self.voucher_type
    }

    pub fn magnitude(&self) -> &ExactDecimal {
        &self.magnitude
    }

    pub fn balanced(&self) -> bool {
        self.balanced
    }
}

/// One observed window of a company's book. It can only be constructed from a
/// read that observed its whole range, so "the window was too dense to read"
/// can never reach a comparison as "nothing matched".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookWindow {
    from: TallyDate,
    to: TallyDate,
    remote_id_evidence: RemoteIdEvidence,
    vouchers: Vec<BookVoucher>,
}

impl BookWindow {
    pub fn observed(
        from: &str,
        to: &str,
        read: WindowRead,
        remote_id_evidence: RemoteIdEvidence,
        vouchers: Vec<BookVoucher>,
    ) -> Result<Self, PresenceError> {
        if read != WindowRead::Complete {
            return Err(PresenceError::WindowIncomplete);
        }
        let from = TallyDate::parse(from.to_string()).map_err(|_| PresenceError::DateInvalid)?;
        let to = TallyDate::parse(to.to_string()).map_err(|_| PresenceError::DateInvalid)?;
        if from.as_str() > to.as_str() {
            return Err(PresenceError::WindowRangeInvalid);
        }
        if vouchers.len() > MAX_WINDOW_VOUCHERS {
            return Err(PresenceError::WindowTooLarge);
        }
        let mut keys = BTreeSet::new();
        for voucher in &vouchers {
            if voucher.date() < from.as_str() || voucher.date() > to.as_str() {
                return Err(PresenceError::WindowVoucherOutsideRange);
            }
            if !keys.insert(voucher.key()) {
                return Err(PresenceError::WindowDuplicateVoucherKey);
            }
        }
        Ok(Self {
            from,
            to,
            remote_id_evidence,
            vouchers,
        })
    }

    pub fn from(&self) -> &str {
        self.from.as_str()
    }

    pub fn to(&self) -> &str {
        self.to.as_str()
    }

    pub fn vouchers(&self) -> &[BookVoucher] {
        &self.vouchers
    }

    pub fn remote_id_evidence(&self) -> RemoteIdEvidence {
        self.remote_id_evidence
    }

    fn covers(&self, date: &str) -> bool {
        date >= self.from.as_str() && date <= self.to.as_str()
    }
}

/// The numbering method of every voucher type a proposal names. A type that is
/// missing is an error, not a default: the declaration decides whether the only
/// decisive key is usable at all.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NumberingDeclaration {
    methods: BTreeMap<String, NumberingMethod>,
}

impl NumberingDeclaration {
    pub fn new<I, S>(entries: I) -> Result<Self, PresenceError>
    where
        I: IntoIterator<Item = (S, NumberingMethod)>,
        S: AsRef<str>,
    {
        let mut methods = BTreeMap::new();
        for (voucher_type, method) in entries {
            let key = comparison_key(&validated_text(voucher_type.as_ref())?);
            if methods
                .insert(key, method)
                .is_some_and(|prior| prior != method)
            {
                return Err(PresenceError::NumberingMethodConflict);
            }
        }
        Ok(Self { methods })
    }

    /// Whether a voucher type has a declared method, without needing the
    /// caller to reproduce this crate's comparison key. A consumer validating
    /// its own arguments before performing a read uses this.
    pub fn declares(&self, voucher_type: &str) -> bool {
        self.methods.contains_key(&comparison_key(voucher_type))
    }

    fn method(&self, type_key: &str) -> Option<NumberingMethod> {
        self.methods.get(type_key).copied()
    }
}

/// How a proposal's party name resolved against the observed ledger catalog.
/// It reports the binding and nothing more: an ambiguous party's candidates
/// belong to `validate_masters`, which owns that vocabulary, and naming one of
/// them here would be the auto-resolution ADR 0016 forbids.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "party_state")]
pub enum PartyOutcome {
    NotSupplied,
    Bound {
        catalog_name: String,
    },
    Ambiguous {
        reason: String,
        candidate_count: usize,
    },
    Unmatched {
        reason: String,
    },
}

/// The evidence that decided a `Present`. Both are identity. Date, amount and
/// party are never a basis; they are the keys that measurably collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceBasis {
    RemoteId,
    ManualVoucherNumber,
}

/// The rule that surfaced a candidate. Ordered by `rank`, never by similarity,
/// and no candidate is marked best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRule {
    SharedRemoteId,
    SharedVoucherNumber,
    SameDatePartyAmount,
    SamePartyAmount,
    SameDateAmount,
    SameDateParty,
}

impl CandidateRule {
    fn rank(self) -> u8 {
        match self {
            Self::SharedRemoteId => 0,
            Self::SharedVoucherNumber => 1,
            Self::SameDatePartyAmount => 2,
            Self::SamePartyAmount => 3,
            Self::SameDateAmount => 4,
            Self::SameDateParty => 5,
        }
    }
}

/// A book voucher an operator may judge, with the rule that surfaced it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PresenceCandidate {
    pub book_key: String,
    pub rule: CandidateRule,
}

/// Why a proposal was not decided. Exactly one, by the precedence in
/// `assess`: a collision outranks a resemblance, and a resemblance outranks an
/// incomplete party comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UndecidedReason {
    /// One `REMOTEID` is carried by more than one voucher on either side.
    RemoteIdCollision,
    /// The number selects more than one book voucher. This is the book that
    /// held twenty-five invoices sharing numbers in one month.
    BookNumberCollision,
    /// More than one proposal claims the number under manual numbering.
    ProposalNumberCollision,
    /// An identity key landed on a cancelled or optional voucher. It occupies
    /// the number but has no accounting effect.
    MatchedVoucherNotPosted,
    /// A number matched, but the voucher type is not declared `Manual`, so the
    /// number is not identity (§9.8).
    NumberNotDecisive,
    /// A number matched under a voucher type this window never observed. The
    /// number was compared across every type rather than manufacture an
    /// absence, so it is a resemblance and not a series position.
    VoucherTypeNotObserved,
    /// Date, party or amount resembles a book voucher. These collide in real
    /// data and never decide.
    ResemblesBookVoucher,
    /// The party comparison could not be completed, so no rule that needs a
    /// party actually ran and `Absent` is not available.
    PartyNotDecidable,
    /// Two proposals both resolved to the same book voucher, possibly by
    /// different identity bases. One book voucher can satisfy at most one
    /// proposal, so every claimant is demoted rather than one being chosen.
    BookVoucherClaimedTwice,
    /// A number matched uniquely while the two sides carried *different*
    /// `REMOTEID`s. Two identity signals disagree, and a disagreement is
    /// reported rather than settled in the number's favour.
    IdentityConflict,
    /// The proposal carries a `REMOTEID` the window never read, so the
    /// strongest key available to this proposal was never compared. An
    /// `Absent` here would rest on evidence that was not gathered.
    RemoteIdEvidenceUnavailable,
}

impl UndecidedReason {
    /// A stable code safe to surface to an operator or a tool result.
    pub fn safe_reason_code(self) -> &'static str {
        match self {
            Self::RemoteIdCollision => "presence_remote_id_collision",
            Self::BookNumberCollision => "presence_book_number_collision",
            Self::ProposalNumberCollision => "presence_proposal_number_collision",
            Self::MatchedVoucherNotPosted => "presence_matched_voucher_not_posted",
            Self::NumberNotDecisive => "presence_number_not_decisive",
            Self::VoucherTypeNotObserved => "presence_voucher_type_not_observed",
            Self::ResemblesBookVoucher => "presence_resembles_book_voucher",
            Self::PartyNotDecidable => "presence_party_not_decidable",
            Self::BookVoucherClaimedTwice => "presence_book_voucher_claimed_twice",
            Self::IdentityConflict => "presence_identity_conflict",
            Self::RemoteIdEvidenceUnavailable => "presence_remote_id_evidence_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DifferenceField {
    Date,
    Amount,
    Party,
}

/// A field on which an identified voucher disagrees with its source. The match
/// was decided by identity, so a difference is a finding about the book — not
/// evidence against the match.
///
/// **It is a finding for a person, and the obvious way to act on it in code is
/// destructive.** On the observed instance a voucher `Alter` returns
/// `CREATED=1, ALTERED=0` and makes a duplicate while leaving the target
/// untouched (`TALLY_PROTOCOL_REFERENCE.md` §9.7, four keys tested and all four
/// duplicating), and `Cancel` behaves the same way (§9.6). A caller that reads
/// "amount differs" and reaches for an `Alter` creates the duplicate this whole
/// contract exists to prevent, and Tally's counters report success. The only
/// correction that works is re-import under the same client `REMOTEID`
/// (`IMPLEMENTATION_GUIDE.md` §3.3a), which reaches only vouchers Bridge itself
/// wrote — so for the hand-keyed voucher this contract is built for there is no
/// programmatic correction path at all, and the operator fixes it in Tally.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Difference {
    pub field: DifferenceField,
    pub proposed: Option<String>,
    pub observed: Option<String>,
}

/// What could not be decided, and why. This is the operator's work item, not
/// an error path — and it carries no field that names a match.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Undecided {
    pub reason: UndecidedReason,
    pub candidates: Vec<PresenceCandidate>,
    /// Candidates found before truncation.
    pub candidate_count: usize,
    pub candidates_truncated: bool,
}

/// Exactly one outcome per proposed voucher.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "presence")]
pub enum PresenceStatus {
    /// An identity key matched uniquely on both sides. This is the only status
    /// that names a book voucher, and the only one that authorises excluding a
    /// voucher from an import.
    Present {
        book_key: String,
        basis: PresenceBasis,
        differences: Vec<Difference>,
    },
    /// Something resembles it, or something prevented a decision. Authorises
    /// nothing.
    PossiblyPresent(Undecided),
    /// No rule produced any candidate, in a window proven to cover it.
    /// `Absent` is always relative to that window.
    Absent,
}

/// One proposed voucher and its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VoucherPresence {
    pub position: usize,
    /// The voucher number exactly as the source document gave it.
    pub voucher_number: Option<String>,
    pub numbering_method: NumberingMethod,
    /// Whether this proposal's voucher type was observed anywhere in the
    /// window. When it was not, type stops discriminating and number matching
    /// widens to every observed type — narrowing on an unobserved type name
    /// would manufacture absence.
    pub voucher_type_observed: bool,
    pub party: PartyOutcome,
    #[serde(flatten)]
    pub status: PresenceStatus,
}

impl VoucherPresence {
    pub fn present_book_key(&self) -> Option<&str> {
        match &self.status {
            PresenceStatus::Present { book_key, .. } => Some(book_key.as_str()),
            _ => None,
        }
    }

    pub fn undecided(&self) -> Option<&Undecided> {
        match &self.status {
            PresenceStatus::PossiblyPresent(undecided) => Some(undecided),
            _ => None,
        }
    }

    pub fn is_absent(&self) -> bool {
        matches!(self.status, PresenceStatus::Absent)
    }
}

/// A (voucher type, number) pair that identifies more than one book voucher.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DuplicateNumberGroup {
    pub voucher_type: String,
    pub voucher_number: String,
    pub book_keys: Vec<String>,
    pub book_voucher_count: usize,
}

/// What the book gave away while it was being indexed. These cost nothing to
/// compute and one of them is a filed-return problem.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BookObservations {
    pub duplicate_numbers: Vec<DuplicateNumberGroup>,
    pub duplicate_number_group_count: usize,
    pub duplicate_numbers_truncated: bool,
    pub unbalanced_vouchers: Vec<String>,
    pub unbalanced_voucher_count: usize,
    /// Vouchers of a proposed voucher type that no proposal matched or even
    /// resembled — the other half of a reconciliation. Counted, not listed.
    pub unmatched_book_vouchers: usize,
    pub window_voucher_count: usize,
    /// Whether any observed voucher carried a `REMOTEID` at all. Without this,
    /// an absence of remote-id matches reads as evidence that none exist.
    pub remote_id_observed: bool,
}

/// Control totals for one run. `requested == present + possibly_present +
/// absent` always holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct PresenceTotals {
    pub requested: usize,
    pub present: usize,
    pub possibly_present: usize,
    pub absent: usize,
}

/// The result of one presence run, scoped to the window it was computed over.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PresenceReport {
    window_from: String,
    window_to: String,
    vouchers: Vec<VoucherPresence>,
    observations: BookObservations,
}

impl PresenceReport {
    /// The window every verdict is relative to. `Absent` means absent from
    /// this range, never absent from the book.
    pub fn window(&self) -> (&str, &str) {
        (&self.window_from, &self.window_to)
    }

    pub fn vouchers(&self) -> &[VoucherPresence] {
        &self.vouchers
    }

    pub fn observations(&self) -> &BookObservations {
        &self.observations
    }

    /// The vouchers an import may carry. Nothing else is safe to include
    /// without a person.
    pub fn absent(&self) -> impl Iterator<Item = &VoucherPresence> {
        self.vouchers.iter().filter(|entry| entry.is_absent())
    }

    pub fn present(&self) -> impl Iterator<Item = &VoucherPresence> {
        self.vouchers
            .iter()
            .filter(|entry| entry.present_book_key().is_some())
    }

    pub fn possibly_present(&self) -> impl Iterator<Item = &VoucherPresence> {
        self.vouchers
            .iter()
            .filter(|entry| entry.undecided().is_some())
    }

    pub fn totals(&self) -> PresenceTotals {
        let present = self.present().count();
        let possibly_present = self.possibly_present().count();
        let absent = self.absent().count();
        PresenceTotals {
            requested: self.vouchers.len(),
            present,
            possibly_present,
            absent,
        }
    }
}

/// Already-valid inputs for one presence run. Every cross-input refusal — the
/// window covering the proposals, a declared numbering method for every
/// proposed type, a ledger catalog, the party binding itself — happens here,
/// so `assess` cannot fail and no caller can compensate differently.
#[derive(Debug)]
pub struct PresenceRequest<'a> {
    window: &'a BookWindow,
    numbering: &'a NumberingDeclaration,
    proposals: &'a [ProposedVoucher],
    party_bindings: Vec<PartyResolution>,
}

/// A bound party reduced to what the rules need: the names to compare against,
/// and whether the comparison was complete enough to justify `Absent`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PartyResolution {
    outcome: PartyOutcome,
    compare_keys: BTreeSet<String>,
    /// True when names that might have matched were never compared.
    incomplete: bool,
}

impl<'a> PresenceRequest<'a> {
    pub fn new(
        window: &'a BookWindow,
        catalog: &'a MasterCatalog,
        numbering: &'a NumberingDeclaration,
        proposals: &'a [ProposedVoucher],
    ) -> Result<Self, PresenceError> {
        if catalog.class() != MasterClass::Ledger {
            return Err(PresenceError::CatalogClassInvalid);
        }
        if proposals.is_empty() {
            return Err(PresenceError::ProposalsEmpty);
        }
        if proposals.len() > MAX_PROPOSED_VOUCHERS {
            return Err(PresenceError::TooManyProposals);
        }
        for proposal in proposals {
            if !window.covers(proposal.date()) {
                return Err(PresenceError::WindowDoesNotCover);
            }
            if numbering.method(&proposal.type_key).is_none() {
                return Err(PresenceError::NumberingMethodUndeclared);
            }
        }
        let party_bindings = bind_parties(catalog, proposals)?;
        Ok(Self {
            window,
            numbering,
            proposals,
            party_bindings,
        })
    }
}

/// Binds every distinct proposed party name through `master_binding`, once, and
/// reduces each to the names the rules may compare against.
fn bind_parties(
    catalog: &MasterCatalog,
    proposals: &[ProposedVoucher],
) -> Result<Vec<PartyResolution>, PresenceError> {
    let mut distinct: Vec<&str> = proposals
        .iter()
        .filter_map(|proposal| proposal.party.as_deref())
        .collect();
    distinct.sort_unstable();
    distinct.dedup();
    let entities = distinct
        .iter()
        .enumerate()
        .map(|(position, name)| SourceEntity::new(position, name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(PresenceError::PartyBinding)?;
    let report = master_binding::bind(catalog, &entities).map_err(PresenceError::PartyBinding)?;
    let resolved = distinct
        .iter()
        .zip(report.entities())
        .map(|(name, binding)| ((*name).to_string(), resolution_of(binding)))
        .collect::<BTreeMap<_, _>>();
    Ok(proposals
        .iter()
        .map(|proposal| match proposal.party.as_deref() {
            None => PartyResolution {
                outcome: PartyOutcome::NotSupplied,
                compare_keys: BTreeSet::new(),
                incomplete: false,
            },
            Some(name) => resolved
                .get(name)
                .cloned()
                .expect("every proposed party name was bound"),
        })
        .collect())
}

fn resolution_of(binding: &master_binding::EntityBinding) -> PartyResolution {
    match &binding.status {
        BindingStatus::Bound { catalog_name, .. } => PartyResolution {
            outcome: PartyOutcome::Bound {
                catalog_name: catalog_name.clone(),
            },
            compare_keys: BTreeSet::from([comparison_key(catalog_name)]),
            incomplete: false,
        },
        // Every candidate is compared, never one of them. Widening the net can
        // only produce more resemblance, which is the safe direction here.
        BindingStatus::Ambiguous(unresolved) => PartyResolution {
            outcome: PartyOutcome::Ambiguous {
                reason: unresolved.reason.safe_reason_code().to_string(),
                candidate_count: unresolved.candidate_count,
            },
            compare_keys: unresolved
                .candidates
                .iter()
                .map(|candidate| comparison_key(&candidate.catalog_name))
                .collect(),
            // A name family is deliberately not listed, and a truncated list
            // leaves names uncompared. Either way `Absent` would rest on a
            // comparison that never ran.
            incomplete: unresolved.reason == UnboundReason::NoDiscriminatingCandidate
                || unresolved.candidates_truncated,
        },
        // Nothing in this book resembles the party, so no posted voucher can
        // be carrying it. Party rules simply do not run.
        BindingStatus::Unmatched(unresolved) => PartyResolution {
            outcome: PartyOutcome::Unmatched {
                reason: unresolved.reason.safe_reason_code().to_string(),
            },
            compare_keys: BTreeSet::new(),
            incomplete: unresolved.reason == UnboundReason::NoDiscriminatingCandidate
                || unresolved.candidates_truncated,
        },
    }
}

/// Indexes of one window, built once per run.
struct WindowIndex<'a> {
    by_remote_id: BTreeMap<&'a str, Vec<usize>>,
    by_type_and_number: BTreeMap<(&'a str, &'a str), Vec<usize>>,
    by_number: BTreeMap<&'a str, Vec<usize>>,
    by_date: BTreeMap<&'a str, Vec<usize>>,
    by_ledger: BTreeMap<&'a str, Vec<usize>>,
    type_keys: BTreeSet<&'a str>,
}

impl<'a> WindowIndex<'a> {
    fn build(window: &'a BookWindow) -> Self {
        let mut index = Self {
            by_remote_id: BTreeMap::new(),
            by_type_and_number: BTreeMap::new(),
            by_number: BTreeMap::new(),
            by_date: BTreeMap::new(),
            by_ledger: BTreeMap::new(),
            type_keys: BTreeSet::new(),
        };
        for (position, voucher) in window.vouchers.iter().enumerate() {
            index.type_keys.insert(voucher.type_key.as_str());
            if let Some(remote_id) = voucher.remote_id.as_deref() {
                index
                    .by_remote_id
                    .entry(remote_id)
                    .or_default()
                    .push(position);
            }
            if let Some(number_key) = voucher.number_key.as_deref() {
                index
                    .by_type_and_number
                    .entry((voucher.type_key.as_str(), number_key))
                    .or_default()
                    .push(position);
                index
                    .by_number
                    .entry(number_key)
                    .or_default()
                    .push(position);
            }
            index
                .by_date
                .entry(voucher.date.as_str())
                .or_default()
                .push(position);
            for ledger in &voucher.ledger_keys {
                index
                    .by_ledger
                    .entry(ledger.as_str())
                    .or_default()
                    .push(position);
            }
        }
        index
    }
}

/// Decides every proposal against the window.
///
/// `Present` requires identity unique on both sides. `Absent` requires that no
/// rule produced any candidate. Everything between is `PossiblyPresent` and is
/// never resolved here. See `docs/adr/0017-voucher-presence-authority.md` for
/// why the two bars are set at different heights.
pub fn assess(request: &PresenceRequest<'_>) -> PresenceReport {
    let window = request.window;
    let index = WindowIndex::build(window);

    let mut proposal_remote_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut proposal_number_counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for proposal in request.proposals {
        if let Some(remote_id) = proposal.remote_id.as_deref() {
            *proposal_remote_counts.entry(remote_id).or_default() += 1;
        }
        if let Some(number_key) = proposal.number_key.as_deref() {
            *proposal_number_counts
                .entry((proposal.type_key.as_str(), number_key))
                .or_default() += 1;
        }
    }

    let mut touched_book: BTreeSet<usize> = BTreeSet::new();
    let mut proposed_type_keys: BTreeSet<&str> = BTreeSet::new();
    let mut vouchers = Vec::with_capacity(request.proposals.len());
    for (proposal, party) in request.proposals.iter().zip(&request.party_bindings) {
        proposed_type_keys.insert(proposal.type_key.as_str());
        let decided = decide(
            proposal,
            party,
            window,
            &index,
            request.numbering,
            &proposal_remote_counts,
            &proposal_number_counts,
        );
        touched_book.extend(decided.touched);
        vouchers.push(decided.presence);
    }

    // One book voucher satisfies at most one proposal. Uniqueness was enforced
    // within each identity basis; nothing yet stopped two proposals reaching
    // the same voucher by *different* bases — one by `REMOTEID`, another by a
    // manual number — and a consumer would then exclude two source vouchers
    // against one book row, silently dropping an invoice. Every claimant is
    // demoted; choosing between them would be the auto-resolution this whole
    // contract refuses.
    let mut claims: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &vouchers {
        if let Some(book_key) = entry.present_book_key() {
            *claims.entry(book_key.to_string()).or_default() += 1;
        }
    }
    let contested = claims
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(book_key, _)| book_key)
        .collect::<BTreeSet<_>>();
    if !contested.is_empty() {
        for entry in &mut vouchers {
            let Some(book_key) = entry.present_book_key() else {
                continue;
            };
            if !contested.contains(book_key) {
                continue;
            }
            let book_key = book_key.to_string();
            let rule = match &entry.status {
                PresenceStatus::Present {
                    basis: PresenceBasis::RemoteId,
                    ..
                } => CandidateRule::SharedRemoteId,
                _ => CandidateRule::SharedVoucherNumber,
            };
            entry.status = PresenceStatus::PossiblyPresent(undecided(
                UndecidedReason::BookVoucherClaimedTwice,
                vec![PresenceCandidate { book_key, rule }],
            ));
        }
    }

    let observations = observe(window, &index, &proposed_type_keys, &touched_book);
    PresenceReport {
        window_from: window.from().to_string(),
        window_to: window.to().to_string(),
        vouchers,
        observations,
    }
}

/// One proposal's verdict, plus every book voucher it reached *before* the
/// response candidate cap. The observations need the full set: a candidate
/// dropped by the cap was still resembled, and counting it as untouched would
/// report it as a voucher no proposal came near.
struct Decided {
    presence: VoucherPresence,
    touched: BTreeSet<usize>,
}

#[allow(clippy::too_many_arguments)]
fn decide(
    proposal: &ProposedVoucher,
    party: &PartyResolution,
    window: &BookWindow,
    index: &WindowIndex<'_>,
    numbering: &NumberingDeclaration,
    proposal_remote_counts: &BTreeMap<&str, usize>,
    proposal_number_counts: &BTreeMap<(&str, &str), usize>,
) -> Decided {
    let method = numbering
        .method(&proposal.type_key)
        .expect("PresenceRequest refused an undeclared numbering method");
    let type_observed = index.type_keys.contains(proposal.type_key.as_str());
    let shell = |status: PresenceStatus, touched: BTreeSet<usize>| Decided {
        presence: VoucherPresence {
            position: proposal.position,
            voucher_number: proposal.voucher_number.clone(),
            numbering_method: method,
            voucher_type_observed: type_observed,
            party: party.outcome.clone(),
            status,
        },
        touched,
    };
    // A proposal carrying a `REMOTEID` the window never fetched has had its
    // strongest key silently skipped. That cannot license an absence.
    let remote_id_unverifiable =
        proposal.remote_id.is_some() && window.remote_id_evidence() == RemoteIdEvidence::NotRead;

    // Rule one: identity first. A REMOTEID is a key Bridge itself wrote.
    if let Some(remote_id) = proposal.remote_id.as_deref() {
        let unique_here = proposal_remote_counts.get(remote_id).copied() == Some(1);
        let empty = Vec::new();
        let matches = index.by_remote_id.get(remote_id).unwrap_or(&empty);
        // Proposal-side uniqueness is checked *before* the book lookup, the
        // same way a duplicated manual number is. Two source rows claiming one
        // identity are undecidable whether or not the book holds it, and
        // falling through would report both as safe to import.
        if !unique_here {
            return shell(
                PresenceStatus::PossiblyPresent(undecided(
                    UndecidedReason::RemoteIdCollision,
                    candidates_from(window, matches, CandidateRule::SharedRemoteId),
                )),
                matches.iter().copied().collect(),
            );
        }
        if !matches.is_empty() {
            if matches.len() == 1 && unique_here {
                return shell(
                    settled(
                        proposal,
                        party,
                        &window.vouchers[matches[0]],
                        PresenceBasis::RemoteId,
                    ),
                    BTreeSet::from([matches[0]]),
                );
            }
            return shell(
                PresenceStatus::PossiblyPresent(undecided(
                    UndecidedReason::RemoteIdCollision,
                    candidates_from(window, matches, CandidateRule::SharedRemoteId),
                )),
                matches.iter().copied().collect(),
            );
        }
    }

    // Rule two: a voucher number is identity only where the numbering method
    // preserves it (§9.8), and only when it is unique on both sides.
    let number_matches: Vec<usize> = proposal
        .number_key
        .as_deref()
        .map(|number_key| {
            if type_observed {
                index
                    .by_type_and_number
                    .get(&(proposal.type_key.as_str(), number_key))
                    .cloned()
                    .unwrap_or_default()
            } else {
                // The type name was never observed, so it discriminates
                // nothing. Widen rather than manufacture an absence.
                index.by_number.get(number_key).cloned().unwrap_or_default()
            }
        })
        .unwrap_or_default();

    // Manual numbering only decides within an observed voucher type: numbers
    // are a per-type series, so a cross-type number match is a resemblance.
    if method == NumberingMethod::Manual && type_observed {
        if let Some(number_key) = proposal.number_key.as_deref() {
            let proposed_twice = proposal_number_counts
                .get(&(proposal.type_key.as_str(), number_key))
                .copied()
                .unwrap_or_default()
                > 1;
            let touched = number_matches.iter().copied().collect::<BTreeSet<_>>();
            if proposed_twice {
                return shell(
                    PresenceStatus::PossiblyPresent(undecided(
                        UndecidedReason::ProposalNumberCollision,
                        candidates_from(
                            window,
                            &number_matches,
                            CandidateRule::SharedVoucherNumber,
                        ),
                    )),
                    touched,
                );
            }
            if number_matches.len() > 1 {
                return shell(
                    PresenceStatus::PossiblyPresent(undecided(
                        UndecidedReason::BookNumberCollision,
                        candidates_from(
                            window,
                            &number_matches,
                            CandidateRule::SharedVoucherNumber,
                        ),
                    )),
                    touched,
                );
            }
            if number_matches.len() == 1 {
                let matched = &window.vouchers[number_matches[0]];
                // Two identity signals that disagree are reported, never
                // settled in the number's favour — the same rule ADR 0016
                // applies to an identifier contradicting an exact name.
                let contradicted =
                    match (proposal.remote_id.as_deref(), matched.remote_id.as_deref()) {
                        (Some(proposed), Some(observed)) => proposed != observed,
                        _ => false,
                    };
                if contradicted {
                    return shell(
                        PresenceStatus::PossiblyPresent(undecided(
                            UndecidedReason::IdentityConflict,
                            candidates_from(
                                window,
                                &number_matches,
                                CandidateRule::SharedVoucherNumber,
                            ),
                        )),
                        touched,
                    );
                }
                return shell(
                    settled(proposal, party, matched, PresenceBasis::ManualVoucherNumber),
                    touched,
                );
            }
        }
    }

    // Rule three: everything else is resemblance, and resemblance decides
    // nothing. It only widens what a person is asked to look at.
    let mut found: BTreeMap<usize, CandidateRule> = BTreeMap::new();
    for position in &number_matches {
        keep_strongest(&mut found, *position, CandidateRule::SharedVoucherNumber);
    }
    let mut pool: BTreeSet<usize> = BTreeSet::new();
    if let Some(positions) = index.by_date.get(proposal.date()) {
        pool.extend(positions.iter().copied());
    }
    for key in &party.compare_keys {
        if let Some(positions) = index.by_ledger.get(key.as_str()) {
            pool.extend(positions.iter().copied());
        }
    }
    for position in pool {
        let voucher = &window.vouchers[position];
        let same_date = voucher.date() == proposal.date();
        let same_amount = voucher.magnitude.numeric_eq(&proposal.magnitude);
        let same_party = party
            .compare_keys
            .iter()
            .any(|key| voucher.ledger_keys.contains(key));
        let rule = match (same_date, same_party, same_amount) {
            (true, true, true) => CandidateRule::SameDatePartyAmount,
            (_, true, true) => CandidateRule::SamePartyAmount,
            (true, false, true) => CandidateRule::SameDateAmount,
            (true, true, false) => CandidateRule::SameDateParty,
            _ => continue,
        };
        keep_strongest(&mut found, position, rule);
    }

    if found.is_empty() {
        // Nothing resembled it — but an absence is only evidence when every
        // key this proposal carries was actually compared.
        if remote_id_unverifiable {
            return shell(
                PresenceStatus::PossiblyPresent(undecided(
                    UndecidedReason::RemoteIdEvidenceUnavailable,
                    Vec::new(),
                )),
                BTreeSet::new(),
            );
        }
        if party.incomplete {
            return shell(
                PresenceStatus::PossiblyPresent(undecided(
                    UndecidedReason::PartyNotDecidable,
                    Vec::new(),
                )),
                BTreeSet::new(),
            );
        }
        return shell(PresenceStatus::Absent, BTreeSet::new());
    }

    let reason = match (
        number_matches.is_empty(),
        type_observed,
        method == NumberingMethod::Manual,
    ) {
        (false, false, _) => UndecidedReason::VoucherTypeNotObserved,
        (false, true, false) => UndecidedReason::NumberNotDecisive,
        _ => UndecidedReason::ResemblesBookVoucher,
    };
    let touched = found.keys().copied().collect::<BTreeSet<_>>();
    let mut candidates = found
        .into_iter()
        .map(|(position, rule)| PresenceCandidate {
            book_key: window.vouchers[position].key().to_string(),
            rule,
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.rule
            .rank()
            .cmp(&right.rule.rank())
            .then_with(|| left.book_key.cmp(&right.book_key))
    });
    shell(
        PresenceStatus::PossiblyPresent(undecided(reason, candidates)),
        touched,
    )
}

/// Turns an identity match into a status. A cancelled or optional voucher
/// occupies the number without being posted, so it is never `Present`.
fn settled(
    proposal: &ProposedVoucher,
    party: &PartyResolution,
    voucher: &BookVoucher,
    basis: PresenceBasis,
) -> PresenceStatus {
    if voucher.posting != PostingState::Posted {
        return PresenceStatus::PossiblyPresent(undecided(
            UndecidedReason::MatchedVoucherNotPosted,
            vec![PresenceCandidate {
                book_key: voucher.key().to_string(),
                rule: match basis {
                    PresenceBasis::RemoteId => CandidateRule::SharedRemoteId,
                    PresenceBasis::ManualVoucherNumber => CandidateRule::SharedVoucherNumber,
                },
            }],
        ));
    }
    PresenceStatus::Present {
        book_key: voucher.key().to_string(),
        basis,
        differences: differences(proposal, party, voucher),
    }
}

/// What an identified voucher disagrees with its source about. One engagement
/// found an invoice posted short by exactly one dropped GST head this way.
fn differences(
    proposal: &ProposedVoucher,
    party: &PartyResolution,
    voucher: &BookVoucher,
) -> Vec<Difference> {
    let mut differences = Vec::new();
    if voucher.date() != proposal.date() {
        differences.push(Difference {
            field: DifferenceField::Date,
            proposed: Some(proposal.date().to_string()),
            observed: Some(voucher.date().to_string()),
        });
    }
    if !voucher.magnitude.numeric_eq(&proposal.magnitude) {
        differences.push(Difference {
            field: DifferenceField::Amount,
            proposed: Some(proposal.magnitude.as_str().to_string()),
            observed: Some(voucher.magnitude.as_str().to_string()),
        });
    }
    // Only a bound party can disagree. An ambiguous one has no single name to
    // disagree with, and asserting a difference from a candidate would be the
    // same guess by another route.
    // The diagnostic compares against the *observed party field*, not against
    // every ledger the voucher touches. Widening to all entry ledgers is right
    // for finding a candidate and wrong for reporting a disagreement: a
    // voucher whose party is one name while an entry names another would
    // otherwise report no difference while serializing the other name as
    // `observed`. A voucher with no party field has nothing to disagree with.
    if let (PartyOutcome::Bound { catalog_name }, Some(observed)) =
        (&party.outcome, voucher.party.as_deref())
    {
        if comparison_key(observed) != comparison_key(catalog_name) {
            differences.push(Difference {
                field: DifferenceField::Party,
                proposed: Some(catalog_name.clone()),
                observed: Some(observed.to_string()),
            });
        }
    }
    differences
}

fn observe(
    window: &BookWindow,
    index: &WindowIndex<'_>,
    proposed_type_keys: &BTreeSet<&str>,
    touched: &BTreeSet<usize>,
) -> BookObservations {
    let mut duplicate_numbers = Vec::new();
    let mut duplicate_number_group_count = 0_usize;
    for ((_, _), positions) in &index.by_type_and_number {
        if positions.len() < 2 {
            continue;
        }
        duplicate_number_group_count += 1;
        if duplicate_numbers.len() >= MAX_DUPLICATE_NUMBER_GROUPS {
            continue;
        }
        let first = &window.vouchers[positions[0]];
        duplicate_numbers.push(DuplicateNumberGroup {
            voucher_type: first.voucher_type.clone(),
            voucher_number: first.voucher_number.clone().unwrap_or_default(),
            book_keys: positions
                .iter()
                .take(MAX_KEYS_PER_DUPLICATE_GROUP)
                .map(|position| window.vouchers[*position].key().to_string())
                .collect(),
            book_voucher_count: positions.len(),
        });
    }

    let unbalanced: Vec<&BookVoucher> = window
        .vouchers
        .iter()
        .filter(|voucher| !voucher.balanced())
        .collect();

    let unmatched_book_vouchers = window
        .vouchers
        .iter()
        .enumerate()
        .filter(|(position, voucher)| {
            proposed_type_keys.contains(voucher.type_key.as_str()) && !touched.contains(position)
        })
        .count();

    BookObservations {
        duplicate_numbers,
        duplicate_number_group_count,
        duplicate_numbers_truncated: duplicate_number_group_count > MAX_DUPLICATE_NUMBER_GROUPS,
        unbalanced_vouchers: unbalanced
            .iter()
            .take(MAX_UNBALANCED_LISTED)
            .map(|voucher| voucher.key().to_string())
            .collect(),
        unbalanced_voucher_count: unbalanced.len(),
        unmatched_book_vouchers,
        window_voucher_count: window.vouchers.len(),
        remote_id_observed: !index.by_remote_id.is_empty(),
    }
}

fn keep_strongest(
    found: &mut BTreeMap<usize, CandidateRule>,
    position: usize,
    rule: CandidateRule,
) {
    found
        .entry(position)
        .and_modify(|held| {
            if rule.rank() < held.rank() {
                *held = rule;
            }
        })
        .or_insert(rule);
}

fn candidates_from(
    window: &BookWindow,
    positions: &[usize],
    rule: CandidateRule,
) -> Vec<PresenceCandidate> {
    positions
        .iter()
        .map(|position| PresenceCandidate {
            book_key: window.vouchers[*position].key().to_string(),
            rule,
        })
        .collect()
}

fn undecided(reason: UndecidedReason, candidates: Vec<PresenceCandidate>) -> Undecided {
    let candidate_count = candidates.len();
    let truncated = candidate_count > MAX_CANDIDATES_PER_PROPOSAL;
    let mut candidates = candidates;
    candidates.truncate(MAX_CANDIDATES_PER_PROPOSAL);
    Undecided {
        reason,
        candidates,
        candidate_count,
        candidates_truncated: truncated,
    }
}

/// One definition of a voucher's magnitude, used by both sides so the two can
/// never compute it differently. It is the sum of the positive entry amounts,
/// which is defined whether or not the voucher balances.
fn magnitude_of(
    entries: &[ObservedEntry<'_>],
) -> Result<(ExactDecimal, bool, BTreeSet<String>), PresenceError> {
    if entries.len() > MAX_ENTRIES_PER_VOUCHER {
        return Err(PresenceError::TooManyEntries);
    }
    let mut total = ExactDecimalAccumulator::default();
    let mut positive = ExactDecimalAccumulator::default();
    let mut ledger_keys = BTreeSet::new();
    for entry in entries {
        let amount = ExactDecimal::parse(entry.amount.to_string())
            .map_err(|_| PresenceError::AmountInvalid)?;
        total.add(amount.as_str());
        if !amount.is_negative() {
            positive.add(amount.as_str());
        }
        ledger_keys.insert(comparison_key(&validated_text(entry.ledger)?));
    }
    let magnitude = ExactDecimal::parse(positive.canonical_string())
        .map_err(|_| PresenceError::AmountInvalid)?;
    Ok((magnitude, total.is_zero(), ledger_keys))
}

fn validated_text(value: &str) -> Result<String, PresenceError> {
    if value.trim().is_empty() {
        return Err(PresenceError::TextBlank);
    }
    if value.chars().count() > MAX_TEXT_CHARS {
        return Err(PresenceError::TextTooLong);
    }
    if value.chars().any(|character| {
        character.is_control() || matches!(character, '\u{2028}' | '\u{2029}' | '\u{feff}')
    }) {
        return Err(PresenceError::TextUnsafe);
    }
    Ok(value.to_string())
}

#[cfg(test)]
#[path = "book_presence_tests.rs"]
mod tests;
