# Tally XML gateway protocol reference — voucher writes

> This is a section of the canonical [Tally XML gateway protocol reference](./TALLY_PROTOCOL_REFERENCE.md). Read its confidence-marker convention and legacy-link note before relying on a finding.

---

## 9.5 Identity after write

**VERIFIED.** `LASTMID` is **0** on successful master creates, so this counter does not
identify the created master. `LASTVCHID` is populated for vouchers. Non-numeric
`LASTVCHID` text is also accepted without error when parsed back.

For the implementation's readback identity policy, see `IMPLEMENTATION_GUIDE.md` §3.5
and `PROMPT_PLAYBOOK.md` Phase 4 step 4. Their prescriptions are separate from this observation.

---

### 9.8 Voucher numbering method changes everything — **use Manual**

**VERIFIED.** The voucher type's numbering method silently determines both whether your
voucher number survives and how a failed Alter behaves.

| Numbering method | Your `<VOUCHERNUMBER>` | Failed Alter behaviour |
| --- | --- | --- |
| **Automatic** (`Auto Retain`) | **Discarded.** Tally assigns its own — we sent `BRIDGE-PROBE-VCH-001` and Tally stored `1` | **Silently creates a duplicate** (`CREATED=1`) |
| **Manual** + `PREVENTDUPLICATES=Yes` | **Preserved.** `BRIDGE-MAN-0001` stored verbatim | **Cleanly rejected** (`CREATED=0, ALTERED=0, EXCEPTIONS=1`) |

Two consequences, both significant:

1. **What was measured is the FAILED-ALTER column, and the consequences below are about that
   column.** §9.8 sent a failed `Alter`; it did not test a `Create` retry, a restart, or another
   voucher type, and its own rule below forbids carrying the observation to a different request
   identity mechanism. Read "idempotency" here as "this failure mode, under this setting".
   Voucher-number-based idempotency in that sense only works with Manual numbering. Under automatic
   numbering the client-supplied number is thrown away, so any dedupe key built on it is
   silently ineffective. This was not obvious — the create returned `CREATED=1, ERRORS=0`
   and looked entirely successful.
2. **Manual numbering converts a dangerous failure into a safe one.** The same failed Alter
   duplicates a client's voucher under automatic numbering and is rejected under manual.

> **RULE: a flow that relies on voucher numbers for identity or duplicate
> rejection requires Manual numbering with `PREVENTDUPLICATES=Yes`.** Do not
> apply the failed-`Alter` observation to a different request identity mechanism.

**Scope clarification — verified 2026-09-06, recorded 2026-09-07.** The licensed
Journal file workflow uses `ACTION="Create"` with a stable client `REMOTEID`, as
described in [Implementation Guide §3.3a](IMPLEMENTATION_GUIDE.md#33a-remoteid-is-the-idempotency-key--supersedes-34s-conclusion).
The measured file omitted `VOUCHERNUMBER`. Its first import returned
`CREATED=1, ALTERED=0`; importing the exact file again returned
`CREATED=0, ALTERED=1`. Readback retained one voucher with the same GUID, numeric
master ID, and assigned voucher number. The file SHA-256 was
`7c02fd1b598d70157fa676e5a41ae16184034d39169793d8554e31713cd0f127`.

This qualifies that exact-file repeat on the observed licensed Journal path. It
does not establish voucher-number-based identity, the configured numbering
method, other request shapes or voucher types, restart behavior, or universal
REMOTEID semantics.

**Automatic Journal dispatch — verified 2026-09-07, bounded observation.**
On the synthetic TallyPrime Silver 7.1 instance, a macOS MCP host obtained
explicit native approval and sent one saved Journal with `ACTION="Create"`
and its generated `REMOTEID`. The response reported `CREATED=1`, with all
other import counters zero. Independent effective-voucher readback matched
the date, ledger entries and amounts. A fresh MCP process then reconciled
the original batch: the voucher identity, number and AlterID were unchanged,
and the local journal still contained one dispatch intent and one response.
The observed executable SHA-256 was
`208e6c95fb5f2a18aba4592767f6eeee9121922e02bb55371792925d42114a54`.
This establishes that single dispatch and restart reconciliation; it does
not qualify interactive Windows approval or Gold/Education live posting.

Bridge can dispatch only a locally built, saved one-Journal batch on this
source-specific path: it binds the saved endpoint, requires an independent
native approval, records one durable attempt before sending, persists any
response, and reads the original batch back. It does not automatically retry.
A timeout, missing response, dirty counters, or incomplete readback stays with
the original saved batch for read-only reconciliation. This operational guard
is not evidence for a different product, licence mode, voucher type, endpoint,
or request shape. A mandatory manual-numbering preflight would require a
separately observed voucher-type read contract; it cannot be inferred from the
failed-`Alter` case.

**Native mutation selector — verified 2026-09-07, bounded fresh-dispatch observation.**
A new native attempt uses a fresh private `REMOTEID`, separate from the selected
public file's identity. The narration keeps the original batch attribution.
The durable dispatch intent binds the exact UTF-16LE native request SHA-256 before
sending; the response must match that commitment. Older intents without this
field remain readable and are never resent. This prevents native posting from
reusing the public file's mutation selector after a manual import. It does not
prove semantic absence after arbitrary edits to an earlier business event.
Native posting refuses a supplied `VOUCHERNUMBER` until its matching precedence
is qualified.

A fresh synthetic Silver 7.1 Journal using this separate selector and attribution
returned `CREATED=1`, all other counters zero, and explicit zero exceptions.
Independent readback matched its original batch attribution, date and balanced
entries. Restarting with both generation and posting disabled reconciled the same
GUID, master ID, voucher number and AlterID with one intent and one response.
The original earlier Journal was unchanged in before/after readback. The executable
SHA-256 was `c2df9ffd76bf687e50b9b78916cb83fdacc6cddde2f317b53c46f538f1b2a96f`;
the durable native request commitment matched the actual transport request hash.
This qualifies a fresh dispatch and read-only restart for that binary and source.
**Preservation follow-up — verified 2026-09-08, bounded observation.** The
macOS desktop at `f1ffcc4`, using shared posting code at `7c3266f`, posted one
human-approved synthetic Journal in the quiet-company workflow. Its signed
executable SHA-256 was
`078e6bfe95fdd57f8f5ef905167b49a509bb0f7a3d3dd94734ae9aadc36c32ca`.
Readback found exactly one new Journal and all six prior accounting records
unchanged, including the manually edited public-file comparison voucher.
The original selected XML stayed unchanged. The saved response included all
seven counter-presence flags: one create and all other counters zero. A fresh
`7c3266f` MCP process with writes disabled reconciled the original batch without
resend; history retained one intent and one response. This closes that measured
preservation/recovery comparison, not arbitrary concurrent editing or other
platform, product-mode, or voucher-type qualification.

**Batch identity qualification — verified 2026-09-06, recorded 2026-09-07.**
A new synthetic Journal reused the earlier caller transaction label in a separate
local batch. Its `REMOTEID` and narration marker used the same batch-derived UUID;
the caller label was not the wire key. The first exact-file import returned
`CREATED=1, ALTERED=0`; its repeat returned `CREATED=0, ALTERED=1`. Both had zero
errors, exceptions and deletions. Readback retained new master ID 5, voucher number
2, and the same GUID while AlterID advanced from 9 to 10. The old voucher's GUID,
master ID 4, number 1 and AlterID 8 were unchanged in immediate before/after reads.
The new file SHA-256 was
`e39eb3c0bfe53144bdd9c0f4afcb88c3d63a2050214233ee77465d42a54245ef`.
The unchanged UTF-16LE readback is retained as
`native-namespaced-journal.utf16le.xml` in the protocol fixture tree, with capture
metadata. This is a Silver 7.1 Journal observation; it does not qualify other
profiles or deduplication after losing/rebuilding a batch.

Creating such a type over XML works: `<VOUCHERTYPE ACTION="Create">` with
`<NUMBERINGMETHOD>Manual</NUMBERINGMETHOD>` and `<PREVENTDUPLICATES>Yes</PREVENTDUPLICATES>`
returned `CREATED=1`.

Note also that `EXCEPTIONS=1` arrived with **no `LINEERROR`** — a parser must treat a
non-zero `EXCEPTIONS` as failure on its own, without waiting for an error string.

### 9.13 Payment, Receipt and Contra — the bank-statement voucher shapes

**VERIFIED 2026-09-10 (licensed TallyPrime 7.1 Gold; five files imported by hand through
Gateway of Tally → Import → Vouchers).** **157 vouchers** in total, counted from the retained
files themselves: **147** of the three bank types across four files — a 1-voucher pilot, 61
Payments with 54 Receipts, 3 Contras, and 20 Payments with 8 Receipts — plus **10** reallocation
Journals in a fifth. `CREATED` equalled the voucher count on every file with zero errors and zero
exceptions, and each affected bank ledger reproduced, on readback, the debit total, credit total
and closing balance its own statement printed.

*(The session record headlined 148. That figure does not reconcile with its own per-file table
or with the artifacts, both of which give 157; the count above is taken from the files.)*

**A bank statement cannot be expressed as Journals.** Booking bank lines as Journals reconciles
arithmetically and misfiles every one of them: wrong voucher register, wrong day book grouping,
and visibly unlike the book's existing entries. The type decides which side holds the money:

| statement line | voucher | entries |
| --- | --- | --- |
| withdrawal | **Payment** | Dr party, Cr bank |
| deposit | **Receipt** | Dr bank, Cr party |
| own-account or cash movement | **Contra** | both legs cash/bank |

The imported element shape, per voucher:

```xml
<VOUCHER VCHTYPE="Payment" ACTION="Create" OBJVIEW="Accounting Voucher View" REMOTEID="...">
  <DATE>20260801</DATE>
  <EFFECTIVEDATE>20260801</EFFECTIVEDATE>
  <VOUCHERTYPENAME>Payment</VOUCHERTYPENAME>
  <PARTYLEDGERNAME>...</PARTYLEDGERNAME>       <!-- omitted on Contra -->
  <NARRATION>...</NARRATION>
  <ALLLEDGERENTRIES.LIST>
    <LEDGERNAME>...</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-100000.00</AMOUNT>
  </ALLLEDGERENTRIES.LIST>
  ...
</VOUCHER>
```

Four properties of it are not guessable, and each was measured:

1. **A debit is `ISDEEMEDPOSITIVE Yes` with a NEGATIVE `AMOUNT`.** The flag and the sign say the
   same thing and must agree; they are not independent fields.
2. **`EFFECTIVEDATE` accompanied `DATE` on every voucher**, always equal to it. Whether these
   types import without it was not tested, so omitting it is outside this measurement.
3. **No `<VOUCHERNUMBER>`.** These types numbered automatically in the observed book, and §9.8
   established that automatic numbering discards a supplied number in silence. Tally assigned
   its own. The bank's reference goes in the narration, which survives.
4. **`REMOTEID` on every voucher.** Delete by the client-supplied `REMOTEID` is the correction
   path, and §9.12b confirms it on a live book — the value you sent stays addressable as a delete
   key even though the export shows Tally's own. Use Delete + Create because it is the confirmed
   path, not because `Alter` is known to fail: §9.12b is explicit that `Alter` is unverified on a
   licensed profile rather than ruled out. A batch imported without a `REMOTEID` has no correction
   path at all, which is the point of sending one.

   **But the readback does not echo it in that attribute, and that is a trap.** A `Voucher`
   collection returns a `REMOTEID` attribute holding *Tally's own* `<company GUID>-<master id>`
   identifier, not the value the client sent. From the committed live capture
   `fixtures/agent/native-namespaced-journal.utf16le.xml`, a readback of a voucher Bridge
   imported:

   ```
   REMOTEID attribute : <TALLY-COMPANY-GUID>-<MASTER-ID>
   NARRATION          : ... [BRIDGE:<CLIENT-BATCH-KEY>]
   ```

   The values above are redacted illustrative placeholders; they preserve the observed distinction between Tally's company-GUID/master-id attribute and the client batch key in narration.

   **The client key is not absent from the response — it is in the narration.** §9.8 records
   that this batch used the same batch-derived UUID for its `REMOTEID` and its narration marker,
   and the marker came back intact while the attribute did not. The rule is field-specific and
   more useful stated that way: **Tally overwrites the attribute it owns, and preserves a marker
   placed in a field it does not.** That is why Bridge attributes readback by the narration tag —
   a deliberate choice of a durable carrier, not a workaround for a missing one.

   A verifier that compares the observed attribute against the value it sent therefore refuses
   every legitimate readback. The client value is still *stored*, still matches for a
   byte-identical repeat (§9.3), and still deletes (§9.12b); it is only unreadable through this
   attribute.

§9.1b applies unchanged and bites hardest here: a single unescaped `&` in a counterparty name
rejects the whole file with no field hint.

**Correcting a posted batch — reallocation Journals, not delete-and-recreate.** Lines whose
counterparty could not be identified were booked against a suspense ledger and corrected later
by a Journal moving the amount off suspense onto the real ledger. That leaves the bank side
untouched, keeps the correction auditable, and sidesteps the no-Alter restriction entirely; 10
such Journals were verified the same day.

**Naming the company does not aim the write.** The generated envelope carries
`<SVCURRENTCOMPANY>`, and §9.11d records a *verified* case of a mismatched value posting into the
**loaded** company with `CREATED=1, ERRORS=0, EXCEPTIONS=0`. **Which** mismatches behave that way
is UNVERIFIED: a separate measurement had an existing-but-unloaded name fail closed, but the
silent case's instance was never enumerated, so "the name matched nothing" is an inference from
its shape. Do not reason about which *kind* of wrong name is dangerous — there is a verified
silent case and no rule saying when it applies. Bridge cannot guard a hand import it never sees,
so the check is the operator's: immediately before importing, read the company and compare the
**complete identity tuple** `(canonical_origin, COMPANYNUMBER, GUID, NAME, BOOKSFROM)` — not the
GUID alone, because §9.11b is verified that a year-end split gives the child its parent's GUID.
`verify_import` takes a GUID as its argument but already enforces the rest: it refuses with
`company_identity_mismatch` unless the name, GUID, company number and books-from recorded at
build time all still match what the instance reports, so a readback aimed at a split sibling
fails closed. Per §9.11d, neither check closes the window between the check and the send; it is
narrowed only by running on an instance where no other company is loaded.

**Scope and limits.** One company, one build, two statement layouts, and files imported through
the UI rather than dispatched by Bridge. Bill-wise allocation was never exercised — every party
amount landed On Account, which is **not** established as correct for a book that reconciles
bills. This qualifies the three file shapes. It does not qualify a Bridge dispatch of them,
which remains one unnumbered Journal (§9.8).

**Which mixes are qualified.** One file may carry more than one voucher type — two of the
measured files did, 61 Payments with 54 Receipts and 20 with 8, both importing clean. The three
Contras and the ten reallocation Journals each went in on their own file, so:

| file contents | basis |
| --- | --- |
| Payment + Receipt | **observed** |
| Contra alongside either | **inferred** — Contra renders a strict subset of the Payment shape (same envelope and elements, minus the party), and a statement carrying a transfer line is the ordinary composition |
| Journal alongside any of the three | **refused** — a Journal renders no `EFFECTIVEDATE`, names no party, may carry a number and a reference, and comes from §9.8's separate lineage. Holding both citations is not evidence for their union |

**What Bridge builds from it.** `build_import_xml` renders exactly this shape for Payment,
Receipt and Contra, and leaves the Journal shape byte-identical to the file §9.8's own
measurement ran on. Each of the three is admitted with two or more entries, at least one debit
and one credit, no ledger on both sides, and every leg classified (bridge#466: the multi-entry
rule and party choice are the owner's decisions of 2026-09-22; one Bridge-built three-entry Receipt
was imported over the gateway and verified live, and Tally read the bank ledger back as its party,
but no multi-entry Payment or Contra has been, and none of the three, including that Receipt, through Tally's Import menu), carrying neither a supplied voucher number nor a `REFERENCE`: no file carrying either has been
imported and read back on these types, and `verify_import` compares accounting entries rather
than those annotations, so nothing downstream would notice Tally dropping or rewriting one.

Both legs are classified, not just the funding one:

| leg | requirement | refused by |
| --- | --- | --- |
| Payment credit, Receipt debit, both Contra legs | must be a **admitted** money group | anything else, **including "could not be established"** — a positive fact is required and absent |
| the counterparty leg of a Payment or Receipt | must be **established as holding no money** | any known money group, **and "could not be established"** |

Both legs need a positive fact; they differ only in which one. An earlier version made the
counterparty rule the looser of the two — refusing only a leg established *as* money — on the
reasoning that an unclassifiable counterparty is not evidence of a disguised Contra. Both halves
of that were weaker than they sounded. An ordinary party never lands unclassified: one under
`Sundry Debtors` resolves directly, and one under a user-created group walks up to its reserved
ancestor, so only anomalies reach that state. And the consequences are not symmetric — a
misjudged money leg makes Tally reject the import, which is loud, while a misjudged counterparty
files a Contra into the Payment register, which is silent and found later. The silent failure
earns the stricter rule.

The two columns also ask different questions of the same group, and that gap matters.

**Admitted** means a captured ledger was observed sitting under a captured group — the whole
edge the classifier walks, not just its far end. A group row proves the identity exists; it does
not show a ledger's `PARENT` resolving to it. Three identities clear that bar: `Bank Accounts`,
`Cash-in-Hand` and `Bank OD A/c`.

**Known money** is wider, and covers one more:

| identity | group row captured | ledger under it captured | admitted |
| --- | --- | --- | --- |
| `Bank Accounts`, `Cash-in-Hand` | yes | yes | yes |
| `Bank OD A/c` | yes | yes — 2026-09-17 | yes |
| `Bank OCC A/c` | **no** | no | no |

> **Scoped correction, 2026-09-17 — `Bank OD A/c` is admitted.** This row used to read "ledger
> under it captured: **no**", and an overdraft or cash-credit book could not be imported through
> Bridge. The promoting read was taken from the synthetic `BRIDGE SHAPE LAB` on licensed TallyPrime
> 7.1 Silver: the `StandardLedgerCatalogV1` request and the native `List of Groups` request the
> build renders, each sent twice with byte-identical responses. The catalogue carries `HDFC CC`
> with `PARENT` `Bank OD A/c`, and the group collection carries that group with `RESERVEDNAME`
> `Bank OD A/c`. Both are committed as
> `tests/fixtures/agent/native-shape-lab-{ledger-catalogue,groups}.utf16le.xml`. One company, one
> release: a book whose overdraft ledger sits under a user group beneath `Bank OD A/c` walks the same
> ancestry and is classified the same way, but that shape was not itself captured.

A ledger under the unadmitted identity is refused on a money leg for want of an observed edge,
and refused on a counterparty leg because it plainly holds money. Both refusals are the same
ignorance pointed in the safe direction; reading "not admitted" as "not money" would wave through
exactly the bank-to-bank Payment the counterparty rule exists to catch.

Classification walks the ledger's group ancestry through `RESERVEDNAME` per §8.2b, so a renamed
predefined group still classifies. A book whose money ledger sits under a group the Group
collection does not carry at all is refused the same way.

**What that check does not cover**, written here because a passing verdict invites being read
for more than it proves:

- It is a **group** check, not a ledger-suitability check. A bill-wise party, a foreign-currency
  bank account and a ledger requiring cost-centre allocation all classify identically to one
  needing none of that. It answers where the ledger sits, not whether Tally can use it here.
- It is true **as of the read**. The build reads the masters twice and refuses if they moved,
  which establishes stability across the build and nothing after it. The file is imported by
  hand later and Bridge never observes that import, so a ledger regrouped in between — an
  ordinary operation — leaves a stale verdict with no later gate. `verify_import` compares
  entries and would not notice a party that has since become a bank ledger.
- An **incomplete read refuses rather than admits**: a group missing from a truncated collection
  reads as unresolvable ancestry. The failure mode of a partial read is a rejected batch, never
  an accepted one.

**What it still does not do.** Two gaps, both stated here rather than left to be discovered.

Nothing detects a party ledger configured for bill-wise accounting: the catalogue Bridge reads
carries no such flag, and adding one would mean authoring a request shape with no live capture
behind it. Every party amount therefore lands On Account, exactly as the measured import did,
and every build naming a counterparty says so in its warnings.

**Two written elements are not verified: `EFFECTIVEDATE` and `PARTYLEDGERNAME`.** *(Written before
#467 and #469. `EFFECTIVEDATE` is now verified and `PARTYLEDGERNAME` is still not; see the two
scoped corrections below.)* The
verification collection of §9.8 fetches neither, so `verify_import` compares the date, voucher
type and signed entries and cannot see whether Tally kept, rewrote or dropped either — nor
whether an operator later edited them. A readback with a wrong effective date, or a party
silently dropped, still reports `posted_verified`.

The two are not equally unknown, and the difference decides how to close them:

| element | is it returned by a voucher collection? |
| --- | --- |
| `PARTYLEDGERNAME` | **yes, observed** — a captured `Sales` readback carries it populated. Whether the three bank types echo it is not observed |
| `EFFECTIVEDATE` | **unobserved** — no captured response in this tree carries it |

Neither may be fetched and *required* on that basis alone. Requiring an element that a
response does not return refuses every legitimate verification, which is a worse failure than
the one it guards — the same trap as comparing `REMOTEID` above, where the field comes back
carrying Tally's value rather than the client's. Both close with one live read that adds them to
the `FETCH` list and looks at what arrives.

> **Scoped correction, 2026-09-17 — both were read back, and they differ.** The table above
> predates this read. Licensed TallyPrime 7.1 Silver, synthetic `BRIDGE SHAPE LAB`: the §9.8
> verification collection's request, with `EFFECTIVEDATE` and `PARTYLEDGERNAME` appended to its
> `FETCH`, sent once over `20250422..20250423`. The window held six vouchers. Three were written by
> earlier lab runs with narrations of their own (a Contra, a Journal and a Sales voucher). Their
> written values were not recorded here, so they are not tabulated, but each also returned
> `EFFECTIVEDATE` equal to `DATE`. The three Bridge-built vouchers, found by their `[BRIDGE:…]`
> markers, are the rows below. The Receipt and Payment were built by
> `bridge_mcp`, imported from its file and amended from a Bridge-built file; the Payment was last
> re-imported from a copy of that file with only its amount edited. The Contra was built by
> `bridge_mcp` and imported from its file. Every file wrote the `PARTYLEDGERNAME` and
> `EFFECTIVEDATE` shown here:
>
> | voucher | written `DATE` / `EFFECTIVEDATE` | returned `EFFECTIVEDATE` | written `PARTYLEDGERNAME` | returned `PARTYLEDGERNAME` |
> | --- | --- | --- | --- | --- |
> | Receipt | `20250422` / `20250422` | `20250422`, `TYPE="Date"` | the counterparty (`Shape Buyer 5`) | **the bank ledger** (`Bank of Baroda CA`) |
> | Payment | `20250422` / `20250422` | `20250422`, `TYPE="Date"` | the counterparty (`Power Charges`) | **the bank ledger** (`Bank of Baroda CA`) |
> | Contra | `20250423` / `20250423` | `20250423`, `TYPE="Date"` | none written | the debit bank ledger (`HDFC CC`) |
>
> - **`EFFECTIVEDATE` is returned, equal to `DATE`**, on all three types. Comparing it is now
>   possible. `verify_import` did not fetch it when this read was taken; #469 closed that, see the
>   next correction.
> - **`PARTYLEDGERNAME` is returned but does not echo what was written.** On these vouchers it held
>   a cash or bank ledger that was on the voucher, not the counterparty Bridge wrote. **Do not
>   compare it with the written value:** that comparison would refuse every one of these legitimate
>   vouchers, which is the `REMOTEID` trap again. Whether a readback can detect a silently dropped
>   counterparty is still open. The entries comparison already requires the counterparty's ledger
>   among the signed entries.
>
> One read, one company, one release, three Bridge-built vouchers in a six-voucher window. It does not establish
> what a Tally UI edit to either field returns.

> **Scoped correction, 2026-09-17 — `EFFECTIVEDATE` is now verified (#469).** A live read
> (#467: licensed 7.1 Silver, synthetic lab) returned `EFFECTIVEDATE` with `TYPE="Date"`, equal
> to `DATE`, on a Bridge-built Receipt, Payment and Contra. The committed capture
> `native-three-vouchers.utf16le.xml` also carries it on two of its three vouchers. The §9.8
> verification read now appends `EFFECTIVEDATE` to its `FETCH`. For Payment, Receipt and Contra, a
> returned value that differs from the written date is an `effective_date` diff, in `verify_import`
> and in the amendment compare-and-swap. An absent or empty element is **not** a diff, because
> requiring it would refuse every verification on a release that does not return it; the row
> reports `"not_observed": ["effective_date"]` instead. A Journal is written without the element
> and is neither compared nor flagged. The public voucher tools do not fetch it or return it.
> `PARTYLEDGERNAME` stays unfetched: on the same read it held a bank ledger, not the written
> counterparty.

### 9.9 Bulk import throughput

**VERIFIED.** One import request may carry many `<VOUCHER>` elements; the counters aggregate.

| Objects per request | Elapsed | Rate |
| --- | --- | --- |
| 1 | 0.53 s | 2/s |
| 5 | 0.57 s | 9/s |
| 25 | 0.83 s | 30/s |
| 100 | 2.08 s | 48/s |

Per-request overhead dominates at small batch sizes. Extrapolating, 10,000 vouchers is
roughly 3–4 minutes and 100,000 roughly 35 minutes — bulk test-corpus generation is
practical.

**This is for test-data generation only.** Bridge's production write path remains
batch-of-one, because import counters are unattributable at N>1: a request carrying 100
objects that returns `CREATED=99` gives no way to identify which one failed.

### 9.7 Operation support matrix — **vouchers cannot be modified**

> **Qualified by §9.14 and §9.5 (2026-09-25):** on licensed TallyPrime 7.1 Silver, a same-`REMOTEID`
> `ACTION="Create"` alters a voucher in place (`ALTERED=1`, same GUID), and `ACTION="Cancel"` with
> the full body cancelled in place (observed once). The matrix below records the earlier instance.

**VERIFIED.** Every cell tested directly; "verified" below means the resulting state was read
back and confirmed, not merely that a counter moved.

| Object | Create | Alter | Cancel | Delete |
| --- | --- | --- | --- | --- |
| **Master** (`Ledger`) | ✓ `CREATED=1` | ✓ `ALTERED=1` — keyed by **name**; parent change confirmed by readback | n/a | ✓ `DELETED=1` — keyed by name; absence confirmed by readback |
| **Voucher** | ✓ `CREATED=1` | ✗ **`CREATED=1`** — a duplicate is made, target untouched | ✗ **`CREATED=1`** — a new cancelled voucher is made, target untouched | ✓ `DELETED=1` — keyed by `REMOTEID` |

**Voucher Alter was tested with four different keys** — `MASTERID` element, `GUID` element,
`REMOTEID` element, and `REMOTEID` attribute combined with `MASTERID`. All four returned
`CREATED=1, ALTERED=0`. Each produced a duplicate voucher.

`REMOTEID` *is* a valid match key — Delete succeeds with it. So the failure is specific to
the Alter and Cancel operations, not to identification.

**Tally can report a failed match correctly** when it chooses to: deleting a non-existent
master returned `ERRORS=1` with `LINEERROR="Item does not exist!"`. Voucher Alter and Cancel
do not take that path; they create.

**Consequences for the plan's write design:**

1. **§3.1.2's ruling that Cancel is the compensation primitive is inverted by the evidence.**
   Cancel does not work; Delete does.
2. **The "Alter-by-GUID with Cancel+Create fallback saga" has no working leg** on this
   instance. Neither Alter nor Cancel functions.
3. The only working voucher-correction path here is **Delete + Create** — precisely what the
   plan sought to avoid, because it destroys the original rather than superseding it.
4. Masters behave as the plan assumes: name-keyed, alterable, deletable.

**UNVERIFIED and now critical:** whether this is Education-mode behaviour, Edit Log SKU
behaviour, or general. A SKU whose entire purpose is an immutable audit trail plausibly
refuses XML-driven voucher alteration by design. **Qualifying Alter/Cancel on licensed
standard TallyPrime is now a Phase 4 gate, not a nicety.**

**Every one of these failures is caught by the §9.2 rule** and by nothing weaker: the
intended counter (`ALTERED`, `CANCELLED`) never increments, while `ERRORS` and `EXCEPTIONS`
stay at zero. That rule has now caught three distinct silent-failure modes.

### 9.6 `ACTION="Cancel"` by `REMOTEID` creates a new voucher — it does not cancel — **TRAP**

> **Qualified by §9.14 (2026-09-25):** on licensed TallyPrime 7.1 Silver, `ACTION="Cancel"` with
> the full voucher body cancelled in place (`ALTERED=1`, `CANCELLED=0`; observed once). The result
> below is from an earlier Education/Edit Log instance with a different request.

**VERIFIED.** A Cancel request naming an existing voucher's `REMOTEID` returned
`CREATED=1, CANCELLED=0, ERRORS=0, EXCEPTIONS=0` and **created a new voucher** carrying
`ISCANCELLED=Yes`. The targeted voucher was untouched — same `AlterID`, still
`ISCANCELLED=No`.

Two consequences:

1. **The correct identifier or request shape for Cancel is not yet established.** The plan
   designates Cancel as the write-path compensation primitive; until this is solved, Cancel
   cannot be relied on for that role. A compensation that silently *creates* data is worse
   than no compensation.
2. **This validates the §9.2 success rule.** A check of `ERRORS==0 && EXCEPTIONS==0` would
   report success. Only the "intended counter incremented" clause catches it, because
   `CANCELLED` stayed at 0 while `CREATED` moved.

**UNVERIFIED:** whether `ACTION="Cancel"` works when keyed by `GUID`, `MASTERID`, or with a
fuller voucher body. Not yet tested.

**Lead found 2026-07-30.** Tally's official Sample XML page
(`help.tallysolutions.com/sample-xml/`) publishes canonical samples for **"Voucher
Alteration"** and **"Voucher Cancellation"** among its 20 request examples. Our failing
attempts were hand-built. **Before concluding Alter/Cancel is broken on this SKU, retry using
the official sample shapes verbatim** — the §9.5/§9.6 failures may be a request-shape defect
on our side rather than an Edit Log restriction. This is the cheapest next step on the write
path and it should be taken before the Phase 4 licensed-Tally gate.

### 9.14 A `REMOTEID` upsert re-states the voucher: cancel, optional, delete and recreate

**Scope of every item below.** TallyPrime 7.1 Silver, licensed, not Education, on a synthetic
INR-only company. Journal vouchers from one gateway batch, with Automatic numbering (Auto
Retain, duplicates not prevented). Path: raw gateway requests (G), not Bridge's post path. **One
run per item, one voucher each (2026-09-25); per P5 an anomaly is repeated before it is believed, so
each is PARTIAL until it is.** Counters were read from the saved responses and state
from a Voucher collection readback, not from the counters alone.

- **`ACTION="Cancel"` with the full voucher body cancels in place. PARTIAL — observed once (G).**
  - The response reported `ALTERED=1` and `CANCELLED=0`, with all other counters zero.
  - The readback showed the same GUID with `ISCANCELLED=Yes` and its entries gone: one empty entry list, with no ledger and no amount.
  - On this run the `CANCELLED` counter stayed 0; when, or whether, it is ever non-zero is unmeasured. A success rule keyed on it (§9.2) would have called this cancel a failure. Only the readback showed it took effect, so key a cancel on `ALTERED` and the readback.
  - This differs from §9.6, which was measured on an earlier Education/Edit Log instance with a hand-built request. Neither the instance nor the request shape is held constant between the two, so which one causes the difference is **UNVERIFIED**.
- **An upsert (`ACTION="Create"`, same `REMOTEID`) onto a cancelled voucher un-cancels it and restores its entries. PARTIAL — observed once (G).**
  - The response reported `ALTERED=1`.
  - The readback showed the same GUID and number, `ISCANCELLED=No`, and both legs carrying the upserted amounts.
- **`ISOPTIONAL=Yes` by upsert makes the voucher optional and renumbers it. PARTIAL — observed once (G); where the numbers come from is not established.**
  - The response reported `ALTERED=1`, and the voucher number moved from 5 to 815.
  - A later upsert that omitted `ISOPTIONAL` left the voucher optional (omitted fields merge, §9.14 last item) and moved its number from 815 to 816.
  - Where 815 and 816 come from is not established, and the renumbering was seen only under Automatic numbering with Auto Retain. The number cannot be relied on to identify a voucher across an optional change.
- **`ACTION="Delete"` by `REMOTEID` deletes, and an upsert of the same `REMOTEID` then creates a new voucher. PARTIAL — observed once (G).**
  - The delete reported `DELETED=1`, and the voucher was absent from the readback.
  - The upsert reported `CREATED=1`, with a **new GUID**, a **new MASTERID** (7 before, 855 after) and the number 815.
  - A resend therefore undoes a delete, and neither the GUID nor the MASTERID survives it. Anything keyed on either (a baseline, a binding) must treat the re-created voucher as new. The REMOTEID is the only link, and after a delete that link re-creates the voucher rather than restoring it.
  - **Current behaviour:** Bridge's native post sends a fresh random REMOTEID for every post and records it with the dispatch intent (#582), so it never resends one.
  - **Design consequence, not current behaviour:** a batch-posting design must refuse to resend a REMOTEID Bridge has already sent, because a resend after a delete re-creates the voucher under a new GUID.
- **Manual numbering with duplicates prevented refuses, and two of the three refusals are silent. PARTIAL — observed once each (G).** The same Journal type was switched to Manual with Prevent Duplicates, then restored.
  - An upsert (same `REMOTEID`) with no `VOUCHERNUMBER` reported `ERRORS=1` with `LINEERROR` "Voucher Number cannot be left BLANK!", and no create or alter counter moved.
  - A new voucher with no number reported `EXCEPTIONS=1` with **no `LINEERROR`**, and no create counter moved.
  - A second new voucher reusing a number reported `EXCEPTIONS=1` with **no `LINEERROR`**, and a readback held one voucher under that number.
  - An `EXCEPTIONS` count without any line error is the case §9.2's four-part success rule exists for: only the counters show the refusal.
- **Related, same block:**
  - An upsert omitting `REFERENCE` kept the stored value: omitted fields merge. PARTIAL — observed once (G).
  - Each upsert moved the voucher's ALTERID to the book's next mark. PARTIAL — observed across several upserts on one book (G).

**Not measured here:** other voucher types, Gold concurrency, an edit in Tally's
own screens, and repeatability beyond one run. A masters delete in the same session drew no
response, and its cause is **UNVERIFIED**; nothing is recorded about it here.
