# Unit A — implementation prompt (revised 2026-07-30)

Supersedes the earlier Unit A prompt entirely. That version predated the live-probe programme
and contradicts current evidence in at least four places — it told the implementer to keep the
XML parser strict, to build five read profiles at once, to use a segment ceiling since measured
wrong, and to extend a profile family since proven to crash Tally.

**Owner deviation, 2026-07-30:** the request and segmentation clauses inside the retained
prompt below are superseded where they say curated-only or ~5,000 vouchers. The verified
outstandings profile must use the single `ALLLEDGERENTRIES.*` exception in guide §2.4a because
the curated bill paths return silently wrong names and bill types. Calibration and production
segments must use that same wildcard shape and derive capacity from measured bytes and time.

Copy the block below into a **fresh** Codex thread.

---

```text
═══════════════════════════════════════════════════════════════════════
GOAL: UNIT A — OUTSTANDINGS, END TO END, AGAINST A LIVE TALLY
═══════════════════════════════════════════════════════════════════════

THREAD:   Fresh. Do NOT continue the P0/P0b/rectify threads — they carry
          import-counter and compatibility-promotion context that will bias
          this work.
BRANCH:   feat/tally-outstandings-slice
BASE:     master if fix/tally-p0b-rectify-20260729 has merged; otherwise
          branch from it and state in your report that the base is unmerged
          and unreviewed. Confirm which base you used before writing code.

TIER:     HIGH
LENSES:   protocol correctness · type design · resource bounds · testability
ROUNDS:   3
EFFORT:   This is a build-from-verified-spec unit. Every request shape and
          every number below was measured against a live Tally. Spend
          reasoning on TYPE DESIGN and on completeness verification, not on
          rediscovering the protocol.

───────────────────────────────────────────────────────────────────────
AUTHORITY DOCUMENTS — read all before editing, in this order
───────────────────────────────────────────────────────────────────────
1. AGENTS.md § "Engineering principles" (P1–P9). These are binding. P2
   (make illegal states unrepresentable) and P4 (argue every added line)
   shape this unit more than anything else.
2. docs/tally/IMPLEMENTATION_GUIDE.md — in full. The 12 invariants and the
   16-row trap index are the specification.
3. docs/tally/TALLY_PROTOCOL_REFERENCE.md — the evidence behind them.
4. docs/tally/PRODUCT_TEARDOWN.md §5 — the screen you are building toward.
5. docs/tally/IMPROVEMENT_PLAN_2026H2.md §8 — dated deviations; supersedes
   §§0–7 where they conflict.

If any document contradicts another, or contradicts the code, STOP and
report. Plan-versus-reality drift is the most common defect in this
repository and catching it is rewarded, not routed around.

───────────────────────────────────────────────────────────────────────
WHY THIS UNIT EXISTS
───────────────────────────────────────────────────────────────────────
Two facts, both established:

(a) Bridge's read path has never returned a row from a real Tally. Three
    stacked defects: a formula that terminates the Tally process, a report
    shape that renders nothing, and a parser expecting a shape the working
    request never produces.

(b) Bridge has nothing to show in a first demo. Its opening screens
    describe the sync, not the business. Market research is unanimous that
    aged outstanding receivables is the report accountants respond to, and
    every field needed for it is already verified readable.

This unit fixes (a) in service of (b). It is deliberately one thin
vertical slice rather than a horizontal layer.

───────────────────────────────────────────────────────────────────────
THE DESTINATION
───────────────────────────────────────────────────────────────────────
One screen, for one company:

  Receivable / Payable totals
  Ageing buckets: 0–30 / 31–60 / 61–90 / 90+
  Top parties by outstanding, with oldest-bill age
  A freshness line: "synced N minutes ago"

Computed locally from vouchers and bill allocations. See PRODUCT_TEARDOWN §5
for the layout. Do not build configurability, filters, or drill-down in this
unit — one screen, correct numbers.

───────────────────────────────────────────────────────────────────────
SCOPE
───────────────────────────────────────────────────────────────────────
IN:
  - one collection-based voucher read profile (new versioned identity)
  - a parser for the collection response shape
  - tolerant handling of invalid XML character references
  - segmented reading with completeness verification
  - local outstandings computation from bill allocations
  - one Tauri command and one screen

OUT — do not touch:
  - deleting the old report renderers (that is a later unit; they must keep
    compiling)
  - tdl_engine.rs / connector.rs rewiring (later unit)
  - mirror schema, migrations, canonical model changes. Compute IN MEMORY
    for this unit. Per P4: nothing persists until persistence is needed.
  - any write path, canary machinery, DSC/documents/AXAL
  - ClosingBalance — it is a lifetime figure (guide §6.4). Compute balances
    from vouchers or not at all.

PRECONDITIONS
  - Two live instances via SSH loopback forward:
      port 9000 = TallyPrime Edit Log 7.0 EDU
      port 9001 = TallyPrime 7.1 EDU (standard)
    Verify both with /status before starting. Develop against 9000; run the
    final exit check against BOTH and report any divergence.
  - Company "Aarav Trading Company Demo", ~101K vouchers, 87 ledgers,
    Bill-wise entry enabled. Synthetic — values may appear in fixtures.
  - Rust 1.96 (rust-toolchain.toml pins it; a non-rustup cargo may precede
    it on PATH — verify cargo --version reports 1.96).

───────────────────────────────────────────────────────────────────────
TYPE DESIGN — this is the part that matters (AGENTS.md P2)
───────────────────────────────────────────────────────────────────────
Do not implement the guide's invariants as runtime checks scattered through
functions. Implement them as types whose invalid states cannot be
constructed. At minimum:

  DateWindow      Constructor REJECTS any boundary whose day-of-month is not
                  1, 2 or 31. Guide §2.7 — both SVFROMDATE and SVTODATE.
                  A rejected boundary silently returns wrong data, so this
                  must be impossible to express, not merely discouraged.

  PinnedCompany   Constructible ONLY from a response whose company GUID was
                  verified. Guide I2/I3: omitting the pin silently uses the
                  loaded company; a mistyped name returns 0 rows with
                  STATUS=1. A bare String must not be usable as a company.

  ScanResult      Two distinct types, or one enum with no shared success
                  path: CompleteScan and PartialScan. The span-verification
                  and row-count checks are what produce a CompleteScan.
                  Nothing downstream may treat a PartialScan as complete.
                  This is the type that later makes false tombstoning a
                  compile error — build it now even though this unit emits
                  no tombstones.

  Money           Exact decimal only. No f32/f64 anywhere near an amount.
                  An empty TYPE="Amount" is NOT zero — it must parse to an
                  explicit absent/quarantined state, never 0.00.

Reuse what exists: bridge-tally-core's ExactDecimal, and the loopback
validation in bridge-tally-read-transport. Per P4, state in your report what
you reused and what you deliberately did not.

───────────────────────────────────────────────────────────────────────
IMPLEMENT
───────────────────────────────────────────────────────────────────────
1. REQUEST. A TYPE=Collection voucher profile. The exact working shape is in
   TALLY_PROTOCOL_REFERENCE §3. Rules:
     - no $$ function may reference an identifier containing spaces (I1 —
       this terminates the Tally process; it appears 11 times in the existing
       tree, do not copy the pattern)
     - the single outstandings wildcard exception: `ALLLEDGERENTRIES.*`. Curated
       bill-allocation paths are verified to corrupt `NAME` / `BILLTYPE` semantics
     - date scoping via <FILTERS> + <SYSTEM TYPE="Formulae">, never via the
       static variables alone (guide §2.2)
     - SVFROMDATE/SVTODATE always set explicitly (I11 — omitting them
       silently collapses scope to the current display period)

2. PARSE. New parser for ENVELOPE > HEADER(STATUS) > BODY > DATA >
   COLLECTION > VOUCHER. The existing parsers expect ENVELOPE > BODY >
   LEDGER and must not be adapted. Enforce STATUS=1 for exports.
   Unescape attribute values before comparison (guide §6.3).

3. TOLERATE INVALID XML. Tally emits &#4; — an illegal character reference —
   in ordinary responses, sourced from its own metadata fields. A strict
   parser REJECTS a plain Ledger read; this is measured, not hypothetical.
   Handle invalid character references tolerantly. Keep every other
   strictness: this is a narrow, documented exception, not licence to parse
   loosely. Add a test built from a real capture containing &#4;.

4. SEGMENT + VERIFY COMPLETENESS. Calibrate with the same wildcard request shape,
   starting with the smallest valid one-day boundary window. Derive segment size
   from observed bytes and elapsed time at runtime, not a curated-data constant.
   Completeness must be established by the CLIENT (I4): STATUS=1 appears
   before Tally knows whether the response will finish, and a truncated
   response carries STATUS=1 with no trailer. Compare the returned date span
   against the requested span on every read (I12).

5. COMPUTE. Outstandings by party from bill allocations, aged into the four
   buckets. Pure functions over parsed data, unit-testable without Tally.

6. SURFACE. One Tauri command, one screen. The command layer holds no
   business logic and no transport handles (P8).

───────────────────────────────────────────────────────────────────────
TESTS
───────────────────────────────────────────────────────────────────────
- Golden parser tests from REAL captures in .bridge-live/captures/. Do not
  author fixtures by hand (P1). Retain observed numeric values.
- The representative voucher fixture must be wildcard output containing both
  `New Ref` and `Agst Ref`; assert the distribution is not uniformly `On Account`.
- A capture containing &#4; must parse.
- NEGATIVE bounding test: a voucher outside the requested window is absent,
  AND the test FAILS if the <FILTERS> clause is removed. A bounding test
  that passes without the filter is not a test.
- POSITIVE bounding test: a window known to contain N vouchers returns
  exactly N. (The negative test alone would pass on a window returning
  nothing.)
- DateWindow rejects day 15 and day 30 boundaries.
- PinnedCompany cannot be constructed from an unverified name.
- A PartialScan cannot be consumed where a CompleteScan is required —
  demonstrate this is a compile error, e.g. a documented trybuild case or an
  explicit note in the PR body.
- Empty TYPE="Amount" does not become 0.00.
- Ageing computation: unit tests over hand-built parsed structures with
  known expected buckets.
- grep-test: no $$ function references a spaced identifier anywhere,
  including tests, fixtures and the simulator.
- CI must not contact Tally. All tests replay captured bytes offline.

VERIFICATION METHOD — non-negotiable (P5)
Write every live response to a file with curl -o, then inspect the file.
Do NOT pipe curl through head/tail — SIGPIPE truncates the capture and the
short file looks complete. Do NOT count rows with a bare substring grep:
<VOUCHER also matches <VOUCHERNUMBER> and <VOUCHERTYPENAME>, and <GROUP
matches the CMPINFO header. Count opening tags with the trailing space, or
parse. Any helper that can report "nothing found" MUST distinguish that from
"the request failed".

OPERATIONAL SAFETY
Never issue a request you cannot afford to wait out (I7) — a client timeout
does NOT cancel server-side work, and one abandoned read blocks the whole
gateway. Never retry a request that previously hung. Do not probe unknown
collection TYPEs: an invalid type raises a modal dialog that blocks the
gateway until a human dismisses it (guide §5.3a).

───────────────────────────────────────────────────────────────────────
EXIT CRITERION
───────────────────────────────────────────────────────────────────────
Against the live instance, company "Aarav Trading Company Demo":
  - the outstandings screen renders with correct totals, four ageing
    buckets, and top parties — numbers reconcilable by hand against a Tally
    outstandings report
  - a requested window returns exactly the vouchers in it; one outside is
    provably excluded; the negative test fails with the filter removed
  - a read exceeding the segment size is segmented, and a response that
    cannot be proven complete is surfaced as Partial, never as complete
  - no $$ function references a spaced identifier
  - the old report renderers still compile, untouched
  - cargo test --workspace green on 1.96
  - the same exit check run against port 9001 (standard TallyPrime), with
    any divergence reported
  - nothing marked Verified in the compatibility matrix — Education and
    Edit Log EDU cannot promote

REPORT
Per implement-item: what was built, what was verified live (request +
observed response summary), what remains unverified. Per P4: what you
reused, what you deliberately did not reuse, and the net LOC. Name every
assumption you could not confirm. Append one row to EXECUTION_LOG.md's
unmerged working-state table. Do not open a PR, push, or create issues
without explicit owner authorization.

ESCALATION
Stop and report rather than working around, if: a document contradicts the
code; a named FETCH sub-path does not resolve; either instance is
unreachable or hangs; the negative bounding test cannot be made to fail
with the filter removed; or the two instances diverge in a way that changes
the design. Hard limit 4 review/rectify cycles, then escalate.
```

---

## Notes for the orchestrator

**Why in-memory rather than through the mirror.** The mirror is 8,425 lines built on the broken
report path. Routing this slice through it would inherit that coupling and make the first
working read depend on the largest unproven component in the tree. P4's third question — what
breaks if this is not built — answers itself: nothing, for one screen. Persistence arrives when
"what changed since you last looked" needs a baseline, which is the next unit.

**Why both ports in the exit criterion.** The standard-versus-Edit-Log comparison is free once
both are running, and it settles whether any of the read-side findings are SKU-specific before
the design hardens around them.

**What this unit deliberately does not settle.** Voucher Alter/Cancel, the Edit Log report
export, and the licensed-mode date questions are all out of scope. They belong to the write
path and to Drift Sentinel, both of which sit behind a working read.
