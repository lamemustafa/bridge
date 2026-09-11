# 0018 — The narration marker as an identity basis for voucher presence

- Status: Accepted
- Date: 2026-09-11
- Amends: [0017 — Voucher presence authority](0017-voucher-presence-authority.md)
- Depends on: [0016 — Master binding authority](0016-master-binding-authority.md)

## Context

ADR 0017 gives presence two bases for `Present`: a Tally-assigned `REMOTEID`
read back on a previous visit, and a manual voucher number unique on both
sides. Both are real, and **neither reaches the case Bridge is uniquely placed
to answer**: *did I write this voucher myself?*

- `REMOTEID` does not reach it twice over. §3.3a verified Tally **overwrites**
  the attribute, so the client key Bridge sent is not what comes back; and the
  qualified `vouchers` read profile does not `FETCH REMOTEID` at all, which is
  why every proposal carrying one currently returns
  `RemoteIdEvidenceUnavailable` rather than an answer.
- A manual voucher number is the *operator's* series. It says nothing about who
  posted the voucher, and under `Automatic` numbering it does not exist.

Meanwhile Bridge already writes an identity into a field Tally does not own.
Every import appends `[BRIDGE:<identity>]` to `NARRATION`; the qualified
`vouchers` profile already fetches `NARRATION`; and §9.8's batch-identity
observation records readback retaining that attribution across a round trip,
with the `REMOTEID` and the narration marker deriving from the same value.

So the channel exists, is already written, is already read, and has live
evidence behind it — and presence does not use it. That is the gap this ADR
closes.

### The hazard that shapes the whole contract

The marker is **not** the caller's transaction label. It is
`SHA-256("bridge.mcp.import.v1", batch_id, txn_id)` truncated to a UUID, where
`batch_id` is a random v4 UUID persisted per batch. That derivation exists
precisely because a caller's transaction label is, in the import module's own
words, "commonly reused" — §9.8 records a second batch reusing an earlier
caller label, and the domain-separated digest is what stopped the two colliding.

But the scheme field is `Option<ImportIdentityScheme>`, and `None` means an
older file whose marker is **the raw caller label**. Bridge never rewrites a
saved file, so books in the field can hold both kinds. A bare label like
`txn-001` is not unique across batches, and treating one as an identity would
match a proposal against an unrelated voucher from an unrelated import — a
false `Present`, which drops an invoice silently. That is the exact failure ADR
0017 exists to prevent, and it is reachable by *adding* this basis carelessly.

## Decision

### 1. The proposal's marker is derived, never accepted

A caller does not hand presence a marker string. It supplies the `batch_id` and
`bridge_txn_id` of the import it is asking about, and the adapter derives the
marker with the **same `import_identity` function the writer uses**. One
derivation, one place, no second implementation to drift.

This is not ceremony, it is the whole safety argument. A derived marker is a
digest over a random batch UUID, so it cannot equal a legacy caller label
except by deliberate contrivance. Accepting an arbitrary marker string would
let a caller pass `txn-001` and match a legacy voucher from an unrelated batch.
The input shape is what makes the hazard in the Context unreachable, rather
than a validation rule that has to be remembered.

### 2. Extraction is the adapter's job; the crate compares opaque strings

`[BRIDGE:<identity>]` is a **Bridge writer convention, not a Tally fact**, and
`bridge-tally-core` is the portable contract layer. So the adapter extracts the
marker from the observed narration and the crate receives an opaque string it
never parses.

The second reason is cost. An opaque string is hashable, so the book index is
built once and every proposal is a lookup. Extraction inside the crate would
mean a substring scan of every narration for every proposal — the shape of
allocation this contract has already had to correct twice.

### 3. One marker, and it must be identifying

A book voucher yields an identity marker only when its narration carries
**exactly one** well-formed `[BRIDGE:…]` occurrence *and* that occurrence is a
UUID of the version the writer stamps. Everything else — two markers, a
malformed one, or a well-formed legacy caller label — yields **no identity**.

Both halves fail closed, for different reasons. Two markers mean the voucher
claims two imports, which is the middle case ADR 0017 forbids resolving; the
import path already treats it as an error (`import_verification_tag_ambiguous`)
rather than taking the first. A non-UUID marker is a legacy-scheme write, and
§1 is exactly the reason it must not decide.

The **version** is checked and not only the spelling, because a transaction
label may legally *be* UUID-shaped: `valid_txn_id` admits hex and hyphens, so a
legacy-scheme write could carry a canonical v4 and a spelling check alone would
have called it batch-derived. `import_identity` builds through
`Uuid::Builder::from_custom_bytes`, which stamps version 8 and the RFC 4122
variant, so requiring those rejects the whole impersonable class rather than
the fraction of it that happens to look wrong.

Such a voucher is still a Bridge write, and losing that fact silently would be
its own defect. So the window reports `unidentified_bridge_writes`: a count of
rows carrying a reserved marker that could not identify one. It is a book
observation for a person, not a status — a row that also resembles a proposal
already surfaces as a candidate under ADR 0017's existing resemblance rules,
and that is the proportionate response. A count does not need a new rule.

### 4. Whether the column was read is a fact about the read

`BookWindow` declares `ColumnEvidence::Observed` or `NotRead` for the narration
exactly as ADR 0017 §2 requires for `REMOTEID`, and for the same reason: "no
voucher carried a marker" and "the read never fetched narration" are different
facts and only the first is evidence. A proposal supplying an import identity
against a window that did not read narration is `MarkerEvidenceUnavailable` —
it cannot be `Absent`, and it cannot be `Present` on a weaker basis either,
because the evidence that could contradict that basis was skipped.

This ADR generalises `RemoteIdEvidence` into `ColumnEvidence` rather than
adding a second enum of identical shape. The concept was never about
`REMOTEID`; it was always "this column was or was not read".

### 5. The marker ranks with the other identities, and disagreement is reported

`NarrationMarker` is a third basis under ADR 0017 §5, not a tiebreaker and not
a fallback. Every rule there applies to it unchanged: unique on both sides,
one book voucher satisfies at most one proposal across bases, a cancelled or
optional voucher never yields `Present`, and a `Present` reports its
differences.

All identity lookups continue to be resolved **before any of them settles**, so
a marker selecting one voucher while a `REMOTEID` or a manual number selects
another is `IdentityConflict`. Ranking them would be the move ADR 0016 refuses
when an identifier contradicts an exact name, and adding a third signal makes
that more important rather than less: there are now three ways to disagree.

### 6. What this basis does not reach, said plainly

The marker reaches **only vouchers Bridge itself wrote**. It says nothing about
the hand-keyed voucher, which is the case that produced ADR 0017's ₹36.13
finding and the case the capability was built for. This basis adds reach where
Bridge has written before; it does not reduce the other two bases' work, and it
must not be read as making the manual-number basis redundant.

### 7. The evidence, and its scope

§9.8's batch-identity and native-selector observations record the marker
surviving a round trip on a **Silver 7.1 Journal**. That is one product mode
and one voucher type. The marker's survival on other voucher types is
unqualified, and this ADR does not claim it.

What makes shipping that honest is the **direction of the unqualified
failure**. If a marker does not survive, the voucher reads as marker-absent and
the proposal falls through to the other bases — at worst producing a duplicate,
which is visible in the book and correctable. It cannot produce a false
`Present`, because a marker that was not written cannot match one that was.
The unqualified case fails toward the noisy direction, which is the posture ADR
0017 §7 sets out.

**Known limit.** An operator who edits a narration removes the marker, costing
a `Present` and yielding a duplicate. Same direction, same reasoning: visible
and correctable rather than silent.

## Consequences

- Presence answers "did Bridge write this?" for every voucher Bridge wrote
  under the batch-derived scheme, with no change to a qualified read profile —
  `NARRATION` is already fetched.
- The `voucher_presence` input grows `batch_id` and `bridge_txn_id` per
  proposed voucher, both optional. A proposal that supplies neither behaves
  exactly as it does under ADR 0017.
- `RemoteIdEvidence` is renamed to `ColumnEvidence`. One type, two columns.
- Books written by the legacy scheme get a count, not an identity. If those
  turn out to be common in the field, the follow-up is a qualification of
  legacy-label matching under a batch the caller names — deliberately not
  attempted here, because it needs the caller to bound the batch and this ADR
  does not have that input.

## Alternatives rejected

**Accept a marker string from the caller.** Simpler input, and it makes the
legacy-collision hazard a validation rule someone has to remember instead of a
shape that cannot express the bad case. Rejected on §1.

**Match legacy caller labels too, for reach.** This is the one alternative that
would increase coverage on older books, and it produces exactly the silent
failure — a label matching across unrelated batches — that presence exists to
prevent. Reach is not worth a false `Present`.

**Extract the marker inside the crate.** Puts a Bridge writer convention in the
portable layer and turns a hash lookup into a per-proposal substring scan.

**Treat the first of several markers as the identity.** Cheap, matches what one
existing readback helper does, and silently resolves the middle case. The
import path's stricter helper refuses it, and this contract agrees with the
stricter one.
