# ADR 0016: Master binding is deterministic, identifier-first, and never auto-resolves

## Status

Accepted for the shared binding contract in `bridge-tally-core`, consumed by the
agent/MCP layer and by the desktop source-draft flow. Master creation, ledger or
stock-item write authority, voucher generation, posting, and any model-assisted
or scored matching remain rejected without separate evidence.

## Context

Four document-import engagements have run to completion — one failed, three
clean, 152 vouchers, zero rejections. **Not one failure was OCR, parsing, or
model quality. Every failure was binding the document's entities to the target
book's masters.**

- One engagement rejected 61 vouchers: seven ledger masters were missing, four
  of them near-misses of ledgers that already existed, and suspense was posted
  to an account that did not exist.
- One bank engagement survived only because ambiguous truncated payees were
  parked in suspense rather than guessed.
- One sales engagement met five missing stock items, three near-duplicate sales
  ledgers differing by one character and word order, and a customer whose ledger
  name differed from the source name entirely. **Fuzzy name matching offered
  three candidates and all three were the wrong person.** Only a mobile number
  the operator had embedded in the ledger name identified them, and it matched
  exactly.

Bridge has two surfaces that need this and is currently growing two answers:

- `agent_import.rs::master_match` (MCP `validate_masters`) compares a
  case-and-whitespace-folded key by equality or prefix, and — for a near-miss —
  emits `exact_live_spelling: candidates.first()`. That field names one
  candidate as *the* live spelling with no evidence that it is; it is a silent
  auto-resolution of exactly the case that caused the 61-voucher failure.
- PR #276 gave the desktop the opposite and correct behaviour: an operator loads
  the observed ledger list and explicitly assigns a target, with a fresh reread
  proving the selection still exists. It deliberately ranks nothing — but it
  also narrows nothing, so the operator faces the entire catalog per entry.

Neither surface matches on an embedded identifier, which is the one rule that
would have decided the case that defeated fuzzy matching.

## Decision

Binding is a **pure, deterministic function in `bridge-tally-core`** over
(one company's observed masters of one class, the entities named by one source
document). It performs no I/O, holds no transport handle, calls no model, and
depends on nothing above `bridge-tally-primitives`. Both surfaces consume it;
neither reimplements it.

### 1. Inputs are valid by construction

`MasterCatalog::new(class, names)` and `SourceEntity::new(...)` parse at the
boundary and fail closed with a typed `MasterBindingError`. `bind()` then takes
already-valid inputs and returns a report with no `Result`, so no caller can
re-check or compensate differently (P3).

The constructor refuses, rather than degrades, on:

- **an empty catalog** — `CatalogEmpty`. Binding against a book that was never
  read is the May failure exactly, and it is now a typed error rather than a
  report full of "missing";
- **a catalog carrying the same name twice** — `CatalogDuplicateName`. If a name
  does not identify one master, nothing downstream is meaningful;
- **an identifier hint that yields no identifier** — `IdentifierHintUnusable`. A
  hint that silently does nothing is a trap (P7);
- bounds violations on entry count, entity count, name length, and **hint
  count**. The last is checked as the hints arrive rather than on the finished
  set: hints deduplicate, so a million repeated ones fold to a single identifier
  and the finished set never exceeds its bound, while every one of them has
  already been scanned and copied. Each hint yields at least one identifier or
  is refused outright, so the eager bound rejects nothing the late one admitted.

A softer collision — two masters differing only in case, whitespace runs, or
dash and quote style — does **not** fail the catalog. It is carried as a
per-entity ambiguity instead, so every other master still binds and the
colliding pair surfaces in the unbound list where an operator can see it.
Failing a whole read to report one collision would block all the work it was
performed for.

`MasterClass` is `Ledger` or `StockItem`. Both classes failed in practice, the
rules are identical for both, and the class is carried only so a report cannot
be applied to the wrong catalog.

### 2. The identifier is the key; the name is a hint

Operators bury phone numbers, account numbers, and part codes inside master
names. Where such an identifier is present it is matched **before** any name
comparison, because a name comparison on the same pair is actively misleading.

Two identifier shapes are extracted, deterministically, from both source names
and master names, and additionally accepted from the caller when the source
document carries an identifier outside the name (a statement's payment
reference, say):

- **Numeric** — a maximal digit run, allowing internal hyphens and slashes, of
  at least `MIN_NUMERIC_IDENTIFIER_DIGITS` (8) digits. Canonical form is the
  digits alone, so a punctuated account number and a plain one agree. Internal
  *spaces* are deliberately not allowed: fusing separated digit groups would
  manufacture identifiers out of unrelated numbers, so a spaced value fails
  closed to a near-miss instead. Eight digits is the threshold at which a year,
  a rate, a house number and a masked last-four cannot qualify — a last-four
  written as digits falls through to near-miss rather than binding two accounts
  that share four digits.
- **Code** — a token holding at least two letters and at least
  `MIN_CODE_IDENTIFIER_DIGITS` (3) digits, of at least
  `MIN_CODE_IDENTIFIER_CHARS` (8) alphanumeric characters, and not a period
  label. Canonical form is uppercase alphanumerics, so a punctuated part number
  and an unpunctuated one agree.

  These thresholds were raised twice under review, from 4/2/1. Enumerating the
  period spellings that must not become identifiers — `FY25`, then `APR2025`,
  then `SEPTEMBER2025` — kept losing to the next spelling, so length carries
  what a list of prefixes could not: a registration code clears eight
  alphanumerics with three digits, and a period label does not. **Measured
  against 485 live ledger names, exactly one yields a code identifier at all**,
  so the cost of the strictness is nothing observed.

**A token carrying letters never yields a standalone numeric**, whether or not
it qualified as a code. Otherwise `Part A12345678` reaches an unrelated
`Bank 12345678` through the one-letter gap the code test rejects: a token
identifies by its whole shape or not at all.

**A mask is a mask however it is spelled, and wherever it is written.** A value
carrying mask punctuation (`****`, `####`) or a run of one repeated letter
(`XXXX`) exposes a suffix rather than a number, and that suffix is no more
identifying a space away than joined: `XXXX 12345678` is the same statement as
`XXXX12345678`. A mask therefore suppresses the token that follows it, in both
spellings and for both identifier shapes. Two unrelated ledgers sharing a masked
last-eight must reach a near-miss, never a bind.

**A code is a code only in the script it is written in.** Canonical form keeps
ASCII alphanumerics alone, so a name in another script fused to an ASCII suffix
would shed its letters and yield a code the name never contained, binding a
party to an unrelated bank where the ASCII spelling of the same shape did not.
A token holding non-ASCII letters yields no code. This rule has to hold in every
script or the boundary is an ASCII boundary wearing a general name, and the
books this binder reads carry Devanagari, Tamil and Bengali ledger names.

**Period labels are recognized by their numbers, not their words.** A token is
a period when every number in it reads as a year or a small ordinal — which
catches `SEPTEMBER2025` and `2025QUARTER1` that no cap on the alphabetic run
ever would, because a month name can be any length and a year cannot. A fiscal
range (`2025-2026`, `2025/2026`) is excluded before its digits are fused, since
stripping the separator produced an eight-digit run that no calendar reading
rejects. Written without any separator the range arrives as one run that the
splitting step never sees — `FY202425`, `FY20242025` — so a year followed by a
two- or four-digit year is read as a period in its own right. Otherwise a
missing `Purchases FY202425` identifier-binds to a sole live `Sales FY202425`.

One narrow exclusion applies to the numeric shape: an eight-digit run that reads
as a calendar date in 1900–2199 is a date, not an identifier. Without it two
unrelated period-labelled masters fuse on their period. The exclusion can only
make a bind less likely, never more, which is the safe direction for a rule
whose failure mode is posting against the wrong party.

**Coverage is a property of the client's naming habit, not of the problem.**
Measured across four catalogues: a retail motorcycle dealership carries an
embedded identifier in 91 of 214 ledgers (42%), because it literally names
customers that way; a B2B minerals trader, 0 of 105; a third catalogue, 0 of
470; and Bridge's own synthetic books, 0 of 470 until ten were seeded to give
the rule any live coverage at all. So this rule is a **first-pass check that is
decisive when it fires and absent more often than not** — it resolved a customer
three fuzzy name matches got wrong, and it can never be the primary key. The
binder must work with it absent, and does: name matching is not a fallback here
but the ordinary path.

An identifier binds only when it is **unique on both sides**: exactly one
master in the catalog carries it, and the entity's identifiers select exactly
one master overall. Any conflict is `Ambiguous`, never a bind. This keeps the
rule safe in the case that motivates it — a shared identifier is evidence of a
naming collision the operator must see, not licence to choose.

Identifier-first has one more guard. Where a decisive identifier points at one
master while the entity's name is byte-equal to a *different* master, two strong
signals disagree, and the disagreement is reported
(`IdentifierNameConflict`) rather than silently settled in the identifier's
favour.

### 3. Name matching binds only on an exact or normalized-exact unique hit

`Exact` is byte equality with the observed master name. `Normalized` is equality
under **Tally's own rule for when two master names are the same**, and only when
exactly one master shares it.

**There are two folds, and which one may answer is the whole of this section.**

`TALLY_PROTOCOL_REFERENCE.md` §9.4b sent named variants at a live master and
recorded which Tally accepted. Exactly three: ASCII case folding, one trailing
space, and a **space supplied where the master carries a hyphen**. `AND` for
`&`, a missing suffix word and a singular for a plural were rejected. §9.4b
marks everything else UNVERIFIED and states the rule this section now follows —
*a fold is only as safe as its least-verified step, and a looser fold may
**suggest**, never resolve.*

- The **narrow fold** resolves. It implements those three and nothing else. The
  hyphen step is directional, because the measurement was: a source **space**
  was sent at a master **hyphen**, and the reverse was never sent. A symmetric
  key cannot express a direction, so the master side of the index answers to
  both its own spelling and its hyphens-as-spaces, while the source side answers
  only to its own. A source hyphen therefore finds no master space.
- The **wide fold** suggests. It carries the reverse hyphen direction, collapsed
  whitespace runs, leading whitespace and the Unicode dash variants — and
  everything it reaches is offered as a `NormalizedEqual` candidate for a human
  to confirm.

**Trimming a source name is not part of either fold.** `SourceEntity` trims
what the document gave it, at the boundary, because leading and trailing space
in extracted text is transcription noise; an observed master name is retained
byte for byte, because a caller writes it back. So a source reading
`"  Alpha Traders"` reaches `Alpha Traders`, while a *master* spelled
`"  Alpha Traders"` does not resolve from a clean source name — it is offered.
The asymmetry is deliberate and is the P3 rule, not a claim about what Tally
folds.

**This was got wrong first, and the correction is the useful record.** An
earlier version of this ADR claimed the fold "stops exactly where Tally stops"
while the implementation resolved on four transformations §9.4b marks
UNVERIFIED. It read naturally, which is exactly the skimming-implementer failure
§9.4b was written to prevent, and the live slice in `TEST_CORPUS.md` §9 caught
it binding that way against a real instance.

**The cost is real and is stated here rather than discovered later.** `X - Y` is
a common ledger convention — six of the seventeen hyphenated names in the
observed books take that shape — and reaching it from `X Y` needs the measured
hyphen step *and* a whitespace run collapsed. So those no longer resolve. On the
fabricated mutation book, 420 of 995 mutations bind where most once did.

**What makes that a trade and not a loss** is measured alongside it: every
mutation the wide fold would have resolved is still shown, as a candidate
carrying the right master. The sweep asserts it case by case rather than as a
percentage. So narrowing the fold costs a confirmation, never a search — which
is the trade §9.4b prescribes and the same one §4 makes for every other
near-miss in this module. A binder that answers from unverified evidence has not
saved the operator a step; it has moved the step to wherever the wrong posting
is found.

This fold is deliberately **separate from the general comparison key**, which is
shared with other contracts for voucher numbers and voucher-type names. §9.4b
says nothing about those, and widening the shared fold to serve masters would be
the "never to make one caller's case pass" this ADR warns against. One fold per
notion of sameness, each named for the question it answers. Nothing else binds. There is no edit distance, no
phonetic key, no token stemming, and no similarity threshold anywhere in the
implementation.

### 4. Near-misses produce candidates and never resolve

Every non-binding entity carries its candidates, each labelled with the **rule
that produced it** — `SharedIdentifier`, `NormalizedEqual`, `SourcePrefix`,
`CatalogPrefix`, or `SharedToken`. Candidates are ordered by rule and then by
name; **no candidate is marked best, first-choice, or `exact_live_spelling`,
and no numeric score is emitted at all.**

A score is rejected as a matter of contract, not of tuning. A score invites a
threshold, a threshold auto-resolves, and auto-resolution is what put money
against the wrong parties. The vocabulary is therefore a *basis* — a fact about
which rule fired — and never a confidence value (P6: the marker is recorded, and
it records what was observed).

`SharedToken` suppresses tokens that occur in more than
`COMMON_TOKEN_PERCENT` (10%) of a catalog of at least `COMMON_TOKEN_MIN_CATALOG`
(20) entries, so a catalog-wide word cannot pull in every master. The
suppression is measured from the catalog rather than from a built-in word list,
which keeps it free of language and domain assumptions.

Candidates are capped at `MAX_CANDIDATES_PER_ENTITY` (25) with the true
`candidate_count` and an explicit `candidates_truncated` flag retained, so a
truncated list is never mistaken for a short one.

### 4a. An empty candidate list is three different facts, and the producer says which

`candidates` can be empty for three unrelated reasons, and they mean opposite
things to anyone deciding what to do next:

| `reason` | what empty means |
| --- | --- |
| `NoCandidate` | no master resembles this name at all |
| `NoDiscriminatingCandidate` | `candidate_count` masters resemble it and none is separable — **many exist**, none is worth showing |
| any, with `candidates_truncated` | the list was cut, by the per-entity cap or by the report's aggregate byte budget |

So `candidates.is_empty()` alone answers nothing. The disambiguators are
`reason`, `candidate_count` and `candidates_truncated`, and a consumer that
reads the empty vector as "nothing exists" is wrong in two cases out of three.

This is stated here, in the producer's contract, rather than left to each
consumer to rediscover, because **it has already been got wrong twice by
different lanes**: the preparation screen rendered "0 possible ledgers are
listed first" over a family of 120, and the voucher-presence contract had to
add a paired test to stop its own rule collapsing into "no candidates means
unknown" — a reading that is right for the truncated case and wrong for
`NoCandidate`.

It is the same defect class this ADR was written against: a refusal whose
neighbouring value reads as an answer. The vocabulary is deliberately explicit
so that "nothing survived to be shown" and "nothing exists" cannot be confused
by reading one field.

**This is now a type as well as a doc.** `Candidates` is
`None | Listed | Truncated { found } | Withheld { found }`, so a consumer
matching it exhaustively is made to decide each case, and the wrong reading does
not compile rather than failing a test someone remembered to write. `listed()`,
`found()` and `is_incomplete()` cover the callers that do not need to match.
`is_incomplete()` is the predicate that matters: **true means the absence of a
listing is not the absence of a master**, and no consumer may report "nothing
like this is present" over it.

It was taken before merge deliberately. The contract had not shipped, so this is
the cheapest the change would ever be; afterwards it would be a breaking change
to a published contract with three consumers behind it. The consumer who paid
for it measured its own cost at about thirty lines and reported that the change
made its code better rather than merely compatible — a hand-assembled
disjunction became an exhaustive match.

**The fix stops at the crate boundary, and says so.** The MCP result carries an
explicit `listing` discriminator, because a model is precisely the caller that
would read an empty array as "no such ledger exists". The desktop DTO stays
flat: its screen already distinguishes the three cases and is tested on each, so
flattening there is a projection with a tested consumer rather than an
ambiguity. Neither boundary has the compiler behind it — this protects Rust
consumers, and the projections are the two places where that protection ends.

### 5. Status vocabulary

Per entity, exactly one of:

| status | meaning |
| --- | --- |
| `Bound { catalog_name, basis }` | one master, decided by `Identifier`, `ExactName`, or `NormalizedName` |
| `Ambiguous { candidates, .. }` | more than one master is defensible, including every identifier conflict |
| `Unmatched { candidates, .. }` | no rule produced a candidate |

`Ambiguous` and `Unmatched` are the **unbound list, which is the product**. Each
unbound entry carries the source name as given, a stable `safe_reason_code`, its
extracted identifiers as `unresolved_identity`, and its candidates. "Here is
what I could not bind, and why" is the operator's actual work item; it is not an
error path and is not logged as a failure.

### 6. A fallback binding is constructed, never inferred

An ambiguous entity must remain postable. `FallbackBinding::assign` accepts an
**unbound** entry and a catalog-verified fallback master (a suspense ledger),
and retains the entity's `unresolved_identity` so a later reallocation journal
can find it without re-reading the source. It cannot be constructed from a bound
entity, so "silently rebound something that already matched" is not a
representable state (P2). Binding itself never emits a fallback.

### 7. Totals prove the run

`BindingReport::totals()` reports `requested`, `bound`, `ambiguous`,
`unmatched`, and `unbound`. `requested == bound + unbound` and
`unbound == ambiguous + unmatched` are invariants asserted by test, matching the
control-total discipline that proved every clean engagement.

### 8. Identity stays where identity is already proven

The report names masters by their **observed name only**. It holds no GUID and
grants no authority. A caller that intends to act on a binding re-reads the
catalog and revalidates the selection through the existing admission path —
`StandardLedgerCatalog::bind_selected` plus a fresh
`StandardLedgerCatalogBinding::matches` — exactly as PR #276 already requires.
This preserves that PR's rule that matching text alone is never a selected or
approved target, and keeps GUIDs out of a portable crate that has no company
scope to check them against.

Consequently a binding is a **proposal**, never an approval. It selects no
voucher, creates no master, and dispatches nothing.

## Consequences

- `agent_import.rs::master_match` and its private `master_key` are deleted and
  `validate_masters` is re-expressed over the crate. `match_state` gains
  `normalized` and `identifier` alongside `exact`, `near_miss` and `missing`,
  and `exact_live_spelling` now appears only on a bound row. A caller reading
  that field on a near-miss was reading a guess.
- The old implementation classified *every* normalized-equal name as a
  near-miss, so a request differing from the live ledger only in case,
  whitespace, or dash style produced a candidate list instead of an answer.
  Those now bind and report the live spelling.
- `build_import_xml` and the approved-post recheck still admit **`exact` only**.
  The import file carries the name verbatim, so a normalized or identifier bind
  informs the operator without widening what may be written. This PR does not
  move the write gate.
- The MCP result reports an unbound entity's `unresolved_identity` wrapped in
  the same party-name marker as every other name, so egress redaction treats it
  identically. It adds no exposure: those identifiers are extracted from the
  requested name the same result already echoes.
- The desktop catalog load returns an advisory binding per source entry, so the
  operator sees the few relevant ledgers rather than all of them. It confers no
  authority: assignment still runs the unchanged apply path, which rereads the
  catalog and proves the selection is current. An unusable capture narrows
  nothing rather than failing a read the operator just performed.
- Stock items are covered by contract before a stock-item catalog read exists.
  When that read lands it supplies names to the same constructor; nothing in
  this contract changes. Until then the shipped consumers pass `Ledger`, so the
  stock-item half of the recorded failure is designed for but not yet reachable
  from a screen.
- Binding is pure computation over already-observed data, so P1's live-evidence
  requirement is satisfied upstream by the catalog read that produces its input.
  Its own tests are fabricated from a placeholder alphabet: they establish the
  behaviour of the rules, and are not, and may not be presented as, evidence
  about any Tally instance.

## Alternatives rejected

- **Fuzzy or scored matching (edit distance, trigram, phonetic).** Directly
  disproven: on the case that mattered its three best candidates were three
  different wrong people, and a fourth-ranked exact identifier was present.
- **Auto-resolving a single candidate.** A single candidate is exactly the
  four-near-miss situation that rejected 61 vouchers. Uniqueness of a *guess* is
  not evidence.
- **Building this in the MCP and migrating later.** The two surfaces would
  diverge before the migration; the divergence has already begun in
  `master_match` and this ADR ends it rather than duplicating it.
- **Putting binding in `bridge-tally-protocol`.** Binding parses no wire format
  and needs no XML. `bridge-tally-core` is the portable contract layer and holds
  the analogous reconciliation logic (P8: dependencies point inward).
