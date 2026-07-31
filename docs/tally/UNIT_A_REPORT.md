# Unit A — Outstandings Implementation Report

Status: **in progress, unmerged**  
Branch: `feat/tally-outstandings-slice`  
Base: `d9ee4aa` (`fix/tally-p0b-rectify-20260729`), which is not merged into `master`

## Implementation and evidence

| Item | Built | Evidence | Remaining |
| --- | --- | --- | --- |
| Request | Closed `VoucherOutstandingsV1` collection definition, pinned company, a typed narrow date partition plus `$AlterID > N AND $AlterID <= M`, one outstandings-only `ALLLEDGERENTRIES.*` variant, and no compute/function-expression slot. A broad `DateWindow` cannot cross the sealed request boundary; only a `NarrowDateWindow` of at most 31 calendar days can. Date legality is compatibility-profiled: detected Education remains restricted to day 01/02/31, while licensed/unknown mode permits ordinary boundaries and relies on I12. The paired company extent requests `ALTVCHID` as the voucher high-water. The sealed request type is the only input admitted to the 40 MiB transport method; the general cap stays 32 MiB | Request-shape tests reject missing date/AlterID filters, compute, self-count and curated bill paths; compile-fail tests prove neither raw XML nor a broad reporting period can become a sealed request; ruling-4 regressions accept day 15 in mode-agnostic mode and reject an out-of-span returned voucher as Partial | Calibration on the ordered bill-bearing corpus and a complete two-dimensional live scan on both ports |
| Parser | Strict collection-envelope parser with narrow replacement of illegal XML numeric character references | Real `&#4;` capture; real wildcard capture with 75 vouchers, 28 `New Ref`, 24 `Agst Ref` | None for the verified response shapes |
| Completeness | `CompleteSegment` / `CompleteScan` distinct from `PartialScan`; paired encoded SHA-256, byte length and row agreement; company pin; exact contiguous `0..ALTVCHID` coverage inside every narrow date partition; exact contiguous/non-overlapping coverage of the full reporting period; duplicate GUID/AlterID rejection; latency trend stop. A paired zero-row read produces an AlterID-range `EmptySegmentCandidate`; runtime permits one paired adjacent wider range, reuses its adjacent rows, never retries or widens recursively, and returns Partial on every mismatch | Compile-fail doctests; date-partition and AlterID-coverage tests; same-length wire-mutation rejection; out-of-range row rejection; contradiction and wider-pair promotion tests | Two-dimensional segmentation remains uncalibrated and unverified as a complete live scan; native-report reconciliation remains blocked |
| Compute | Exact-decimal bill lifecycle, receivable/payable totals, four receivable ageing buckets, top parties and oldest-bill age | Real wildcard fixture computes 223,055.4 receivable / 295,424.8 payable; bounded live ports both compute 600 / 600 | Manual reconciliation of the complete-book result against Tally's outstandings report |
| Surface | One thin Tauri command and one outstandings screen with totals, ageing, top exposure, freshness, loading, error and Partial states | TypeScript/Vite build from the current worktree; command delegates to runtime and owns no transport handle | Complete live book must be available before the screen can render final totals |

## Live evidence

The identical `20260401..20260401` wildcard request was run once as a paired read on each
Education instance:

| Port | Product | Rows | Encoded bytes | Paired elapsed | Bill types | Result |
| --- | --- | ---: | ---: | --- | --- | --- |
| 9000 | TallyPrime Edit Log 7.0 EDU | 7 | 94,464 | 118 / 106 ms | `On Account: 12` | receivable 600; payable 600 |
| 9001 | TallyPrime 7.1 EDU | 7 | 99,813 | 110 / 102 ms | `On Account: 12` | receivable 600; payable 600 |

The first and second response are byte-identical within each port. The response bytes differ
between SKUs, while the parsed and computed result agrees. These are retained **pre-R1**
wildcard/parser measurements: they did not carry the new AlterID partition and therefore do not
confirm two-dimensional segmentation. The retained ignored Aarav probe is now explicitly
protocol/completeness-only, uses the already measured one-day `0..400` shape, refuses dispatch
if either predicate is absent, and labels its output as forbidden for sizing calibration.

The first owner-run complete-book check on port 9000 after the 40 MiB policy change returned
`Partial` with `tally_segment_deadline_restart_recommended` after 20.06 seconds. After the
operator restarted Tally and confirmed the server was fully loaded, one new sync returned the
same typed `Partial` after 20.09 seconds. Neither run retried, neither emitted totals, and port
9001 was initially withheld. An earlier owner-run complete-book check on port 9001 also returned
`tally_segment_deadline_restart_recommended` after 20.06 seconds, without totals or retry.

Owner ruling 2 isolated the structural failure. The exact wildcard request for
`20240401..20240401` returned 1,632 vouchers / 4,894 allocations in 61.7 seconds and 33.7 MB.
The only correct payload shape therefore cannot fit the immutable 20-second deadline at the
minimum one-day date granularity. The cheaper named bill-allocation shapes are not alternatives:
they return the same allocation-block count while silently emptying `BILLTYPE` and losing
allocation value.

Owner ruling 3 retracted AlterID-only segmentation after measuring the same `0..400` span at
31.5 seconds with whole-book dates and 0.7 seconds with a one-day window. One-day spans of
`0..25,000` and `0..50,000` then took 69.3 and 41.9 seconds, proving that cost follows the
AlterID span scanned, not rows returned, and is non-monotonic. The request boundary now requires
both a narrow date partition and an AlterID range. The runtime has no production initial width:
it returns typed Partial `outstandings_segment_sizing_uncalibrated` before endpoint admission
until the ordered bill-bearing corpus produces reviewed calibration evidence. Aarav is retained
for protocol, completeness and failure-mode checks only and cannot tune the policy.

Owner ruling 4 corrected the universal date-boundary type. The day 01/02/31 restriction now
belongs only to an explicitly detected Educational compatibility profile. Licensed or unknown
mode accepts ordinary boundaries, including an as-of date on day 15, while I12 still withholds
completion if any returned voucher lies outside the requested span. The 31-day cap is unchanged.

## P4 accounting

Reused:

- `bridge-tally-core::ExactDecimal` and `TallyDate`;
- validated company-name and loopback transport boundaries;
- the existing single-attempt runtime/endpoint serialization path;
- the existing Tauri shell and selected-company state.

Deliberately not reused:

- old report renderers and report-shape parsers, whose live shape is incompatible;
- the mirror/schema path, because Unit A is explicitly in-memory;
- curated bill-allocation fetches, because live A/B evidence proves their values are wrong;
- ledger polarity as an outstandings invariant, because live captures prove it is contextual.

Deletion: Unit A is additive by owner ruling; old renderers remain untouched for Unit C. Within
the new slice, per-row computes and ledger-polarity fields/checks were removed after live
evidence invalidated them.

What breaks if Unit A is not built: Bridge retains no live-proven business read and no first
demo screen; the existing evidence UI still cannot answer an accountant's outstandings
question.

Code/test net LOC against the unmerged base: **+4,919** (`+1,086/-49` in tracked implementation
files plus 3,882 lines in new Rust/TypeScript implementation and test files). This excludes
authority documents, captured XML fixtures and generated schemas. The worktree was already
dirty, so this scoped count intentionally does not attribute owner-authored authority changes
to Unit A.

## Verification

- Rust 1.96 protocol and outstandings suites: pass.
- Complete-scan, sealed-request and broad-date-request compile-fail doctests: pass.
- Runtime segmentation and typed-failure tests: pass.
- Ignored live-test target compilation: pass.
- Workspace clippy with warnings denied: pass.
- Current offline `cargo test --workspace`: pass across the workspace, integration suites and
  three compile-fail doctests. Listener-backed tests used the already granted local-listener
  permission; no Tally endpoint was contacted. The focused protocol/runtime checks also pass,
  and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- Frontend type check and production build: pass.

## Remaining evidence and assumptions

1. The owner-approved safety policy is implemented: 40 MiB only for the sealed wildcard
   request, 28 MiB target, unchanged 20-second deadline, no production initial AlterID width,
   no tuning from a single sample, and one adjacent paired wider AlterID read with adjacent-row
   reuse. The uncalibrated production path emits no wildcard segment request.
2. Aarav may still provide protocol/completeness and failure-mode evidence, but no timing from
   it may choose or change a segment width. The already measured one-day `0..400` result is not
   calibration evidence and must not be rerun without a separate protocol reason.
3. R2 is blocked: the purpose-built 200–500-voucher bill-bearing company in
   `TEST_CORPUS.md` §4 does not exist yet. The ignored exit check now refuses Aarav and retains
   non-zero, party-bearing reconciliation assertions for that future company. It must be
   generated strictly in ascending date order before it can calibrate segment sizing.
   `unit_a_ordered_corpus_calibration_sample` is ready but ignored: each manual invocation
   refuses Aarav and broad dates, bounds `ALTVCHID` to the reviewed small-corpus range, performs
   one paired sealed wildcard read with status checks before, between and after, retains exact
   non-overwriting evidence files, reserves the sample identity before any Tally contact so a
   timeout cannot be rerun under the same ID, and reports measurements without changing policy.
4. Final complete-book totals have not been reconciled by hand against Tally's native
   outstandings report.

## Exit-criterion audit

| Requirement | Current evidence | Audit result |
| --- | --- | --- |
| Correct branch and base declared | Branch and unmerged base are recorded above and match the current worktree | Proven |
| One collection voucher profile, pinned and date-filtered | Closed request definition and retained bounded requests; both live ports returned exactly seven in-window vouchers | Proven for the verified bounded shape |
| No self-reference or per-row compute in Unit A | The closed collection type has no function/compute slots; compile and request-shape tests enforce it | Proven for Unit A; owner ruled legacy cleanup belongs to Unit C |
| Wildcard bill data is real and typed | Retained live wildcard fixture includes 28 `New Ref` and 24 `Agst Ref`; curated corruption has an A/B record | Proven |
| Tolerant invalid-character-reference parsing | Retained real `&#4;` capture and focused tolerant-parser regression | Proven |
| Partial cannot become complete implicitly | Distinct typestates and compile-fail doctest | Proven |
| Paired reads detect truncation or mutation | Encoded SHA-256, byte length, raw row count and parsed rows must agree; same-length mutation regression fails closed | Proven offline; bounded pairs agreed live on both ports |
| Empty partitions are corroborated once | AlterID-range candidate, one adjacent paired wider read, adjacent-row reuse, no recursive widening/retry, typed Partial mismatches | Proven offline; live path pending |
| Segments are contiguous and non-overlapping | `NarrowDateWindow` partitions prove exact reporting-period coverage; each partition separately proves half-open/closed `0..ALTVCHID` coverage. Synthetic calibrated-policy tests prove contiguous ranges, terminal clamp, three-sample minimum tuning and shrink-only sizing | Proven offline; initial width intentionally absent in production |
| Date-boundary compatibility and I12 | Recognized Tally product plus detected Education/Educational mode selects strict day 01/02/31 boundaries; licensed/unknown/absent/inconsistent evidence accepts arbitrary valid dates. A live-capture-derived row moved beyond an accepted day-15 window becomes typed Partial | Proven offline; licensed Tally behavior remains untested by ruling |
| 40/32/28/20 resource policy | Sealed outstandings request is the only 40 MiB API input; general cap 32 MiB, target 28 MiB, deadline 20 s | Proven offline |
| Exact outstandings calculation | Exact-decimal lifecycle tests and real wildcard fixture totals | Proven for retained captures |
| Tauri command and screen | Thin command delegates to runtime; current TypeScript and Vite production build pass | Proven buildable; final live rendering pending |
| Complete-book run on ports 9000 and 9001 | Superseded paths returned typed `Partial` at the deadline. Ruling 3 supplies direct one-day/whole-book two-predicate measurements, but no calibrated complete scan has run | Blocked on the ordered bill-bearing corpus and calibration ruling |
| Ordered-corpus calibration evidence | Ignored one-sample harness compiles and enforces a non-Aarav company, one narrow window, a 200–600 `ALTVCHID` safety bound, pre-contact sample reservation, sealed 40 MiB request, paired bytes and inter-request health checks; it never retries or derives policy | Ready offline; no sample run until the corpus exists |
| Full current Rust workspace test run | Current `cargo test --workspace` completed with no failures, including listener-backed transport tests and three compile-fail doctests; workspace clippy is clean with warnings denied | Proven |
| Native Tally report reconciliation | Requires final complete totals and an operator-visible native report | Missing |

Unit A is therefore **not complete**. Offline ruling-3 rectification is implemented. The next
performance evidence must come from the purpose-built bill-bearing company after it is generated
in ascending date order; Aarav must not be used for sizing. That corpus must first calibrate a
safe initial two-dimensional segment policy, then the ignored exit check must run against both
live instances and the result must be compared with Tally's native report.

## Migration, security and rollback

- Migration impact: none; no schema or persisted mirror state is added.
- Security impact: read-only loopback traffic; no credential, DSC, write or cloud path. Company
  identity must match the observed GUID before voucher dispatch.
- Rollback: remove the additive command registration, outstandings screen/profile modules and
  navigation entry. The old read implementation remains compiled and untouched.
