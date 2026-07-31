# Unit A — Outstandings Implementation Report

Status: **in progress, unmerged**  
Branch: `feat/tally-outstandings-slice`  
Base: `d9ee4aa` (`fix/tally-p0b-rectify-20260729`), which is not merged into `master`

## Implementation and evidence

| Item | Built | Evidence | Remaining |
| --- | --- | --- | --- |
| Request | Closed `VoucherOutstandingsV1` collection definition, pinned company, a typed narrow date partition plus `$AlterID > N AND $AlterID <= M`, one outstandings-only `ALLLEDGERENTRIES.*` variant, and no compute/function-expression slot. A broad `DateWindow` cannot cross the sealed request boundary; only a `NarrowDateWindow` of at most 31 calendar days can. Date legality is compatibility-profiled: detected Education remains restricted to day 01/02/31, while licensed/unknown mode permits ordinary boundaries and relies on I12. The paired company extent requests `ALTVCHID` as the voucher high-water. The sealed request type is the only input admitted to the 40 MiB transport method; the general cap stays 32 MiB | Request-shape tests reject missing date/AlterID filters, compute, self-count and curated bill paths; compile-fail tests prove neither raw XML nor a broad reporting period can become a sealed request; ruling-4 regressions accept day 15 in mode-agnostic mode and reject an out-of-span returned voucher as Partial; impossible `20240631` is rejected and partitioning walks through `20240630` with `next_day` | Production sizing remains blocked on a larger ordered/local corpus that actually segments. The selected-company runtime also needs a reviewed endpoint-bound mode source before production admission; live reconciliation remains manual and feature-gated |
| Parser | Strict collection-envelope parser with narrow replacement of illegal XML numeric character references. Company extent deserializes every returned company, selects exactly one by expected GUID, and only then validates its response name/attribute; collection order is never identity evidence | Real `&#4;` capture; real wildcard capture with 75 vouchers, 28 `New Ref`, 24 `Agst Ref`; real-capture-derived multi-company ordering and duplicate-GUID regressions | None for the verified response shapes |
| Completeness | `CompleteSegment` / `CompleteScan` remain distinct from `PartialScan`; paired encoded SHA-256, byte length and row agreement; company pin; exact contiguous `0..ALTVCHID` budget coverage inside every narrow date partition; and exact date-axis tiling of the full `[BooksFrom, LastVoucherDate]` reporting period. A paired zero-row AlterID slice is complete on the budget axis. Empty date partitions are admissible only inside the exact whole-scan tiling; an all-empty whole book with `ALTVCHID > 0` is typed Partial, while `ALTVCHID == 0` is Complete. Optional deletion-sensitive corroboration uses a strictly wider date window, never an adjacent AlterID range. A preflight and runtime hard cap refuse more than 128 segment pairs. Every paired extent/segment read is separated and followed by a status health check | Interior-empty completion; forged whole-book false-empty rejection; zero-high-water completion; I12/out-of-range rejection; wider-date contradiction rejection; plan-boundary and adaptive-shrink budget tests; exact complete scans on both accepted ports | Production width remains deliberately absent |
| Compute | Exact-decimal bill lifecycle, receivable/payable totals, four receivable ageing amount buckets, matching receivable bill-count buckets, top parties and oldest-bill age. As-of is an explicit typed input; production supplies today's local calendar date and the exit harness supplies `20260731` | Real wildcard fixture computes 223,055.4 receivable / 295,424.8 payable; bounded live ports both compute 600 / 600; synthetic lifecycle tests prove settled/payable bills are excluded from receivable counts and changing only as-of preserves totals/counts while moving ageing buckets; both complete live books match ₹45,14,597 / 48 / 4-4-4-36 | None for the accepted reconciliation corpus |
| Surface | One thin Tauri command and one outstandings screen with totals, ageing, top exposure, freshness, loading, error and Partial states. Partial copy reflects date-axis completeness, distinguishes pre-dispatch sizing/plan refusal from an attempted read, and retains the restart-before-next-sync instruction for deadline/latency stops | Eight frontend tests plus TypeScript/Vite build from the current worktree; focused copy regressions reject the superseded empty/adjacent explanation; command delegates to runtime and owns no transport handle | Default production remains Partial until a real segmented corpus supports a production width |

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

Owner ruling 5 accepted `Bridge Billwise Lab` for calibration and disclosed that the company
collection returns every loaded company regardless of `SVCURRENTCOMPANY`. The parser now selects
exactly one returned row by expected GUID rather than taking row one. Three separate paired
calibration invocations then used the identical port-9000 shape:

| Sample | Window | `AlterID` range | Rows | Encoded bytes | Paired elapsed |
| --- | --- | --- | ---: | ---: | --- |
| 1 | `20260701..20260731` | `0..252` | 18 | 280,221 | 106 / 114 ms |
| 2 | `20260701..20260731` | `0..252` | 18 | 280,221 | 113 / 135 ms |
| 3 | `20260701..20260731` | `0..252` | 18 | 280,221 | 111 / 130 ms |

Every pair verified Complete, every surrounding health check passed, and all six response
artifacts are byte-identical. The subsequent offline exit audit correctly withdrew the first
width proposal under the then-current AlterID-adjacency emptiness rule: no width could complete
the corpus's normal empty months. Owner ruling 6 superseded that verification axis. Completeness
now comes from exact date tiling, while AlterID ranges only tile the in-partition request budget;
paired zero-row slices no longer require an adjacent non-empty AlterID range. The runtime also
fails closed if every partition is empty while `ALTVCHID > 0`, and separately completes a truly
empty `ALTVCHID == 0` book.

Ruling 7 rejected width 252 as a production initial width. Because `ALTVCHID` is itself 252,
all three samples were whole-book reads and never exercised segmentation; they prove
repeatability, not a deadline-fitting split. Production therefore continues returning
`outstandings_segment_sizing_uncalibrated` before endpoint admission. Width 252 now exists only
behind the non-default `live-calibration-harness` feature, in a constructor named and fixed for
the ignored Billwise Lab reconciliation check; no generic width setter exists, and a default-build
compile-fail doctest proves that constructor is unreachable.

The same ruling corrected Billwise Lab's live extent to `LastVoucherDate=20260702`. The native
ageing target remains explicitly as of `20260731`, so runtime computation now accepts a typed
as-of input. Production supplies the workstation's current date; the deterministic exit check
supplies `20260731`. Before any voucher segment request, the runtime logs `D/H/W/planned pairs`
and refuses plans over 128 pairs. A second hard cap keeps adaptive shrinking from exceeding that
budget.

That audit also found the ignored exit check was too weak: it asserted only broad voucher bounds
and non-zero values. `OutstandingsReport` now includes exact open-receivable and ageing bill
counts, and the ignored check is bound before contact to ports 9000/9001 plus the accepted
company name/GUID. A Complete result must equal 220 source vouchers, as-of `20260731`,
₹45,14,597 receivable, 48 open receivable bills and ageing counts 4/4/4/36; its four monetary
ageing buckets must sum exactly to receivable total.

The first authorised port-9000 exit invocation returned typed Partial
`voucher_outside_requested_window` in 1.40 seconds and emitted no totals. Port 9001 was withheld.
The offline trace proved the direct harness had no cached product/mode evidence and therefore
used `ModeAgnostic`; its 31-day partition sequence eventually synthesised an Education-invalid
day-3 boundary. I12 caught the adjusted response exactly as designed. The harness is now bound
to the owner-attested Educational profile for the two accepted ports, while default production
retains mode-agnostic fallback and no width admission. A regression proves the corrected harness
uses only day 01/02/31 boundaries and that the superseded unprofiled sequence does not.

The same pre-live audit found that the production paired-read helpers did not actually health
check between their two requests. They now execute `read → status → read → status` for both the
extent and every voucher pair; a synthetic listener regression proves the exact sequence. With
that safety fix and the compatibility-profile cause isolated, one corrected invocation ran on
each port, with status checks before, between and after all data reads:

| Port | Planned pairs (`D=27`, `H=252`, `W=252`) | Vouchers | Encoded source bytes | Elapsed | Receivable | Payable | Open bills | Ageing bill counts |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| 9000 | 27 | 220 | 3,443,776 | 8.30 s | ₹45,14,597 | ₹1,05,000 | 48 | 4 / 4 / 4 / 36 |
| 9001 | 27 | 220 | 3,639,306 | 8.93 s | ₹45,14,597 | ₹1,05,000 | 48 | 4 / 4 / 4 / 36 |

Both reports use as-of `20260731`; both match the owner-accepted native Bills Receivable target.
The encoded payload sizes differ by SKU, while the parsed and computed accounting result is
identical. Both gateways returned the expected TallyPrime status after completion.

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

Code/test net LOC against the unmerged base: **+5,469** (`+5,520/-51`). This excludes authority
documents, captured XML fixtures, lockfiles and generated schemas. The worktree was already
dirty, so this scoped count intentionally does not attribute owner-authored authority changes
to Unit A.

## Verification

- Rust 1.96 protocol and outstandings suites: pass.
- Complete-scan, sealed-request, broad-date-request and default-build lab-constructor
  compile-fail doctests: pass.
- Runtime segmentation and typed-failure tests: pass.
- Ignored live-test target compilation under `live-calibration-harness`: pass; the exact target
  then passed once on each accepted live port after the isolated compatibility fix.
- Full feature-gated Bridge suite: 230 passed, 0 failed; the three live tests remained ignored.
- Workspace clippy with warnings denied: pass.
- Current offline `cargo test --workspace`: pass across the workspace, integration suites and
  four compile-fail doctests. Listener-backed tests used the already granted local-listener
  permission; that command contacted no Tally endpoint. The separately authorised ignored exit
  checks contacted ports 9000 and 9001 sequentially. The focused protocol/runtime checks pass,
  and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- Frontend copy/selection tests (8), type check and production build: pass.

## Remaining evidence and assumptions

1. The owner-approved safety policy is implemented: 40 MiB only for the sealed wildcard
   request, 28 MiB target, unchanged 20-second deadline, no production initial AlterID width,
   no tuning from a single sample, no AlterID-adjacent emptiness proof, and a 128-pair preflight
   plus runtime cap. The uncalibrated production path emits no wildcard segment request.
2. Aarav may still provide protocol/completeness and failure-mode evidence, but no timing from
   it may choose or change a segment width. The already measured one-day `0..400` result is not
   calibration evidence and must not be rerun without a separate protocol reason.
3. The accepted ordered corpus exists and three comparable samples prove its full 252-ID span
   is repeatable. They cannot calibrate a production width because no sample split the corpus.
   A new ordered/local corpus with a materially larger `ALTVCHID` is required; building it is
   outside Unit A.
4. The exact complete-book exit checks remain feature-gated and ignored by default. Their one
   authorised invocation per port produced 220 vouchers, 48 open bills, ₹45,14,597 receivable
   and ageing counts 4/4/4/36 on both SKUs.
5. The target-bound harness can use the owner-attested Educational profile, but the default
   selected-company runtime has no trusted mode input. Persisted company profiles provide
   endpoint/GUID identity only, the direct client probe intentionally reports `Unknown` / no
   mode, and capability snapshots can store mode but are not bound into this command. Unit A's
   in-memory scope excludes adding mirror coupling as a workaround. Default production must not
   infer mode from port or company identity. Once a width is admitted, a reviewed endpoint-bound
   mode source is also required for Educational reads to complete reliably; absent evidence
   remains mode-agnostic and I12 fails closed.

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
| Empty partitions use date-axis completeness | Every paired zero-row AlterID slice remains a complete budget slice only after byte/digest/row agreement; exact `[BooksFrom, LastVoucherDate]` tiling admits interior empty partitions. The optional I5 witness accepts only an already-Complete empty partition and a strictly wider paired date response with rows outside it | Proven offline; forged all-empty whole book with `ALTVCHID > 0` is Partial, `ALTVCHID == 0` is Complete, and an in-window wider-witness row is rejected |
| Segments are contiguous, non-overlapping and bounded | `NarrowDateWindow` partitions prove exact reporting-period coverage; each partition separately proves half-open/closed `0..ALTVCHID` coverage. Preflight computes `D × ceil(H/W)`, admits at most 128 pairs, and a non-constructible-without-admission runtime budget stops adaptive shrinking from exceeding the same cap | Proven offline; initial width intentionally absent in production |
| Date-boundary compatibility and I12 | Recognized Tally product plus detected Education/Educational mode selects strict day 01/02/31 boundaries; licensed/unknown/absent/inconsistent evidence accepts arbitrary valid dates. The first live exit attempt used the absent-evidence fallback and failed closed on an Education-adjusted span; the target harness then completed with its owner-attested Educational profile | Proven live for both accepted Education instances; licensed behavior remains untested by ruling |
| 40/32/28/20 resource policy | Sealed outstandings request is the only 40 MiB API input; general cap 32 MiB, target 28 MiB, deadline 20 s | Proven offline |
| Exact outstandings calculation | Exact-decimal lifecycle tests and real wildcard fixture totals; identical scan at two as-of dates preserves receivable/payable totals and open-bill count while changing ageing buckets; both accepted live ports match the native-report target | Proven live for the accepted corpus |
| Tauri command and screen | Thin command delegates to runtime; current TypeScript and Vite production build pass | Proven buildable; final live rendering pending |
| Complete-book run on ports 9000 and 9001 | The exact target-bound check admits corpus-bound width 252 and owner-attested Educational boundaries only under non-default `live-calibration-harness`, takes explicit as-of `20260731`, and stays ignored. Default builds have no width/profile constructor. It completed in 8.30 s / 8.93 s with identical accounting results | Proven live on both ports; encoded payload sizes differ by SKU |
| Ordered-corpus calibration evidence | Three separate port-9000 invocations used the identical `20260701..20260731`, `0..252` wildcard shape. All six reads were Complete at 280,221 bytes and 106–135 ms; surrounding health checks passed; byte-identical artifacts are retained | Repeatability proven, production sizing not proven: width equalled high-water and never segmented |
| Full current Rust workspace test run | Current `cargo test --workspace` completed with no failures, including listener-backed transport tests and four compile-fail doctests; workspace clippy is clean with warnings denied | Proven |
| Native Tally report reconciliation | Both ports produced 48 open bills, ₹45,14,597 receivable and ageing bill counts 4/4/4/36 as of `20260731` | Matches the owner-accepted target independently derived from raw XML and Tally's native Bills Receivable export |

Unit A is therefore **not complete**. The accepted corpus supplied three comparable live
samples, and ruling 6 resolved the empty-period completeness blocker on the date axis. Ruling 7
correctly withheld a production width because those samples never split the 252-ID corpus.
Production sizing remains blocked on a larger ordered/local corpus that actually exercises
segmentation. Production admission also needs a reviewed way to bind detected mode to this
endpoint-selected read path; the current stored company profile is not that evidence. The
corpus-bound, feature-gated reconciliation check itself is now proven on both accepted Tally
instances; it does not supply a default width or a general production mode override.

## Migration, security and rollback

- Migration impact: none; no schema or persisted mirror state is added.
- Security impact: read-only loopback traffic; no credential, DSC, write or cloud path. Company
  identity must match the observed GUID before voucher dispatch.
- Rollback: remove the additive command registration, outstandings screen/profile modules and
  navigation entry. The old read implementation remains compiled and untouched.
