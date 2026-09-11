//! Deterministic binding of source-document entity names to one company's
//! observed masters.
//!
//! See `docs/adr/0016-master-binding-authority.md`. Three rules carry the whole
//! contract: an embedded identifier is matched before any name, nothing binds
//! unless it is unique on both sides, and a near-miss is never resolved — it is
//! reported with its candidates so an operator decides.
//!
//! This module performs no I/O, holds no company identity, and calls no model.
//! A returned binding is a proposal: a caller that intends to act on one
//! re-reads the catalog and revalidates the selection through the admission
//! path that owns identity.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// Most masters one catalog may carry.
pub const MAX_CATALOG_ENTRIES: usize = 20_000;
/// Most entities one binding request may name. This must stay at or above what
/// a consumer's own parser admits: Bridge's source-draft parser accepts 2,000
/// vouchers of 20 entries, and a bound below that turned a valid draft into a
/// silently empty binding result.
pub const MAX_SOURCE_ENTITIES: usize = 40_000;
/// Longest accepted master or source name, in characters. This bounds
/// pathological input; it is not a claim about what Tally accepts, and a
/// caller with a stricter contract of its own enforces that at its own
/// boundary.
pub const MAX_NAME_CHARS: usize = 16_384;
/// Most candidates retained per unbound entity.
pub const MAX_CANDIDATES_PER_ENTITY: usize = 25;
/// Total candidate-name bytes one report may allocate, across all entities.
///
/// A per-entity cap does not bound a report: a draft the source parser admits
/// can carry tens of thousands of entries that each list 25 long names, and the
/// clones exist the moment the report is built. A consumer capping its own copy
/// afterwards bounds only the second copy. This is spent in entity order;
/// entities past it report their true `candidate_count` with no candidates
/// listed and truncation flagged.
pub const MAX_REPORT_CANDIDATE_BYTES: usize = 256 * 1024;
/// Most identifiers one name may carry. Exceeding it is refused, never
/// truncated.
pub const MAX_IDENTIFIERS_PER_NAME: usize = 32;
/// Digits a numeric run needs before it is treated as an identifier. Eight
/// excludes a year, a rate, a house number and a masked last-four; a mobile,
/// an account number and a customer code all clear it.
pub const MIN_NUMERIC_IDENTIFIER_DIGITS: usize = 8;
/// Alphanumeric characters a mixed letter-and-digit token needs before it is
/// treated as a code identifier.
///
/// Eight, raised twice under review. Enumerating the period shapes that must
/// not be identifiers — `FY25`, then `APR2025`, then `2025Q1` — is a losing
/// game, and each miss binds two unrelated ledgers that merely share a period.
/// Requiring real length is the rule that does not depend on having thought of
/// every label: a part number or registration code clears it, and a period
/// label does not. Measured against 485 live ledger names, exactly one yields a
/// code identifier at all, so this costs nothing observed.
pub const MIN_CODE_IDENTIFIER_CHARS: usize = 8;
/// Digits a code identifier needs alongside at least two letters.
pub const MIN_CODE_IDENTIFIER_DIGITS: usize = 3;
/// Shortest comparison key that may take part in a prefix near-miss.
pub const MIN_PREFIX_KEY_CHARS: usize = 3;
/// Shortest token that may take part in a shared-token near-miss.
pub const MIN_TOKEN_CHARS: usize = 3;
/// Masters a single prefix may match before the prefix stops discriminating.
/// Beyond this the match is a name *family*, and an arbitrary slice of it is
/// worse than saying so.
pub const MAX_PREFIX_FAMILY: usize = MAX_CANDIDATES_PER_ENTITY;
/// Share of the catalog above which a token stops discriminating.
pub const COMMON_TOKEN_PERCENT: usize = 10;
/// Catalog size below which no token is treated as common.
pub const COMMON_TOKEN_MIN_CATALOG: usize = 20;

/// The master class a catalog and a report belong to. Both classes have failed
/// in practice and the rules are identical for both; the class is carried so a
/// report cannot be applied against the wrong catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MasterClass {
    Ledger,
    StockItem,
}

/// Binding refuses rather than degrades. Every variant is a fail-closed
/// boundary check on input that was never observed, never usable, or already
/// undecidable before any matching ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MasterBindingError {
    /// Binding against a book that was never read is the failure this whole
    /// contract exists to prevent. It is an error, not an empty report.
    #[error("master catalog was empty")]
    CatalogEmpty,
    /// Two masters carry byte-identical names, so a name cannot identify one.
    /// Names differing only in surrounding whitespace are *not* duplicates —
    /// they are retained verbatim and collide on the comparison key instead,
    /// which surfaces them as an ambiguity rather than failing the read.
    #[error("master catalog carried a duplicate name")]
    CatalogDuplicateName,
    #[error("master catalog exceeded its bound")]
    CatalogTooLarge,
    #[error("source entity list exceeded its bound")]
    TooManySourceEntities,
    #[error("name was blank")]
    NameBlank,
    #[error("name exceeded its bound")]
    NameTooLong,
    #[error("name carried a control character")]
    NameUnsafe,
    /// A caller-supplied identifier hint that yields no identifier would fail
    /// silently, so it fails loudly instead.
    #[error("identifier hint carried no usable identifier")]
    IdentifierHintUnusable,
    /// Keeping only the first few would discard the identifier that pointed at
    /// a different master, turning a conflict into a bind.
    #[error("name carried more identifiers than the bound")]
    TooManyIdentifiers,
    #[error("fallback master was not a current catalog entry")]
    FallbackNotInCatalog,
    /// A catalog of the wrong class, a catalog the report was not produced
    /// from, or an entity from another report.
    #[error("catalog did not match the report it is used with")]
    ClassMismatch,
}

impl MasterBindingError {
    /// A stable code safe to surface to an operator or a tool result.
    pub fn safe_reason_code(&self) -> &'static str {
        match self {
            Self::CatalogEmpty => "master_catalog_empty",
            Self::CatalogDuplicateName => "master_catalog_duplicate_name",
            Self::CatalogTooLarge => "master_catalog_too_large",
            Self::TooManySourceEntities => "master_source_entities_too_many",
            Self::NameBlank => "master_name_blank",
            Self::NameTooLong => "master_name_too_long",
            Self::NameUnsafe => "master_name_unsafe",
            Self::IdentifierHintUnusable => "master_identifier_hint_unusable",
            Self::TooManyIdentifiers => "master_identifiers_too_many",
            Self::FallbackNotInCatalog => "master_fallback_not_in_catalog",
            Self::ClassMismatch => "master_class_mismatch",
        }
    }
}

/// The shape an identifier was recognized by. Both canonicalize away the
/// punctuation an operator happened to type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierKind {
    /// A digit run: a mobile, an account number, a numeric customer code.
    Numeric,
    /// A mixed letter-and-digit token: a part number, a registration code.
    Code,
}

/// One stable identifier embedded in a name, in canonical form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub struct Identifier {
    pub kind: IdentifierKind,
    pub value: String,
}

/// The rule that produced a candidate. There is deliberately no score: a score
/// invites a threshold, and a threshold auto-resolves the case this contract
/// exists to keep in front of a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRule {
    /// Shares an embedded identifier, but the identifier was not decisive.
    SharedIdentifier,
    /// Equal under the comparison key, but the key was not unique.
    NormalizedEqual,
    /// The catalog name extends the source name — the source was truncated.
    CatalogPrefix,
    /// The source name extends the catalog name.
    SourcePrefix,
    /// Shares a token that discriminates within this catalog.
    SharedToken,
}

impl CandidateRule {
    fn rank(self) -> u8 {
        match self {
            Self::SharedIdentifier => 0,
            Self::NormalizedEqual => 1,
            Self::CatalogPrefix => 2,
            Self::SourcePrefix => 3,
            Self::SharedToken => 4,
        }
    }
}

/// A master an operator may choose, with the rule that surfaced it. No
/// candidate is marked best, and the order is rule-then-name, not similarity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Candidate {
    pub catalog_name: String,
    pub rule: CandidateRule,
}

/// Why an entity did not bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnboundReason {
    /// One embedded identifier is carried by more than one master.
    IdentifierConflict,
    /// An identifier and an exact name pointed at different masters.
    IdentifierNameConflict,
    /// More than one master shares the comparison key.
    NameAmbiguous,
    /// Candidates exist but none was decisive. This is the four-near-miss case
    /// that rejected a batch once; a single candidate stays here too.
    NearMiss,
    /// The source name matches a whole family of masters and distinguishes
    /// none of them — a truncated `DN Party 0` against `DN Party 001`…`120`.
    /// Measured live: listing an arbitrary capped slice of such a family put
    /// the right master out of view about a third of the time, so the family
    /// is counted and deliberately not listed.
    NoDiscriminatingCandidate,
    /// No rule produced a candidate. The master is probably missing.
    NoCandidate,
}

impl UnboundReason {
    /// A stable code safe to surface to an operator or a tool result.
    pub fn safe_reason_code(self) -> &'static str {
        match self {
            Self::IdentifierConflict => "master_binding_identifier_conflict",
            Self::IdentifierNameConflict => "master_binding_identifier_name_conflict",
            Self::NameAmbiguous => "master_binding_name_ambiguous",
            Self::NearMiss => "master_binding_near_miss",
            Self::NoDiscriminatingCandidate => "master_binding_no_discriminating_candidate",
            Self::NoCandidate => "master_binding_no_candidate",
        }
    }
}

/// The evidence that decided a bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingBasis {
    /// An embedded identifier unique on both sides. Checked before any name.
    Identifier,
    /// Byte equality with the observed master name.
    ExactName,
    /// Equality under the comparison key, unique in the catalog.
    NormalizedName,
}

/// The masters worth showing, and — in the variant itself — what an absence of
/// them means.
///
/// Replaces a `Vec` plus two flags, where empty was three different facts and a
/// consumer reading `is_empty()` was wrong in two of them. That shape had
/// already been got wrong twice by different lanes; here the compiler makes
/// each case an explicit decision instead.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "listing")]
pub enum Candidates {
    /// Nothing resembles this name. An absence of masters, not of information.
    None,
    /// Every master found, listed.
    ///
    /// A struct variant, not a newtype: under Serde's internally tagged
    /// representation a tag cannot be merged into a sequence, and a newtype
    /// here failed to serialize at runtime — on the most common unresolved
    /// result, while the other three variants succeeded.
    Listed { listed: Vec<Candidate> },
    /// More were found than could be listed — the per-entity cap, or the
    /// report's aggregate byte budget.
    Truncated {
        listed: Vec<Candidate>,
        found: usize,
    },
    /// A family this name reaches and separates none of: counted, and
    /// deliberately not listed, because an arbitrary slice of it put the right
    /// master out of view about a third of the time against live books.
    Withheld { found: usize },
}

impl Candidates {
    /// The masters actually listed. Empty for `None` and `Withheld` alike, so
    /// never decide anything from this alone.
    pub fn listed(&self) -> &[Candidate] {
        match self {
            Self::None | Self::Withheld { .. } => &[],
            Self::Listed { listed } | Self::Truncated { listed, .. } => listed,
        }
    }

    /// Masters found before any truncation or withholding.
    pub fn found(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Listed { listed } => listed.len(),
            Self::Truncated { found, .. } | Self::Withheld { found } => *found,
        }
    }

    /// Whether masters exist that are not in `listed()`. The predicate a
    /// consumer needs before it may report "nothing like this is present":
    /// true here means the absence of a listing is not the absence of a master.
    pub fn is_incomplete(&self) -> bool {
        matches!(self, Self::Truncated { .. } | Self::Withheld { .. })
    }
}

/// What could not be bound, and why. This is the operator's work item, not an
/// error path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Unresolved {
    pub reason: UnboundReason,
    /// Identifiers extracted from the source name and from caller hints,
    /// retained so a fallback posting can be reallocated later without
    /// re-reading the source document.
    ///
    /// **This must travel in a channel that survives a read back — the
    /// narration.** A client-supplied `REMOTEID` is not it: Tally overwrites
    /// the attribute with its own value, so a key written there cannot be
    /// observed afterwards and cannot identify what to reallocate
    /// (`docs/tally/IMPLEMENTATION_GUIDE.md` §3.3a, fourth property, verified).
    /// A parked amount whose identity went into a write-only field is
    /// unreallocatable, and nothing about the write would say so.
    pub unresolved_identity: Vec<Identifier>,
    pub candidates: Candidates,
}

/// Exactly one outcome per source entity.
///
/// **What a `Bound` does not establish**, written here because a computed check
/// gets read for more than it covers, and the caller cannot see the gap from
/// the value alone:
///
/// - **Not that the master still exists.** The catalog is a snapshot. A caller
///   acting on a binding re-reads and revalidates through the admission path
///   that owns identity; nothing here is a lease on the book.
/// - **Not that the name may be written as given.** Only `ExactName` is byte
///   equality. A `NormalizedName` or `Identifier` bind means the payload and
///   the live name *differ*, and Bridge's write gate admits `exact` only — use
///   `catalog_name`, not what was requested.
/// - **Not that this is the right master in business terms.** It establishes
///   that one deterministic rule selected one master uniquely. Whether that
///   party is the one the document meant is a judgement the rules cannot make.
/// - **Not any authority.** A binding is a proposal: it approves nothing,
///   creates nothing, and dispatches nothing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum BindingStatus {
    Bound {
        catalog_name: String,
        basis: BindingBasis,
    },
    /// More than one master is defensible.
    Ambiguous(Unresolved),
    /// No master is defensible.
    Unmatched(Unresolved),
}

/// One source entity and its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct EntityBinding {
    pub position: usize,
    /// The name exactly as the source document gave it.
    pub source_name: String,
    #[serde(flatten)]
    pub status: BindingStatus,
}

impl EntityBinding {
    pub fn bound_name(&self) -> Option<&str> {
        match &self.status {
            BindingStatus::Bound { catalog_name, .. } => Some(catalog_name.as_str()),
            _ => None,
        }
    }

    pub fn unresolved(&self) -> Option<&Unresolved> {
        match &self.status {
            BindingStatus::Ambiguous(unresolved) | BindingStatus::Unmatched(unresolved) => {
                Some(unresolved)
            }
            BindingStatus::Bound { .. } => None,
        }
    }
}

/// Control totals for one run. `requested == bound + unbound` and
/// `unbound == ambiguous + unmatched` always hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct BindingTotals {
    pub requested: usize,
    pub bound: usize,
    pub unbound: usize,
    pub ambiguous: usize,
    pub unmatched: usize,
}

/// The result of one binding run.
///
/// Serializes but does **not** deserialize, and the asymmetry is the point.
/// `assign_fallback` proves provenance by comparing the catalog fingerprint it
/// recorded, and `catalog.fingerprint()` is public — so a derived `Deserialize`
/// would let any caller restore an invented `Ambiguous` entity carrying the
/// right fingerprint and draw a fallback for an entity the binder never emitted.
/// `bind` is the only way to obtain one. A report that must cross a process
/// boundary needs a restore that re-derives these fields, not a derive that
/// trusts them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BindingReport {
    class: MasterClass,
    catalog: CatalogFingerprint,
    entities: Vec<EntityBinding>,
}

impl BindingReport {
    pub fn class(&self) -> MasterClass {
        self.class
    }

    /// The catalog this report was produced from.
    pub fn catalog(&self) -> CatalogFingerprint {
        self.catalog
    }

    pub fn entities(&self) -> &[EntityBinding] {
        &self.entities
    }

    /// Everything that bound.
    pub fn bound(&self) -> impl Iterator<Item = &EntityBinding> {
        self.entities
            .iter()
            .filter(|entity| matches!(entity.status, BindingStatus::Bound { .. }))
    }

    /// Everything that did not — the product of this module.
    pub fn unbound(&self) -> impl Iterator<Item = &EntityBinding> {
        self.entities
            .iter()
            .filter(|entity| !matches!(entity.status, BindingStatus::Bound { .. }))
    }

    /// Parks one of *this report's* unbound entities against a fallback master
    /// drawn from a catalog of the same class.
    ///
    /// Taking an index rather than an `EntityBinding` is the point: an entity
    /// from another report — a stock-item binding, say — cannot be handed to a
    /// ledger catalog, because it cannot be named here at all. The class is
    /// then checked as well, so a same-shaped catalog of the wrong class is
    /// refused rather than silently accepted, and the result carries the class
    /// forward for anything downstream that needs to prove it.
    pub fn assign_fallback(
        &self,
        entity_index: usize,
        catalog: &MasterCatalog,
        fallback_name: &str,
    ) -> Result<FallbackBinding, MasterBindingError> {
        // Class alone is not provenance: two ledger catalogs are both
        // `Ledger`, and a fallback drawn from the one the report never saw
        // would name a master that was never a candidate for this entity.
        if catalog.class != self.class || catalog.fingerprint != self.catalog {
            return Err(MasterBindingError::ClassMismatch);
        }
        let entity = self
            .entities
            .get(entity_index)
            .ok_or(MasterBindingError::ClassMismatch)?;
        let unresolved = entity
            .unresolved()
            .ok_or(MasterBindingError::FallbackNotInCatalog)?;
        let fallback = catalog
            .exact(fallback_name)
            .ok_or(MasterBindingError::FallbackNotInCatalog)?;
        Ok(FallbackBinding {
            class: self.class,
            position: entity.position,
            source_name: entity.source_name.clone(),
            fallback_name: fallback.to_string(),
            retained: unresolved.unresolved_identity.clone(),
            reason: unresolved.reason,
        })
    }

    pub fn totals(&self) -> BindingTotals {
        let mut totals = BindingTotals {
            requested: self.entities.len(),
            bound: 0,
            unbound: 0,
            ambiguous: 0,
            unmatched: 0,
        };
        for entity in &self.entities {
            match entity.status {
                BindingStatus::Bound { .. } => totals.bound += 1,
                BindingStatus::Ambiguous(_) => {
                    totals.unbound += 1;
                    totals.ambiguous += 1;
                }
                BindingStatus::Unmatched(_) => {
                    totals.unbound += 1;
                    totals.unmatched += 1;
                }
            }
        }
        totals
    }
}

/// An ambiguous entity parked against a fallback master, with its unresolved
/// identity retained for later reallocation.
///
/// Constructed only from an entity that did not bind, so rebinding something
/// that already matched is not a representable state.
///
/// **Reallocate with a Journal moving the amount off the fallback ledger.
/// Never with `Alter`, and never with `Cancel`.** `TALLY_PROTOCOL_REFERENCE.md`
/// §9.7 measured voucher `Alter` returning `CREATED=1, ALTERED=0` and creating
/// a **duplicate with the target untouched** — four keys tested, all four
/// duplicating — and §9.6 the same for `Cancel`. The counters report success
/// either way, so the obvious correction produces exactly the double-posting a
/// parked entry exists to avoid, and says it worked.
///
/// Re-import under the same client `REMOTEID` (§3.3a) is a real correction
/// path, but reaches only vouchers Bridge itself wrote; a hand-keyed voucher
/// has no client key. This is why the retained identity travels in the
/// narration: the Journal that reallocates it is written by a human or a later
/// batch, and the narration is what either can still read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FallbackBinding {
    class: MasterClass,
    position: usize,
    source_name: String,
    fallback_name: String,
    retained: Vec<Identifier>,
    reason: UnboundReason,
}

impl FallbackBinding {
    /// The class of the catalog this fallback was drawn from.
    pub fn class(&self) -> MasterClass {
        self.class
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    pub fn fallback_name(&self) -> &str {
        &self.fallback_name
    }

    pub fn reason(&self) -> UnboundReason {
        self.reason
    }

    pub fn retained(&self) -> &[Identifier] {
        &self.retained
    }

    /// The identity to carry into a narration so the parked amount can be
    /// reallocated without re-reading the source. Empty when the source name
    /// carried no identifier at all — which is itself worth seeing.
    pub fn retained_tag(&self) -> String {
        self.retained
            .iter()
            .map(|identifier| {
                let kind = match identifier.kind {
                    IdentifierKind::Numeric => "numeric",
                    IdentifierKind::Code => "code",
                };
                format!("{kind}:{}", identifier.value)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// One entity named by a source document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntity {
    position: usize,
    name: String,
    /// The wide fold. Suggests; never resolves.
    key: String,
    /// The narrow fold. Resolves.
    binding_key: String,
    identifiers: Vec<Identifier>,
}

impl SourceEntity {
    /// Parses one source name at the boundary, extracting identifiers from it.
    pub fn new(position: usize, name: &str) -> Result<Self, MasterBindingError> {
        Self::with_identifier_hints(position, name, std::iter::empty::<&str>())
    }

    /// Parses one source name together with identifiers the document carries
    /// outside the name — a statement's payment reference, a report's mobile
    /// column. A hint that yields no identifier is refused rather than ignored.
    pub fn with_identifier_hints<'a>(
        position: usize,
        name: &str,
        hints: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, MasterBindingError> {
        let name = validated_name(name)?;
        let mut identifiers = extract_identifiers(&name)?;
        for (index, hint) in hints.into_iter().enumerate() {
            // Bounded here rather than after the loop: the iterator is
            // caller-supplied and may be unbounded, and each turn scans a
            // string and allocates. A million repeated hints deduplicate to one
            // identifier, so the check below never fired while the work to
            // reach it was already done. Every hint yields at least one
            // identifier or is refused outright, so more hints than the
            // identifier bound cannot produce a usable entity however they fold
            // together — the eager bound rejects nothing the late one admitted.
            if index >= MAX_IDENTIFIERS_PER_NAME {
                return Err(MasterBindingError::TooManyIdentifiers);
            }
            // A hint is caller-supplied document text like any other name, and
            // must clear the same bound before anything scans or copies it.
            validate_name_bounds(hint)?;
            let extracted = extract_identifiers(hint)?;
            if extracted.is_empty() {
                return Err(MasterBindingError::IdentifierHintUnusable);
            }
            identifiers.extend(extracted);
        }
        identifiers.sort();
        identifiers.dedup();
        if identifiers.len() > MAX_IDENTIFIERS_PER_NAME {
            return Err(MasterBindingError::TooManyIdentifiers);
        }
        Ok(Self {
            position,
            key: master_identity_key(&name),
            binding_key: verified_fold(&name),
            name,
            identifiers,
        })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn identifiers(&self) -> &[Identifier] {
        &self.identifiers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogEntry {
    name: String,
    key: String,
    identifiers: Vec<Identifier>,
    tokens: BTreeSet<String>,
}

/// One company's observed masters of one class, indexed for binding.
///
/// Valid by construction: `bind` cannot fail because everything that could fail
/// was decided here.
/// Which catalog a report was produced from.
///
/// Not a security property and not a Tally identity — it distinguishes two
/// catalogs of the same class held in one process, which is the state that let
/// a fallback be drawn from a book the report never saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogFingerprint(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterCatalog {
    class: MasterClass,
    fingerprint: CatalogFingerprint,
    entries: Vec<CatalogEntry>,
    by_name: BTreeMap<String, usize>,
    by_key: BTreeMap<String, Vec<usize>>,
    by_binding_key: BTreeMap<String, Vec<usize>>,
    by_identifier: BTreeMap<Identifier, Vec<usize>>,
    by_token: BTreeMap<String, Vec<usize>>,
    common_tokens: BTreeSet<String>,
}

impl MasterCatalog {
    /// Parses the observed master names of one class.
    ///
    /// Names arrive in whatever order the book returned them; the index is
    /// ordered, so a report does not depend on that order.
    pub fn new<I, S>(class: MasterClass, names: I) -> Result<Self, MasterBindingError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut entries = Vec::new();
        let mut by_name = BTreeMap::new();
        for name in names {
            if entries.len() >= MAX_CATALOG_ENTRIES {
                return Err(MasterBindingError::CatalogTooLarge);
            }
            let name = validated_name(name.as_ref())?;
            if by_name.contains_key(&name) {
                return Err(MasterBindingError::CatalogDuplicateName);
            }
            by_name.insert(name.clone(), entries.len());
            let key = master_identity_key(&name);
            entries.push(CatalogEntry {
                identifiers: extract_identifiers(&name)?,
                tokens: tokens_of(&key),
                key,
                name,
            });
        }
        if entries.is_empty() {
            return Err(MasterBindingError::CatalogEmpty);
        }

        let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_binding_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_identifier: BTreeMap<Identifier, Vec<usize>> = BTreeMap::new();
        let mut by_token: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            by_key.entry(entry.key.clone()).or_default().push(index);
            for binding_key in master_binding_keys(&entry.name) {
                by_binding_key.entry(binding_key).or_default().push(index);
            }
            for identifier in &entry.identifiers {
                by_identifier
                    .entry(identifier.clone())
                    .or_default()
                    .push(index);
            }
            for token in &entry.tokens {
                by_token.entry(token.clone()).or_default().push(index);
            }
        }

        // A token carried by a large share of the catalog says nothing about
        // which master is meant. The threshold is measured from the catalog
        // rather than a built-in word list, so it carries no language or
        // domain assumption.
        let common_tokens = if entries.len() >= COMMON_TOKEN_MIN_CATALOG {
            let limit = entries.len() * COMMON_TOKEN_PERCENT / 100;
            by_token
                .iter()
                .filter(|(_, holders)| holders.len() > limit)
                .map(|(token, _)| token.clone())
                .collect()
        } else {
            BTreeSet::new()
        };

        // Order-independent, so the same masters read twice fingerprint alike
        // however the book returned them.
        let fingerprint =
            CatalogFingerprint(entries.iter().fold(class as u64 + 1, |accumulated, entry| {
                accumulated
                    ^ entry
                        .name
                        .bytes()
                        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                            (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
                        })
            }));
        Ok(Self {
            class,
            fingerprint,
            entries,
            by_name,
            by_key,
            by_binding_key,
            by_identifier,
            by_token,
            common_tokens,
        })
    }

    pub fn class(&self) -> MasterClass {
        self.class
    }

    /// Which catalog this is, for a caller that must prove a later value came
    /// from the same one.
    pub fn fingerprint(&self) -> CatalogFingerprint {
        self.fingerprint
    }

    /// Masters in this catalog. Never zero: an empty catalog is refused at
    /// construction, so there is no emptiness for a caller to test.
    pub fn master_count(&self) -> usize {
        self.entries.len()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.name.as_str())
    }

    /// The observed name, when it is byte-identical to a current entry.
    pub fn exact(&self, name: &str) -> Option<&str> {
        self.by_name
            .get(name)
            .map(|index| self.entries[*index].name.as_str())
    }
}

/// Binds every source entity against the catalog.
///
/// The entity bound is enforced here rather than left to callers: a source
/// document is untrusted input, and an unbounded variant would be a rule
/// someone has to remember. Everything else was already decided by the two
/// constructors, so this is the only way binding can fail.
pub fn bind(
    catalog: &MasterCatalog,
    entities: &[SourceEntity],
) -> Result<BindingReport, MasterBindingError> {
    if entities.len() > MAX_SOURCE_ENTITIES {
        return Err(MasterBindingError::TooManySourceEntities);
    }
    let mut budget = MAX_REPORT_CANDIDATE_BYTES;
    Ok(BindingReport {
        class: catalog.class,
        catalog: catalog.fingerprint,
        entities: entities
            .iter()
            .map(|entity| bind_one(catalog, entity, &mut budget))
            .collect(),
    })
}

fn bind_one(catalog: &MasterCatalog, entity: &SourceEntity, budget: &mut usize) -> EntityBinding {
    let exact = catalog.by_name.get(&entity.name).copied();

    // Rule one: the identifier is the key, the name is a hint. A name
    // comparison on a pair that carries a decisive identifier is not merely
    // weaker evidence, it is actively misleading.
    // Which masters *each* identifier reached, not merely which masters were
    // reached. Flattening the two loses the only fact that separates a number
    // shared by several masters from several numbers pointing at different
    // ones, and those need opposite answers.
    let mut per_identifier: Vec<BTreeSet<usize>> = Vec::new();
    let mut identifier_matches = BTreeSet::new();
    let mut identifier_conflict = false;
    for identifier in &entity.identifiers {
        if let Some(holders) = catalog.by_identifier.get(identifier) {
            let reached = holders.iter().copied().collect::<BTreeSet<_>>();
            if reached.len() > 1 {
                identifier_conflict = true;
            }
            identifier_matches.extend(reached.iter().copied());
            per_identifier.push(reached);
        }
    }

    // An identifier shared by two masters, and an entity whose identifiers
    // reach two masters, are the same refusal: the operator has a naming
    // collision to see, and neither case licenses a choice.
    // Byte equality with an observed master name is the strongest evidence
    // there is, and it names exactly one master. An identifier that happens to
    // be ambiguous does not undermine it: refusing here would make a ledger
    // whose embedded number is shared with another permanently unimportable —
    // the same dead end that reporting `Identifier` for an exact name created.
    // Only a *decisive* identifier pointing elsewhere outranks a byte-exact
    // name, and that stays a reported conflict rather than a silent choice.
    //
    // Found by seeding two live ledgers that share an embedded number. No
    // fabricated fixture had produced the combination.
    // A byte-exact name survives an identifier that is merely *shared*: that
    // one identifier reached the master the name spells along with its
    // siblings, and the name is what separates them. It does not survive an
    // identifier that reached somewhere else entirely — that is disagreement,
    // and preferring the name silently discards it.
    //
    // The test is per identifier, not over their union. Asking whether the
    // union contains the exact master answers the shared case correctly and the
    // mixed case wrongly: `ACME 11111111` with a hint reaching `BETA 22222222`
    // has the exact master in the union while one identifier plainly disagrees.
    let identifier_points_elsewhere = exact.is_some_and(|index| {
        per_identifier
            .iter()
            .any(|reached| !reached.contains(&index))
    });
    let status = if identifier_points_elsewhere {
        unresolved_status(
            catalog,
            entity,
            UnboundReason::IdentifierNameConflict,
            exact,
            &identifier_matches,
            budget,
        )
    } else if let Some(index) = exact {
        BindingStatus::Bound {
            catalog_name: catalog.entries[index].name.clone(),
            basis: BindingBasis::ExactName,
        }
    } else if identifier_conflict || identifier_matches.len() > 1 {
        // An identifier shared by two masters, and an entity whose identifiers
        // reach two masters, are the same refusal: the operator has a naming
        // collision to see, and neither case licenses a choice.
        unresolved_status(
            catalog,
            entity,
            UnboundReason::IdentifierConflict,
            exact,
            &identifier_matches,
            budget,
        )
    } else if let Some(matched) = identifier_matches.iter().copied().next() {
        // A decisive identifier, with no byte-exact name to outrank it. This is
        // the rule that decided the case fuzzy matching got wrong.
        BindingStatus::Bound {
            catalog_name: catalog.entries[matched].name.clone(),
            basis: BindingBasis::Identifier,
        }
    } else {
        // The narrow index, not the wide one: only a transformation Tally was
        // measured performing may settle which master was meant. Everything the
        // wide fold reaches and this does not falls through to `collect_candidates`
        // below, where it is offered as `NormalizedEqual` for a human to confirm.
        match catalog
            .by_binding_key
            .get(&entity.binding_key)
            .map(Vec::as_slice)
        {
            Some([index]) => BindingStatus::Bound {
                catalog_name: catalog.entries[*index].name.clone(),
                basis: BindingBasis::NormalizedName,
            },
            Some(_) => unresolved_status(
                catalog,
                entity,
                UnboundReason::NameAmbiguous,
                exact,
                &identifier_matches,
                budget,
            ),
            None => {
                let (candidates, masters_found) =
                    collect_candidates(catalog, entity, &identifier_matches);
                let reason = if !candidates.is_empty() {
                    UnboundReason::NearMiss
                } else if masters_found > MAX_PREFIX_FAMILY {
                    UnboundReason::NoDiscriminatingCandidate
                } else {
                    UnboundReason::NoCandidate
                };
                unresolved_from(catalog, entity, reason, candidates, masters_found, budget)
            }
        }
    };

    EntityBinding {
        position: entity.position,
        source_name: entity.name.clone(),
        status,
    }
}

fn unresolved_status(
    catalog: &MasterCatalog,
    entity: &SourceEntity,
    reason: UnboundReason,
    exact: Option<usize>,
    identifier_matches: &BTreeSet<usize>,
    budget: &mut usize,
) -> BindingStatus {
    let (mut candidates, masters_found) = collect_candidates(catalog, entity, identifier_matches);
    if let Some(index) = exact {
        if !candidates.iter().any(|(candidate, _)| *candidate == index) {
            candidates.push((index, CandidateRule::NormalizedEqual));
        }
    }
    unresolved_from(catalog, entity, reason, candidates, masters_found, budget)
}

fn unresolved_from(
    catalog: &MasterCatalog,
    entity: &SourceEntity,
    reason: UnboundReason,
    candidates: Vec<(usize, CandidateRule)>,
    masters_found: usize,
    budget: &mut usize,
) -> BindingStatus {
    let mut ordered = candidates;
    ordered.sort_by(|left, right| {
        left.1.rank().cmp(&right.1.rank()).then_with(|| {
            catalog.entries[left.0]
                .name
                .cmp(&catalog.entries[right.0].name)
        })
    });
    // The variant is derived here, in one place, from the same facts that chose
    // the reason — so "empty" can never mean something the variant does not say.
    let candidates = if ordered.is_empty() {
        if masters_found > 0 {
            Candidates::Withheld {
                found: masters_found,
            }
        } else {
            Candidates::None
        }
    } else {
        let capped = ordered.len().min(MAX_CANDIDATES_PER_ENTITY);
        let listed = ordered
            .into_iter()
            .take(MAX_CANDIDATES_PER_ENTITY)
            .map_while(|(index, rule)| {
                let catalog_name = &catalog.entries[index].name;
                *budget = budget.checked_sub(catalog_name.len())?;
                Some(Candidate {
                    catalog_name: catalog_name.clone(),
                    rule,
                })
            })
            .collect::<Vec<_>>();
        let found = masters_found.max(capped);
        if listed.len() < found {
            Candidates::Truncated { listed, found }
        } else {
            Candidates::Listed { listed }
        }
    };
    let unresolved = Unresolved {
        reason,
        unresolved_identity: entity.identifiers.clone(),
        candidates,
    };
    if matches!(reason, UnboundReason::NoCandidate) {
        BindingStatus::Unmatched(unresolved)
    } else {
        BindingStatus::Ambiguous(unresolved)
    }
}

/// Produces every defensible master, each labelled with the rule that surfaced
/// it. The strongest rule wins where several apply. Nothing here ranks by
/// similarity, and nothing here chooses.
fn collect_candidates(
    catalog: &MasterCatalog,
    entity: &SourceEntity,
    identifier_matches: &BTreeSet<usize>,
) -> (Vec<(usize, CandidateRule)>, usize) {
    // Masters this name reaches by prefix. The key index is ordered, so this is
    // a range walk rather than a scan of the catalog per entity.
    let extending = if entity.key.chars().count() >= MIN_PREFIX_KEY_CHARS {
        catalog
            .by_key
            .range(entity.key.clone()..)
            .take_while(|(key, _)| key.starts_with(&entity.key))
            .filter(|(key, _)| *key != &entity.key)
            .flat_map(|(_, holders)| holders.iter().copied())
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    // Beyond the bound they are a family this name does not separate, and an
    // arbitrary capped slice of one omitted the right master about a third of
    // the time against live books. Counted, and withheld rather than listed.
    let withheld = if extending.len() > MAX_PREFIX_FAMILY {
        extending.clone()
    } else {
        BTreeSet::new()
    };

    let mut best: BTreeMap<usize, CandidateRule> = BTreeMap::new();
    let mut offer = |index: usize, rule: CandidateRule| {
        best.entry(index)
            .and_modify(|current| {
                if rule.rank() < current.rank() {
                    *current = rule;
                }
            })
            .or_insert(rule);
    };

    // A decisive rule reaches a master on its own evidence, so it still applies
    // to a member of a withheld family: the identifier, or the whole key, is
    // exactly what separates that one from its siblings.
    for index in identifier_matches {
        offer(*index, CandidateRule::SharedIdentifier);
    }
    for index in catalog.by_key.get(&entity.key).into_iter().flatten() {
        offer(*index, CandidateRule::NormalizedEqual);
    }
    if withheld.is_empty() {
        for index in &extending {
            offer(*index, CandidateRule::CatalogPrefix);
        }
    }

    // Weaker rules must not reinstate what the prefix pass withheld. A token
    // shared across a family *is* the family, and re-listing 25 of them is the
    // arbitrary slice the withholding exists to prevent — reachable whenever
    // the family stays under the common-token threshold, as 30 rows in a
    // 330-master catalog do.
    if !entity.key.is_empty() {
        // One pass over character boundaries; recomputing a prefix length per
        // split made this quadratic in a field the source parser admits at 4 KiB.
        for (characters, (split, _)) in entity.key.char_indices().enumerate() {
            if characters < MIN_PREFIX_KEY_CHARS {
                continue;
            }
            for index in catalog
                .by_key
                .get(&entity.key[..split])
                .into_iter()
                .flatten()
                .filter(|index| !withheld.contains(index))
            {
                offer(*index, CandidateRule::SourcePrefix);
            }
        }
    }
    for token in tokens_of(&entity.key) {
        if catalog.common_tokens.contains(&token) {
            continue;
        }
        for index in catalog
            .by_token
            .get(&token)
            .into_iter()
            .flatten()
            .filter(|index| !withheld.contains(index))
        {
            offer(*index, CandidateRule::SharedToken);
        }
    }

    // The total is the union: a withheld family and the candidates still worth
    // listing are not necessarily the same masters, so the larger of the two
    // counts would under-report what the name reaches.
    let found = best
        .keys()
        .copied()
        .chain(withheld)
        .collect::<BTreeSet<_>>()
        .len();
    // Indices, not names — cloning every match before the cap and the budget
    // discarded most of the work, once per entry of an admitted draft.
    (best.into_iter().collect(), found)
}

/// A name is retained **verbatim**, on both sides.
///
/// An observed master name is written back to Tally byte for byte by a caller
/// that acts on a binding, so trimming it would report a spelling the book does
/// not contain. A requested source name is what byte equality is judged
/// against, so trimming it would let `Bank ` claim an exact match on `Bank`
/// while the import file still carries the trailing space. The comparison key
/// collapses surrounding whitespace anyway, so the two still meet as a
/// normalized match — which is a bind the write gate does not admit, and that
/// is the correct, loud outcome.
fn validated_name(value: &str) -> Result<String, MasterBindingError> {
    validate_name_bounds(value)?;
    Ok(value.to_string())
}

fn validate_name_bounds(value: &str) -> Result<(), MasterBindingError> {
    if value.trim().is_empty() {
        return Err(MasterBindingError::NameBlank);
    }
    if value.chars().any(char::is_control) {
        return Err(MasterBindingError::NameUnsafe);
    }
    if value.chars().count() > MAX_NAME_CHARS {
        return Err(MasterBindingError::NameTooLong);
    }
    Ok(())
}

/// Folds the punctuation an operator happened to type: NFC-equivalent dash and
/// quote variants become ASCII, case is lowered, whitespace runs collapse.
/// Nothing else is folded — no stemming, no transliteration, no vowel removal.
///
/// **This is a contract, not an implementation detail.** Anything in this crate
/// that decides whether two operator-typed strings are "the same" — master
/// names here, and voucher numbers or voucher-type names elsewhere — must fold
/// through this one function. A second, subtly different normaliser is exactly
/// the divergence ADR 0016 exists to end, and it would diverge silently:
/// the two agree on every name anyone tests by hand and disagree on the
/// punctuation nobody thinks to try.
///
/// The corollary is that changing what this folds changes every consumer's
/// notion of sameness at once. Widen it only with the same care as a wire
/// format, and never to make one caller's case pass.
pub(crate) fn comparison_key(value: &str) -> String {
    value
        .nfc()
        .flat_map(|character| match character {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => vec!['-'],
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => vec!['\''],
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => vec!['"'],
            other => other.to_lowercase().collect(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The **wide** fold: which masters are worth showing a human.
///
/// This is deliberately looser than anything measured, and it may never decide
/// a binding. `verified_fold` does that. The separation is the whole design:
/// §9.4b verified three transformations and marks the rest UNVERIFIED, and its
/// own remedy is that a looser fold may *suggest* while only the measured ones
/// resolve. So the reverse hyphen direction, collapsed whitespace runs, leading
/// whitespace and the Unicode dash variants all live here, where the worst they
/// can do is put the right master in front of an operator.
///
/// An earlier version of this module let this fold bind. It read naturally and
/// was wrong: `X - Y` is a common ledger convention — six of seventeen
/// hyphenated names in the observed books take that shape — and reaching it
/// from `X Y` needs the measured hyphen step **and** a whitespace run collapsed,
/// which nothing measured. Answering from here was a Bridge guess wearing
/// Tally's authority, and `TEST_CORPUS.md` §9 caught it binding that way
/// against a live instance.
///
/// It is a **separate** function rather than a widening of `comparison_key`
/// precisely because that one is shared: voucher numbers and voucher-type names
/// fold through it too, and §9.4b says nothing about those. One fold per notion
/// of sameness, each named for the question it answers — and here there are two
/// notions, because "could this be the master?" and "is this the master?" are
/// different questions with different evidence behind them.
fn master_identity_key(value: &str) -> String {
    comparison_key(value)
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The fold that may **resolve** a name to a master, held to exactly what
/// `TALLY_PROTOCOL_REFERENCE.md` §9.4b measured Tally doing.
///
/// Three transformations were verified: ASCII case folding, one trailing space
/// ignored, and a **space supplied where the master carries a hyphen**. That
/// last one is directional — `BRIDGE PROBE LEDGER A` was sent against a live
/// `BRIDGE-PROBE-LEDGER-A`, and the reverse was never sent — so it cannot be a
/// symmetric replacement in a shared key. It lives in `master_binding_keys`,
/// on the master side only, which is the side the evidence is about.
///
/// **Canonical equivalence is not folded here, and that one is measured rather
/// than merely unverified.** A voucher naming a UI-created `Cafe\u{301}...`
/// ledger in its canonically equivalent NFD spelling was rejected —
/// `EXCEPTIONS=1`, `LINEERROR`, ledger does not exist — while the NFC spelling
/// created it. Tally stores a master name as the bytes that made it and matches
/// on exact codepoints, so NFC and NFD spellings are *different masters*.
/// Folding them together here would resolve a source name onto a master Tally
/// itself keeps apart. It reads like decoding rather than folding, which is
/// exactly why it nearly stayed.
///
/// Every other unverified step — the reverse hyphen direction, collapsed
/// whitespace runs, leading whitespace, Unicode dash variants, non-ASCII case —
/// is deliberately absent too. None is lost: `master_identity_key` carries them
/// all, and everything it reaches is offered as a candidate.
fn verified_fold(value: &str) -> String {
    // One trailing space, because one is what was sent.
    let value = value.strip_suffix(' ').unwrap_or(value);
    value.to_ascii_lowercase()
}

/// The keys a **master** name answers to.
///
/// Its own, and — because a source space was measured matching a master hyphen
/// — the same name with its hyphens read as spaces. Offering the second from
/// the master side is what keeps the measured direction measured: a source
/// hyphen finds no master space, while a source space finds a master hyphen.
///
/// Two masters that answer to one key are an ambiguity and are refused there,
/// which is the same answer Tally's own behaviour implies: it would match that
/// source name to both.
fn master_binding_keys(value: &str) -> BTreeSet<String> {
    let base = verified_fold(value);
    let hyphens_as_spaces = base.replace('-', " ");
    BTreeSet::from([base, hyphens_as_spaces])
}

/// Splits a comparison key into words.
///
/// The separator set is defined **positively** — whitespace, and ASCII
/// punctuation — rather than as "not alphanumeric". The difference is not
/// pedantic: `char::is_alphanumeric` is **false for a Devanagari virama**, the
/// halant that joins consonants, and false for a nukta. Splitting on it tore
/// Indic ledger names apart at the joins, so `राय एण्ड सन्स` yielded one usable
/// token instead of three and `ट्रेडर्स` was cut to a fragment. Those names are
/// in the books this binder reads.
///
/// So: an ASCII class may decide a question about ASCII characters, and
/// everything outside ASCII — letters, digits, marks, joiners alike — is word
/// content. That keeps the rule correct for scripts nobody here has tested,
/// which is the property worth having.
fn tokens_of(key: &str) -> BTreeSet<String> {
    key.split(|character: char| {
        character.is_whitespace() || (character.is_ascii() && !character.is_ascii_alphanumeric())
    })
    .filter(|token| token.chars().count() >= MIN_TOKEN_CHARS)
    .map(str::to_string)
    .collect()
}

/// Extracts every identifier a name carries, in canonical form.
///
/// A numeric run may hold `-` and `/` internally, so a punctuated account
/// number and a plain one agree; it may not hold spaces, so separated digit
/// groups fail closed to a near-miss rather than fusing into a false
/// identifier. Digits that sit inside a mixed letter-and-digit token belong to
/// that token's code and are never also emitted on their own — otherwise
/// `Part AB12345678` would collide with an unrelated `Bank 12345678`.
///
/// Refuses rather than truncates when a name carries more identifiers than the
/// bound: silently keeping the first few can turn a conflict into a bind by
/// discarding the identifier that pointed elsewhere.
fn extract_identifiers(value: &str) -> Result<Vec<Identifier>, MasterBindingError> {
    let mut identifiers = BTreeSet::new();
    // A mask and the digits it hides are often written apart — `**** 12345678`
    // is the same statement as `********12345678`, and reading tokens
    // independently lost the relationship between them.
    let mut previous_was_mask = false;
    for token in value.split(char::is_whitespace) {
        if token.is_empty() {
            continue;
        }
        let masked_here = is_mask_punctuated(token) || is_mask_alphabetic(token);
        let masked = masked_here || previous_was_mask;
        // A token carrying no alphanumeric content is a delimiter, not a value,
        // and a delimiter between a mask and its suffix does not unmask it:
        // `XXXX - 12345678` says exactly what `XXXX 12345678` says. Clearing
        // the state here let a `-` or a `/` walk the suffix out as a whole
        // account number.
        if masked_here {
            previous_was_mask = true;
        } else if token.chars().any(char::is_alphanumeric) {
            previous_was_mask = false;
        }
        let canonical = token
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .map(|character| character.to_ascii_uppercase())
            .collect::<String>();
        let digits = canonical.chars().filter(char::is_ascii_digit).count();
        let letters = canonical.chars().filter(char::is_ascii_alphabetic).count();
        // Canonicalization keeps ASCII, and so does the digit-run split below.
        // Everything else in a token is silently discarded, and what survives
        // is a code or a number the name never contained: a Devanagari party
        // name fused to `AB12345678` yielded `AB12345678`, and `12345678`
        // followed by Devanagari digits yielded `12345678` — each reaching an
        // unrelated master that the same shape spelled in ASCII never would.
        //
        // Guarding "non-ASCII letters" was the first attempt and was too
        // narrow: `char::is_alphabetic` is false for a Devanagari digit, so the
        // numerals walked straight through it. The admitted set is positive
        // instead — ASCII, plus the dash variants this module already treats as
        // separators — because the question is not which scripts exist but
        // which characters canonicalization is entitled to drop.
        let foreign_content = token
            .chars()
            .any(|character| !character.is_ascii() && !DASH_VARIANTS.contains(&character));
        if canonical.len() >= MIN_CODE_IDENTIFIER_CHARS
            && digits >= MIN_CODE_IDENTIFIER_DIGITS
            && letters >= 2
            && !foreign_content
            && !masked
            && !is_period(token)
            && !is_masked(&canonical)
        {
            identifiers.insert(Identifier {
                kind: IdentifierKind::Code,
                value: canonical,
            });
        }
        // Digits sitting beside letters belong to that token, whether or not it
        // qualified as a code. Emitting them separately let `Part A12345678`
        // reach an unrelated `Bank 12345678` through the one-letter gap that
        // the code test rejects — a token either identifies by its whole shape
        // or not at all.
        //
        // The test is **Unicode alphabetic**, not ASCII. The books observed
        // here carry Devanagari, Tamil and Bengali ledger names, and an
        // ASCII-only guard read `पार्टी12345678` as digits standing alone,
        // binding a party to an unrelated `Bank 12345678`.
        if token.chars().any(char::is_alphabetic) || masked || foreign_content {
            continue;
        }
        for run in token.split(|character: char| {
            !(character.is_ascii_digit() || character == '-' || character == '/')
        }) {
            let digits = run.chars().filter(char::is_ascii_digit).collect::<String>();
            if digits.len() >= MIN_NUMERIC_IDENTIFIER_DIGITS
                && !is_plausible_date(&digits)
                && !is_period(run)
            {
                identifiers.insert(Identifier {
                    kind: IdentifierKind::Numeric,
                    value: digits,
                });
            }
        }
    }
    if identifiers.len() > MAX_IDENTIFIERS_PER_NAME {
        return Err(MasterBindingError::TooManyIdentifiers);
    }
    Ok(identifiers.into_iter().collect())
}

/// A value written with mask punctuation is partial by construction, so the
/// digits it does expose are a suffix rather than the number.
///
/// `********12345678` split cleanly on the asterisks and yielded its visible
/// eight digits as though they were the whole account. `is_masked` guards the
/// code branch only, because a mask spelled with letters is caught by the
/// letter test; a mask spelled with punctuation reaches the numeric branch,
/// where every non-digit is an ordinary delimiter.
/// A mask spelled with letters hides its digits exactly as one spelled with
/// punctuation does. `is_masked` already reads the shape inside a single token,
/// so `XXXX1234` never became a code; written a space apart as `XXXX 12345678`
/// the same statement lost its mask, and the visible suffix escaped as though
/// it were a whole account number — enough to bind a missing purchase ledger to
/// a sole live sales one carrying the same last eight.
///
/// Two characters minimum, and letters only: a lone `A` is an ordinary name
/// word, and a token carrying digits is already judged whole by `is_masked`.
fn is_mask_alphabetic(token: &str) -> bool {
    let canonical = token
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_uppercase())
        .collect::<String>();
    canonical.len() >= 2
        && canonical
            .chars()
            .all(|character| character.is_ascii_alphabetic())
        && is_masked(&canonical)
}

fn is_mask_punctuated(token: &str) -> bool {
    token
        .chars()
        .any(|character| matches!(character, '*' | '#' | '\u{2022}' | '\u{00d7}'))
}

/// The separators an operator writes a range with. The comparison key already
/// folds these dash variants to ASCII; the period boundary has to admit the
/// same set, or `FY2025\u{2013}26` fuses where `FY2025-26` splits.
const DASH_VARIANTS: [char; 9] = [
    '-', '/', '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{2212}',
];

/// A masked value exposes a non-unique suffix and identifies nothing.
///
/// `XXXXX1234X` clears every length and composition test — ten characters, six
/// letters, four digits, no period — while the only information in it is a last
/// four that any number of parties share. Two unrelated ledgers carrying the
/// same mask would bind to each other.
///
/// Recognized by its letters being a single repeated character, which is what a
/// mask is and what an identity-bearing code never is: `PH01AB00` and a
/// registration number both carry distinct letters.
fn is_masked(canonical: &str) -> bool {
    let mut letters = canonical.chars().filter(char::is_ascii_alphabetic);
    match letters.next() {
        Some(first) => letters.all(|letter| letter == first),
        None => false,
    }
}

/// A period label identifies a period, not a party or an item. Two unrelated
/// ledgers routinely share one, and identifier-first matching would bind the
/// source to whichever exists before it ever compared the names.
///
/// Applied to the **raw token**, because operators write ranges with the very
/// separators canonicalization strips: `FY2025-26` fuses to `FY202526`, whose
/// six-digit run reads as no period at all, and the label walked straight into
/// being a code. Splitting on the separator first keeps `FY2025` and `26`
/// legible as what they are.
///
/// The test is on the **numbers**, not the words: every part carries only
/// alphabetic markers and numbers that read as a year or a small ordinal, and
/// at least one number appears. Capping the length of the alphabetic run was an
/// earlier attempt that kept losing to longer spellings — a month name can be
/// any length; a year cannot.
///
/// An identity-bearing code survives: `PH-01A-B00` splits to a `PH` carrying no
/// number at all, and `AB12345678` holds a run no calendar would produce. Like
/// every exclusion here it can only make a bind *less* likely.
fn is_period(token: &str) -> bool {
    let mut any_number = false;
    for part in token.split(DASH_VARIANTS) {
        let canonical = part
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|character| character.to_ascii_uppercase())
            .collect::<String>();
        if canonical.is_empty() || !part_reads_as_period(&canonical, &mut any_number) {
            return false;
        }
    }
    any_number
}

fn part_reads_as_period(canonical: &str, any_number: &mut bool) -> bool {
    let mut rest = canonical;
    while !rest.is_empty() {
        let alphabetic = rest.starts_with(|character: char| character.is_ascii_alphabetic());
        let split = rest
            .find(|character: char| character.is_ascii_alphabetic() != alphabetic)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(split);
        rest = tail;
        if alphabetic {
            continue;
        }
        let value = run.parse::<u32>().unwrap_or(u32::MAX);
        let reads_as_period = match run.len() {
            1 | 2 => (1..=99).contains(&value),
            4 => (1900..=2199).contains(&value),
            // A range written without its separator. `FY2025-26` splits and is
            // read part by part; `FY202425` arrives whole, and every length
            // test above missed it — so the token passed as a code and bound a
            // missing `Purchases FY202425` to a sole live `Sales FY202425`.
            // Both spellings of the suffix occur: `202425` and `20242025`.
            6 | 8 => {
                let (lead, suffix) = run.split_at(4);
                let lead = lead.parse::<u32>().unwrap_or(u32::MAX);
                let suffix_value = suffix.parse::<u32>().unwrap_or(u32::MAX);
                (1900..=2199).contains(&lead)
                    && if suffix.len() == 2 {
                        (1..=99).contains(&suffix_value)
                    } else {
                        (1900..=2199).contains(&suffix_value)
                    }
            }
            _ => false,
        };
        if !reads_as_period {
            return false;
        }
        *any_number = true;
    }
    true
}

/// An eight-digit run that reads as a calendar date in any order this project
/// admits is a date, not an identifier. Recognizing only `YYYYMMDD` left
/// `01012026` binding a source to an unrelated master that shares its period
/// label. Being generous here can only make a bind *less* likely, which is the
/// safe direction for a rule whose failure mode is money against the wrong
/// party.
fn is_plausible_date(digits: &str) -> bool {
    if digits.len() != 8 {
        return false;
    }
    let number = |range: std::ops::Range<usize>| digits[range].parse::<u32>().unwrap_or(0);
    let (first, second, third, fourth) = (number(0..4), number(4..6), number(6..8), number(4..8));
    let (day, month) = (number(0..2), number(2..4));
    let year_first =
        (1900..=2199).contains(&first) && (1..=12).contains(&second) && (1..=31).contains(&third);
    // DDMMYYYY and MMDDYYYY are indistinguishable from each other without a
    // locale, so either reading is enough to disqualify the run.
    let year_last = (1900..=2199).contains(&fourth)
        && ((1..=31).contains(&day) && (1..=12).contains(&month)
            || (1..=12).contains(&day) && (1..=31).contains(&month));
    year_first || year_last
}

#[cfg(test)]
#[path = "master_binding_tests.rs"]
mod tests;
