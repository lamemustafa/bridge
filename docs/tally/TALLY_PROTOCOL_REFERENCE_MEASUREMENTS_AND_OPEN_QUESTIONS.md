# Tally XML gateway protocol reference — measurements and open questions

> This is a section of the canonical [Tally XML gateway protocol reference](./TALLY_PROTOCOL_REFERENCE.md). Read its confidence-marker convention and legacy-link note before relying on a finding.

---

## 10. Change detection

**VERIFIED.** Company-level `ALTVCHID` and `ALTMSTID` are monotonic high-water marks and move
in step with writes — two master creates advanced `ALTMSTID` by 2; one voucher create
advanced `ALTVCHID` by 1. Per-object `ALTERID` is exposed on vouchers and masters.

**UNVERIFIED but high value:** `AUDITENTRIES.LIST`, `OLDAUDITENTRIES.LIST` and
`ACCOUNTAUDITENTRIES.LIST` containers appear in voucher exports on the Edit Log SKU. They
were empty on un-edited demo data. If editing a voucher populates them with before/after
values, the Edit Log SKU exposes change history directly and AlterID diffing becomes
unnecessary on that SKU. **Not tested.**

### 10.1 Server-side `AlterID` filtering works — **contradicts published community guidance**

**VERIFIED.** A `<FILTERS>` predicate of the form `$AlterID > N` filters correctly at the
server. Measured on the demo company:

| Predicate | Rows |
| --- | --- |
| `$AlterID > 0` | 150 (whole book) |
| `$AlterID > 100` | 51 |
| `$AlterID > 200` | 3 |
| `$AlterID > 500` | 0 |

Semantically verified, not merely monotonic: `$AlterID > 200` returned exactly three
vouchers with `AlterID` 440, 441 and 442.

This matters because the published community position is the opposite. A TallyForum thread
on conditional export states *"Tally Prime does not respond to any filtering criteria other
than date duration"*, and the accepted workaround is to download the entire book on every
sync and diff `AlterID` client-side — described there as "network intensive". That workaround
is unnecessary, at least on this release.

**Consequence for incremental sync:** the design becomes cheap. Probe company-level
`ALTVCHID`/`ALTMSTID`; if moved, fetch only `$AlterID > checkpoint`. Three rows instead of
one hundred and fifty.

**Known gap — deletions.** A deleted object has no `AlterID` to exceed the checkpoint, so an
AlterID-filtered scan cannot see it. Deletion detection still requires either a complete
scan with absence reasoning (subject to §5.3's corroboration rule) or, on the Edit Log SKU,
the audit-entry containers above. **A cancelled voucher IS detected** — the cancel in §9.6
produced a new object at `AlterID` 443, visible to a `> 440` filter.

**UNVERIFIED:** whether other fields filter server-side (`$IsCancelled`, `$VoucherTypeName`,
`$PartyLedgerName`). Only `$Date` and `$AlterID` have been tested. Given the forum's claim
was wrong about `AlterID`, it is probably wrong more broadly — worth testing before accepting
any "Tally can't filter that" advice.

---

## 11b. Scale findings at 101,287 vouchers — the two that matter most

**VERIFIED 2026-07-29** on a 101,287-voucher production-shaped corpus.

| Request | Time | Bytes | Rows | B/row |
| --- | --- | --- | --- | --- |
| Minimal fetch, whole book | 108 s | 123.4 MB | 101,287 | 1,218 |
| **Curated, whole book** | **600 s (client gave up)** | **282.7 MB** | **78,320 of 101,287** | 3,609 |
| Curated, one month | 7.3 s | 17.7 MB | 4,891 | 3,612 |
| Curated, `$AlterID` filtered | 24.1 s | 5.7 MB | 1,583 | 3,615 |
| Wildcard, one month | 69.2 s | 105.3 MB | 4,891 | 21,532 |

### 11b.1 A truncated read is indistinguishable from a complete one — **P0**

The whole-book curated read returned **78,320 of 101,287 rows and still carried
`STATUS=1`**. `STATUS` sits in the `HEADER` at the *start* of the document, so it is emitted
long before Tally knows whether the response will complete. There is **no trailer, no row
count, and no completeness marker anywhere in the response.**

A consumer that checks `STATUS=1` and parses what arrived will silently conclude it has the
whole book while missing 22,967 vouchers. Under §3.1.7's absence-implies-deletion rule that
is mass false tombstoning.

**Required:** completeness must be established by the *client*, from something other than the
response's own claims — expected row count from a prior probe, byte-length agreement, or an
explicit second read. `STATUS=1` proves the request started, not that it finished.

**Tally never enforced a size cap** — it streamed 282 MB without complaint. The 32 MiB limit
is Bridge's own client-side rule. Tally will not refuse an oversized request on your behalf.

### 11b.2 A client timeout does not cancel server-side work — **P0 operationally**

After the client abandoned the 600 s read, **the gateway stayed completely unresponsive for a
further 523 s**, then recovered by itself with no restart. Total server-side occupancy for
that one request: roughly **19 minutes**, of which nearly nine minutes came *after* the
client had disconnected.

Throughout that window every other request failed — including `/status`. Because the gateway
serialises (§1), one abandoned expensive read blocks the entire instance.

**Consequences:**

1. **Abandoning a request frees your socket, not Tally.** There is no observed way to cancel
   server-side work once issued.
2. **A user-facing "cancel sync" cannot stop Tally.** Phase 3's "cancellable mid-segment"
   requirement is achievable only as "stop consuming" — the accountant's Tally remains busy
   regardless.
3. **The blast radius is the accountant's own session.** For ~19 minutes that Tally was
   unusable to the person sitting in front of it. This is the failure that generates support
   tickets and destroys trust in a sync product.

**Design rule: never issue a read you cannot afford to wait out.** Segment size must be
chosen by expected *duration*, not only by byte size — a segment that fits under 32 MiB but
takes ten minutes is still unacceptable. The one-month curated read (4,891 rows, 17.7 MB,
7.3 s) is the right order of magnitude; **~5,000 vouchers per request** is a defensible
default, well under the ~9,300 the byte cap alone would allow.

### 11b.3 Filter cost is fixed, not proportional to results

`$AlterID > 190000` matched **zero** rows and still took 22.09 s. `$IsCancelled` matched
**one** row in 22.61 s. Tally evaluates the predicate across the whole collection, so every
filtered query at 101K costs ~22 s regardless of how little it returns.

Server-side filtering saves bandwidth and parsing, **not scan time**. This makes the cheap
company-level `ALTVCHID`/`ALTMSTID` probe (§10) essential rather than an optimisation — it is
the only way to avoid paying 22 s to discover that nothing changed.

### 11b.4 Bytes-per-row is stable and predictable

1,218 minimal · 3,609–3,615 curated (identical across whole-book, one-month and filtered
reads) · 21,532 wildcard. Segment sizing can be derived from a single sample read with
confidence.

## 11c. A windowed voucher read is bounded before it is sent — **rule; the measurements below are VERIFIED, the bound's own requests are UNVERIFIED live**

> **Status 2026-09-21.** The bound's request shapes — the census, its AlterID spans and the
> AlterID-narrowed data parts — are now **VERIFIED live** (§11c.5), and the rectified bound was run
> end to end on one inventory-heavy book. The heading keeps its original wording so existing links
> resolve; §11c.4 lists what is still open.

### 11c.1 What was measured

**VERIFIED**, measured on a licensed lab gateway the week of 2026-09-14. Figures are UTF-8 bytes
unless marked; Bridge's wire is UTF-16LE (§1.2), which costs exactly twice as much.

- **Response size follows vouchers returned, not window width**, with a fixed per-request overhead
  of about 0.15 s. This repeats §11b.4 and §12a.8 on more books.
- **Bytes per voucher depend on the book and on the request shape:**

  | Book | Shape | Bytes per voucher |
  | --- | --- | --- |
  | Synthetic, accounting-only | named ledger-entry fields | ~4.7 KB |
  | Larger accounting book | named ledger-entry fields | ~6.7 KB |
  | Inventory-heavy trading book, whole year (16,367 vouchers, 587 MB) | named ledger-entry fields | **35.0 KB** mean |
  | Same book, one day (41 vouchers) | named ledger-entry fields | 28 KB |
  | Same book, one day | `ALLLEDGERENTRIES.*` entry wildcard | ~128 KB |

- **A one-day probe understated the year on that book** (28 KB against a 35.0 KB mean). A single
  small sample is not a reliable estimator on its own.
- **Half-month windows were barely inside the cap.** With named fields the heaviest half-month of
  that year reached 98.4% of Bridge's 32 MiB transport cap. It succeeded; it had no margin.
- **Month-scale entry-wildcard windows on that book returned hundreds of megabytes and took
  minutes.** A response that large can fail mid-transfer, and afterwards the gateway can stop
  accepting connections for every company for tens of minutes; a client that gives up does not stop
  Tally (§11b.2). The reproducing detail is deliberately not recorded here.

### 11c.2 Why the existing limits do not protect Tally

Bridge's response cap (`XML_RESPONSE_MAX_BYTES`, 32 MiB) and per-leg deadline (20 s) are enforced
on the response, client-side. By the time either trips, Tally has already committed to building the
whole response and will finish it regardless (§11b.1, §11b.2). They protect Bridge's memory and its
caller; they cannot protect the gateway. The only bound that protects Tally is one applied before a
request is sent — which is what §11b.2's "never issue a read you cannot afford to wait out" and
§12a.8's pre-flight count already ask for.

### 11c.3 The rule

Before a windowed voucher read, predict its response and divide the window so that no single
request is predicted over a budget well below the cap.

1. **Budget, and the one margin.** Half the transport cap, in encoded wire bytes: 16 MiB. A part is
   planned at the book's measured cost per voucher, not at an inflated one; the other half of the
   cap is the only allowance for that measurement being wrong. It is sized for the understatement
   measured in §11c.1 (a one-day probe 1.25 times below the year) with room.
   **The floor makes that margin hold.** A measured cost never replaces the default below **half** of
   it. A part planned within the budget at a figure of at least half the default holds at most
   `2 × budget / default` vouchers, so if no voucher in it is heavier than the default — which is set
   above the heaviest cost measured for the shape — the part is at most twice the budget: the cap.
   Without the floor, one light first part (bank receipts at a tenth of an inventory voucher's cost)
   would plan the next part at ten times its safe size. A light book pays in more, smaller parts.
2. **Vouchers in the window, cheapest estimate first.**
   - The company's voucher high-water mark, `ALTVCHID` (§10), bounds the whole book: every voucher
     carries an AlterID no greater than it. If the mark times the shape's conservative per-voucher
     cost fits the budget, no window of the book is predicted to exceed it, and the window is sent **undivided,
     exactly as before**. A company that has never held a voucher omits `ALTVCHID`; that is a zero.
   - Otherwise, a **census** of the window: one row per voucher carrying only `GUID`, `ALTERID` and
     `DATE` (the §12.7 witness fetch), **every census request bounded before it is sent**. A date
     range can hold every voucher the book has, so a date census is bounded only by the mark itself:
     a book whose mark fits one census is counted in **one date census of the window**. A larger
     book is counted in **AlterID spans across `(0, mark]`, each also narrowed to the window's
     dates**, and each bounded by construction because AlterIDs are distinct. The spans are produced
     one at a time, and a mark needing more than 256 of them is refused before any census is sent.
   - **A census is sized against the whole transport cap,** not the half-budget: 8,192 rows at the
     planning figure of 4 KiB a row. The half-budget exists to absorb the error in an *estimated*
     data part; a census span's row count is fixed by construction, so its only uncertainty is the
     per-row figure, measured live at 2.42–2.72 KB (§11c.5). A full census is about 22 MB.
   - The cost is a census count proportional to the **book**, not the window: a mark of 250,000 is
     31 census requests however short the window. It is a cost in elapsed time, not in gateway
     safety: every request stays bounded, so a caller that abandons the read leaves at most one
     bounded request in Tally. An earlier revision sized a date census from the
     book's average density; that size was an expectation, learned too late when it was wrong, so it
     was withdrawn. A request returning only a count per date range would avoid the span walk, but no
     such shape is qualified live.
   - A census Tally does not serve — oversized, or timed out — is not divided or retried: every
     census is already bounded, so an oversized one means the per-row figure was wrong, and a
     timed-out one may still be building on the gateway. The window is refused as unestimated.
   - A caller already holding such a witness count for the same window may supply it instead, and
     no census is sent. A count that names a day outside the window is refused.
3. **Bytes per voucher for the request shape.** Until something is measured, a **conservative
   per-shape default** above the heaviest figure measured for that shape: 96 KiB per voucher on the
   wire for named ledger-entry fields (48 KiB of UTF-8, against a 35.0 KB measured mean) and 384 KiB
   for the entry wildcard (192 KiB of UTF-8, against ~128 KB measured). It sizes only the **first
   part**, which is real data, not a discarded sample. That part's measured cost then replaces the
   default for the rest of the window; each later part heavier than the planning figure raises it,
   and a lighter one never lowers it again. Nothing is remembered between calls.
4. **Divide by date, then within a day by AlterID.** Days are packed in order into contiguous
   date ranges, each predicted within the budget; a day without vouchers joins whichever range it
   falls in. A day too heavy for one read is read alone, in AlterID spans of that day that together
   cover every AlterID up to the high-water mark. Reading every part observes the same vouchers one
   undivided read would, provided the book does not change during the read (rule 6).
5. **Refuse by name, before any further data read,** rather than send:
   - `voucher_window_part_over_budget` — a single voucher is predicted over the budget, so no
     division can help.
   - `voucher_window_too_many_reads` — the read would dispatch more than 128 data requests. The
     allowance is spent **when a request is dispatched**, not when a plan is made, so a part divided
     after Tally could not serve it — and the failed attempt itself — count against it.
   - `voucher_window_book_too_large` — the book's mark needs more than 256 census spans (a mark above
     about 2.1 million). A **product limit**: narrowing the window does not help, because the census
     walks the book's AlterIDs whatever the window.
   - `voucher_window_volume_unestimated` — a census could not be read, was refused as oversized, or
     timed out. A mark that cannot be read is refused under the mark parser's own code.
   - `window_part_boundary_unsupported_in_education` — the endpoint is in Education mode and a part
     still to be read starts or ends on a day other than the 1st, 2nd or 31st. Education serves a
     read **starting** on such a day as a well-formed empty collection, not an error (bridge#581,
     lab capture 2026-09-22); the end side is unmeasured in the voucher shapes and held to the same
     rule. The mode is read from the `EDUMODE` field of the `CompanyListV2` response that brackets
     every read, so it costs no request. The whole remaining plan is checked before each part, so a
     divided read is refused before its first part, and the runtime refuses any single read with
     such a boundary before sending it, or after it when only the closing bracket reports Education.
     An `EDUMODE` other than `No` counts as Education; a list with no `EDUMODE` keeps ordinary
     boundaries. `EDUMODE = Yes` was observed live on 2026-09-22, alongside `SILVER = Yes` and
     `GOLD = No`, so Education still reports Silver and the mode is read from `EDUMODE` alone.
6. **Every part is admitted, and so is their union.** Each row of a part must lie in the part's dates
   and AlterID span. When the window was counted, a part's vouchers must be **exactly** the ones the
   census counted for it, by AlterID and GUID — a matching count is not enough, because a substituted
   voucher preserves it. And GUIDs and master IDs must be unique across the union of parts, not only
   within each response: a voucher re-dated between two parts is returned by both, each valid alone.
   Either failure refuses as `voucher_window_part_not_admitted` or
   `voucher_source_identity_invalid`. A part refusal names its `cause`: `part_row_unreadable`,
   `part_row_outside_dates`, `part_row_outside_alter_id_span`, `part_row_duplicated`, or
   `part_census_mismatch`, which also carries `counts` (`returned` against `counted`) so that an
   empty part reads as "returned 0 of N".
7. **A divided read is bracketed on both marks.** `ALTVCHID` **and** `ALTMSTID` are read again after
   the last part, and either moving refuses the read as `voucher_window_changed_during_read`. Live
   (§11c.5): creating, altering, cancelling, re-dating and deleting a voucher each advance
   `ALTVCHID`, but renaming a ledger advances only `ALTMSTID` while changing that ledger's name in
   every voucher export. A read bracketed on `ALTVCHID` alone returned `complete` across a rename
   made between two of its parts. Every divided read is bracketed — including one planned whole and
   divided only after Tally could not serve it. An undivided read is one observation, as before.
8. **A corroborating second read replays exactly the first read's parts and carries its witness** —
   it never re-plans from a measurement, which could merge parts back into a request the first read
   found Tally could not serve. The witness is the marks the first read opened and closed on, and
   the census its parts were admitted against. The replay is admitted against that census and
   closes against those marks. Without them, a replay of
   AlterID-limited parts reads only up to the first read's ceilings: a voucher posted in the window
   between the two reads takes an AlterID above them, both reads miss it, and their responses match
   byte for byte. A replay of a divided read without a witness is refused as
   `voucher_window_replay_unwitnessed`.
9. **A caller that must send the undivided request itself decides on what was measured.** The
   pre-post check inside the import dispatch lease sends the whole verification window as one
   request. Before approval it is admitted on the `verify_import` read of the same window that runs
   just before: an undivided read was that request, and a divided read's parts together (one copy of
   each response) are its size, which must be within the budget — unless Tally refused one of that
   read's requests as too large or timed out, since a timed-out request has no size to sum and the
   refused one may be the whole window itself. Otherwise the batch is refused as
   `import_post_window_not_bounded`. The book can still grow between that read and the lease; the
   transport cap is the backstop there.
10. **The transport cap remains the final safeguard.** Where a caller already divided a window after
   a deadline or an oversized response (#485, the import-verification read), it still does: by date,
   then, for a single day, by the day's counted AlterIDs.

### 11c.4 What this does not establish

- **The rectified bound was run end to end live only twice.** A build of the tree committed as
  `986c1d77` made the two calls timed below (§11c.5). Two changes made after review — a replay
  that no longer re-plans, and the measured pre-post admission — are established by simulator and
  unit regressions only.
- **Latency on a large book.** Measured end to end on the rectified bound (§11c.5), on an
  inventory-heavy book with a mark of about 250,000 (31 census spans): a one-day `vouchers` call took
  34 s (7.5 s before the rectify) and a one-month `ledger_movement` 105 s (53 s before). The MCP
  host's own timeout is not measured.
- **An abandoned call is not cancelled mid-read.** For every tool but `post_import`, the stdio server
  awaits a tool call to completion before reading its input again, so a host's cancellation or
  closed input is seen only afterwards, and the read dispatches its remaining (bounded) requests.
- **`ALTVCHID` and `ALTMSTID` moving on every change** was observed only for changes made through
  the XML gateway, once each, on one release (§11c.5). Changes made in the Tally UI, the Edit Log
  SKU and Education mode were not tested.
- **The first part is one place in the window.** A book whose later vouchers are much heavier than
  its first part's is protected only by rule 3's raising and rule 1's margin. Live, three equal-count
  spans of one day on an inventory-heavy book differed 1.8× in bytes.
- **A mark that is loose as a density prior.** On the inventory-heavy book the mark was ten times the
  voucher count. It is still a correct upper bound; it only makes the whole-book shortcut rarer.
- **The empty-window corroboration reuses the first read's marks.** Its widened read (±1 day) opens
  on the marks the window's own read observed. Only when that widened read is itself divided is it
  bracketed, and then a change anywhere in the company between the two reads refuses it. An undivided
  widened read is not bracketed; there, a voucher in the window refuses the read as
  `window_contradicted`, and otherwise the empty result is corroborated or reported partial by the
  existing empty-window control, exactly as before the bound.
- **AlterID 0.** Every span starts above an exclusive lower bound of 0, so a voucher with AlterID 0
  could not be read by a divided day. None has been observed; a census row carrying AlterID 0 is
  refused rather than planned around.

### 11c.5 Live evidence, 2026-09-21

**VERIFIED** on a licensed TallyPrime 7.1 Silver lab (the last licensed day), one request at a time,
`/status` probed before and after each, on four synthetic companies and one inventory-heavy client
book (read-only there). The request strings were the branch's own at `cf618c00`.

| Check | Result |
| --- | --- |
| Census row | `DATE` and `ALTERID` are direct children of `VOUCHER`; 2.42–2.72 KB per row in UTF-16 on every book (about forty default fields come with each row); an empty window parses to zero rows with `CMPINFO`'s `<VOUCHER>0</VOUCHER>` not counted |
| Census count equals the full read | yes, on every whole-day window tried, in every shape |
| `ALTVCHID` as a count bound | the count never exceeded the mark and no AlterID exceeded it, on every book: 67/90, 2,790/2,797, 29,900/29,900, and 23,634 against 249,948; a company with no vouchers omits `ALTVCHID` |
| AlterID spans, three shapes | three contiguous spans of one day, in the entry-wildcard, movement and import-verification shapes, on three books: disjoint, their union equal to the unspanned read and to the census, no row outside its span; every read under 1.6 s |
| Charset | the same 1,230-voucher read is exactly twice as large in UTF-16 as in UTF-8 |
| `ALTVCHID` on each change (gateway writes, synthetic company) | create +1; alter (re-post with the same client `REMOTEID`) +1; cancel (official `TAGNAME` shape; the counter that moves is `ALTERED`) +1; re-date +1; delete +2; a no-op re-post +1. A ledger rename: `ALTVCHID` +0, `ALTMSTID` +1 |
| End to end (`bridge_mcp` @ `cf618c00`, before #520) | whole-FY, one-day and one-month `vouchers`, a quarter and a month of `ledger_movement`, on three books: complete, every part under 16 MiB and 2 s. A voucher created between two parts refused the read as `voucher_window_changed_during_read`; a ledger renamed between two parts did not (fixed by rule 7) |
| Paired reads | every Tally request is sent twice, back to back (the repeated-source read), so wire traffic is about twice the data |
| End to end after the #520 rectify (census spans of 8,192; a build of the tree committed as `986c1d77`) | inventory-heavy book, mark ~250,000: one-day `vouchers` complete in 34.3 s (31 census spans, 2 data parts); one-month `ledger_movement` complete in 104.7 s (31 census spans, 5 data parts of at most 6.6 MB, and the replay closed against the first read's marks). Every request under 16 MiB and 2 s |

## 11d. Education refuses Bridge's report-family TDL with a blocking dialog — **VERIFIED live for `ledgers_v1`, 2026-09-22; the rest inferred**

On a TallyPrime 7.1 instance in Education mode, `ledgers_v1`'s custom report raised a modal
**Error** dialog, `Cannot understand. Bad formula! '$$NumItems:BRIDGE Ledger Collection V1'`, and sent
no response. The dialog holds the XML gateway until someone dismisses it on the Tally screen. From
the network, that looks like a busy or dead gateway (bridge#45). A formula-free ledger `Collection`
export to the same instance returned `STATUS 1` promptly, with no dialog.

Only `ledgers_v1` was observed. Two things are **inferred**: that the other builders using the same
construct raise it too, and that the space in the argument is the cause. Those builders are every
`function-argument-with-space` entry of `scripts/check-tally-request-builder-hazards.mjs`:
`vouchers_v2`/`v3`, the ledger canary and the period-balance report.

All of them are refused **before sending** once Bridge knows the endpoint is in Education, with
`education_report_family_unsupported`. Admission is unchanged: nothing is sent, and nothing is
promoted.

- **Sync period-balance tie-out.** Reads the mode from the `CompanyListV2` identity read that
  already precedes it, or from the run's own probe. The run carries on and records
  `report_tie_out_unavailable` and `education_report_family_unsupported`. A later window's
  successful report clears the second code, as it does the other tie-out codes.
- **Selected-ledger and selected-voucher qualifiers.** Read the mode from their opening identity
  bracket.
- **The live-read tool.** Takes its mode from its configuration; it does not observe it. In
  Education, the ledger step is recorded as failed with `education_report_family_unsupported`, and
  the voucher steps as not attempted.
  - **Residual:** a run configured as `Licensed` against an endpoint that is actually in Education
    still sends `ledgers_v1`. The tool's company read (`CompanyListV1`) carries no `EDUMODE`. Moving
    it to an observed mode means changing its profile sequence, which is not done here.
- **The native-outstandings qualification.** Accepts only an Education configuration, and its
  identity brackets read through `ledgers_v1`. It therefore refuses on load, so the tool **cannot
  run** until Phase 2 Unit A.
- **`groups_request`.** Has no production caller, and compiles only for tests.

Reading ledgers and vouchers in Education waits for the Collection-based profiles (Phase 2 Unit A,
`IMPROVEMENT_PLAN_2026H2.md` §8.12).

**Unmeasured:**
- whether Education accepts `$$` functions whose arguments contain no space;
- whether Education accepts a `COMPUTE` of `$GUID:Company:##SVCurrentCompany`;
- whether Education accepts `CompanyListV1`, a custom report with no `$$` function that the
  live-read tool still sends.

## 11a. Scale measurements — 11,287-voucher corpus

**VERIFIED 2026-07-29** on a generated production-shaped corpus: 25 customers and 15
suppliers with valid-format GSTINs across six state codes, Sales/Purchase/Payment/Receipt
mix, 9%+9% CGST/SGST splits, invoice-referencing narrations, spread across every
Education-legal date from 2024-04 to 2026-03.

### Read performance

| Request | Elapsed | Bytes | Rows | B/row |
| --- | --- | --- | --- | --- |
| Minimal fetch (`DATE,ALTERID`), whole book | 5.44 s | 13.7 MB | 11,287 | 1,214 |
| **Curated fetch, whole book** | 15.88 s | **40.6 MB** | 11,287 | **3,598** |
| Curated fetch, one-month window | 0.72 s | 1.94 MB | 538 | 3,605 |
| Curated + `$AlterID` filter (no matches) | 1.99 s | 1.5 KB | 0 | — |
| Wildcard fetch, one-month window | 2.94 s | 11.6 MB | 538 | 21,532 |

**Key results:**

1. **The 32 MiB cap binds at ~9,300 vouchers** with a curated fetch. A whole-book read of
   11,287 vouchers produced **40.6 MB — already over the cap**. Segmentation is not an
   optimisation, it is a correctness requirement at any realistic client size.
2. **Bytes-per-row is highly stable** — 3,598 whole-book versus 3,605 for a single month.
   Segment sizing can be predicted reliably from a sample read.
3. **Wildcard costs 6.0×** curated (21,532 vs 3,598 B/row), confirming the earlier 6.3×
   measurement on a smaller sample.
4. **Server-side filtering is not free.** A filter matching zero rows still took 1.99 s —
   Tally evaluates the predicate across the whole collection. Filtering saves bandwidth and
   parsing, not scan time.
5. Curated read throughput is roughly **710 rows/second**.

### Write performance and degradation

10,000 vouchers imported in 624 s, zero errors, batches of 250. **Throughput degraded
monotonically as the book grew:**

| Progress | Rate |
| --- | --- |
| 250 | 21/s |
| 2,750 | 21/s |
| 5,250 | 19/s |
| 7,750 | 17/s |
| 10,000 | 16/s |

Roughly a 25% drop across the first 10K rows. **Bulk import cost is superlinear in existing
book size**, which matters for any initial-load or migration story: a 100K-voucher onboarding
will not take ten times a 10K load.

## 11. Measurements

| Quantity | Value | Basis |
| --- | --- | --- |
| Curated voucher payload | **3,142 B/voucher** | 6 vouchers, curated FETCH |
| Wildcard voucher payload | **19,658 B/voucher** | same window, `ALLLEDGERENTRIES.*` |
| Response cap | 32 MiB general; 40 MiB only for the closed wildcard outstandings request | Bridge-side limits; guide §2.5b |
| Segmentation ceiling | **Profile-specific; derived at runtime** | ~10,600 applies only to the older curated shape, not wildcard outstandings |
| Independent corroboration | 10,000 vouchers/batch | `tally-database-loader` caps here to avoid hangs |
| Whole demo book, wildcard | 2.94 MB / 150 vouchers | wide filter |
| Typical collection latency | 30–90 ms | demo company |

Segment size must be derived from **observed** per-voucher cost at runtime — inventory lines
and long narrations run heavier than this demo data.

---

## 12. Verification method — how to test this gateway

Learned the hard way; every one of these produced a wrong conclusion at least once.

1. **Write responses to a file, then inspect the file.** Never pipe `curl` through
   `head`/`tail` — SIGPIPE truncates the capture mid-element and the truncated file looks
   like a complete short response.
2. **Never count rows with a bare substring grep.** `<VOUCHER` also matches `<VOUCHERNUMBER>`
   and `<VOUCHERTYPENAME>`; `<GROUP` matches the `CMPINFO` header. Match the opening tag with
   its trailing space, or parse.
3. **Diff whole files, not fragments.** A narrow `grep -A1` window missed a 13-byte
   difference that reversed a conclusion.
4. **Check the returned span, not just the row count.** A plausible row count can come from
   entirely the wrong period.
5. **Repeat before believing anything anomalous.** Determinism distinguishes a protocol rule
   from a transient.
6. **Change one variable at a time.** A multi-variable change established that a combination
   works while leaving the actual cause unknown.

---

### 12.7 Empty-date witness profile qualification

**VERIFIED 2026-08-02** under owner-supervised dispatch against the reference
corpus. `VoucherEmptyPartitionWitnessV1` was healthy before and after every
request and produced no dialog or hang.

| Purpose | Window | Observation |
| --- | --- | --- |
| Non-empty control | `20240401..20240501` | 20 vouchers dated `20240401..20240402` |
| Empty primary | `20240502..20240601` | no rows |
| Shifted cover A | `20240501..20240531` | no rows |
| Shifted cover B | `20240531..20240601` | no rows |

The non-empty control proves the profile returns rows when rows exist; the two
date-shifted covers corroborate that the primary partition is genuinely empty.
This qualifies the runtime to use the profile only as the nearest non-empty
control and the bounded shifted cover for a positive-high-water empty primary
partition. It does not qualify a wider date window, change the universal
31-day cap, establish a compatibility claim, or establish a sizing rule.

**Parser trap.** An empty witness response also contains `CMPINFO` with bare
`<VOUCHER>0</VOUCHER>`. Counting `VOUCHER` over the whole document sees one
element and reverses the verdict. This was observed three times in the
qualification session. Count only `VOUCHER` start elements inside `<DATA>` and
deserialize through `BODY.DATA.COLLECTION`; the implementation does both.

---

## 12a. Bill-wise semantics, the native reports, and volume — live measurement 2026-08-02

**VERIFIED 2026-08-02** against TallyPrime Edit Log 7.0 EDU on port 9000, using
`Bridge Ageing Lab` — a company created for this session, every shape placed deliberately —
and `Aarav Trading Company Demo` for scale. All data synthetic.

Several entries here **correct or qualify earlier sections**; each says which.

### 12a.1 Built-in named reports work — this qualifies §2.2's blanket AVOID

§2.2 tests a **custom** `REPORT`/`FORM`/`PART` and concludes `<TYPE>Data</TYPE>` should be
avoided. That conclusion does not extend to Tally's **own built-in reports addressed by name**:

```xml
<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>
<TYPE>Data</TYPE><ID>Bills Receivable</ID></HEADER>
<BODY><DESC><STATICVARIABLES>
  <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
  <SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY>
  <SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE>
</STATICVARIABLES></DESC></BODY></ENVELOPE>
```

Accepted on the first attempt. `Bills Payable` works identically. Per-bill rows:

```
<BILLFIXED><BILLDATE/><BILLREF/><BILLPARTY/></BILLFIXED> <BILLCL/> <BILLDUE/> <BILLOVERDUE/>
```

Four properties that matter more than the speed:

1. **It was fast on the observed book.** 101,603 vouchers returned 22 rows in **0.10 s /
   5 KB**. This single measurement does not establish an asymptotic bound: voucher count,
   open-bill count, cache state, and release were not varied independently. A voucher scan over
   the same book was much more expensive, but capacity and deadline planning must treat that as
   book- and release-specific evidence.
2. **`STATUS` is emitted only on failure.** A successful response contains **no `STATUS` tag
   at all**, so you cannot assert `STATUS=1` as §2.1 allows.
   **But absence of `STATUS` does not prove success.** An unrecognised report `ID` returns a
   bare `<RESPONSE>Unknown Request, cannot be processed</RESPONSE>`, which also carries no
   `STATUS` — a parser treating "no `STATUS`" as success would read it as a report with zero
   bills and silently publish no exposure. **Require the expected report structure, and reject
   `RESPONSE` or `LINEERROR` independently of `STATUS`.**
3. **It fails closed on a company that is not loaded**, with
   `LINEERROR: Could not set 'SVCurrentCompany' to '<name>'` — see 12a.6.
4. **It carries no GUID anywhere**, so the response cannot be identity-bound the way a
   voucher or ledger collection can. Bracket every candidate native read with GUID-verified
   extent probes before and after it, and reject drift. Bracketing detects some changes; it does
   not create an atomic source cut.

An unrecognised report `ID` returns a clean `<RESPONSE>Unknown Request, cannot be
processed</RESPONSE>` — **not** the modal-dialog hang of §1.2. The hazards are different
failure paths and should not be conflated.

**They scope by balance SIGN, not by party group.** On the demo book, each report included
parties from both Sundry *Creditors* and Sundry *Debtors*. That is correct accounting — a
supplier advance is a debit balance — but a screen labelled "receivables" that renders this
list is showing every debit-balance bill, including advances paid out. The measurement records
rows, not a complete distinct-ledger census.

### 12a.2 Which date each allocation kind ages from

Measured against Tally's own *Ledger Outstandings* screen and both native reports, not
inferred from the data model.

| Kind | Ages from | Note |
| --- | --- | --- |
| `New Ref` | `BILLDATE` | which on an opening equals the voucher date |
| `Agst Ref` re-opening a settled bill | **the original bill's `BILLDATE`** | not the voucher that reused it |
| `Advance` | `BILLDATE` | always its own voucher date; see 12a.4 |
| `On Account` | **not aged at all** | shown at the *report* date, blank overdue |

The second row was contested in the codebase. Constructed explicitly — a bill opened at
3,000, settled to exactly zero, then reused by a later `Agst Ref` for 1,500 — Tally reports:

```
1-Jun-26  ZBR  3,000.00 Dr opening  1,500.00 Cr pending  Due on 1-Jun-26  Overdue 60
```

Aged from **1-Jun-26**, the original bill, though the reusing voucher is dated 1-Jul-26.
Confirmed three ways: on screen, in `Bills Receivable`, and in `Bills Payable`.

The mechanism is 12a.4: an `Agst Ref` **carries** the original bill's `BILLDATE`, so ageing
from `BILLDATE` reaches the true opening without special handling.

### 12a.3 Tally offers two ageing methods, and the anchor differs

`F6: Ageing Method` offers *Ageing by Bill Date* and *Ageing by Due Date*, and the choice
changes the buckets. On a bill dated 1-May-26 with a 30-day credit period, due 31-May-26:

| as-of | method | days | bucket |
| --- | --- | --- | --- |
| 2-Jul-26 | by Due Date | 32 | 30 to 60 |
| 2-Jul-26 | by Bill Date | 62 | 60 to 90 |

Both are correct. Neither is "the" answer, so **any tool computing ageing must state which
basis it used.**

Licensed-gateway capture/read-back evidence on **2026-08-23** establishes these accepted units:

| entered value | returned `BILLCREDITPERIOD` |
| --- | --- |
| `2 Months` | `2 Months` |
| `3 Weeks` | `3 Weeks` |
| `1 Day` | `1 Days` |

The same licensed TallyPrime 7.1 import/read-back on **2026-08-23** measured the
day-unit persistence ceiling and confirms that it is not a shared ceiling for
other units:

| entered value | returned `BILLCREDITPERIOD` |
| --- | --- |
| `365 Days` | `365 Days` |
| `3650 Days` | `3650 Days` |
| `9999 Days` | `9999 Days` |
| `10000 Days` | empty |
| `100 Months` | `100 Months` |
| `1000 Months` | `1000 Months` |

Tally silently discards a day count above `9999` to an **empty** credit period.
That is not neutral: due-date ageing then uses the bill date as the due date,
which can move the bill to a different ageing bucket. The parser therefore
rejects day counts above the measured `9999` ceiling, but does not invent a
matching weeks or months ceiling; those units are constrained by checked
resulting-date arithmetic instead.

The voucher parser accepts explicit `N Day(s)`, `N Week(s)`, and `N Month(s)` forms only. It
rejects an unknown unit rather than guessing a day count. Weeks add exactly seven days each;
months use a calendar-month operation that preserves the day of month when possible and otherwise
clamps to the target month's last day. The same licensed-gateway evidence measured:

| bill date and period | due date |
| --- | --- |
| 15-Jan-26 + 4 Weeks | 12-Feb-26 |
| 30-Jan-26 + 1 Month | 28-Feb-26 |
| 31-Jan-26 + 1 Month | 28-Feb-26 |
| 31-Mar-26 + 2 Months | 31-May-26 |

2026 is not a leap year. The 29-Feb clamp is implemented from the Gregorian calendar rule but was
not directly observed in this capture.

**`BILLOVERDUE` in the native report is not usable as an ageing oracle.** Its as-of is
Tally's own period date, not the caller's, and it does **not** follow the `F6` setting:
toggling the UI to *Ageing by Bill Date* left the exported `BILLOVERDUE` unchanged at the
due-date value. Recompute ageing from `BILLDATE`/`BILLDUE` against an explicit as-of on
every path.

### 12a.4 The import path rewrites what you send — extends §9

§9.2 already records that `ERRORS=0` does not mean success. Eight further rewrites, each
measured by writing a distinguishable value and reading it back. **Every one reported
`CREATED=1, ERRORS=0, EXCEPTIONS=0`.**

| # | Behaviour |
| --- | --- |
| 1 | `BILLDATE` on a bill-**opening** allocation is overwritten with the voucher date; a differing supplied value is discarded |
| 2 | `BILLDATE` on an `Agst Ref` is **inherited** from the settled bill — so it legitimately differs from the voucher date |
| 3 | The allocation **kind** is rewritten when the reference name already exists: `Advance` naming an existing ref is stored as `Agst Ref` |
| 4 | `On Account` names are **stripped** — it is structurally unnamed |
| 5 | An allocation on a ledger with `ISBILLWISEON=No` is **silently discarded**; the entry stores with no allocations. Treat `No` as a mandatory fail-closed preflight for any bill-wise write |
| 6 | `VOUCHERNUMBER` is **overridden** with Tally's own per-type sequence — **under automatic numbering only.** §9.8 establishes that Manual + `PREVENTDUPLICATES=Yes` preserves it verbatim. The company measured here used the default, so this observation does not generalise; the numbering method decides it |
| 7 | On the automatically numbered voucher type measured here, `ACTION="Alter"` carrying the target `GUID` returns `CREATED:1, ALTERED:0` — it creates a duplicate rather than editing. This is not a numbering-method-independent result; see §9.8 |
| 8 | The company-level `Enable Bill-wise entry` gate does **not** apply to XML import — see 12a.5 |

Consequence for §9 generally: **a write is only verified by reading it back.** Counters
describe the operation, not the result.

One error message is actively misleading. A voucher dated on a day Education forbids returns:

```
CREATED=0  ERRORS=0  EXCEPTIONS=1
LINEERROR: Voucher date is missing for: 'Sales' voucher BAD-1.
```

The date was present and well-formed — it was illegal for the mode. Note `ERRORS=0`; the
failure lives in `EXCEPTIONS`.

### 12a.5 Configuration is not a diagnostic, in either direction

`F11 → Enable Bill-wise entry` set to **No** on a company holding bill-wise data, confirmed
on screen, changed **nothing** observable over XML: per-ledger `ISBILLWISEON` stayed `Yes`,
existing allocations survived, both native reports returned identical rows, and a **new**
allocation imported afterwards was still accepted and retained.

So the company flag gates Tally's own voucher-entry screen, not the API or the data. It is
also **not fetchable over XML**, so a client could not read it even if it meant something.

And the per-ledger flag is not a sufficient **positive** diagnostic. `ISBILLWISEON=Yes` does
not prove that a ledger has bill references, but `No` remains a mandatory fail-closed signal
before a bill-wise XML write. On the 101,603-voucher book:

```
party ledgers (Sundry Debtors / Creditors) : 60
  ISBILLWISEON = Yes : 60
  ISBILLWISEON = No  :  0
  absolute closing balance                 : 208,232,027.79
```

Every ledger correctly configured — yet only **7 of 60** carry any bill reference at all.

### 12a.6 The unallocated remainder, and how to see it

Because a voucher can post to a bill-wise ledger without allocating to a reference, a book
can be fully configured and almost entirely unallocated. On the demo book the residual is
**≈ ₹2.83 crore, about 98.4% of debtor balance** — and it is **invisible to both native
reports**, which return no error.

The residual is recoverable:

```
On Account = ledger CLOSINGBALANCE − Σ BILLCL
```

Verified to the paisa on every bill-carrying party — **at a current as-of only.** This is a
cross-request candidate calculation, not an atomic source cut: a voucher can change between
the report and ledger reads. Bracket the reads with unchanged content/high-water evidence and
retain the non-atomic qualification; no cross-request combination becomes `Verified` solely
because its arithmetic agrees.

**CORRECTION — 2026-08-24.** §7 now establishes that `CLOSINGBALANCE` is as-of scoped, so
this identity can be evaluated at a historical as-of when the ledger snapshot and `BILLCL`
report use the same validated `SVFROMDATE`/`SVTODATE` window. Mixing windows would still
misclassify later activity as `On Account` and silently overstate the residual. The ledger
read costs **0.08 s / 36 KB** for 88 ledgers. A candidate as-of view needs at least both
sign-scoped native reports plus the ledger read, and separate identity bracketing; it must not
be described as full atomic coverage.

**Any outstandings figure that omits this is silently incomplete.** Depending on the residual's
sign, it can understate receivables, overstate them, or hide a payable position. On the observed
debtor-heavy book a native-report-only answer would present **1.6%** of the position as complete.

### 12a.7 The `Company` collection ignores `SVCURRENTCOMPANY` — qualifies §9.11

§9.11's table records a *non-existent* company name returning 0 rows on a `Ledger`
collection. A `Company` collection behaves differently: it **enumerates every loaded
company regardless of `SVCURRENTCOMPANY`**.

So requesting an existing-but-**unloaded** company returns the loaded company's rows, with
`STATUS=1`, in under 100 ms. Reading "row 1" binds to the wrong book. This is the default
state after a Tally restart, which reopens only the last-active company.

The native report path does **not** share this — it fails closed with
`Could not set 'SVCurrentCompany'`.

**No working XML company-load path was observed.** `<TALLYREQUEST>Load Company</TALLYREQUEST>`
returns `STATUS=0` with empty `DATA`; an `SVLOADCOMPANY` variable returns `STATUS=1` with
`<COMPANY>0</COMPANY>`. Those two attempts establish only that these shapes do not load a
company. Until a working supported path is observed, a client must detect the condition and
instruct the operator to open the company on the server by hand.

### 12a.8 Payload scales with voucher count; measure request time separately

The wildcard voucher fetch averaged **~21.7 KB per voucher on this corpus**, stable across a
360× range.

**Treat that as a mean, not an upper bound.** Per-voucher cost varies with content — §11 already
records this — so a book with nested inventory lines or long narrations will exceed it. A segment
projected under a transport cap using this constant can still breach it after dispatch. Sizing
must use **observed** bytes for the profile in hand, with headroom, and split adaptively when a
projection proves wrong.

| AlterID range | vouchers | payload | time | bytes/voucher |
| --- | --- | --- | --- | --- |
| 0..500 | 9 | 0.17 MiB | 1.30 s | 19,482 |
| 0..2000 | 78 | 1.59 MiB | 1.77 s | 21,390 |
| 0..8000 | 369 | 7.63 MiB | 6.72 s | 21,685 |
| 0..20000 | 948 | 19.63 MiB | 9.92 s | 21,713 |
| 2-day window, unbounded | 3,264 | 67.67 MiB | 43.67 s | 21,740 |

**A date window is not a volume bound.** A 2-day window on a dense book returned 67.67 MiB
in 43.67 s; a 31-day window returned 101.5 MiB in 81.3 s. Both exceed every stated response
limit, and both are ordinary requests.

**A pre-flight count reduced response payload on the observed windows.** A minimal `FETCH`
over those windows returned the observed voucher count with about **1/17th** the wildcard
response payload. Its elapsed-time reduction was measured separately and is not a general
cost estimate:

| window | minimal `FETCH` | wildcard | payload / elapsed ratio |
| --- | --- | --- | --- |
| 2-day | 3.98 MB / 2.61 s | 67.67 MiB / 43.67 s | 17× / 17× |
| 31-day | 5.96 MB / 5.74 s | 101.5 MiB / 81.3 s | 17× / 14× |

Incidentally, `FETCH ALTERID` and `FETCH GUID, ALTERID, DATE` return **byte-identical**
responses — Tally emits a fixed minimal envelope regardless of which narrow fields are
named. The wildcard is a **17.8× amplifier** on that baseline.

The count response can itself exceed a response limit on a dense book, or be incomplete while
appearing successful. Bound and completeness-check the count probe before using it; otherwise
recursively partition that probe too. Even then, observed bytes per voucher are a planning
estimate, not a bound. A pre-flight projection does not replace the closed streaming transport
boundary: it must fail closed when encoded response bytes exceed the cap. Planning stays upstream
to avoid dispatching likely-oversize requests; the transport cap remains the final safeguard.

**Elapsed time does not follow the same model, and subdivision does not reduce it.** The cost
has a large fixed component: Tally scans the whole collection regardless of how many rows match.
The 9-row request above took **1.30 s**, not the ~117 ms a purely linear model predicts, and
§11b.3 records a **zero-row** filter taking ~22 s on the large book. So time is closer to
`fixed_scan(book) + linear(rows)`, and **every subdivision pays the fixed cost again** — halving
a partition can increase total wall-clock even as it reduces peak payload.

Deadline planning must therefore model a book-dependent fixed scan cost separately from
serialization, and subdivision must be justified by payload, not by time.

> A separate stability hazard observed during this work is recorded privately rather than
> here, per the project's handling of crash triggers.

### 12a.9 A ledger GUID survived an observed UI rename — coverage must compare GUID to name

`LedgerOpeningCoverageV1` was captured immediately before and after renaming one ledger
through the Tally UI. The response contained the same six ledger GUIDs on both sides, while
exactly one GUID's `Name` changed. Therefore a count-only comparison, and a comparison of
GUID membership alone, are both blind to this master-data change.

Bridge must compare the full GUID-to-name map and return a partial result when it changes
during a scan. This catches the observed rename without treating a stable GUID as evidence
that the ledger master itself stayed unchanged.

This establishes only a GUI rename on the observed TallyPrime Edit Log EDU profile. XML
rename behaviour, other releases, and other configurations remain unverified.

**VERIFIED 2026-09-11; one synthetic company on TallyPrime 7.1 licensed Silver
(`education_mode: false`), read through `StandardLedgerCatalogV1`.** Two of the axes named above
are now observed. One ledger was renamed in the UI and then restored; the ledger's GUID and master
slot were unchanged, the ledger count held, and no other ledger's GUID moved — a rename in place,
not a delete-plus-create. So the finding holds across two SKUs and two read profiles. The
before/after responses are retained as
`fixtures/agent/native-ledger-catalogue{,-renamed}.utf16le.xml` and drive the binding revalidation
regression test.

Three further facts fell out of that capture. They are marked separately because two of them are
generalisations from a single company, and the rule behind them is not established:

- **PARTIAL.** A ledger GUID appears to be **company-scoped with a master suffix** —
  `<company GUID>-000000d0`. The composition was observed on every ledger of the one company
  captured, so the *shape* is verified there; that it holds across other companies, releases and
  SKUs is inferred, not tested. If it does hold, a ledger GUID is meaningful only within its
  company and the company GUID is recoverable from it — and since company GUIDs are not unique
  across split companies (§9.11b), a ledger GUID inherits that ambiguity.
- **VERIFIED 2026-09-11; the nine ledgers in the captured company.** Ledger rows carry
  `RESERVEDNAME` and use it **the same way group rows do** — populated for a reserved master, empty
  for one that is not. Eight of the nine are empty; `Profit & Loss A/c` carries
  `RESERVEDNAME="Profit &amp; Loss A/c"`.

  This is the three-state convention already implemented in
  `TallyNamedMaster::reserved_name`: a non-empty value is a trustworthy reserved identity, an
  **empty** value is Tally's own explicit signal that the row is *not* a reserved master, and an
  absent attribute is no signal at all. A parser must therefore key on neither the attribute's
  presence nor an assumption of emptiness.

  Note that "reserved" is narrower than "auto-created": `Cash` is created by Tally and its
  `RESERVEDNAME` is empty, so Tally itself declines to treat it as reserved. Do not infer
  reservation from a ledger having appeared without the operator creating it.

  An earlier revision of this section claimed ledgers emit `RESERVEDNAME` *always empty*. That was
  wrong, and contradicted by the very fixtures it cited.
- **UNVERIFIED — the `CMPINFO` counters, and a claim withdrawn.** An earlier revision of this
  section said the response was "identical except `<CMPINFO><LEDGER>` advancing 60 → 62", read that
  as a monotonic per-master-type alteration counter, and marked it PARTIAL. **Both the figure and
  the interpretation are withdrawn.** The committed fixtures show neither. Diffing them gives six
  changed lines, and five of them are `CMPINFO`:

      <LEDGER>0</LEDGER>               ->  <LEDGER>61</LEDGER>
      <CURRENCY>0</CURRENCY>           ->  <CURRENCY>14</CURRENCY>
      <TAXUNIT>0</TAXUNIT>             ->  <TAXUNIT>20</TAXUNIT>
      <VOUCHERNUMBERSERIES>0</...>     ->  <VOUCHERNUMBERSERIES>20</...>
      <VOUCHER>0</VOUCHER>             ->  <VOUCHER>19</VOUCHER>

  (the sixth is the renamed ledger itself). A counter that is **zero across every master type** in
  one capture and populated in the next is not an alteration count incremented by the rename, and
  `61` does not correspond to the nine ledgers the collection returns. What `CMPINFO` reports here,
  and why one capture reports all zeros, is **not established** — do not build on it.

  **The actionable consequence survives, and is strengthened.** A "restore and prove nothing
  changed" check must compare master **identity**, never a response hash or byte equality: fields
  outside the master set differ between two captures of the same company for reasons this reference
  cannot yet explain. That is a stronger reason to compare identity than the withdrawn one.

**UNVERIFIED — XML-driven rename.** Neither capture used one; both renames were performed in the
UI. Deletion was not exercised at all. Per P6, neither may be built upon.

---

## 13. Open questions

| Question | Why it matters |
| --- | --- |
| Does editing a voucher populate `AUDITENTRIES.LIST`? | Could replace AlterID diffing entirely on the Edit Log SKU |
| Is there a syntax for two-level `FETCH`? | Decides the 6.3× GST payload penalty |
| Why does `ClosingBalance` render empty at some dates? | Blocks trusting any balance field |
| Which report attribute makes rendering work? | Only matters if custom reports are retained |
| Behaviour on standard TallyPrime and Tally.ERP 9 | Every finding here is single-SKU |
| Behaviour with third-party TDL installed | The demo instance has none; client machines will. Could alter the report surface §12a.1 depends on |
| Is the crash observed on 2026-08-02 volume-driven or UI-driven? | It did not reproduce on an identical repeat; a UI keypress during generation is at least as likely. Recorded privately |
| What is the correct key for `ACTION="Alter"` on a voucher? | On automatically numbered types observed, `GUID`, `REMOTEID`, `MASTERID`, and the `REMOTEID`/`MASTERID` combination have each created duplicates (§9.7, §12a.4). Manual + `PREVENTDUPLICATES=Yes` instead rejects the failed Alter (§9.8); other request shapes and licensed-SKU behavior remain untested |
| Does the `On Account` residual identity hold on a bill-dominated book? | Verified on a book that is 98% unallocated; the opposite composition is the case where a false zero would look like success |
| What do the `CMPINFO` counters report, and why can one capture return all zeros? | Two catalogue captures of the same company minutes apart returned `LEDGER` 0 then 61, with `CURRENCY`, `TAXUNIT`, `VOUCHERNUMBERSERIES` and `VOUCHER` likewise 0 then populated. Until this is explained, no two responses can be compared by hash or byte equality (§12a.9) |

---

## 14. Changelog

| Date | Change |
| --- | --- |
| 2026-07-29 | Created. All VERIFIED entries established this day against TallyPrime Edit Log 7.0 EDU. |
| 2026-07-30 | Recorded the outstandings-only wildcard exception, curated bill-type corruption, and the contextual polarity finding. |
| 2026-08-02 | Added §12a from a live measurement session: built-in named reports (qualifying §2.2), per-kind ageing semantics, the two ageing methods, eight import rewrites (extending §9), configuration as a non-diagnostic, the unallocated remainder and its recovery, the `Company` collection ignoring `SVCURRENTCOMPANY` (qualifying §9.11), and a linear volume model with a cheap pre-flight count. |
| 2026-08-22 | Updated §5.3 with the observed Education `{1,2,31}` boundary rule and the limited TallyPrime Silver arbitrary-day observations; this settles #115 item 1 for the recorded profile. |
| 2026-08-28 | Added §8.1's read-only ledger-master field-presence observation and explicit public-fixture privacy boundary. |
| 2026-09-10 | Added §9.13's Payment/Receipt/Contra import shapes from a licensed 7.1 Gold bank-statement import, and §8.2b's `RESERVEDNAME` group-identity rule that its cash/bank gate is built on. |
| 2026-09-11 | Extended §12a.9 to TallyPrime 7.1 licensed Silver and the `StandardLedgerCatalogV1` profile from a live rename/restore capture (VERIFIED), and recorded three structural facts with separate markers: ledger `RESERVEDNAME` follows the same reserved/not-reserved convention as groups, one of nine populated (VERIFIED), company-scoped ledger GUIDs (PARTIAL — verified on all nine rows of one company). XML-driven rename and deletion remain UNVERIFIED. A later revision the same day withdrew a `CMPINFO` alteration-counter claim that the committed fixtures did not support. |
| 2026-09-11 | Narrowed §9.13's company-guard paragraph to match §9.11d: which *kind* of mismatched `SVCURRENTCOMPANY` posts silently is UNVERIFIED, so the classification by name shape was withdrawn, and the pre-write check was corrected from the GUID alone to the whole §9.11b identity tuple. |
| 2026-09-18 | Added §8.2c: `REFERENCE`/`ISPOSTDATED`/`ISINVOICE`/`PARTYGSTIN` added to the voucher `FETCH` list and captured on licensed TallyPrime 7.1 Silver (`BRIDGE SHAPE LAB`, 67 vouchers, twelve windows). `ISINVOICE` never carries `TYPE="Logical"`, unlike the other three; `PARTYGSTIN` round-trips but was empty on every observed row (population UNVERIFIED). |
| 2026-09-18 | Added §1.1(d): the rule `mark_forbidden_numeric_references` applies before parsing, now public in `bridge-tally-protocol`, including that a `&#` with no `;` in its window no longer ends the rewrite. |
| 2026-09-18 | Added §11c: the pre-flight volume bound for windowed voucher reads, with the bytes-per-voucher measurements behind it (a whole-year 35.0 KB mean on an inventory-heavy book, understated by a one-day probe; half-month windows at 98.4% of the cap). The measurements are VERIFIED; the date-first census and the AlterID-narrowed parts the rule sends are UNVERIFIED live. |
| 2026-09-21 | §11c after the bridge#520 rectify: every census is bounded before it is sent (one date census when the mark fits one, otherwise AlterID spans of 8,192 sized against the whole cap; a mark needing more than 256 spans is refused as `voucher_window_book_too_large`); parts are admitted against the census and their union; the read allowance is spent at dispatch; a divided read is bracketed on `ALTVCHID` and `ALTMSTID`; a replay carries and closes against the first read's witness; the pre-post check refuses a window the bound would divide. Added §11c.5, the first live evidence (licensed 7.1 Silver lab): census and span shapes, `ALTVCHID` as a count bound and on every voucher change, a ledger rename moving only `ALTMSTID`, and end-to-end timings. |
