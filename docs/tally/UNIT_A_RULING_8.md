# Owner ruling 8 — Unit A is complete; the two "blockers" are Unit B

**Date:** 2026-07-31. Answers Codex's report that rulings 6 and 7 are implemented, the live
reconciliation passed on both ports, and Unit A is "formally blocked, not complete".

---

## 1. Correctness is proven. Close it.

Verified against the working tree, not the report:

| Check | Result |
| --- | --- |
| Exit constants unweakened | `20260731` / 220 / `4514597` / 48 / `[4,4,4,36]` — unchanged |
| Assertions | all five present, plus ageing buckets must sum exactly to receivable total |
| Result on 9000 | 220 vouchers, ₹45,14,597, 48 bills, 4/4/4/36, 8.30 s |
| Result on 9001 | identical accounting, different encoded bytes (SKU), 8.93 s |
| Plan actually executed | `D=27`, `H=252`, `W=252` → **27 pairs**, under the 128 cap |
| Width admission | `for_billwise_lab_reconciliation_exit_check` is `#[cfg(feature)]`-gated, with a **`compile_fail` doctest** when the feature is off |
| Health checks | `read → status → read → status`, plus status before and after the run |

`D=27` is exactly the estimate ruling 7 asked to be turned into evidence. The target was
independently agreed by Tally's own Bills Receivable report *and* a raw-XML computation, and
Bridge now matches both, on two SKUs. **This is the exit criterion for "the numbers must be
correct". It is met.**

Two things in that run deserve to be recorded as wins rather than buried:

- **I12 earned its place.** The first authorised invocation returned typed `Partial`
  `voucher_outside_requested_window` in 1.40 s: with no cached mode evidence the harness used
  `ModeAgnostic`, whose 31-day stride eventually synthesised an Education-illegal day-3
  boundary. The invariant caught a real live fault, fast, with no totals emitted. That is a
  mode-agnostic guarantee doing exactly what ruling 4 designed it to do.
- **The pre-live audit found that paired reads had no health check between the two requests.**
  Fixed before the run, with a synthetic listener regression. That gap would have made a
  mid-pair gateway stall indistinguishable from a clean pair.

## 2. Unit A meets its stated scope. Declaring it blocked is wrong.

Unit A's scope: *one screen, one verified company, outstandings presented, numbers correct and
computed locally, **visibly tied to a complete-or-explicitly-partial read**, no filters,
drill-down or configurability.*

Neither remaining item is in that scope. A book that exceeds the request budget returns a
typed `Partial` with a reason and a restart instruction — that **satisfies** "visibly tied to
a complete-or-explicitly-partial read"; it does not violate it. Nothing in the scope promises
any book at any scale.

**Ruling: Unit A is complete.** Both remaining items move to Unit B with the evidence below.
Holding Unit A open on them keeps it open indefinitely, because neither can be closed by
writing code in this unit.

## 3. Blocker 1 is misframed — no width can unblock production

The defect is not a missing width. It is that **AlterID subdivision is unconditional**:
`let mut cursor = 0_u64;` sits *inside* the date-partition loop
([runtime.rs:1200](../../src-tauri/src/tally/runtime.rs:1200)), so every partition rescans the
full `0..ALTVCHID` range. The scan is **O(D × H)** by construction.

That is *correct* and must stay: `AlterID` is an **alteration** id, so a voucher with an old
date can carry a high id after an edit. Locality is a performance accident and can never be a
correctness assumption. But it means the AlterID axis is paid on every partition, when
ruling 3 introduced it only to subdivide a *single dense date partition* that overflowed the
budget.

Illustrative consequence, using the budget ruling 7 set — a modest real book, `H` = 20,000,
`D` = 36, `W` = 5,000: `ceil(20000/5000) × 36` = **144 pairs > 128**, refused before contact.
Bridge would decline a 20,000-voucher book. Choosing a different `W` moves the number around
but not the shape: `D × ceil(H/W)` grows without bound in `H`, and per-request cost grows in
`W`. **There is no `W` that makes this fit.** Ruling 7 said so from Aarav's arithmetic; the
budget now makes it concrete at ordinary scale.

The fix is a different plan, not a calibrated constant: **default to one AlterID range
(`0..H`) per partition — i.e. no subdivision — and subdivide only partitions projected to
exceed budget.** Billwise's 27 pairs then become the general behaviour rather than a lucky
special case. Projecting that requires a real cost sample *for the book being read*, which
means caching a per-endpoint sizing observation across syncs — **persistence, which Unit A's
in-memory scope explicitly excludes.** Hence Unit B.

Do not, in the meantime, derive a span-cost constant from Aarav to fake a projection. Aarav
remains prohibited for sizing, and a constant taken from it would be exactly the
plausible-but-wrong number that prohibition exists to prevent.

### New finding — the plan is a lower bound, not a bound

`SegmentPlan::new` computes `ranges_per_partition` from `policy.initial_width()`, but
`SegmentTrendGuard` **shrinks** `next_width` as the scan proceeds and is constructed once for
the whole scan. A width that shrinks mid-scan makes later partitions need *more* ranges than
planned, so **actual pairs can exceed planned pairs**. The pre-flight check can therefore pass
while execution runs over.

Codex's second cap (`pair_budget.admit_next()`) catches this, which is why I am recording it
rather than ruling it a defect — but it catches it *after* spending the live requests, which
is precisely what the pre-flight check exists to avoid. **Latent today** (unreachable without
a production width). Fix it in Unit B when sizing is redesigned: either plan from the minimum
width the guard may shrink to, or re-plan on each shrink and fail closed before the next
request.

## 4. Blocker 2 is real, but it is a hardware gap, not a code gap

The mechanism already exists and is correct:
`select_outstandings_date_boundary_profile` ([runtime.rs:102](../../src-tauri/src/tally/runtime.rs:102))
derives the profile from a `CapabilityProfile`'s product **and** mode, and falls back to
`ModeAgnostic` when there is no profile. What is missing is a *reviewed evidence source* in
the selected-company path.

**Codex's refusals are both upheld.** Do not infer mode from the port, company name or GUID —
port 9001 is a restore copy of 9000 with the same GUID, so identity provably cannot carry mode.
And do not couple Unit A to stored mirror capability snapshots; that breaches its in-memory
scope and would make a stale snapshot silently decide date legality.

But the framing understates the problem. The deeper fact:

> **Bridge has never run against a licensed Tally.** Every measurement in this project —
> every timing, every boundary rule, every reconciliation — comes from an Education instance.
> `ModeAgnostic` is the production default and has **never successfully completed a scan
> anywhere.**

So mode *detection* is not the blocker; mode *coverage* is. Detecting "licensed" would select
a code path with zero live evidence behind it. Note also that the day-01/02/31 rule being
Education-only is still **BELIEVED, never verified** — if it turns out to be general, the
production default is wrong for every user.

This is resolved by obtaining one licensed TallyPrime instance and running the same
reconciliation against a book on it. That is an owner procurement action; no amount of code
closes it. **Until then, no positive claim may be made that Unit A works on licensed Tally.**

## Required

1. Update `UNIT_A_REPORT.md` to state Unit A **complete for its scope**, with the two items
   moved to a named Unit B section — not listed as Unit A blockers.
2. Record the "plan is a lower bound" finding in `IMPLEMENTATION_GUIDE.md` §2.5b, with the
   shrink interaction spelled out, so it is not rediscovered.
3. Record in the guide that the AlterID axis is unconditional and O(D × H), that this is
   required for correctness because AlterID moves on edit, and that conditional subdivision is
   the Unit B design.
4. Add the licensed-Tally gap to the guide's unverified section (§7) in the same words as §4
   above. It must be impossible to read the docs and believe production is proven.
5. Do **not** weaken, re-tune or re-run the reconciliation. It is done.

## Constraints unchanged

- No commit, push, PR or merge without explicit owner authorisation. The 7 unpushed commits
  and this working tree still await review.
- One live request at a time, health check between, never loop.
- Aarav remains prohibited for sizing calibration, including as a source of cost constants.
