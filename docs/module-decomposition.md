# Decomposing a large module

Bridge has fourteen production files over 2,000 lines and six over 3,500. They are
hard to read, hard to review, and hard to hold in mind. This is how to split one,
and — more importantly — what will go wrong if you split it the obvious way.

Written after measuring the tree rather than from general advice, because the two
disagree in one place that matters.

## The constraint general advice does not know about

**Most large files here are pinned in the compatibility surface, and splitting a
pinned file silently removes the extracted code from it.**

The parent's hash changes, `scripts/reseal.sh` succeeds, `reseal.sh --verify`
passes, and CI is green. The seal shrank by exactly what you moved.

There is one net, and it is worth stating precisely because it is narrow:
`validate_required_directory_coverage` fails closed for two directories
(`src-tauri/src/db/migrations`, `src-tauri/src/reports`) and for four named
`REQUIRED_SURFACE_FILES` — `agent_catalog.rs`, `agent_desktop_journal.rs`,
`agent_ledgers.rs`, `source_draft/lifecycle.rs`. Outside those six rules nothing is
examined at all: `validate_files` and `rehash_files` iterate the manifest's own
list, so a file that ought to be pinned and is not is not a check that fails — it
is a check nobody asked.

So:

> **Splitting a pinned file means pinning every part that decides what Bridge
> posts or lets leave the machine** -- each with its own named reason beside
> `MAX_SURFACE_FILES`.

And the companion rule, because the same hazard arrives without anyone splitting
anything:

> **A pinned file's collaborators are pinned or explicitly exempted.**

Before bridge#416 nothing recorded either, and the boundary showed it.
`agent_import.rs` was pinned while `agent_import_post.rs` (the posting path),
`agent_import_ledger.rs` (whether a batch was dispatched),
`agent_import_cash_bank.rs` (which ledgers may sit on a Payment, Receipt or
Contra's cash/bank side) and `agent_import_persistence.rs` were not -- so the
module that *renders* the qualified write shape was sealed and the modules that
*decide* it were not. #434 pinned those, together with the other admission and
egress files found the same way, and recorded beside `MAX_SURFACE_FILES` both
the reason for each pin and which kinds of file were left out on purpose. That
paragraph is the exemption record; add to it rather than leaving a collaborator
silently unpinned.

Two production collaborators of `agent_import.rs` remain unpinned, both
deliberately: `agent_import_schema.rs` is the argument schema for
`build_import_xml`, and loosening it cannot widen what is admitted, because
the pinned `agent_import.rs` re-checks those bounds (see #416);
`agent_desktop_journal_review.rs` is the desktop file picker, a 5 MB
selection cap and the refusal-to-message mapping, delegating review, post and
reconcile to the pinned `agent_desktop_journal.rs`.

**The gate will not tell you when a split leaves code unsealed.** A moved
module's parent changes hash, the reseal succeeds, and the extracted file is
outside the seal with every check green. `scripts/reseal.sh` prints a report
that catches the common case -- a new module declared directly by a pinned one
-- but not a new module under an unpinned parent or deeper descendants, so check
the new files against the rule above yourself.

**Capacity is not free, and that is deliberate.** `MAX_SURFACE_FILES` is set to
the exact pin count, so any branch adding a pin raises
it in the same PR. Setting the cap to the exact count has been the convention
since #260, and recording a named reason beside the constant for each raise
since #278; neither is how the reserve was first designed. `RESERVED_SURFACE_FILES`
(15) was introduced in #223 with the cap at exactly count + 15, and in that
slack period #246 added eight pins without touching the cap. With no slack, a
new pin cannot land without an edit to the constant, which is where its reason
now goes. Then reseal with `scripts/reseal.sh`, which since bridge#760 handles a
changed pin *list* the same way as changed hashes.

Budget for that when planning. Splitting a 6,000-line module four ways is four
surface decisions, not one refactor.

Check before you start:

```sh
jq -r '.files[].path' docs/tally/compatibility/compatibility-surface.json | grep -qx "$FILE" \
  && echo PINNED || echo unpinned
```

## Which file to pick — size is not the only signal

Size finds *module* incoherence. It says nothing about a file whose problem is one
enormous function, and that is a different defect with a different fix.

Measure both. Per file, take each function's span and compare the longest to the
median:

Every figure below is **brace-matched** — the span from a function's opening brace
to its matching close — not inferred from where the next `fn` starts. The two
disagree, and inferring gave me wrong numbers the first time.

| ratio | lines | fns | median | longest | file :: function |
|---:|---:|---:|---:|---:|---|
| 67x | 1231 | 61 | 3 | 202 | `bridge-tally-protocol/src/bills_payments_observation.rs :: parse_unbound_party_outstanding_observation` |
| 59x | 1028 | 56 | 3 | 178 | `bridge-tally-protocol/src/india_tax_observation.rs :: parse_unbound_india_tax_observation` |
| 49x | 443 | 19 | 6 | 295 | `agent_voucher_parse.rs :: parse_agent_rows_with_accounting_state` |
| 45x | 3669 | 83 | 18 | **811** | `sync/snapshot.rs :: run` |
| 34x | 2278 | 66 | 9 | 307 | `bridge-tally-core/src/book_presence.rs :: decide` |

`snapshot.rs::run` is **811 lines in one function** and its file ranks only eighth
by size — a size-ranked list never surfaces it. `agent_voucher_parse.rs` is 443
lines and would never be looked at at all. Both top entries are 1,000-line parser
files nobody would call large.

The converse also holds, and is why size stays on the list. The biggest files:

Counting only **code** functions — excluding those inside inline
`#[cfg(test)] mod` blocks, which are otherwise measured as if they were the
module's own:

| code lines | code fns | median | file |
|---:|---:|---:|---|
| 6,407 | 166 | 13 | `bridge-tally-protocol/src/lib.rs` |
| 5,919 | 112 | 21 | `db/tally_mirror.rs` |
| 3,571 | 104 | 12 | `commands.rs` |
| 3,488 | 117 | 14 | `tally/runtime.rs` |

Their medians are 12-21 — the functions are small. They are not one giant function
wearing a file as a coat; they are 104-166 cohesive small things in one place.
That is a module problem, and the ratio does not distinguish it from the
single-huge-function case: `lib.rs` at 28.8x scores *worse* than `snapshot.rs`
would on median alone, for an entirely different reason.

### Count code, not the file

Those totals include inline `#[cfg(test)] mod tests { … }` blocks, and for two of
them that is most of the file:

| code | total | inline tests | file |
|---:|---:|---:|---|
| 6,407 | 6,508 | 101 | `bridge-tally-protocol/src/lib.rs` |
| 5,919 | 5,919 | 0 | `db/tally_mirror.rs` |
| 3,571 | 4,684 | **1,113** | `commands.rs` |
| **3,488** | 5,441 | **1,953** | `tally/runtime.rs` |

`runtime.rs` ranks third by size and seventh by code. Sorting by `wc -l` aims the
work at a file that is 36% test.

Measuring this is harder than it looks and I got it wrong before I got it right.
`runtime.rs` carries **fourteen** `#[cfg(test)]` markers: thirteen declare
*already-extracted* modules (`#[path = "…"] mod x;`) and exactly one opens an
inline block. A scan that treats the first marker as the start of the test block
reports the file as ~64 lines of code; one that mishandles the extracted
declarations reported 4,336 test lines to me, more than twice the truth. **Match
the brace, and only for a `mod` whose declaration opens one.**

The distinction and the corrected table are from a parallel measurement in
[`rust-module-conventions.md`](./rust-module-conventions.md) (#414), which reached it first.

The same correction applies to the function counts and medians above, and it was
worth checking rather than assuming: a counter that matches `fn name` picks up
every `#[test] fn` too, so a file whose tests are inline has its module's shape
measured through its tests' style. Excluding them moves `runtime.rs` from 171
functions at median 18 to **117 at median 14**, and `commands.rs` from 147/15 to
**104/12**. `lib.rs` barely moves and `tally_mirror.rs` not at all, because #395
already extracted its tests.

The conclusion survives the correction — 104-166 small functions in one file is
still a module problem — but it survives having been checked, which is the only
way it was worth stating.

**And the files that do not move are an argument for extracting tests at all.**
`tally_mirror.rs` and `snapshot.rs` are identical on both counts, because #395
extracted their test modules. Every later measurement of those files is honest by
default: there is no inline test block for a counter to mistake for the module,
so no one measuring them has to know about this trap. Extraction is usually
argued for on readability; this is a second benefit, and it compounds — each
extraction removes a way for every future measurement of that file to be wrong.

**So the ratio is a detector, not a ranking.** Use it to find files a size list
misses; use the file size and function count to tell which of the two defects you
are looking at.

**So run both triggers and treat them as naming different defects.** One 800-line
function is extracted into named steps within its module. A 6,000-line module of
small functions is split by subject into new modules.

When counting function spans in Rust, match `fn` at any indent with optional
`pub`/`async`/`const`/`unsafe`/`extern` — methods inside `impl` blocks are the
ones you most want and the easiest to miss. (The equivalent mistake in a Go tree,
missing method receivers, produced a wrong first measurement for the Axal session
this trigger came from.)

**Check for a deletion entry before planning a split.** Decomposing something
already scheduled for removal is work thrown away twice.

## Where to cut

**Cut where the code changes for the same reason, not by layer.** A `parse`,
`types`, `helpers` split puts three files on the path of every change. A split by
subject — ledger, voucher, company, group — puts one file on the path of a change
to one Tally collection, which is how changes actually arrive here.

Measure before choosing. Clustering `bridge-tally-protocol/src/lib.rs`'s items by
subject puts roughly a third of the file on ledger handling, with shared XML,
company and voucher each several hundred lines behind it — seams that were already
there, not ones a design imposed. Treat that as a direction to look, not a
specification: the clustering is keyword-based and a second pass with different
keywords moved every figure, so it says which groups exist and not how big they
are to the line.

**Do not reach for a new crate.** Crate boundaries cost compile time and dependency
management and are worth it only when they enforce a boundary that must not be
crossed. Bridge has five crates already; the large files are *inside* them, and
another crate does not make a 6,000-line `lib.rs` smaller.

**Never create `utils`, `helpers`, or `common`.** They are where cohesion goes to
die: nothing changes for the same reason as anything else in them.

## What the parent keeps

After the split, the parent should be a façade: module declarations and `pub use`
re-exports. Callers keep importing from where they imported before, so a
decomposition is not also an API break.

Default to `pub(crate)`. A decomposition is the moment an item's visibility is
easiest to narrow, because you are already touching every reference.

## Tests

The established pattern here — `#[path]` appears 101 times in all, 63 of them
pointing at a `*_tests.rs` file and 20 in exactly this `mod tests;` form:

```rust
#[cfg(test)]
#[path = "thing_tests.rs"]
mod tests;
```

Preserve the module's **visibility** when you extract it. `snapshot.rs`'s tests
were `pub(crate) mod tests` and referenced from another file's tests; extracting
them as a private `mod tests` compiles until the other file does not.

Test files are mostly unpinned by existing norm — 59 of 63 — so extracting tests
from a pinned file usually does not shrink the seal in practice. Usually is not a
rule; check.

## The order that keeps the seal honest

Regenerate, verify, **then** stage. Every time.

```sh
./scripts/reseal.sh            # also when the file list changed
./scripts/reseal.sh --verify   # read the exit status directly, never through a pipe
git add docs/tally/compatibility/*.json
git status --porcelain         # must be empty after committing
```

Staging the pair and then resealing leaves the regenerated bytes out of the index.
`validate_files` reads the **working tree**, so every local check passes while the
commit is wrong; CI reads the commit and refuses with `surface_file_changed`. This
has cost a CI round at least once.

## Proving a decomposition changed nothing

A pure move should be provably pure:

- **Test count before and after must match exactly.** #395 moved **8,465 lines**
  into two new test files and held **919 tests** on both sides; that count is the
  evidence, not the diff.
- `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings
  -A clippy::pedantic`, and the full suite on **both** workspaces (`src-tauri/` and
  `tools/` — a `--workspace` run in one does not touch the other).
- If behaviour changed, it is not a decomposition. Land the behaviour change
  separately, before or after, so review can see it.

## Order of work

1. Measure: line counts, item clustering, pin status.
2. If pinned and there is no cap headroom, stop and resolve #416 first.
3. Extract the largest cohesive group to one new module; leave the parent a façade.
4. Prove purity (test count, both workspaces, fmt, clippy).
5. Reseal in the order above; pin the new files if the parent was pinned.
6. Repeat for the next group. One group per PR — a 6,000-line file split four ways
   in one diff is not reviewable, which is the problem you set out to fix.

## Sources

General Rust guidance consulted, and where it needed adapting:

- [Rust Module and Crate Organization Best Practices](https://softwarepatternslexicon.com/patterns-rust/5/11/) — `pub use` as the façade tool; `pub(crate)` by default.
- [Crate layout best practices: lib.rs, mod.rs and src/bin](https://dev.to/sgchris/crate-layout-best-practices-librs-modrs-and-srcbin-4abd) — split when boundaries are stable and valuable, not for appearance.
- [Mastering Large Project Organization in Rust](https://leapcell.medium.com/mastering-large-project-organization-in-rust-a21d62fb1e8e) — when a crate split earns its compile-time cost.
- [Tauri v2 architecture](https://v2.tauri.app/concept/architecture/) — the Core/Shell boundary, and why heavy work belongs behind a command rather than in the webview.

None of them cover the compatibility surface, which is the constraint that actually
governs decomposition in this repository.

The longest-function-versus-median trigger came from a parallel measurement on the
Axal tree (`docs/CODE-SHAPE.md` there), where a size rule would have split three
healthy handler files and missed both files that actually had a defect. It does not
transfer unchanged — see the section above for where this tree disagrees — but it
finds targets a size ranking cannot.
