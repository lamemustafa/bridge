//! Walking a ledger to the predefined group identity that classifies it.
//!
//! Three things in this repository need the same walk — a Schedule III head, a
//! cash/bank leg, and an outstandings party — and they need it for different
//! answers, so what is shared is the traversal and its refusals, never the
//! verdict. Each caller keeps its own policy above this: the outstandings
//! reader, for instance, additionally requires the *whole* group collection to
//! be coherent before walking any of it, because a report must be whole or
//! refused while a classifier answers one ledger at a time.
//!
//! Every rule below is a measured property of Tally's group model, recorded in
//! `docs/tally/TALLY_PROTOCOL_REFERENCE.md` §8.2b:
//!
//! * **Classify by `RESERVEDNAME`, never by `NAME`.** A predefined group can be
//!   renamed over XML while its reserved identity survives, so a rule written
//!   against the visible name silently stops matching in a renamed book.
//! * **An empty `RESERVEDNAME` is a positive signal**, not a missing value: it
//!   is Tally stating the group is user-created, so the walk climbs through it.
//!   A whitespace-only value is treated the same way — it is no more an
//!   identity than an empty one, and returning it as an ancestor would let a
//!   caller read "established" from a row that established nothing.
//!   A `None` is a third thing — a reader that never captured the attribute —
//!   and carries no claim either way, so it refuses.
//! * **A ledger exposes `PARENT` and no `PARENTSTRUCTURE`**, so ancestry is one
//!   hop at a time through the group collection rather than read off the row.
//! * **The reserved root is control-marked**, and arrives through the tolerant
//!   reader as a replacement marker rather than the bare word. That spelling is
//!   already the crate's [`is_tally_reserved_root`], which this reuses rather
//!   than re-deriving — a second copy would be a second thing to get wrong.
//!
//! **The hop itself is matched exactly, not normalized.** Tally matches master
//! names by exact codepoint, and a `PARENT` is emitted verbatim from the group
//! `NAME` it refers to, so within one coherent snapshot the two are identical
//! bytes — measured across both captured companies in this tree: 21 distinct
//! `PARENT` values, every one an exact match to a group `NAME` except the
//! reserved root, and not a single case- or whitespace-only near match. A pair
//! that differs is therefore not a spelling variant to be helpfully resolved;
//! it is an incoherent or cross-snapshot pair, and resolving it would admit a
//! ledger on evidence that does not hold. Only the terminating `RESERVEDNAME`
//! is compared loosely, and only because a caller's own list of identities is
//! hand-written rather than read from Tally.
//!
//! **Both hops arrive verbatim, and that took four separate fixes.** Every
//! reader upstream of this walk once normalized the value it produced: the
//! standard catalogue's ledger `PARENT` twice over, in a validator and then
//! again in the reader calling it; and, on the native side, the group `PARENT`
//! and the ledger `PARENT`, both through a shared text helper the file's other
//! fourteen call sites legitimately want trimming from. Each trim was
//! invisible from here, and each resolved an incoherent pair against a real
//! group before the walk could refuse it — or, once the hops became exact,
//! failed a coherent one. The fix in every case was a reader that judges
//! emptiness on the trimmed view and retains the bytes.
//!
//! **The trap is worth naming**, because three of the four were found only
//! after the property had been asserted somewhere: the code claiming verbatim
//! bytes and the code defeating it sat in different files, so nothing looked
//! inconsistent. An exactness rule is only as good as its furthest upstream
//! reader, and that reader is not usually the one you are editing.
//!
//! Every outcome that is not a reserved identity is an [`AncestryGap`]. A
//! caller decides what each gap means for its own question; none of them is an
//! answer, and an incomplete group collection therefore refuses rather than
//! misclassifies.

use std::collections::{BTreeMap, BTreeSet};

use crate::{is_tally_reserved_root, TallyNamedMaster};

/// Why a ledger has no reserved group identity.
///
/// Kept as distinct variants rather than one message because the two callers
/// word them differently, and because the difference is operationally real: a
/// group absent from the collection is a different problem from a group whose
/// name repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AncestryGap {
    /// Tally returned no parent group for the ledger.
    NoParent,
    /// The walk reached the reserved account root, which is not a group row.
    ReachedRoot,
    /// A group in the chain is not in the collection that was read.
    GroupAbsent,
    /// Two rows share a name, so the hop has no single answer.
    GroupNameRepeated,
    /// A group in the chain omitted `RESERVEDNAME` entirely, so nothing about
    /// its identity survives a rename.
    ReservedNameMissing,
    /// The observed ancestry loops.
    Cycle,
    /// The chain outran the collection, which a cycle check should already
    /// have caught; retained so a pathological index cannot spin.
    Exhausted,
}

/// One group collection, indexed for repeated ancestry walks.
///
/// Owns its rows so a caller can hold it across many classifications without
/// threading a borrow of the response it came from.
#[derive(Debug, Clone, Default)]
pub struct GroupIndex {
    by_name: BTreeMap<String, Vec<TallyNamedMaster>>,
}

impl GroupIndex {
    pub fn build(groups: impl IntoIterator<Item = TallyNamedMaster>) -> Self {
        let mut by_name: BTreeMap<String, Vec<TallyNamedMaster>> = BTreeMap::new();
        for group in groups {
            let key = group.name.clone();
            if !key.is_empty() {
                by_name.entry(key).or_default().push(group);
            }
        }
        Self { by_name }
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Climbs from a ledger's `PARENT` to the first predefined group identity,
    /// returning that group's `RESERVEDNAME` in Tally's own spelling.
    ///
    /// `parent` is the ledger's observed parent, `None` when Tally returned
    /// none. The returned name is a Tally predefined identity and never the
    /// book's own naming: a user-created group has an empty `RESERVEDNAME` and
    /// the walk passes through it, so a caller may repeat this value back to a
    /// user without redacting it.
    pub fn reserved_ancestor(&self, parent: Option<&str>) -> Result<&str, AncestryGap> {
        let mut current = parent.ok_or(AncestryGap::NoParent)?.to_string();
        let mut visited = BTreeSet::new();
        // Each hop consumes one distinct group; the visited set bounds the walk
        // independently, so this only guards a pathological index.
        for _ in 0..=self.by_name.len() {
            if current.is_empty() || is_tally_reserved_root(&current) {
                return Err(AncestryGap::ReachedRoot);
            }
            if !visited.insert(current.clone()) {
                return Err(AncestryGap::Cycle);
            }
            let [group] = self
                .by_name
                .get(&current)
                .ok_or(AncestryGap::GroupAbsent)?
                .as_slice()
            else {
                return Err(AncestryGap::GroupNameRepeated);
            };
            let reserved = group
                .reserved_name
                .as_deref()
                .ok_or(AncestryGap::ReservedNameMissing)?;
            // Blank after trimming is not a predefined identity. Tally signals
            // user-created with an empty value; a whitespace-only one is
            // neither observed nor usable, and returning it as an ancestor let
            // a caller read "established as something" from a row that
            // established nothing.
            if !reserved.trim().is_empty() {
                return Ok(reserved);
            }
            current = group
                .parent
                .nonempty_returned_text()
                .unwrap_or_default()
                .to_string();
        }
        Err(AncestryGap::Exhausted)
    }
}

#[cfg(test)]
#[path = "group_ancestry_tests.rs"]
mod tests;
