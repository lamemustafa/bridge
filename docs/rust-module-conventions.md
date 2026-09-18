# Rust module conventions

**Status: working conventions, adopted 2026-09-17. Improve them by PR.** An earlier draft of this
file, `proposed-rust-module-conventions.md` (#414), was written but not adopted. This version
keeps its measurements and adds three things:

- the wider practice it didn't cite;
- item-graph measurements of the largest files;
- a decision checklist to run before splitting anything.

## Its sibling

[`module-decomposition.md`](./module-decomposition.md) covers **how**: where to cut, what the
parent keeps, the reseal order, and how to prove a move is pure. This document covers **whether
and what**: which files to touch, what counts as a good seam, and the principles behind both.
Read both before splitting a file.
[`frontend-module-conventions.md`](./frontend-module-conventions.md) is the TypeScript and React
counterpart.

## The constraint general advice does not know about

Most large files here are **pinned** in `docs/tally/compatibility/compatibility-surface.json`. A
split changes the pin set, raises `MAX_SURFACE_FILES` by one named reason per new file, and needs
`scripts/reseal.sh --pins-changed`. The evidence that attests the old file does not automatically
attest the new ones. **The cost of splitting a pinned file is not the edit; it is the
attestation.** Everything below assumes that cost is real.

## Principles, and where they pull against each other

These are the principles people cite when arguing about decomposition. Each is a question to ask
of a proposed split, not a rule to satisfy literally.

- **Cohesion: one reason to change.** Group items that change for the same reason. SOLID's single
  responsibility principle says the same thing under another name. Cut by subject (ledger,
  voucher, company), never by layer name (`types`, `helpers`, `parse`): a layer split puts three
  files on the path of every change.
- **Coupling: measure it, don't infer it.** Static coupling is what the item graph shows: who
  references whom. **Change coupling** is what git history shows: files that change in the same
  commits with no static link between them ([code-maat](https://github.com/adamtornhill/code-maat)).
  The two are different signals. A seam that is clean statically but coupled in history is not a
  seam.
- **Deep modules beat shallow ones** (Ousterhout, *A Philosophy of Software Design*). A module
  earns its file when its interface is small relative to what it hides. A module whose whole job is
  calling another module and reshaping the result is shallow, and a split that produces one has
  added a file without reducing what a reader must hold in mind.
- **Duplication is cheaper than the wrong abstraction** ([Sandi Metz, 2016](https://sandimetz.com/blog/2016/1/20/the-wrong-abstraction);
  one author's widely cited opinion, not a study). Don't let an existing thin-wrapper relationship
  dictate a boundary, and don't merge two similar-looking functions whose reasons to change differ.
- **The best code is no code** (AGENTS.md P4). Check for deletion before decomposition: moving
  unreachable code into a new file is work thrown away twice.
- **Dependencies point inward** (AGENTS.md P8). The Tauri command layer holds no business logic;
  protocol and canonical layers don't depend on transport, storage or Tauri. A split must not
  create an arrow in the wrong direction.

**Where they conflict.** DRY says keep a thin wrapper instead of duplicating. Depth says a module
that is only that wrapper is shallow. SRP says the wrapper and its callee may still have different
reasons to change. Reason about the boundary you actually found by reading the code; don't recite
all three as if they agree.

**SOLID in Rust.** Single responsibility, interface segregation (small traits) and dependency
inversion (depend on a trait) transfer cleanly. Liskov substitution and open/closed are
inheritance-shaped: building trait-object hierarchies to satisfy them produces needlessly boxed,
non-idiomatic Rust. Treat them as questions only.

## Conventions

1. **Rank by code lines, never by file length.** Count `#[cfg(test)] mod … { }` spans separately,
   brace-matched. `tally/runtime.rs` was 36% test before its tests were extracted.
2. **Run both triggers; they name different defects.**
   - **Size** finds a module of many small things; the fix is to split by subject into new
     modules.
   - **Longest-function-to-median ratio** finds one huge function; the fix is to extract named
     steps inside the same file, with no pin change.

   `sync/snapshot.rs::run` was 811 lines at 45 times its file's median and ranked only eighth by
   size (#419, #422).
3. **Extract inline tests first.** Use the established `#[cfg(test)] #[path = "…_tests.rs"] mod
   tests;` pattern. It changes no production behaviour, and every later measurement of the file
   becomes honest by default. Keep `#[path]` for tests; elsewhere a file whose name doesn't match
   its module is a discoverability cost.
4. **Split on a named responsibility.** If the best available name is `utils`, `helpers`, `common`
   or `misc`, the seam is wrong.
5. **Distinguish a layer from a family.** Items reached by many public roots, such as XML reader
   primitives or export envelopes, form a base layer. Split them out as `pub(crate)` modules
   *under* the families, not beside them as peers.
6. **The parent becomes a façade.** It keeps module declarations and `pub use` re-exports, so
   callers don't change and the diff reads as a move. Default new items to `pub(crate)`: a split
   is the cheapest moment to narrow visibility.
7. **One named group per PR.** Keep diffs small enough that `git diff --color-moved` and a reader
   can jointly confirm purity, and keep moves free of edits so `git blame -C` and
   `git log --follow` still work.
8. **Prove purity with test names, not counts, then run the suite.** Diff `cargo test -- --list`
   output before and after, on both workspaces (`src-tauri/`, `tools/`): a count can match while a
   test was dropped and another added. But listing is not running.
   - **Tests that read source as text still compile after a move and then fail.** Before moving, grep
     for `include_str!` and script `readFile` calls naming the file. The first `commands.rs` split
     (#472) broke a `lib.rs` test that sliced `include_str!("commands.rs")`; test names were
     identical and CI caught it.
   - **Also:** `cargo fmt --check`, clippy, and the reseal verification.
9. **A pinned split carries its reasons.** Record one named reason per new pin beside
   `MAX_SURFACE_FILES`, and pin every new file that decides what Bridge posts or lets leave the
   machine ([`module-decomposition.md`](./module-decomposition.md)).
10. **Don't reach for a new crate** unless you need something a module cannot give:
    - a compile or incremental-build boundary worth its dependency management;
    - an independently versioned release;
    - the orphan rule;
    - a feature-gated optional dependency.

    Beware Cargo feature unification: a new internal crate can silently widen a dependency's
    features across the whole workspace.
11. **Tauri commands: feature modules, thin adapters.**
    - Group `#[tauri::command]` functions by feature.
    - Each command validates input and calls a module that has no Tauri types in its signatures.
    - Registration stays one `generate_handler!` list, which nothing checks against the source. A
      declared but unregistered command fails only at the IPC boundary. Whenever a command moves,
      compare the declared commands with the registered list (see the `commands.rs` map below).
12. **Measure before and after with a real parser.** Regex counts have been wrong here repeatedly.
    Use a `syn`-based item graph, [`cargo-modules`](https://github.com/regexident/cargo-modules)
    (`structure`, `dependencies`), or rust-analyzer's SCIP index. Add change coupling from git
    history when a static seam looks clean.

## Decision checklist

Answer these in the PR body before splitting a file:

1. Is any of this code unreachable or scheduled for deletion? Delete first.
2. Is the file pinned? Is each new file's pin reason written, and are collaborators pinned or
   explicitly exempted?
3. Did you rank it by code lines, with inline tests separated?
4. Is the defect "many small things" (split the module) or "one huge function" (extract steps in
   place)?
5. Did you read the boundary rather than cluster names by prefix? Does the proposed family call
   into another family's internals?
6. Is a candidate module a layer under others, or a thin wrapper around one?
7. Will each new module be deep (small interface, real hidden work) or shallow?
8. Does git change coupling agree with the static seam?
9. Can the parent stay a pure façade?
10. Is this one named group, small enough to review as a move?
11. Do test *names* match before and after on both workspaces, with fmt and clippy clean?
12. Did you reseal in order (regenerate, verify, stage) and leave `git status` clean?

## The current map

Measured on master on 2026-09-17 with a `syn` item reference graph: every top-level item, its
span, and which other top-level items in the same file it names. It is identifier-based, so a
local variable sharing an item's name adds a false edge, and nothing outside the file is visible.
Grep external callers before any move.

### `crates/bridge-tally-protocol/src/lib.rs` (~6,400 code lines, pinned)

The earlier draft's open questions now have measured answers:

- **Canonical source records are a layer, not a family.** Four base layers are each reached by
  11–29 public roots:
  - XML reader primitives;
  - the export envelope and its evidence;
  - source record identity;
  - company context.
- **"Native" is split per collection, not a crate-wide axis.** The native ledger collection's
  closure (18 items, 904 lines) is disjoint from the standard ledger path, apart from the base
  layers and shared output types.
- **The ten "ledger data" functions are at least four responsibilities,** each with its own
  closure: standard ledger source records (`parse_ledger_source_records_with_evidence` and its
  wrappers), native ledger collection, period balance report, write read-back. The standard ledger
  catalogue and identity (`parse_standard_ledger_catalog*`, 622 lines) is a separate family beside
  them.
- **Text encoding is the first safe split.** It is 22 items and 586 lines, with no coupling to the
  rest of `lib.rs` in either direction.

Proposed order: text encoding, then import outcome (492 lines, depending only on XML reader
primitives), then the base layers as `pub(crate)` modules, then the ledger families one per PR.
This work is owned by a separate lane and waits for #456's test extraction.

### `src/commands.rs` (~3,350 item lines, pinned)

"Item lines" sum the spans of top-level items, so they exclude the `use` block and the blank lines
between items. They run below the code-line totals in
[`module-decomposition.md`](./module-decomposition.md) (3,571 for this file). Both are correct for
what they count.

**Shape.** 55 `#[tauri::command]` functions sit on a shared base of about 716 lines. The largest
shared items:
- error mapping: `TallyCommandError`, `tally_command_error`, `tally_runtime_command_error`;
- company identity verification: `verify_observed_company_tuple*`;
- download and export file naming.

**Feature groups visible in the graph:**
- setup, probe and write-fixture enrolment;
- snapshot and sync evidence;
- live reads: companies, ledgers, outstandings, selected ledger entries;
- client group labels and preferences;
- exports and party statements;
- thin delegations to documents, Axal and the desktop journal.

`commands_trial_balance.rs` already shows the per-feature pattern as `commands::trial_balance`.

**First split done:** the six All Clients commands (filing labels, their migration plan, the sort
preference) moved to `commands/all_clients.rs` (#472). They shared no item with any other command,
the new file sits under the existing unpinned "operator filing labels" exemption, and test names
were identical before and after.

**Delete-first finding, resolved (#474).** Four declared commands were deliberately
unregistered: `qualify_selected_tally_reads`, `fetch_tally_ledgers`,
`fetch_standard_tally_ledger_catalog` and `fetch_tally_vouchers`.
`scripts/tally-setup-safety.test.mjs` asserted they stayed unexposed as "unqualified legacy
reads", and 533 lines of `commands.rs` (`cargo check` plus `wc -l`, not the item-graph estimate
below) were reachable only from them, 290 of which were `qualify_selected_tally_reads`. The
owner decided **delete** over ADR 0015's "keep, with a reason" alternative; #474 removed the
four commands, their twelve commands.rs-exclusive helpers, and the one test that existed only to
exercise one of those helpers, and withdrew ADR 0015's accepted status accordingly. The
55-command, "about 510 lines", and 3,350-item-line figures above predate that deletion and have
not been re-measured; a future `commands.rs` split should re-run the item-graph measurement
rather than trust these numbers.

### `src/sync/snapshot.rs` (pinned)

**Done:** #419 and #422 lifted phases out of `run`, from 811 lines to 248.

**Left whole on purpose:** `plan_and_execute_windows` (510 lines). Its steps share about a dozen
per-iteration locals, so cutting between them would trade one long function for several coupled
ones. That is the "deep module over shallow modules" judgement applied to a function. Revisit it
only if the locals can become one named per-iteration state type.

### `src/db/tally_mirror.rs` (~5,900 lines, pinned)

**Shape.** About 3,660 lines are one `impl TallyMirrorRepository`, plus about 1,170 lines of free
helpers and 800 lines of types.

**Families.** Grouping the repository's methods by name gives this rough split. It is keyword-based
and indicative only, so read the methods before drawing a boundary:

| Family | Lines | Methods |
|---|---:|---:|
| Snapshot, commit and batch | ~1,480 | 21 |
| Write canary and fixture enrolment (ADR 0004) | ~800 | 12 |
| Proofs, explorer and reconciliation | ~490 | 7 |
| Migrations and connection pool | ~380 | 3 |
| Company, profile and setup | ~370 | 7 |

The item graph can't help here: it roots on free `pub fn`, and finds only 2 roots in this file.
A method-aware root set is needed before measuring closures.

### `crates/bridge-tally-core` (pinned files)

- **`bills_reconciliation.rs::assess_party_outstanding`** is 416 of the file's 688 item lines, 26
  times the median: the crate's clearest one-huge-function case. Its body reads as six phases, from
  scope admission to on-account handling. Extracting those steps in place is claimed tonight by a
  separate lane.
- **`master_binding.rs`** is one pipeline under `bind`, not a module of many things. Its one clean
  seam is identifier extraction: 18 items and 442 lines behind `extract_identifiers` and the
  `Identifier` types. That would be a deep module. It is also the most scrutinised code here
  (ADR 0016), so splitting it needs the master-binding owner's agreement.
- **`book_presence.rs::decide`** is 393 lines, 12 times the file's median. Step extraction in place
  is claimed by a separate lane.

### Not yet measured

`tally/runtime.rs` (~3,500 code lines) may be one large cohesive responsibility. Read it before
assuming otherwise.

## Sources

- [The Rust Book, separating modules into files](https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html)
  and [the Reference on visibility](https://doc.rust-lang.org/reference/visibility-and-privacy.html)
- [Effective Rust, Item 22: minimize visibility](https://www.lurklurk.org/effective-rust/visibility.html)
- [matklad, Large Rust Workspaces](https://matklad.github.io/2021/08/22/large-rust-workspaces.html),
  for layout once a workspace already has several crates (one experience report; it does not argue
  when to add a crate)
- [Cargo feature unification pitfall](https://nickb.dev/blog/cargo-workspace-and-the-feature-unification-pitfall/)
- [Tauri 2: calling Rust from the frontend](https://v2.tauri.app/develop/calling-rust/) and
  [`generate_handler!`](https://docs.rs/tauri/latest/tauri/macro.generate_handler.html)
- [code-maat](https://github.com/adamtornhill/code-maat), for change coupling from git history
- [git blame `-C`](https://git-scm.com/docs/git-blame) and `git diff --color-moved`, for proving and
  preserving moves
- Ousterhout, *A Philosophy of Software Design* (deep vs shallow modules); Metz,
  [The Wrong Abstraction](https://sandimetz.com/blog/2016/1/20/the-wrong-abstraction)

None of them cover the compatibility surface, which is the constraint that governs decomposition
here.
