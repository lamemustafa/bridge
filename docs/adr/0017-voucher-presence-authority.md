# ADR 0017: Voucher presence is decided by identity, never by resemblance, and the doubt is never resolved

## Status

Accepted for the shared presence contract in `bridge-tally-core`, consumed by
the agent/MCP layer. It decides, for a set of proposed vouchers, which are
already in one company's book **within one observed window**. It selects
nothing, writes nothing, and dispatches nothing. Master creation, voucher
generation, posting, deletion, and any scored or model-assisted matching remain
rejected without separate evidence.

## Context

**Tally dedupes on one key and no other.** Re-sending an identical voucher
payload with the same `VOUCHERNUMBER` creates a second voucher — verified, and
recorded in [`TALLY_PROTOCOL_REFERENCE.md`
§9.3](../tally/TALLY_PROTOCOL_REFERENCE.md). A *client-supplied* `REMOTEID` is
the exception and the only one:
[`IMPLEMENTATION_GUIDE.md`
§3.3a](../tally/IMPLEMENTATION_GUIDE.md#33a-remoteid-is-the-idempotency-key--supersedes-34s-conclusion)
verified that re-importing the same payload under the same client `REMOTEID`
**upserts** — `CREATED=0, ALTERED=1`, one voucher, not two — and §9.8's scope
clarification records the same result on the licensed Journal path.

Read §9.3's heading alone and you get "no idempotency", flat; that heading is
narrower than it reads and does not point at the exception. **Neither fact
rescues this contract, because both are about a voucher the *client* keyed.**
A voucher an operator typed into Tally by hand carries no client `REMOTEID` to
dedupe against, and duplicated invoices inside a filed GST period are a return
problem, not a cosmetic one.

So before any generated batch can be imported, one question has to be answered
and Bridge cannot answer it:

- One engagement's month of sales: twenty invoices in the source report, **fifteen
  already keyed in by hand.** That was discovered only because the operator
  happened to send a Day Book screenshot. Without it the run would have posted
  twenty and duplicated fifteen.
- Another engagement's month of sales: **forty-nine vouchers generated, validated,
  arithmetic-checked, and un-importable at the end of the day**, waiting for a
  Day Book to arrive by hand the next morning.

Both books were **hand-keyed**, so no voucher in either carried a `REMOTEID`
Bridge had written. Both engagements were blocked on the same day.

The question also has diagnostic value on its own. One of those books already
held **twenty-five invoices in a single month sharing voucher numbers** —
visible in Tally's own `Duplicate Voucher No.` exceptions before any import ran.
Indexing a book window by voucher number finds that for free.

Bridge already performs the read this needs: `vouchers` returns a literal-window
voucher list with date, voucher type, voucher number, party ledger, entry
ledgers and amounts. What is missing is the comparison and, more importantly,
the contract that says what a comparison is allowed to conclude.

### Why the obvious keys are each insufficient

| Candidate key | Where it holds | Where it fails |
| --- | --- | --- |
| `REMOTEID` | A voucher whose Tally-assigned value the caller has already observed. | The **attribute** does not carry the client's key back: §3.3a verified Tally overwrites it with its own value. It does not follow that the key is unreadable — §9.8's batch-identity run recorded the `REMOTEID` and the **narration marker** sharing one batch-derived UUID, so it survives in the field Tally does not own. Whether a voucher keyed by hand in the Tally UI carries a Tally-assigned value is **untested in either direction**. |
| `VOUCHERNUMBER` | Voucher types numbered **Manual**. One book preserved a long alphanumeric invoice series verbatim, another a plain three-digit bill number. | Under **Automatic** numbering Tally *discards* the supplied number (§9.8), so a number-based key is silently ineffective. And a book that does not set `PREVENTDUPLICATES` can hold the same number twice — one did, twenty-five times. |
| date + party + amount | Needs neither of the above. | Collides. In one month of real data `141,600`, `177,000` and `16,992` each recurred across *unrelated* parties. |

No single key decides. A contract that pretends one does will be wrong in the
field, quietly.

## Decision

Presence is a **pure, deterministic function in `bridge-tally-core`** over (one
observed window of a company's book, the ledger catalog observed for that same
company, the vouchers a source document proposes). It performs no I/O, holds no
transport handle, calls no model, and depends on nothing above
`bridge-tally-primitives` and `master_binding`.

### 1. Party matching is not reinvented — it is `master_binding`

"Is this the same customer" is the question ADR 0016 already answers, and there
must not be a second answer to it. A proposal's party name is bound to the
observed ledger catalog through `master_binding::bind`, and the result is
consumed as-is:

- **`Bound`** — the bound catalog name is the one name party rules compare
  against.
- **`Ambiguous`** — *every* candidate name is compared against. Using the whole
  candidate set can only produce more resemblance, never less, which is the
  safe direction here; picking one of them would be the auto-resolution ADR
  0016 forbids.
- **`Unmatched`** — no name is compared. A party with no ledger and nothing
  resembling one cannot be carrying a posted voucher in this book, so
  party-independent rules are all that remain and `Absent` stays available.

Three further conditions withhold `Absent` before any of that, and they share
one shape: **the source offered nothing the decisive rules could use.** A
proposal that names no party (`PartyNotSupplied`) leaves an absence resting on
date and amount, the pair this contract says collides. A proposal under a
`Manual` declaration that supplies no voucher number
(`ManualNumberNotSupplied`) has withheld the one key that could decide. And a
proposal carrying a `REMOTEID` the window never read
(`RemoteIdEvidenceUnavailable`) had its strongest key skipped. None of these
blocks `Present` — identity still settles where it can; only the *absence*
claim is withheld, and supplying the missing field is what makes it available.

Two binding outcomes withhold `Absent` outright: `NoDiscriminatingCandidate`
(a name family that is deliberately not listed) and a truncated candidate list.
In both, names that might have matched were never compared, and reporting
"absent" off an incomplete comparison is the failure this ADR exists to prevent.

That withholding is not a precaution reasoned from the contract alone. Measured
over 470 ledger names from sixteen loaded synthetic companies and 2,257
mutation cases: where binding lists candidates the right master is present in
403 of 403 rows, and where a source name reaches a family it cannot distinguish
an alphabetically capped slice of that family **omitted the right master about
a third of the time** — which is why the family is counted and not listed. On a
book with systematic party naming `NoDiscriminatingCandidate` is expected to be
common, and a third of the `Absent` verdicts it would otherwise license would
have been wrong.

The trap this guards is not hypothetical, and it is not confined to this
contract. A sibling surface consuming the same state rendered an
undiscriminable family as "0 possible ledgers are listed first" — a count of
nothing, presented to an operator as a result. An empty candidate list means
"nothing survived to be shown", never "nothing exists"; every consumer of
`master_binding` has to decide which of those it is reporting, and the two
readings are one word apart in the output.

### 2. A window is a *claim about a window*, and it must be complete

`BookWindow::observed` is a boundary parse. It refuses, rather than degrades,
on:

- **a `REMOTEID` column that was never read.** A window declares
  `RemoteIdEvidence::Observed` or `NotRead`, because "no voucher carried one"
  and "the profile never fetched it" are different facts and only the first is
  evidence. Where a proposal carries a `REMOTEID` and the window is `NotRead`,
  that proposal's strongest key was never compared, so it **cannot be
  `Absent`** — it becomes `RemoteIdEvidenceUnavailable`. A report-level marker
  would not have done: a status field does not neutralise the per-voucher
  verdict printed beside it, which is the defect this contract cites elsewhere
  and had reproduced here.
- **a read that was not complete** — `WindowIncomplete`. A window whose
  emptiness was only partially corroborated is not "no match found". This is
  the single most dangerous confusion available here, so it is a typed error
  rather than a flag a caller may overlook. A window that could not be read at
  all never reaches this constructor: the read fails, and the tool fails with
  it. What this does **not** cover is named in the Consequences — a response
  Tally answers short without saying so;
- a window that does not **cover** every proposed date — `WindowDoesNotCover`.
  A voucher outside the window is invisible, so a verdict over it would be
  fiction;
- a voucher dated outside the window's own range, a duplicate voucher key, an
  invalid range, or a window past its bound.

Every verdict is therefore explicitly scoped to the window the report carries.
`Absent` means *absent from this window* — it never means "absent from the
book". A voucher keyed in September against an August window is not visible,
and widening the window is the caller's decision, made in the open.

### 3. The numbering method is declared, and its absence is an error

The decisive power of a voucher number depends entirely on the voucher type's
numbering method (§9.8), and Bridge has **no qualified voucher-type read**: the
protocol reference records that a numbering preflight "would require a
separately observed voucher-type read contract".

So the method is supplied by the caller as an explicit declaration per voucher
type, with three values — `Manual`, `Automatic`, `Unknown` — and a voucher type
named by a proposal but absent from the declaration is
`NumberingMethodUndeclared`, a typed error. A silently defaulted declaration
would silently decide whether the strongest available key is usable at all.
`Unknown` remains fully legal and is the honest answer most of the time; it
simply demotes the number from identity to resemblance.

This makes a real protocol fact operationally visible: on an automatically
numbered voucher type, Bridge will decide nothing, and will say so, rather than
matching on a number Tally threw away.

### 4. Three statuses. The middle one is never resolved

Per proposed voucher, exactly one of:

| status | meaning | what it authorises |
| --- | --- | --- |
| `Present { matched, basis, differences }` | An identity key matched, uniquely on both sides | excluding this voucher from the import |
| `PossiblyPresent { reason, candidates, .. }` | Something resembles it, or something prevented a decision | **nothing** |
| `Absent` | No rule produced any candidate, in a window proven to cover it | including this voucher in the import |

`PossiblyPresent` carries candidates labelled with the **rule that surfaced
each** — `SharedVoucherNumber`, `SameDatePartyAmount`, `SamePartyAmount`,
`SameDateAmount`, `SameDateParty` — ordered by rule and then by the book
voucher's own ordering. **No candidate is marked best, likely or preferred, and
no score is emitted anywhere.**

This is not a stylistic echo of ADR 0016; it is the same defect being refused
twice. Bridge has already shipped a bug in exactly this family — a `near_miss`
status that carried a guessed `exact_live_spelling` in the same object, so
whichever field a consumer read first decided the outcome. A status does not
disarm a value printed beside it. Here the guard is structural: a
`PossiblyPresent` has no field that names a match, and `Present` is the only
variant that can carry one.

### 5. Only identity produces `Present`

Two bases, and nothing else:

- **`RemoteId`** — the proposal and exactly one book voucher carry the same
  `REMOTEID`, and no other proposal carries it. Note carefully what a caller
  may put there: **not** the client key it wrote on a previous import, which
  §3.3a verified Tally overwrites in the attribute, but a Tally-assigned value
  it has previously read back.
- **`ManualVoucherNumber`** — the voucher type is declared `Manual`, and the
  (voucher type, normalized number) pair selects **exactly one book voucher and
  exactly one proposal**. Uniqueness on both sides is ADR 0016's rule 2, and it
  is what makes the twenty-five-duplicates book safe: those numbers select more
  than one voucher, so they decide nothing and surface as an ambiguity instead.

Uniqueness within a basis is not enough, and two further rules close what it
leaves open:

- **One book voucher satisfies at most one proposal, across bases.** Each basis
  enforced its own uniqueness while nothing stopped two proposals reaching the
  *same* voucher by *different* bases — one by `REMOTEID`, another by a manual
  number. A consumer would then exclude two source vouchers against one book
  row and silently drop an invoice, which is the failure this contract exists
  to prevent. Every claimant of a contested voucher is demoted to
  `BookVoucherClaimedTwice`; choosing between them would be auto-resolution.
- **Two identity signals that disagree are reported, not ranked.** Both
  lookups are resolved *before* either settles, so a `REMOTEID` selecting one
  voucher while the number selects another is `IdentityConflict` — as is a
  number matching uniquely while the two sides carry different `REMOTEID`s.
  Settling on whichever basis happened to be evaluated first would rank them,
  which is the move ADR 0016 refuses when an identifier contradicts an exact
  name.
- **Evidence that was never gathered cannot settle a `Present` either.** The
  rule that withholds `Absent` when a key was not compared applies with more
  force to `Present`, because `Present` carries the higher bar and its error is
  the silent one. So where a proposal supplies a `REMOTEID` and the window is
  `RemoteIdEvidence::NotRead`, a unique number match returns
  `RemoteIdEvidenceUnavailable` rather than `Present`: the number is decisive
  on its own terms, but the evidence that could contradict it was skipped. A
  proposal carrying no `REMOTEID` skipped nothing and still settles. An earlier
  revision of this ADR allowed that `Present`, reasoning that withholding it
  would make the tool less useful — which is the posture in §7 argued
  backwards, and review caught it.

Number comparison uses the same NFC / dash-and-quote / case / whitespace
comparison key as master binding, so a long alphanumeric invoice number and its
differently punctuated twin agree.

Voucher types are compared on that key too, and a manual number decides only
**within an observed voucher type** — numbers are a per-type series, so a match
across types is a coincidence, not a series position. If a proposal's voucher
type is **not observed anywhere in the window**, type discriminates nothing, so
number matching widens to every observed type *and is demoted to a
resemblance*: it can surface candidates and can never produce `Present`. A book
carrying both `Part Sale` and `Parts Sale` is exactly why narrowing on an
unobserved type name would manufacture absence, and exactly why widening must
not be allowed to decide.

A book voucher that is **cancelled or optional** never yields `Present`. It has
no accounting effect but does occupy its number, so a number that lands on one
is reported as `MatchedVoucherNotPosted` for a human. A struck-through and
re-issued bill is a real case, met four times in one month of one book.

Date, amount and party are **never** a basis for `Present`. They are the keys
that measurably collide.

### 6. `Present` reports what disagrees, and that is half the value

A `Present` verdict compares the proposal against the book voucher it matched
and lists every difference in date, amount, or bound party. The match is on
identity, so a difference is not evidence against the match — it is a finding
about the book.

This is not speculative: in one engagement an invoice was posted **₹36.13
short** because one 9% GST head was dropped when it was keyed by hand, and its
voucher number still matched perfectly. Under this contract that invoice comes
back `Present` with an amount difference — precisely the report the client
needed and nobody had asked for.

**Say plainly what a caller may do about it, because the obvious move is a
trap.** A difference is a finding for a person, not a work item for code. On
the observed instance §9.7 verified that voucher `Alter` returns
`CREATED=1, ALTERED=0` and **makes a duplicate while leaving the target
untouched** — tested against four different keys, all four duplicating — and
that `Cancel` behaves the same way (§9.6). Only `Delete` works, keyed by
`REMOTEID`. So a caller that reads "amount differs" and reaches for an `Alter`
to correct it would create the very duplicate this whole contract exists to
prevent, and Tally's counters would report success.

The correction that does exist is re-import under the same client `REMOTEID`
(§3.3a), and it reaches **only vouchers Bridge itself wrote**. For a
hand-keyed voucher — the case that produced the ₹36.13 finding, and the case
this contract is for — Bridge holds no client key and the `vouchers` profile
does not even fetch the Tally-assigned one, so there is **no programmatic
correction path at all**. The operator fixes it in Tally. A report that names a
disagreement it cannot act on must say so, or the next person writes the
`Alter`. (§9.7's Alter and Cancel results carry their own "unverified whether
this is SKU-specific" caveat; that widens the uncertainty, it does not narrow
the advice.)

### 7. The error posture, stated

The two errors are not symmetric, and the asymmetry is **detectability**, not
severity:

- A false `Present` silently drops an invoice. Nothing records it. It is not in
  Tally, not in the return, not in Bridge, and not in any exceptions report.
  There is no artifact to find later.
- A false `Absent` on a voucher **Bridge previously imported** creates nothing
  at all *when the re-sent payload is byte-identical* — that is the case §3.3a
  measured, and its own untested list names "when the payload differs from the
  original". So the duplicate risk is confined to vouchers an operator keyed by
  hand **plus** any re-send whose content has changed: smaller than
  "everything", and larger than "hand-keyed only".
- A false `Absent` on a hand-keyed voucher does create a duplicate, and that
  duplicate is **visible and correctable**: Tally's own `Duplicate Voucher No.`
  exceptions report surfaces it, and re-importing under the same client
  `REMOTEID` overwrites the earlier row (§3.3a's correction path) — the **least
  unverified** correction available rather than a confirmed one, since §9.7's
  operation matrix and the Delete row it rests on were measured on an Edit Log
  7.0 Educational baseline and are not qualified on a licensed profile. The
  mechanism is *re-import*, not reading the key back out of the attribute.

**Therefore the bar for `Present` is set higher than the bar for `Absent`, and
both are set higher than a resemblance.** `Present` requires identity;
`Absent` requires that no rule produced any candidate at all. Doubt in either
direction lands in `PossiblyPresent`, which authorises nothing and is handed to
a person.

**This asymmetry has one dependency, and it is outside this contract.** It
holds only while the import writer derives a `REMOTEID` that is stable for a
business event and distinct between different ones. A key that collides across
two distinct events — a row ordinal within a re-downloaded window, say — turns
`REMOTEID`'s upsert from a safety property into a **silent overwrite of a
different voucher**, which lands on the same side of the ledger as a false
`Present`: no duplicate to see, no exception raised, nothing to find later. A
consumer acting on `Absent` inherits that risk from the writer, not from this
report. Any key proposed for that writer should be tested against both the
re-download case and the overlapping-window case before it is trusted.

The cost of this posture is operator review time. That is the intended cost:
the middle is where a human is genuinely faster than any rule, and the
alternative to reviewing it is an invisible omission or an invisible duplicate.

### 8. What the book itself gives away

Building the index makes three book-side observations free, and they are
reported alongside the verdicts rather than discarded:

- `duplicate_numbers` — a (voucher type, number) that selects more than one
  book voucher. This is the twenty-five-invoice finding, computed rather than
  noticed.
- `unbalanced_vouchers` — a book voucher whose entries do not sum to zero.
- `unclaimed_book_vouchers` — how many vouchers in the window no proposal
  matched. Counted only; listing them is a different report.

These sit **outside** the paged rows, so a consumer's response machinery cannot
trim them: an unbounded echo here could push a complete report past a byte
budget that trimming rows could no longer rescue. The two echoed strings are
therefore bounded, and treated as what they are — **recognition labels, not
keys.** A group's identity is its `book_keys`, which are bounded by count.

A voucher's magnitude is the sum of its positive entry amounts, computed in
exact decimal. It is defined whether or not the voucher balances, so an
unbalanced book voucher still participates in every amount rule — it is
reported, never excluded, because excluding it would make `Absent` *more*
likely, which is the wrong direction.

### 9. Totals prove the run

`PresenceReport::totals()` reports `requested`, `present`, `possibly_present`,
and `absent`, with `requested == present + possibly_present + absent` asserted
by test. This is the same control-total discipline that proved every clean
import engagement, and it is what lets an operator reconcile a generated file
against a source document by count alone.

### 10. A verdict is a proposal, not an approval

The report names book vouchers by an opaque caller-supplied key and names
masters by observed name only. It holds no company GUID and grants no
authority. A caller acting on `Absent` still goes through the unchanged
build-and-approve path, whose write gate — byte-exact master names, one
human-approved batch — this ADR does not move.

## Consequences

- `bridge_tally_core::book_presence` is new and is the only implementation. The
  MCP tool `voucher_presence` is its first consumer; it performs the existing
  qualified ledger-catalogue and `vouchers` window reads, refuses to build a
  window from a partial read, and shapes the report through the same party-name
  marking and egress redaction as every other read result.
- **The verdict is built from two independently timed reads, so the catalogue
  is corroborated after the window.** A ledger renamed between them would let a
  proposal bind the old name while the rows carry the new one, removing the
  only resemblance and manufacturing an `Absent`. The adapter re-reads the
  catalogue and refuses on drift, the same paired-snapshot rule the
  selected-voucher read already applies.
- **Every refusal that depends only on the arguments happens before any Tally
  request** — including the two cross-input ones, a proposal dated outside the
  window and a voucher type absent from the numbering declaration. The crate
  enforces them again at its own boundary; the adapter check exists so a
  request that was always going to be refused does not first spend a company
  probe, a catalogue read and a full window read.
- **The published `inputSchema` is enforced to its leaves, and by the schema
  itself.** The shared argument validator bounds only outer arrays — every tool
  predating nested inputs owns a typed boundary below that line, so tightening
  the shared path would change their refusal codes — and the crate's own limits
  are far wider than this tool advertises. The gap is closed by
  `validate_against_schema`, a small recursive check that reads `type`, `enum`,
  string and array bounds, `required` and `additionalProperties` straight from
  the published fragment. Restating those limits in the parser would put two
  copies of every bound in the tree, and the copy that drifts is the one nobody
  is looking at. The helper lives beside the existing validator so the next
  tool with a nested schema reuses it rather than restating anything.
- Voucher numbers and voucher-type names fold through
  `master_binding::comparison_key` — the *same* key master names use, now an
  explicit crate-wide contract point owned by ADR 0016 rather than a private
  helper. A second, subtly different normalizer is exactly the divergence that
  ADR was written to end, and it would diverge silently: two folds agree on
  every name anyone tests by hand and disagree on the punctuation nobody thinks
  to try. This contract consumes that function and defines no fold of its own.
- **The desktop source-draft flow is deliberately not wired yet, and the reason
  is a shape gap rather than a scheduling one.** A draft row carries a
  `source_remote_id`, a date, a voucher type and entries — but no voucher
  number and no party field, and its voucher type is restricted to Payment,
  Receipt, Journal and Contra. Of the two keys that can produce `Present`, the
  number is absent from the draft and the `REMOTEID` is absent from the read
  (below). Wiring a screen to a function that can only ever return
  `PossiblyPresent` would misrepresent the capability. The crate is shared, and
  the desktop consumes the same function once a draft row carries a number and
  a party.
- **`RemoteId` is contract-complete and not reachable from the shipped read**,
  so the adapter declares `RemoteIdEvidence::NotRead` and the tool's schema
  does not accept a `remote_id` at all. `render_agent_vouchers` does not
  `FETCH REMOTEID`; only the AlterID change feed does. Accepting an input that
  could only ever *withhold* a verdict would be worse than refusing it.
- **The identity channel that does survive a round trip is the narration, and
  this read already fetches it.** §9.8's batch-identity run recorded the
  `REMOTEID` and the narration marker sharing one batch-derived UUID: Tally
  overwrites the field it owns and leaves alone the field it does not, and
  `render_agent_vouchers` fetches `NARRATION`. A marker a generator writes into
  the narration is therefore readable back **today**, with no change to a
  qualified read profile — which makes it the named path to a reachable
  identity basis for vouchers Bridge itself wrote. Deliberately not built here:
  a new basis is its own change, and this contract is under review. It is the
  first thing to build on top of it.
- **A content fingerprint is never promoted to identity, which is the rule
  `agent_import.rs` already enforces.** There a fingerprint-only match is
  `matching_content_observed` and `posted_verified` needs a narration-tagged
  match. Here date, party and amount can only ever produce candidates. The
  hazard is identical in both: a company with a recurring same-day payment
  already holds a voucher with that tuple, so the tuple would let a pre-existing
  voucher stand in for one that was never written. Both motivating engagements were hand-keyed
  and would not have had one regardless.
- The window is read in full before any comparison; `vouchers`' own pagination
  bounds output, not Tally's work. A window past `MAX_WINDOW_VOUCHERS` is
  refused with a narrow-the-range error rather than silently truncated.
- **The completeness guarantee is exactly as strong as the window read, and no
  stronger.** Three ways a window read can go wrong are closed: a transport or
  source-limit failure never produces a window because the read itself fails; a
  malformed or short body fails the strict parse; and the paired read refuses a
  pair whose two responses differ. The case that remains open is a
  **well-formed response that is silently short** — Tally answering a dense
  window with fewer vouchers than it holds and saying nothing. No layer beneath
  this contract detects that, and a deterministic short answer agrees with
  itself across the pair, so pairing does not catch it either. `BookWindow`
  therefore inherits the `vouchers` profile's own qualification, which states
  that dense windows are unqualified. Closing it needs a source-side control
  total — a count the window read asserts about itself — and that is a separate
  read contract with its own live evidence. Until then, prefer several narrow
  windows to one dense one, and read `Absent` as scoped to a window that was
  read narrow enough to trust.
- **The identifier rule that binds a party across spellings is bimodal, not
  general.** Measured across three catalogs: zero of 470 names across sixteen
  loaded synthetic companies, zero of 105 on one real book, and **91 of 214 —
  about two in five — on another real book** whose operator embeds a contact
  number in each customer's ledger name. So the rule has near-total coverage or
  none at all depending on one operator's naming habit, and it is a bonus
  signal a binder must work without rather than a key it may rely on. It is
  decisive where it does fire: on that third book it resolved a customer that
  the three closest name matches all got wrong. It is load bearing for
  `master_binding`'s own consumers; it is deliberately **not** load
  bearing here, because a party binding can never produce `Present` — it only
  selects which names the resemblance rules compare, which widens the net. A
  wrong bind can therefore cost a `SamePartyAmount` candidate and turn a
  `PossiblyPresent` into an `Absent` — a visible, deletable duplicate — and can
  never turn an `Absent` into a `Present`, which is the silent direction. That
  is the asymmetry of §7 holding under a rule that is not yet proven.
- Presence is pure computation over already-observed data, so P1's live-evidence
  requirement is satisfied upstream by the two reads that produce its input. Its
  own tests are fabricated from a placeholder alphabet: they establish the
  behaviour of the rules, and are not, and may not be presented as, evidence
  about any Tally instance. **No verdict from this contract has yet been checked
  against a real book.**

## Alternatives rejected

- **A boolean `already_present`.** Every key available collides or is absent in
  the cases that actually blocked. A boolean forces the collision to be
  resolved by the code, silently, in whichever direction the author guessed.
  The third status is the whole design.
- **Auto-resolving a sole candidate.** A single date-party-amount candidate is
  the *most* seductive wrong answer, because it looks decisive. Uniqueness of a
  resemblance is not identity; ADR 0016 rejected the same move for the same
  reason.
- **Scoring candidates and thresholding.** A score invites a threshold, a
  threshold auto-resolves, and here auto-resolution silently deletes an invoice
  from a filed GST period.
- **Defaulting an undeclared numbering method to `Manual`.** It would make the
  common case work and the automatic-numbering case fail invisibly, matching on
  a number Tally discarded — §9.8's trap, re-implemented.
- **Defaulting it to `Unknown`.** Safe, but it means a caller who simply forgot
  loses the only key that decides and is never told. Explicit ignorance is
  cheap; implicit ignorance is not.
- **Fuzzy party matching inside this module.** It would fork the answer ADR
  0016 owns, and it was directly disproven on the case that mattered: the three
  closest names were three different wrong people.
- **Deriving the numbering method from the window** (for example, inferring
  `Automatic` from dense consecutive numbering). It is an inference about a
  configuration, presented as an observation, and it decides whether the
  strongest key is trusted. Exactly the shape of value this project has already
  been burned by.
