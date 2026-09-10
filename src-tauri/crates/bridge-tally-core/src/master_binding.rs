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
/// Most entities one binding request may name.
pub const MAX_SOURCE_ENTITIES: usize = 5_000;
/// Longest accepted master or source name, in characters. This bounds
/// pathological input; it is not a claim about what Tally accepts, and a
/// caller with a stricter contract of its own enforces that at its own
/// boundary.
pub const MAX_NAME_CHARS: usize = 16_384;
/// Most candidates retained per unbound entity.
pub const MAX_CANDIDATES_PER_ENTITY: usize = 25;
/// Most identifiers extracted from one name.
pub const MAX_IDENTIFIERS_PER_NAME: usize = 8;
/// Digits a numeric run needs before it is treated as an identifier. Eight
/// excludes a year, a rate, a house number and a masked last-four; a mobile,
/// an account number and a customer code all clear it.
pub const MIN_NUMERIC_IDENTIFIER_DIGITS: usize = 8;
/// Alphanumeric characters a mixed letter-and-digit token needs before it is
/// treated as a code identifier.
pub const MIN_CODE_IDENTIFIER_CHARS: usize = 4;
/// Digits a code identifier needs alongside at least one letter.
pub const MIN_CODE_IDENTIFIER_DIGITS: usize = 2;
/// Shortest comparison key that may take part in a prefix near-miss.
pub const MIN_PREFIX_KEY_CHARS: usize = 3;
/// Shortest token that may take part in a shared-token near-miss.
pub const MIN_TOKEN_CHARS: usize = 3;
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
    #[error("fallback master was not a current catalog entry")]
    FallbackNotInCatalog,
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
            Self::FallbackNotInCatalog => "master_fallback_not_in_catalog",
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

/// What could not be bound, and why. This is the operator's work item, not an
/// error path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Unresolved {
    pub reason: UnboundReason,
    /// Identifiers extracted from the source name and from caller hints,
    /// retained so a fallback posting can be reallocated later without
    /// re-reading the source document.
    pub unresolved_identity: Vec<Identifier>,
    pub candidates: Vec<Candidate>,
    /// Candidates found before truncation.
    pub candidate_count: usize,
    pub candidates_truncated: bool,
}

/// Exactly one outcome per source entity.
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BindingReport {
    class: MasterClass,
    entities: Vec<EntityBinding>,
}

impl BindingReport {
    pub fn class(&self) -> MasterClass {
        self.class
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FallbackBinding {
    position: usize,
    source_name: String,
    fallback_name: String,
    retained: Vec<Identifier>,
    reason: UnboundReason,
}

impl FallbackBinding {
    /// Parks one unbound entity against a catalog-verified fallback master.
    ///
    /// Refuses a bound entity and refuses a fallback name that is not a current
    /// catalog entry — a suspense ledger that does not exist is how one
    /// engagement lost a batch.
    pub fn assign(
        entity: &EntityBinding,
        catalog: &MasterCatalog,
        fallback_name: &str,
    ) -> Result<Self, MasterBindingError> {
        let unresolved = entity
            .unresolved()
            .ok_or(MasterBindingError::FallbackNotInCatalog)?;
        let fallback = catalog
            .exact(fallback_name)
            .ok_or(MasterBindingError::FallbackNotInCatalog)?;
        Ok(Self {
            position: entity.position,
            source_name: entity.source_name.clone(),
            fallback_name: fallback.to_string(),
            retained: unresolved.unresolved_identity.clone(),
            reason: unresolved.reason,
        })
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
    key: String,
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
        let name = validated_source_name(name)?;
        let mut identifiers = extract_identifiers(&name);
        for hint in hints {
            let extracted = extract_identifiers(hint);
            if extracted.is_empty() {
                return Err(MasterBindingError::IdentifierHintUnusable);
            }
            identifiers.extend(extracted);
        }
        identifiers.sort();
        identifiers.dedup();
        identifiers.truncate(MAX_IDENTIFIERS_PER_NAME);
        Ok(Self {
            position,
            key: comparison_key(&name),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterCatalog {
    class: MasterClass,
    entries: Vec<CatalogEntry>,
    by_name: BTreeMap<String, usize>,
    by_key: BTreeMap<String, Vec<usize>>,
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
            let name = validated_catalog_name(name.as_ref())?;
            if by_name.contains_key(&name) {
                return Err(MasterBindingError::CatalogDuplicateName);
            }
            by_name.insert(name.clone(), entries.len());
            let key = comparison_key(&name);
            entries.push(CatalogEntry {
                identifiers: extract_identifiers(&name),
                tokens: tokens_of(&key),
                key,
                name,
            });
        }
        if entries.is_empty() {
            return Err(MasterBindingError::CatalogEmpty);
        }

        let mut by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let mut by_identifier: BTreeMap<Identifier, Vec<usize>> = BTreeMap::new();
        let mut by_token: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            by_key.entry(entry.key.clone()).or_default().push(index);
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

        Ok(Self {
            class,
            entries,
            by_name,
            by_key,
            by_identifier,
            by_token,
            common_tokens,
        })
    }

    pub fn class(&self) -> MasterClass {
        self.class
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
    Ok(BindingReport {
        class: catalog.class,
        entities: entities
            .iter()
            .map(|entity| bind_one(catalog, entity))
            .collect(),
    })
}

fn bind_one(catalog: &MasterCatalog, entity: &SourceEntity) -> EntityBinding {
    let exact = catalog.by_name.get(&entity.name).copied();

    // Rule one: the identifier is the key, the name is a hint. A name
    // comparison on a pair that carries a decisive identifier is not merely
    // weaker evidence, it is actively misleading.
    let mut identifier_matches = BTreeSet::new();
    let mut identifier_conflict = false;
    for identifier in &entity.identifiers {
        if let Some(holders) = catalog.by_identifier.get(identifier) {
            if holders.len() > 1 {
                identifier_conflict = true;
            }
            identifier_matches.extend(holders.iter().copied());
        }
    }

    // An identifier shared by two masters, and an entity whose identifiers
    // reach two masters, are the same refusal: the operator has a naming
    // collision to see, and neither case licenses a choice.
    let status = if identifier_conflict || identifier_matches.len() > 1 {
        unresolved_status(
            catalog,
            entity,
            UnboundReason::IdentifierConflict,
            exact,
            &identifier_matches,
        )
    } else if let Some(matched) = identifier_matches.iter().copied().next() {
        // An identifier pointing at one master while the name exactly names
        // another is a disagreement between two strong signals; it is shown,
        // not silently decided in the identifier's favour.
        if exact.is_some_and(|index| index != matched) {
            unresolved_status(
                catalog,
                entity,
                UnboundReason::IdentifierNameConflict,
                exact,
                &identifier_matches,
            )
        } else {
            BindingStatus::Bound {
                catalog_name: catalog.entries[matched].name.clone(),
                basis: BindingBasis::Identifier,
            }
        }
    } else if let Some(index) = exact {
        BindingStatus::Bound {
            catalog_name: catalog.entries[index].name.clone(),
            basis: BindingBasis::ExactName,
        }
    } else {
        match catalog.by_key.get(&entity.key).map(Vec::as_slice) {
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
            ),
            None => {
                let candidates = collect_candidates(catalog, entity, &identifier_matches);
                let reason = if candidates.is_empty() {
                    UnboundReason::NoCandidate
                } else {
                    UnboundReason::NearMiss
                };
                unresolved_from(entity, reason, candidates)
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
) -> BindingStatus {
    let mut candidates = collect_candidates(catalog, entity, identifier_matches);
    if let Some(index) = exact {
        let name = catalog.entries[index].name.as_str();
        if !candidates.iter().any(|(candidate, _)| candidate == name) {
            candidates.push((name.to_string(), CandidateRule::NormalizedEqual));
        }
    }
    unresolved_from(entity, reason, candidates)
}

fn unresolved_from(
    entity: &SourceEntity,
    reason: UnboundReason,
    candidates: Vec<(String, CandidateRule)>,
) -> BindingStatus {
    let mut ordered = candidates;
    ordered.sort_by(|left, right| {
        left.1
            .rank()
            .cmp(&right.1.rank())
            .then_with(|| left.0.cmp(&right.0))
    });
    let candidate_count = ordered.len();
    let candidates_truncated = candidate_count > MAX_CANDIDATES_PER_ENTITY;
    let candidates = ordered
        .into_iter()
        .take(MAX_CANDIDATES_PER_ENTITY)
        .map(|(catalog_name, rule)| Candidate { catalog_name, rule })
        .collect();
    let unresolved = Unresolved {
        reason,
        unresolved_identity: entity.identifiers.clone(),
        candidates,
        candidate_count,
        candidates_truncated,
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
) -> Vec<(String, CandidateRule)> {
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

    for index in identifier_matches {
        offer(*index, CandidateRule::SharedIdentifier);
    }
    if let Some(holders) = catalog.by_key.get(&entity.key) {
        for index in holders {
            offer(*index, CandidateRule::NormalizedEqual);
        }
    }
    if entity.key.chars().count() >= MIN_PREFIX_KEY_CHARS {
        // The key index is ordered, so both prefix directions are range or
        // point lookups rather than a scan of the whole catalog per entity.
        for (key, holders) in catalog.by_key.range(entity.key.clone()..) {
            if !key.starts_with(&entity.key) {
                break;
            }
            if key == &entity.key {
                continue;
            }
            for index in holders {
                offer(*index, CandidateRule::CatalogPrefix);
            }
        }
        for split in MIN_PREFIX_KEY_CHARS..entity.key.len() {
            if !entity.key.is_char_boundary(split) {
                continue;
            }
            let prefix = &entity.key[..split];
            if prefix.chars().count() < MIN_PREFIX_KEY_CHARS {
                continue;
            }
            if let Some(holders) = catalog.by_key.get(prefix) {
                for index in holders {
                    offer(*index, CandidateRule::SourcePrefix);
                }
            }
        }
    }
    for token in tokens_of(&entity.key) {
        if catalog.common_tokens.contains(&token) {
            continue;
        }
        if let Some(holders) = catalog.by_token.get(&token) {
            for index in holders {
                offer(*index, CandidateRule::SharedToken);
            }
        }
    }

    best.into_iter()
        .map(|(index, rule)| (catalog.entries[index].name.clone(), rule))
        .collect()
}

/// An observed master name is retained **verbatim**. Surrounding whitespace is
/// part of what the book returned, and a caller that acts on a binding writes
/// this string back to Tally byte for byte; trimming it here would report a
/// spelling that does not exist and refuse at the write gate with no
/// explanation. The comparison key collapses whitespace anyway, so a source
/// name still matches across the difference.
fn validated_catalog_name(value: &str) -> Result<String, MasterBindingError> {
    validate_name_bounds(value)?;
    Ok(value.to_string())
}

/// A source name is trimmed: leading and trailing whitespace is document noise
/// rather than an observation, and nothing is ever written back from it.
fn validated_source_name(value: &str) -> Result<String, MasterBindingError> {
    validate_name_bounds(value)?;
    Ok(value.trim().to_string())
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
fn comparison_key(value: &str) -> String {
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

fn tokens_of(key: &str) -> BTreeSet<String> {
    key.split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= MIN_TOKEN_CHARS)
        .map(str::to_string)
        .collect()
}

/// Extracts every identifier a name carries, in canonical form.
///
/// A numeric run may hold `-` and `/` internally, so a punctuated account
/// number and a plain one agree; it may not hold spaces, so separated digit
/// groups fail closed to a near-miss rather than fusing into a false
/// identifier.
fn extract_identifiers(value: &str) -> Vec<Identifier> {
    let mut identifiers = BTreeSet::new();
    for run in value.split(|character: char| {
        !(character.is_ascii_digit() || character == '-' || character == '/')
    }) {
        let digits = run.chars().filter(char::is_ascii_digit).collect::<String>();
        if digits.len() >= MIN_NUMERIC_IDENTIFIER_DIGITS && !is_plausible_date(&digits) {
            identifiers.insert(Identifier {
                kind: IdentifierKind::Numeric,
                value: digits,
            });
        }
    }
    for token in value.split(char::is_whitespace) {
        let canonical = token
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .map(|character| character.to_ascii_uppercase())
            .collect::<String>();
        let digits = canonical.chars().filter(char::is_ascii_digit).count();
        let letters = canonical.chars().filter(char::is_ascii_alphabetic).count();
        if canonical.len() >= MIN_CODE_IDENTIFIER_CHARS
            && digits >= MIN_CODE_IDENTIFIER_DIGITS
            && letters >= 1
        {
            identifiers.insert(Identifier {
                kind: IdentifierKind::Code,
                value: canonical,
            });
        }
    }
    let mut identifiers = identifiers.into_iter().collect::<Vec<_>>();
    identifiers.truncate(MAX_IDENTIFIERS_PER_NAME);
    identifiers
}

/// An eight-digit run that reads as a calendar date is a date. Excluding it
/// costs a near-miss on an account number that happens to look like one, and
/// prevents a period label binding two unrelated masters together.
fn is_plausible_date(digits: &str) -> bool {
    if digits.len() != 8 {
        return false;
    }
    let number = |range: std::ops::Range<usize>| digits[range].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (number(0..4), number(4..6), number(6..8));
    (1900..=2199).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
#[path = "master_binding_tests.rs"]
mod tests;
