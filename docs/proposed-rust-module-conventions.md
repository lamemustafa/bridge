# Proposed Rust module conventions and a decomposition order

This document proposes conventions that are **not adopted**. Like
`docs/proposed-dependency-policy.md`, it is written here rather than applied,
because "how this codebase is split into files" is a maintainer decision with
review-cost consequences, and because the first thing it recommends is *not
splitting most of the files that look too big*.

## Its sibling

`docs/module-decomposition.md` covers the mechanics: where to cut, what the
parent keeps as a façade, the order that keeps the seal honest, and how to
prove a decomposition changed nothing. This document covers what to decompose
and how to measure before deciding. Read that one before doing the work.

The two were written in parallel by different sessions and reached the same
constraint independently — that pinning, not the edit, is what makes splitting
expensive here. Where they differ is convention 1 below: a file's length and
its quantity of code are different numbers, and which you rank by changes which
file you pick first.

## What the tree actually looks like

Measured on `b854369b`, counting lines inside `#[cfg(test)] mod … { }` blocks
separately from code, across `src-tauri/**/*.rs` excluding `target/`:

| code | total | inline tests | already-extracted test mods | file |
| ---: | ---: | ---: | ---: | --- |
| 6,385 | 6,486 | 101 | 0 | `crates/bridge-tally-protocol/src/lib.rs` |
| 5,919 | 5,919 | 0 | 2 | `src/db/tally_mirror.rs` |
| 3,669 | 3,669 | 0 | 1 | `src/sync/snapshot.rs` |
| 3,571 | 4,684 | 1,113 | 1 | `src/commands.rs` |
| 3,487 | 5,441 | 1,954 | 6 | `src/tally/runtime.rs` |
| 2,488 | 2,488 | 0 | 3 | `src/agent_import.rs` |
| 2,333 | 2,333 | 0 | 1 | `crates/bridge-tally-core/src/master_binding.rs` |
| 2,021 | 3,872 | 1,851 | 0 | `src/tally/connection.rs` |

**20 files exceed 1,500 lines of code. The total is 128,827 code lines.**

Two things that measurement teaches, before any conventions:

**The obvious metric is wrong twice over.** `wc -l` ranks
`crates/bridge-tally-protocol/src/lib.rs` and `src/tally/runtime.rs` as
comparably sized. They are not: one is 6,385 lines of code, the other 3,487
with 1,954 lines of inline tests. Sorting by total length would put effort
where the tests are.

**Test extraction is already the house pattern and is already partly done.**
`#[path = "…_tests.rs"] mod …_tests;` appears 6 times in `runtime.rs` alone,
and #395 extracted the test modules from `tally_mirror.rs` and `snapshot.rs`.
That is why those two now show 0 inline test lines against 5,919 and 3,669
code lines. The convention exists; the remaining work is applying it.

*(A note on method, because it bit me three times. Taking the first
`^#[cfg(test)]` as the start of the test block reports `runtime.rs` as 64 lines
of code, because the first such line declares an already-extracted test module;
and `snapshot.rs` as 54, because its first is a test-only
`const WORKER_LEASE_HEARTBEAT_INTERVAL`. Only counting braced
`#[cfg(test)] mod … { }` spans gives the shape above. The third error survived
into the first version of this document and was caught in review: splitting on
`\n` leaves a phantom trailing element for a file ending in a newline, so every
count was one too high and the total 233 too high. The numbers here are
`wc -l`-consistent. A fifth error was of a different kind and also caught in
review: two of the eight examples in the family table named functions that were
wrong — one, `parse_import_evidence_with_limit`, does not exist in the crate at
all. An illustrative example is as checkable as a count, and a plausible name is
not evidence that the function is there.)*

## What the wider practice says

The recurring advice is that there is no line count at which splitting becomes
mandatory — the decision is cohesion, not size ([Sling
Academy](https://www.slingacademy.com/article/best-practices-for-structuring-large-scale-rust-applications-with-modules/),
[the Rust
book](https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html)).
The failure modes named most often are over-splitting and misnaming: *a crate
with five files does not need a workspace with eight packages*, and `utils.rs`
grows into unrelated functions because its name describes nothing
([Software Patterns
Lexicon](https://softwarepatternslexicon.com/rust/anti-patterns-and-common-pitfalls/avoiding-monolithic-crate-structures/)).
A common concrete target is ~400 lines per file *without a clear reason*, with
the qualifier doing the work.

Rust adds one property that matters here: moving an item between modules in the
same crate does not change the crate's public API if the re-exports are kept.
So decomposition inside a crate is close to free in compatibility terms — which
argues for doing it where it helps and against treating it as risky.

## Where the general advice does not fit this repository

This codebase has a constraint most advice does not consider: **files are
pinned**. `docs/tally/compatibility/compatibility-surface.json` binds 217 paths
by SHA-256, and the evidence receipts beneath it attest behaviour *of those
bytes*. Splitting a pinned file is not a neutral move:

- the pin set changes, which needs `scripts/reseal.sh --pins-changed` and, at
  the cap, a deliberate `MAX_SURFACE_FILES` raise;
- `MAX_SURFACE_FILES` is documented as "one file for one named reason — not
  headroom", so a decomposition that turns one pinned file into four is four
  named reasons, not a formatting change;
- and the evidence that attests the old file does not automatically attest the
  new ones.

**So the cost of splitting is not the edit. It is the attestation.** That
inverts the usual advice for exactly the files most likely to look worst.

## Proposed conventions

1. **Rank by code lines, never by file length.** Count
   `#[cfg(test)] mod … { }` spans separately. A 5,000-line file that is 3,000
   lines of tests is a different problem from one that is not.

2. **Extract inline test modules first, always.** It is the established pattern
   here (`#[path = "…_tests.rs"]`), it is mechanical, it changes no production
   behaviour, and test files are largely unpinned — `book_presence_tests.rs`
   and `agent_presence_tests.rs` differ on this, so check before assuming.
   `runtime.rs` (1,954), `connection.rs` (1,851) and `commands.rs` (1,113) are
   the remaining candidates.

3. **Split on a named responsibility, never on size.** The module name must say
   what it owns. If the best name available is `utils`, `helpers`, `common` or
   `misc`, the seam is wrong — stop and find the real one.

4. **Do not split a pinned file without a named reason, recorded where
   `MAX_SURFACE_FILES` records its raises.** One decomposition, one entry,
   stating what each new file owns and why the evidence still covers it.

5. **Keep the crate's public surface stable across a move.** Re-export from the
   old path so a decomposition is invisible to consumers, and so the diff a
   reviewer reads is a move rather than an API change.

6. **A file is not too long if its contents are one thing.** `tally_mirror.rs`
   at 5,920 lines may be a legitimately large single responsibility; that is a
   judgement to make by reading it, and this document does not make it.

## A decomposition order, highest value first

**`crates/bridge-tally-protocol/src/lib.rs` — 6,385 lines of code, the clearest
case.** It is a crate root that still holds 35 structs, 14 enums, 12 impls, 10
consts and **138 free functions**, while the crate already has 12 submodules
beside it. 138 free functions in a crate root is the signal.

The 60 `parse_*` functions fall into eight families, and the sizes matter more
than the names:

| count | family | example |
| ---: | --- | --- |
| 13 | company and gateway discovery | `parse_companies_with_evidence` |
| 12 | canonical source records | `parse_group_source_records_with_evidence` |
| 10 | ledger data — masters, period balances, write read-back | `parse_ledger_write_readback_with_evidence` |
| 10 | native collection rows | `parse_native_voucher_collection_row` |
| 5 | standard ledger catalogue and identity | `parse_standard_ledger_catalog` |
| 4 | import evidence and outcome | `parse_import_evidence` |
| 3 | voucher | `parse_vouchers_with_evidence` |
| 3 | primitives | `parse_xml`, `parse_tally_boolean` |

Beside them, 11 `validate_*`, and 8 `read_*` with 4 `decode_*` forming the XML
reader primitives (`read_identifier_text`, `read_required_text`, …).

**An earlier draft of this document named only three of those eight families —
company/gateway, standard ledger catalogue, and source records — and said the
names "cluster cleanly". That is 30 of 60 functions, and review caught it.**
The omitted half is not a rounding error: ledger data and native collection
rows are 20 functions between them, and "native versus standard export format"
is a *fourth* axis that cuts across the other three rather than sitting beside
them. A decomposition planned from the three-family version would have met
half the crate root with no home for it, and the obvious repair — putting every
`parse_ledger*` function into a file called `ledger_catalog.rs` because the
names look similar — is exactly the trap convention 3 warns about: "which
ledgers exist" and "full ledger data, including write read-back" are different
responsibilities that a shared name prefix would hide.

So no file layout is proposed here. Eight families is a starting map, not a
plan, and the axis question — whether `native` is a family or a dimension —
has to be answered by reading before anything is moved.

Two concrete reasons to expect the seams to resist, both found by looking
rather than by reasoning.

`parse_company_context` and its `ParsedCompanyContext` type sit among the
company functions and are called from `parse_ledger_write_readback_with_evidence`
(`lib.rs:4017`, `:4056`). A company/ledger split would put a shared helper on
one side of a boundary the other side reaches across.

The second is worse for the idea of peer families. **"Canonical source records"
is not a family beside the others — it is the layer underneath at least two of
them.** `parse_ledgers_with_evidence` (`lib.rs:2352`) is a three-line wrapper
whose body is a call to `parse_ledger_source_records_with_evidence`, and
`parse_vouchers_with_evidence` (`:4405`) stands in exactly the same relation to
`parse_voucher_source_records_with_evidence`. Split those into separate files
and two of them have a public surface consisting entirely of "call the other
file and reshape the result". That is a layering, and a layering drawn as peers
produces modules that cannot justify their own existence.

Both were found in minutes. Six families remain unexamined.

**This file is pinned**, so any split needs convention 4.

**Then the test extractions** in `runtime.rs`, `connection.rs`, `commands.rs` —
mechanical, no attestation cost where the target is unpinned.

**Then `src/commands.rs` (3,572).** A Tauri command surface is a natural
grouping by feature, and it already has one `#[path]` extraction to follow.

**`db/tally_mirror.rs` and `sync/snapshot.rs` last**, and only after reading
them: both are pinned, both already extracted their tests, and both may be one
cohesive responsibility rather than a pile.

## What this document does not establish

No decomposition here has been performed or reviewed. The families above are
derived from function names and counts, not from reading all 6,385 lines — a
name cluster is a hypothesis about cohesion, not a measurement of it, and the
one cross-family dependency that has been looked for was found immediately.

Specifically unestablished, and each is checkable rather than a matter of
judgement, so the next person should check rather than assume:

- whether `native` is a family or an axis crossing the others;
- whether "canonical source records" is a peer at all, or the foundation the
  ledger and voucher families are built on — read what
  `parse_ledgers_with_evidence` and `parse_vouchers_with_evidence` actually do
  before drawing any boundary between those three;
- whether the 10 ledger-data functions are one responsibility or several;
- how much state and how many helper types the families share, beyond the one
  instance found;
- whether the 11 `validate_*` functions validate the same kind of thing.

The claim that test extraction is behaviour-preserving is a claim about the
pattern, not a promise about any particular file.

The measurement was taken at one commit and this tree moves fast: `lib.rs`
gained 25 lines between the first draft of this document and its review. Treat
the table as a shape, not as current line counts.

Nothing here justifies a mass mechanical refactor. Each file on the list is its
own change, with its own review and — for the pinned ones — its own named
reason.
