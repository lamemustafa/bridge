# Tally XML gateway protocol reference — write responses and master identity

> This is a section of the canonical [Tally XML gateway protocol reference](./TALLY_PROTOCOL_REFERENCE.md). Read its confidence-marker convention and legacy-link note before relying on a finding.

---

## 9. Writes (import)

**VERIFIED.** Writes succeed on **Education mode** — the restriction is on the *voucher date*
(1st, 2nd, 31st), not on the calendar day of entry.

### 9.1 Response shape

A bare `<RESPONSE>` root with **no `ENVELOPE`, no `HEADER`, no `STATUS`**:

```xml
<RESPONSE>
 <CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED>
 <LASTVCHID>295</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED>
 <IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS>
</RESPONSE>
```

A `STATUS=1` rule cannot apply to imports — there is no `STATUS` to check.

### 9.1a A malformed request returns a counter-less response — **fourth response shape**

**VERIFIED.** A request whose XML is not well-formed returns:

```xml
<RESPONSE>Unknown Request, cannot be processed</RESPONSE>
```

No counters. No `STATUS`. No `LINEERROR`. A parser that reaches for `CREATED` or `ERRORS`
will find nothing — and code that defaults missing counters to zero would read this as
"nothing created, no errors", i.e. a benign no-op, when in fact the request never executed.

**Treat a counter-less import response as a hard failure**, distinct from both success and
from a counted rejection.

### 9.1b XML escaping is mandatory — a default Tally group breaks naive builders

**VERIFIED.** The above failure was caused by `<PARENT>Duties & Taxes</PARENT>`. An
unescaped `&` makes the document malformed and Tally rejects the whole request.

**`Duties & Taxes` is a stock Tally group present in every company**, so this is not an edge
case — any request builder that does not escape `&`, `<` and `>` in names, narrations, party
names or group names will fail on ordinary Indian book data. Ledger names containing `&`
("Ram & Sons", "Bharat Iron & Steel") are extremely common.

Escape on the way out; the failure is total and gives no hint of which field caused it.

### 9.2 `ERRORS=0` does not mean success — **TRAP**

**VERIFIED.** A rejected voucher returned `CREATED=0, ERRORS=0, EXCEPTIONS=1` plus a
`LINEERROR`. Any success test of the form `ERRORS == 0` reports a rejected write as posted.

> **Success requires all four: the intended counter incremented, `ERRORS=0`,
> `EXCEPTIONS=0`, and no `LINEERROR`.**

An `EXCEPTIONS=1` can also come with **no** `LINEERROR` at all: under Manual numbering with
duplicates prevented, a missing or reused voucher number was refused that way on licensed 7.1
Silver (PARTIAL — observed once; §9.14). A rule that looks for a line error to explain a refusal
misses it.

**Bridge admission rule (2026-09-07).** Clean-result classification additionally
requires source presence for all seven counters: `CREATED`, `ALTERED`, `DELETED`,
`IGNORED`, `ERRORS`, `CANCELLED`, and `EXCEPTIONS`. An omitted counter is not an
observed zero. Saved responses retain these presence bits alongside their counts.
Older records without presence bits remain readable but cannot establish a clean
response, even when readback matches. Preserve the original batch for investigation;
do not infer missing evidence or resend it to obtain a cleaner receipt.

`LINEERROR` text is **untrustworthy for cause attribution** — an out-of-range date produced
"Voucher date is missing" when the date was present.

### 9.3 Voucher idempotency depends on `REMOTEID` — **this section's title used to say the opposite**

**VERIFIED, on an automatically numbered voucher type.** Re-sending an identical voucher payload
carrying the same `VOUCHERNUMBER` created a **second voucher**.

Read that precisely, because the obvious paraphrase — "Tally does not dedupe on the voucher
number" — is false in two directions. Under **automatic** numbering the supplied number is
*discarded* (§9.8), so the two sends never shared a stored voucher number and nothing could have
deduped on it. Under **Manual + `PREVENTDUPLICATES=Yes`**, §9.8 records that a repeated number is
**cleanly rejected** — a qualified rejection that a reader of this sentence would otherwise never
look for. Qualified narrowly, though: §9.8 measured a **failed `Alter`**, and its own rule forbids
carrying that observation to a different request identity mechanism. A crash retry sends a
`Create`, which is **UNVERIFIED** here, as is the behaviour on any licensed SKU — §9.8's scope
clarification covers a licensed Journal `REMOTEID` repeat and says in terms that it establishes
neither voucher-number identity nor the configured numbering method. Do not read this sentence as
promising a crash-retry is safe under Manual numbering.

So: on the numbering method measured here, a crash-retry duplicates client data unless the
integrator prevents it.

**That measurement stands; the conclusion drawn from it did not.** This section was headed *"No
natural idempotency for vouchers"*, and it was read — including by me, repeatedly — as saying no
idempotency mechanism exists.

Note what that means about the failure. This document was never *wrong*: **§9.8's scope
clarification has carried the upsert result since 2026-09-06**, with the counters and the file
SHA, and links `IMPLEMENTATION_GUIDE.md` §3.3a. The defect was navigational — the narrow case
stated under a heading that reads as the general one, five sections earlier, with no pointer to the
exception. A reader who lands here stops here. That is the whole reason a cross-reference is worth
as much as a measurement.

§3.3a supersedes the general reading:

```
import #1  REMOTEID="…-001"  ->  CREATED=1  ALTERED=0
import #2  byte-identical    ->  CREATED=0  ALTERED=1
vouchers in Tally afterwards ->  1
```

**With a client-supplied `REMOTEID`, a re-import upserts. It does not duplicate.** The vouchers
measured here carried **no** client `REMOTEID`, so Tally assigned its own and every send was a new
object — that was the uncontrolled variable, and the title generalised past it.

Consequences, since a stale reading of this section is expensive in both directions: an integrator
who believes there is no idempotency builds a dedup table or a narration hack it does not need, and
one who supplies a `REMOTEID` without knowing it upserts can silently **overwrite** an earlier
voucher by reusing a key.

**Not the outbox, though.** `REMOTEID` prevents a duplicate; it does not tell you, after a crash,
*what you sent*. The `REMOTEID` **attribute** does not echo the client key on readback (below), so
the exact key and payload must remain on disk for read-only reconciliation. They do not
authorize a resend after an unknown outcome; restart behaviour remains unqualified. See
`IMPLEMENTATION_GUIDE.md` §§3.3–3.5 and the playbook's held recovery flow. Be precise about the
field: the key itself does survive anywhere Tally does not own — a narration marker comes back —
and a categorical "Tally does not return the key" would send recovery work to discard the one
attribution channel that works. The durable dispatch intent stays — see the
`row fsynced before dispatch` invariant in `IMPROVEMENT_PLAN_2026H2.md` and the
restart-reconciliation flow in `docs/agent/README.md`.

§3.3a has the full table, including that `ACTION="Alter"` with a `REMOTEID` creates a duplicate —
inverted from intuition. **That is a reason not to use `Alter`, not a reason to use `Create` as a
correction path:** every correction sends a *different* payload, and changed-payload behaviour is
UNVERIFIED (below). This section prescribes no correction path.

**The `REMOTEID` *attribute* does not echo the client key on the one path where this was checked.**
§3.3a records that Tally overwrites the attribute with its own value; what came back was a
*company-GUID + master-id* pair. The committed capture
`src-tauri/crates/bridge-tally-protocol/tests/fixtures/agent/native-namespaced-journal.utf16le.xml`
shows a voucher Bridge imported returning
`REMOTEID="<TALLY-COMPANY-GUID>-<MASTER-ID>"` where the client key was
`<CLIENT-BATCH-KEY>`. These are redacted illustrative placeholders that preserve the observed field relationship.

**Be precise about which field.** The client key is *not* absent from that response — the same
capture returns
`<NARRATION>… [BRIDGE:<CLIENT-BATCH-KEY>]</NARRATION>`, and §9.8 records that the
`REMOTEID` and the narration marker were the same batch-derived UUID. So the observation is
field-specific: **the attribute is overwritten; a marker you place in a field Tally does not own
survives.** Stating it as "the client key appears nowhere" would contradict a byte-level check of
the very capture cited.

On that path, then, the client value is **stored and matched for upsert** while not being readable
back *through that attribute*, and a readback comparing the returned `REMOTEID` against the one you
sent rejects a legitimate import.

Whether the client key also works as a **`Delete`** selector is a separate question and is
**UNVERIFIED here**: §9.7's Delete row was measured on this document's Edit Log 7.0 Educational
baseline (§0), not on a licensed instance, and not by client key on these voucher types. Do not
read "usable as a key" into it.

**Scope: one Silver 7.1 Journal readback. PARTIAL.** Whether another voucher type, request shape or
Tally version preserves the client value is **UNVERIFIED**. Do not generalise this into a rule that
the attribute is never useful — on a release that did preserve it, that rule would discard real
identity evidence. Check what your own readback returns before relying on it either way.

**Do not replace attribution with a content fingerprint.** The obvious substitute — confirm the
voucher by date, ledger entries and amount — is not an attribution key: a company holding a
recurring or duplicate same-day payment already contains a voucher with that tuple, so a
pre-existing voucher can stand in for a write that never happened. Bridge's own verifier treats a
fingerprint-only match as `matching_content_observed` and reaches `posted_verified` only through a
narration-tagged match, with pre-import boundary and identity checks around it
(`src-tauri/src/agent_import.rs`).

**Carry a marker in a field Tally does not rewrite** — which is what the capture above shows
working, and why Bridge's verifier is built on the narration tag rather than on the attribute.

**§9.8's scope limit still applies to all of the above.** The exact-file repeat was measured on the
licensed **Journal** path; §9.8 says explicitly that it does not establish other request shapes,
other voucher types, or universal `REMOTEID` semantics. Treat upsert-on-repeat as verified for
Journal and **UNVERIFIED elsewhere**.

**Qualifying another voucher type takes a captured response, not a count.** A repeat that was
rejected, or whose transport failed before Tally processed it, also leaves the voucher count at
one — so "re-import and check the count" can promote semantics that were never exercised. Match
what §9.8 required of the Journal evidence: a captured response showing `CREATED=0, ALTERED=1`
with every failure counter zero, plus a readback proving it is the **same object** (unchanged GUID
and master ID), and only then record the type as qualified.

> **Scoped correction, 2026-09-16 — changed payloads were measured over the gateway, and they
> replace.** The paragraph below was right that nothing had measured them; it no longer describes
> the licensed gateway path. On **licensed TallyPrime 7.1 Silver**, over the XML gateway, into a
> synthetic company, using hand-built XML that copies the envelope and voucher element shape
> `render_import_xml` produces. None of it was sent through Bridge's own binary:
>
> | Voucher | Sent again under the same `REMOTEID` | Response | Readback |
> |---|---|---|---|
> | Journal, 3 entries | amounts changed | `CREATED=0 ALTERED=1` | new amounts |
> | Journal | one entry removed | `CREATED=0 ALTERED=1` | **2 entries; the removed one gone** |
> | Journal | an entry added back | `CREATED=0 ALTERED=1` | 3 entries |
> | Payment, 3 entries | amounts changed, then one entry removed | `CREATED=0 ALTERED=1` each | as sent |
> | Payment | date moved a day | `CREATED=0 ALTERED=1` | new date |
> | Payment, bare lowercase UUID `REMOTEID` | amount and date changed | `CREATED=0 ALTERED=1` | as sent, GUID unchanged |
> | Receipt | amount changed, then counterparty ledger changed | `CREATED=0 ALTERED=1` each | as sent |
> | Contra | amount changed | `CREATED=0 ALTERED=1` | as sent |
> | Payment | re-sent as a **Receipt** | `CREATED=0 ALTERED=1` | **now a Receipt** |
> | Receipt, 2 entries | a party entry added, then removed again | `CREATED=0 ALTERED=1` each | 3 entries, then **2; the removed one gone** |
> | Contra, 2 entries | an entry added, then one removed | `CREATED=0 ALTERED=1` each | 3 entries, then **2; the removed one gone** |
> | Payment, 2 entries | an expense entry added | `CREATED=0 ALTERED=1` | 3 entries, date kept |
>
> The Journal amount and entry-removal rows and the three-entry Payment row are bridge#429; the rest
> are follow-ups the same day, in which every other counter was zero, including `EXCEPTIONS`. The
> last three rows came from a second follow-up, which re-ran the unknown-ledger canary first; it still
> failed closed. **Contra's added entry was a second entry on a ledger the voucher already carried**,
> because the synthetic company holds only two bank ledgers absent from the client book loaded beside it. Adding
> an entry on a *different* ledger is observed for Journal, Receipt and Payment only. Date
> and voucher-type replacement were each observed on a Payment only. Exactly one voucher carried each marker
> afterwards and `ALTERID` advanced on every alteration. **Same object:** each voucher kept the GUID
> it was created with, and creations interleaved with the alterations took the next GUIDs, so no
> alteration allocated a new object.
>
> So the entry set is **replaced, not merged**, and so are date and even voucher type — which is
> itself a hazard: an upsert will silently turn a Payment into a Receipt. Bridge's amendment path
> (`src-tauri/src/agent_import_amend.rs`) relies on the replacement and refuses type changes.
>
> **Still not measured:** a file imported through Tally's own Import menu rather than the gateway;
> Gold, Education or any other release; bill allocations, inventory or tax entries; a voucher that
> is cancelled or optional; and an alteration made in Tally's UI between the two imports. The
> `REMOTEID` attribute on readback returned Tally's own `<company GUID>-<id>`, as recorded above,
> so the voucher is still located by its narration marker.

**Changed payloads are a separate, untested case.** Everything above is a *byte-identical* repeat.
`IMPLEMENTATION_GUIDE.md` §3.3a's own untested list includes "when the payload differs from the
original (partial update semantics)". So a corrected voucher re-sent under the same `REMOTEID` may
overwrite, may partially update, or may duplicate: **UNVERIFIED**. Do not prescribe re-import as a
correction path on that basis. Delete by `REMOTEID` and create afresh is the path with the
better evidence, but read §9.7's boundary before treating it as settled: the Delete row in that
matrix belongs to this document's **Edit Log 7.0 Educational** baseline (§0), which qualifies
nothing for licensed standard TallyPrime.

**But §9.12b does.** A one-sided invoice voucher was removed on a **licensed TallyPrime 7.1 Gold**
book with `ACTION="Delete"` keyed by the **client-supplied** `REMOTEID`, and a corrected voucher
created in its place. So the delete is confirmed on Gold for that voucher shape, and reading only
§9.7 here would give a Gold implementer the opposite of what this document already establishes
five sections later.

Scope it precisely rather than in either direction, and mind what §9.12b actually is:
**§9.12 is marked PARTIAL — a hand import through the desktop UI, which produces no gateway
response at all.** So §9.12b establishes a **stored-state** effect on licensed Gold for an invoice
voucher: the voucher was gone afterwards. It does not establish the gateway path, and by this
section's own requirement below it cannot — there is no delete response to show `DELETED=1` with
clean counters.

So the two measurements cover different halves and neither covers the third case:

- **Gateway, Edit Log 7.0 Educational — VERIFIED.** §9.7's Voucher/Delete cell is `DELETED=1`
  keyed by `REMOTEID`, read back and confirmed. That *is* the gateway path, and it is the working
  correction primitive for anyone targeting that environment.
- **Stored state, licensed 7.1 Gold — confirmed via the UI (§9.12b).** The voucher was gone
  afterwards. No gateway response exists to corroborate it.
- **Gateway on a licensed SKU — UNVERIFIED**, on Gold and on the Silver/Journal profile this
  section is about alike. §9.7 does not reach it because its baseline is Educational; §9.12b does
  not reach it because a UI import returns nothing.

Say which of the three you are standing on. "Delete works" is true in two of them and unproven in
the one a licensed integration actually runs in.

**Qualifying it on a new SKU or voucher type takes more than "read one voucher back".** A single
read cannot tell *the original is gone* from *my read did not cover it*: an incomplete, failed or
mis-scoped read looks exactly like a successful delete, and finding the **replacement** proves
nothing about the original. If the delete silently failed and the create succeeded, both vouchers
are in the book and a naive check qualifies the path for the next batch. What it takes:

- the delete's **own response**, showing `DELETED=1` with every failure counter zero — not the
  create's;
- a **company-pinned** read of the same date range before and after, complete enough that an empty
  result is distinguishable from an unfiltered one (read a range you know holds other vouchers, and
  confirm those still come back);
- the voucher count moving by exactly the expected amount across the pair;
- **the original's own identity absent from the after-read** — capture its `GUID` and `MASTERID`
  *before* sending the delete, and confirm those exact values are gone, not merely that one fewer
  voucher came back.

That last bullet is the one the others cannot cover, and it is the failure this procedure exists to
detect. **The thing being qualified is the selector.** If `REMOTEID` selected the wrong voucher,
the response still says `DELETED=1`, the count still falls by one, the read is still complete — and
the original is still in the book while the procedure records the selector as working. Every
count-based check is satisfied by *a* deletion; only an identity check is satisfied by *the right*
one.

The same reasoning rules out identifying the original by its date, amount or ledger set: a
destructive selector that hit a similar voucher passes that comparison too. Use the identity Tally
assigned.

**A resend after a person's change undoes that change** — **PARTIAL 2026-09-25; licensed TallyPrime
7.1 Silver, one synthetic book, gateway, one run of each, captures not committed.** Three Journals
were created with client `REMOTEID`s, then each was changed and resent with `ACTION="Create"`, the
same `REMOTEID` and a changed amount:

| Change before the resend | The change's own response | The resend's response and effect |
| --- | --- | --- |
| `ACTION="Cancel"` | `ALTERED=1`, `CANCELLED=0`; the voucher shows `ISCANCELLED` Yes, entries removed | `ALTERED=1`; the voucher is **un-cancelled** with its entries restored |
| upsert with `ISOPTIONAL` Yes | `ALTERED=1`; optional, and the voucher number moved | `ALTERED=1`; still optional (the omitted flag is kept), renumbered again |
| `ACTION="Delete"` by the `REMOTEID` | `DELETED=1` | `CREATED=1`; the voucher is **re-created** under a new GUID |

So a resend with a changed payload reverses a person's cancel or delete, and a delete leaves no
voucher to show that the `REMOTEID` was ever used: a book check cannot stand in for a record of
what was sent. A byte-identical resend after a cancel or delete was **not measured**, so it may not
be assumed safe either. Note also that a cancel reports `ALTERED`, not `CANCELLED`.

### 9.4 Master re-create is a silent Alter

**VERIFIED.** Re-sending an identical ledger `ACTION="Create"` returned `CREATED=0,
ALTERED=1` with no error — the existing master was **overwritten** with the retry payload.
The observed counters distinguish an alteration of an existing master from creation
of a new one. This experiment did not establish protection against a concurrent
foreign writer or recovery of an unobserved prior master.

The required implementation workflow is maintained in
[Implementation Guide §3.6](IMPLEMENTATION_GUIDE.md#36-master-re-create-is-a-silent-alter)
and `PROMPT_PLAYBOOK.md` Phase 4 step 3a. This section records the gateway observation;
it does not grant dispatch authority from a pre-read.

### 9.4a A partial ledger `Alter` preserves the omitted Party GSTIN

**VERIFIED (2026-08-28; one licensed TallyPrime Silver synthetic lab company).** A ledger was
created with `INCOMETAXNUMBER=ZZZZZ0000Z` and
`PARTYGSTIN=27ZZZZZ0000Z1Z5`. A subsequent `ACTION="Alter"` request identified that ledger
by its `NAME` attribute and carried only
`<INCOMETAXNUMBER>ZZZZZ0001Z</INCOMETAXNUMBER>`. It returned `ALTERED=1`, `ERRORS=0`,
`EXCEPTIONS=0`, and no `LINEERROR`. The readback then contained both the updated PAN and the
original Party GSTIN.

This is evidence for the observed `PARTYGSTIN` preservation behaviour only; it does **not**
establish that every omitted ledger field is preserved on every Tally version or SKU. The raw
synthetic read responses and the exact native master/balance/group responses are retained in
the repository's `master_fields_lab` fixtures; every request in the lab run was bracketed by a
200 `/status` response and every write was explicitly scoped to the lab company.

### 9.4b Master-name matching: case- and separator-insensitive, otherwise exact

**VERIFIED 2026-07-30** — recorded in `IMPLEMENTATION_GUIDE.md` §3.3a's sibling §3.3b since then,
and promoted here because it is observed gateway behaviour and this document is where behaviour
lives. Measured against a ledger named `BRIDGE-PROBE-LEDGER-A` and one named `ZZ Ram & Sons Pvt Ltd`:

| Supplied name | Result |
| --- | --- |
| exact | **matched** |
| lowercase | **matched** |
| trailing space | **matched** |
| `BRIDGE PROBE LEDGER A` (hyphens → spaces) | **matched** |
| `ZZ Ram AND Sons Pvt Ltd` (`AND` for `&`) | **rejected** |
| `ZZ Ram & Sons` (missing suffix word) | **rejected** |
| `ZZ Ram & Son Pvt Ltd` (singular for plural) | **rejected** |
| entirely different name | **rejected** |

Tally folds **ASCII case**, and accepted a **space supplied where the master carries a hyphen**. It
is otherwise **exact on letters**.

**That separator result is directional.** The measurement sent `BRIDGE PROBE LEDGER A` against a
master named `BRIDGE-PROBE-LEDGER-A`. The reverse — supplying `A-B` against a master named `A B` —
was never sent, and a fold treating the two as interchangeable would substitute a name Tally might
reject.

Stated that narrowly on purpose. "Normalises separators" reads as *separators generally*, and a
skimming implementer folds underscores, slashes and en dashes together — binding a voucher to the
wrong ledger. One separator was measured, in one direction. The table below marks every row.

> **RULE: wherever the question is "will Tally treat these as the same master?", ask an
> asymmetric predicate `accepts(candidate, tally_name)` — never string equality, never a looser
> fold, and never a canonical form.**

**Name the sides by where the name lives, not by which way it is travelling.** `tally_name` is the
spelling **Tally holds**; `candidate` is the other one, whatever its provenance. The substitution
below belongs on the `tally_name` side because *that is the side the measurement placed it on* —
not because that side happened to be "stored".

The distinction is load-bearing for every offline consumer. A binder holds a document name and a
catalogue name and asks which master the operator meant; it then writes the **catalogue's**
spelling, so Tally is never asked to match the document name at all. Nothing is supplied, and
nothing travels. Parameters named for the direction of travel give such a caller the right answer
for a reason that misrepresents why — and the next reader, with no travel direction to reason
from, swaps the arguments to whatever reads naturally and lands on the UNVERIFIED direction with
nothing to catch it. A rule that is accidentally correct for a class of consumer will eventually
be wrong for one of them. Named this way, the import case and the binder case are the same rule
rather than a rule and an analogy.

**Why not a canonical form.** `canon(x) == canon(y)` is symmetric by construction: it cannot hold
in one direction and not the other. The one separator result here *is* directional — a space was
supplied where the master carried a hyphen, and the reverse was never sent — so any canonical form
expressing it also asserts the direction that was not measured, and binds `A-B` to a master named
`A B` on no evidence. The table below marks that row UNVERIFIED and a canonical form quietly
overrides it.

**One measured transformation per alternative — never two at once.** The captures tested case,
one trailing space, and the hyphen separator in *separate* requests. A predicate that applies all
three and then compares once asserts their **combinations**, which were never sent: a lowercased,
space-substituted name with trailing whitespace is three untested steps deep. So each alternative
below transforms an otherwise untouched pair:

```text
accepts(candidate, tally_name):
    # `tally_name` is the spelling Tally holds. Each line is one measured
    # result. Do not compose them; do not add a line without a capture.
    return candidate == tally_name                                 # exact — VERIFIED
        or candidate == ascii_lower(tally_name)                    # candidate is the master lowercased — VERIFIED
        # NOT included: ascii_lower(candidate) == ascii_lower(tally_name).
        # That also accepts an UPPERCASE candidate against a lowercase master,
        # a direction never sent. See the third note below.
        or drop_one_trailing_space(candidate) == tally_name        # ONE trailing space — VERIFIED
        or candidate == tally_name.replace("-", " ")               # space for Tally's hyphen — VERIFIED
```

Three things this spelling is careful about, each of which was wrong in an earlier draft:

- **`drop_one_trailing_space`, not `rstrip(" ")`.** One trailing space was measured. `rstrip`
  removes every trailing space, so `accepts("A  ", "A")` becomes true on no evidence, and could
  bind a voucher to a master Tally would not have selected. Remove at most one.
- **The separator substitution is applied to `tally_name` only.** `tally_name="A-B"` accepts
  `candidate="A B"`; `tally_name="A B"` does **not** accept `candidate="A-B"`. That asymmetry is
  the entire point of the clause and is what a canonical form cannot express.
- **The case clause is directional, because the capture was.** The measurement sent a **lowercase**
  candidate against a master carrying uppercase. `ascii_lower(candidate) == ascii_lower(tally_name)`
  also accepts an **uppercase** candidate against a lowercase master, which was never sent — so the
  symmetric form asserts a second experiment, exactly as a canonical form does for the separator.
  An earlier draft admitted that in this note and left the symmetric clause in the predicate
  anyway; a qualification in the prose does not qualify the code beside it. Written as
  `candidate == ascii_lower(tally_name)`, the predicate now says only what was sent. Qualify it before
  building on it.

If a further direction is later measured, one clause is added and the table row changes. Until
then a directional predicate fails the way this section wants — it may refuse a pair Tally would
have accepted, which a human sees, rather than binding one Tally would reject.

**What a resolving fold may contain, and what it may not.** Only three transformations were
measured: ASCII case folding, **one** trailing space, and a hyphen matching a single space. A fold
is only as safe as its least-verified step, and every step beyond those three can merge names Tally
keeps apart — which posts to the wrong account, silently. Express them as the alternatives above
rather than as a canonical form: a fold that normalises first and compares once is symmetric, and
symmetry is exactly the property the separator result does not have.

| Transformation | State |
| --- | --- |
| ASCII case folding | **VERIFIED** — lowercase matched |
| supplying a **space** where the master has a **hyphen** | **VERIFIED** — `BRIDGE PROBE LEDGER A` matched `BRIDGE-PROBE-LEDGER-A` |
| supplying a **hyphen** where the master has a **space** | **UNVERIFIED here** — the reverse direction was never sent on this SKU. Measured **matched** on licensed 7.1 Silver, §9.4d |
| one trailing space ignored | **VERIFIED** |
| **two or more** trailing spaces ignored | **UNVERIFIED** — only one was sent |
| *leading* whitespace ignored | **UNVERIFIED here**. Measured **matched** on licensed 7.1 Silver, §9.4d |
| runs of internal whitespace collapsed to one | **UNVERIFIED here** — only a single space was tested. Measured **matched** on licensed 7.1 Silver, §9.4d |
| non-ASCII case folding (Devanagari, Tamil, Bengali, Turkish dotted I) | **UNVERIFIED** |
| **Unicode canonical equivalence (NFC/NFD)** | **MEASURED — folding it is wrong.** See below. |
| any other separator (underscore, en dash, `/`) treated as a space | **UNVERIFIED here**, and §9.4d splits it on licensed 7.1 Silver: `/` **matched**, underscore and en dash **rejected**. Not one row — do not fold them together |

**A wider result exists for a different SKU.** §9.4d re-ran this measurement on **licensed
TallyPrime 7.1** and found the gateway folds more than these rows establish. It is a separate
section on purpose: these rows are about Edit Log 7.0 Educational, and absorbing a licensed-Silver
result into them would silently widen the scope of a measurement nobody repeated here.

**The NFC/NFD row is the only one with evidence pointing the wrong way**, rather than no evidence
at all, and it is the one most likely to be folded in by accident.

`tally-matches-master-names-by-exact-codepoint` recorded the observation on
2026-08-19: a voucher naming a UI-created NFC ledger in its **canonically equivalent
NFD** spelling was
rejected — `EXCEPTIONS=1`, `LINEERROR` saying the ledger does not exist — while the NFC spelling
created it. A create with a programmatically-constructed NFD name returned `CREATED=1` and read
back with identical NFD codepoints, so storage is verbatim too. **The observed
instance matched these spellings by exact codepoints.** The checked-in encoding
provenance records only a TallyPrime EDU instance and date; it does not establish
release, port, or standard-versus-Edit-Log product identity for this observation.
Those classifications remain **UNVERIFIED**. This row therefore qualifies neither
§0's Edit Log 7.0 baseline nor any licensed SKU. A fold that normalises before comparing therefore resolves a name onto a master Tally
itself keeps apart — the precise failure this section exists to prevent.

**Why it needs saying twice.** This bug shipped, and the fold was then audited against this section
**twice** without anyone seeing it — `.nfc()` sat in the same expression both times. Canonical
equivalence reads as *decoding* rather than folding: the same characters, spelled two ways, nothing
an operator could type differently on purpose. So it never entered the audit as a row to check, and
every other row in this table is a **judgement** step — case, whitespace, separators. A reader
auditing a canonical form against a table of judgements finds nothing saying that normalising first
is a decision at all, concludes it is fine, and ships it.

> **A step that reads like decoding deserves the same evidence as a step that reads like folding.**
> Enumerate **every** operation in the comparison — normalisation, trimming, encoding conversion,
> case — not only the ones that look like judgements. The ones that look automatic are the ones
> that get audited by eye and missed.

Implementing only the verified rows fails in the direction that matters: it may *fail to match* a
pair Tally would accept, which surfaces as a refusal a human sees. Adding an UNVERIFIED row risks
the opposite — a silent match onto a different ledger — and adding the MEASURED row is known to
produce one. Qualify each independently, and note that the demo company this project reads carries
ledgers in three non-Latin scripts, so the non-ASCII case-folding row and the NFC/NFD row are both
reachable rather than theoretical.

> **RULE: prefer an exact spelling, and refuse an ambiguous fold. Never pick one.** The fold tells
> you which masters are *candidates*; it does not tell you which one Tally would choose, and one
> successful alternate-spelling experiment does not establish that a catalogue cannot hold both
> `A-B` and `A B`. Those collapse together here, and nothing measured says what happens then.

Bridge's own resolver encodes the **ambiguity discipline** to copy
(`src-tauri/src/agent.rs`): take the exact spelling if a candidate is exactly what was requested;
none is `ledger_not_found`; and **more than one is `ledger_ambiguous` — an error, not a choice.** A
comparison that returns the first match is the failure this rule exists to prevent.

**Copy its discipline, not its fold.** That resolver's `ledger_lookup_key` keeps only alphanumerics,
which is *looser* than anything §3.3b measured — it drops `&` outright, so `A & B` and `AB` share a
key.

Be exact about what was and was not tested there, because I was not. §3.3b sent
`ZZ Ram AND Sons Pvt Ltd` against `ZZ Ram & Sons Pvt Ltd` and it was **rejected** — that tested
*replacing* `&` with the letters `AND`. **Nobody has tested deleting `&`**, so whether Tally treats
`A & B` and `AB` as the same master is **UNVERIFIED**.

That cuts both ways and the rule below is written for it: a loose fold might merge masters Tally
keeps apart, or it might not, and neither is established. Resolving automatically on an untested
equivalence is the part that is unsafe — not the equivalence itself.

That loosening creates a hole the ambiguity rule cannot close, because **a sole candidate under a
loose fold is not a resolution.** Ask for `A & B` in a catalogue holding only `AB` and there is
exactly one candidate, no ambiguity to refuse, and the write goes to a master Tally would not have
matched. Uniqueness under a fold is only as meaningful as the fold.

> **RULE: resolve automatically only on an exact spelling, or on a fold no looser than §3.3b.** A
> looser fold may *suggest* — it is a good way to surface "did you mean?" — but its output is a
> candidate for a human to confirm, never a binding.

Both directions are live hazards, and they fail in opposite ways:

- **Too strict** (plain `==`) silently rejects a name Tally would have accepted. A binder that
  compared exactly refused **16 of 16** hyphenated masters on a real book, all of them near-misses
  it should have bound; and a tool comparing its suspense ledger exactly posted to suspense while
  reporting the row as resolved, dropping it from the very report it existed to appear in.
- **Too loose** (stripping every non-alphanumeric, say) may merge masters Tally keeps apart. The
  standing example is `A & B` against `AB` — and it is **hypothetical**: whether Tally treats those
  as one master is UNVERIFIED, for the reason two paragraphs above (§3.3b replaced `&` with `AND`
  and never tested deleting it). Stating it as fact here would make a resolver refuse, or demand
  confirmation for, a unique match Tally may well accept — the too-strict failure, arrived at
  through the too-loose warning.

  What *is* established is the shape of the risk, and it does not need the example to be true: a
  fold used for *lookup* may be looser than §3.3b deliberately, but it must then refuse an
  ambiguous result rather than pick one, and a sole candidate under a loose fold is not a
  resolution.

**Consequence for anything that generates a file.** Abbreviation, symbol expansion and
pluralisation are **not** normalised away: `AND` for `&`, a missing suffix word and a singular for a
plural are all rejected. Those have to be resolved *before* the file is generated — no amount of
comparison at write time recovers a name the operator shortened.

**Scope — and it is narrower than the promotion made it look.** This measurement is inherited from
`IMPLEMENTATION_GUIDE.md` §3.3b, dated 2026-07-30, which belongs to this document's §0 baseline:
**TallyPrime Edit Log 7.0 in Educational mode.** Not licensed, not standard TallyPrime. I first
wrote "one licensed instance" here, which would have let a reader treat master-name matching as
qualified on the SKU they are actually writing to.

So: **ledgers, on Edit Log 7.0 Educational. Licensed and standard TallyPrime are UNVERIFIED.**
Whether stock items, groups and voucher types match by the same rule is UNVERIFIED too, and §3.3b
says nothing about voucher numbers — a fold shared between master names and voucher numbers is
assuming something nobody has measured.

### 9.4d Master-name matching on **licensed** TallyPrime 7.1

**Superseded as generic binding authority, 2026-09-12.** The observations below remain
an exact record for their one licensed 7.1 instance, company, ledger class and import-time
operation. They do not authorize a scope-free `MasterCatalog` to bind a folded spelling:
that constructor carries none of the product, release, endpoint or approval information the
measurement requires. Generic binding therefore presents every folded result as a candidate
and requires operator selection plus exact revalidation; it must not treat these directional
observations as a symmetric, portable canonicalization rule.

**VERIFIED 2026-09-12**, and it widens §9.4b rather than confirming it. §9.4b is inherited from a
2026-07-30 measurement on **Edit Log 7.0 Educational** and marks licensed TallyPrime UNVERIFIED.
This is that measurement re-run on the SKU this project actually writes to: **TallyPrime 7.1,
licence tier silver, `education_mode=false`**, ledgers, one lab company.

**Method is §9.4b's own.** Import a voucher naming a folded spelling of a ledger that exists, and
let Tally answer: a created voucher means the name resolved, a `LINEERROR` naming that ledger
means it did not. Twelve variants in the first run and six more in the second described below, one
voucher each, then the **day book was read back** to record which master each voucher actually
posted against — the counters alone would not have said. Every created voucher was then deleted by
`REMOTEID` and the day read back empty (eight from the first run, two from the second).

| Supplied against a live master | Licensed 7.1 Silver | §9.4b on Educational |
| --- | --- | --- |
| exact | **matched** | matched |
| ASCII lowercase | **matched** | matched |
| one trailing space | **matched** | matched |
| a **space** where the master has a **hyphen** | **matched** | matched |
| a **hyphen** where the master has a **space** | **matched** | *UNVERIFIED* |
| leading whitespace | **matched** | *UNVERIFIED* |
| an internal whitespace run collapsed | **matched** | *UNVERIFIED* |
| a **slash** where the master has a **space** | **matched** | not sent |
| an **en dash** where the master has a space | **rejected** | *UNVERIFIED* |
| an **underscore** where the master has a space | **rejected** | *UNVERIFIED* |
| `AND` for `&` | **rejected** | rejected |
| a **missing** suffix word | **rejected** | rejected |
| an **added** suffix word | **rejected** | not sent |
| **NFD** against an NFC master | **rejected** | not sent |

**One row here was mislabelled and is corrected.** The first run of this probe recorded `AND` for
`&` as rejected, but what it actually sent was a name with `AND CO` **appended** — against a master
carrying no `&` at all. That measures an added suffix, not a substitution, and the label was wrong
even though the verdict happened to be. It was re-run against `Profit & Loss A/c`, a reserved
ledger present in every company:

| supplied against live `Profit & Loss A/c` | result |
| --- | --- |
| `Profit & Loss A/c` | **matched** — control |
| `profit & loss a/c` | **matched** — case folds on a name carrying `&` and `/` |
| `Profit AND Loss A/c` | **rejected** — the substitution, now measured here |
| `profit and loss a/c` | **rejected** |
| `Profit & Loss` | **rejected** — a missing suffix word |
| `Profit & Loss A/c AND CO` | **rejected** — an added suffix word, what the first run really sent |

So §9.4b's abbreviation findings hold on licensed 7.1 Silver as well, and this section now says which
of them it measured rather than which it meant to.

**Composition was measured separately, because twelve single-axis results do not license it.**
Each row above is **one** transformation away from exact, so together they say each transformation
works alone and nothing about applying several at once — which is exactly what any fold does. Two
reviewers raised that independently, and it was worth a second run rather than an argument. Eight
more variants, same method, same readback and deletion:

| supplied | axes stacked | result |
| --- | --- | --- |
| `MB PILOT ALPHA (5550001001)` | control | **matched** |
| `  mb pilot alpha (5550001001)  ` | case + leading + trailing | **matched** |
| `mb-pilot-alpha-(5550001001)` | case + hyphen-for-space | **matched** |
| `  mb-pilot-alpha-(5550001001)  ` | case + hyphen + leading + trailing | **matched** |
| `MB/PILOT  ALPHA (5550001001)` | slash + collapsed run | **matched** |
| `mb-pilot alpha/(5550001001)` | case + hyphen + slash, mixed in one name | **matched** |
| `  mb-pilot/alpha  (5550001001) ` | all five at once | **matched** |
| `  mb probe  ledger a ` against `MB-PROBE-LEDGER-A` | case + space-for-hyphen + surrounding + run | **matched** |

All eight posted against the intended master, confirmed by day-book readback.

**Superseded interpretation, 2026-09-12.** The rows above remain the scoped Silver 7.1
observations. They do **not** license a generic symmetric or canonical separator fold: the
measured slash direction is a slash in the supplied name reaching a space in the live master;
the reverse direction was not sent. A scope-free binder must therefore keep all folded spellings
candidate-only and require operator selection plus exact revalidation. The earlier statements
that `space`, `-`, and `/` are interchangeable, or that a canonical form is licensed, are
withdrawn as binding authority rather than erased from the probe history.

**What remains measured in this scope.** The listed forward slash-to-space case, the recorded
hyphen and whitespace cases, and the rejected en dash, underscore, abbreviation, suffix, and
NFD cases are observations of this one operation. They do not generalize across product, tier,
object class, direction, or caller.

> **RULE: use only the recorded directional alternatives; do not fold separators into a canonical
> form.** The slash result is candidate `/` against master space, not the reverse. An en dash and
> an underscore were rejected in their recorded directions, so a fold that treats punctuation or
> separators as a class is wider than the gateway and will merge masters it keeps apart.

That is the trap §9.4b warned about, arriving from the other side: the danger was never only that
a reader would fold too much, it was that "normalises separators" hides both the particular
substitutions and their directions. Nothing about a separator's appearance predicts which comparison
the gateway accepts.

**Canonical equivalence is still refused**, consistent with the exact-codepoint finding recorded
elsewhere in this document: an NFD spelling of an NFC ledger does not resolve. A fold that
normalises before comparing merges masters this gateway keeps apart.

**Scope.** One instance, one build, one licence tier, **ledgers only**, one company, and the
measurement is of *import-time* name resolution — not collection filters, not stock items, groups
or voucher types, and not voucher numbers. §9.4b's Educational scope stands as its own row; this
does not retire it, and where the two disagree they disagree about different SKUs rather than
about the same one.

**Fixtures.** `MB-PROBE-LEDGER-A` and `MB CAFÉ PROBE` remain in `BRIDGE CORPUS OPENING` under
`Suspense A/c`, carrying no balances, so this is repeatable. They post-date the catalogue digest
recorded in `TEST_CORPUS.md` §9.2.


### 9.4c Real catalogues carry families a partial name cannot separate

**VERIFIED 2026-09-10** for the counts, across 16 loaded companies on both lab instances; the rule
built on them is PARTIAL. `TEST_CORPUS.md` §9.1 carries the procedure, the per-company figures and
what they do not cover.

Live books name parties in **sequentially-numbered families** — one observed catalogue runs a single
prefix across more than a hundred ledgers that differ only in a trailing number. A source name that
is a truncation of one of them reaches the whole family and distinguishes no member of it.

**Why that is a protocol-level fact and not an implementation detail:** any client matching a
supplied name against a read catalogue meets it, and the tempting response — offer the first N and
let a human pick — is measured wrong. Listing an arbitrary capped slice of such a family **put the
intended master outside the offered list about a third of the time** (present in 65.6% of lists,
against 100% once families beyond the cap were withheld and counted instead).

> **RULE: where a supplied name reaches a family it does not separate, report the count and withhold
> the list. An arbitrary slice of a family is not a shortlist — it is a wrong answer that looks like
> a shortlist.**

The scope is narrow and matters: the catalogue side is live, and every *source* name in the
measurement is a fabricated mutation of a live name. It measures the rule against real naming
habits, not against real operator input.


### 9.4e When fold-equal ledgers coexist, an import binds the exact name

**VERIFIED 2026-09-26, licensed TallyPrime 7.1 Silver** (`education_mode=false`). One synthetic company, one run of each step. **Confidence: PARTIAL.**

§9.4b and §9.4d measure a folded spelling against **one** live ledger. This measures an import naming one of **two** ledgers whose names fold equal. The twins differed by a trailing CR LF (written as `&#13;&#10;`, §9.4d), and in one case also by ASCII case.

**Tally lets such twins coexist.** A gateway ledger `Create` for the second of each pair returned `CREATED 1`, `ALTERED 0`, three times:
- plain after the CR LF name;
- the CR LF name after plain;
- a lowercase CR LF name after an uppercase plain one.

A folded twin is therefore a second master, not a silent Alter of the first (contrast §9.4).

Every voucher below was imported as a file over the gateway, with no errors. The ledger it posted to was read back per ledger GUID.

| Order of creation | Name the voucher carried | Posted to |
| --- | --- | --- |
| CR LF ledger first, then plain | the CR LF name | the **CR LF** ledger |
| plain first, then the CR LF ledger | the plain name | the **plain** ledger |
| uppercase plain first, then lowercase CR LF | the uppercase plain name (a Payment) | the **uppercase plain** ledger |
| uppercase plain first, then lowercase CR LF | the lowercase CR LF name | the **lowercase CR LF** ledger, the one created **second** |

**When a ledger carries exactly the imported name, the import posts to it**, and creation order does not decide. In the first three rows the matched ledger was also the older twin. The last row names the newer one and still posts to the exact match. That voucher was a probe: a file built for the uppercase ledger, before the twin existed, with its one `LEDGERNAME` then changed to the lowercase CR LF name. `verify_import` against the built batch reported it `posted_divergent`, naming both ledgers.

**Not measured:**
- a name that matches **neither** twin exactly, only folding to both (e.g. mixed case without the CR LF): which twin Tally picks there is open;
- twins made or edited in Tally's own screens;
- Gold and Education.

Until the fold-only case is measured, bridge#708 has Bridge refuse to build or post against a ledger that has a folded twin (`ledger_has_folded_twin`).


#