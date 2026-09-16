# Frontend module conventions

**Status: working conventions, written 2026-09-17. Improve them by PR.** This is the TypeScript and
React sibling of [`rust-module-conventions.md`](./rust-module-conventions.md). Read that document's
principles section first; they are not repeated here. [`module-decomposition.md`](./module-decomposition.md)
covers pinning and resealing, and applies to `src/` unchanged.

## The two constraints general advice does not know about

1. **Most of `src/` is pinned.** 30 of the 36 tracked files under `src/` are on the compatibility
   surface, including `main.tsx`. Moving code out of a pinned file removes it from the seal, and
   every check stays green (see `module-decomposition.md`).
2. **Tests read `src/main.tsx` as text.** Eight test files under `scripts/` read `main.tsx` with
   `readFile` and assert on regexes, substrings or `indexOf` slices of it. A pure move can fail them.
   Worse, it can leave an assertion passing while it no longer checks anything. For example:
   - a `doesNotMatch` against a file the code has left;
   - a `slice(indexOf(a), indexOf(b))` whose anchor now returns `-1`.

## Conventions

1. **Look for duplicates before splitting.** Compare top-level items by exact text across files with
   the TypeScript AST, not by name. `MirrorProofScreen.tsx` was extracted from `main.tsx` by copying:
   22 items were byte-identical and 3 types had drifted (#468). Removing a copy shrinks both files
   and moves no behaviour.
2. **Name a module by its subject.** Examples: `tally-command-error.tsx`,
   `tally-capability-evidence.tsx`, `tally-mirror-contract.ts`. Keep types beside the subject they
   describe. A file that can only be called `types.ts`, `utils.ts`, `helpers.ts` or `common.ts` is
   the wrong seam.
3. **Keep a move PR free of edits, and prove it.** The proof has three parts:
   - Every moved item's text equals its text on master, apart from an added `export`.
   - Every item left behind is unchanged.
   - The only added lines are imports.

   Run `corepack pnpm exec tsc --noEmit -p .` and `corepack pnpm build`, and put the comparison in
   the PR body. `tsc` is TypeScript 7 here, which has no JavaScript compiler API. The AST comparison
   used a scratch install of `typescript@5`.
4. **Export only what another module imports.** A move is the cheapest moment to keep helpers
   module-private.
5. **Never import from `main.tsx`.** It calls `createRoot` at module scope, so importing it mounts
   the app. Shared code moves out of `main.tsx`; nothing reaches into it.
6. **When a source-text test's subject moves, retarget the test and assert where it lives now.**
   Point the read at the new module. Also assert that the old place imports the shared item and
   defines no local copy, so a re-duplication fails. Then mutation-check the test:
   - re-add the local copy, and the test must fail;
   - remove the asserted line, and the test must fail.

   For new code, prefer behaviour tests (vitest + jsdom, like `scripts/*-screen.test.tsx`) over
   source-text assertions.
7. **Assert `indexOf` anchors before slicing.** `slice(indexOf(a), indexOf(b))` returns the wrong
   region, not an error, when an anchor is gone.
8. **Pin new frontend files by the evidence criterion, not by where they came from.** Ask of the
   destination file: could a silent change make a workbook or drawer attribute a report to the wrong
   book? Keep in mind:
   - Presentation modules are in scope when they describe what was read, as
     `outstandings-provenance.ts` does.
   - Focus handling and pure types are not in scope.
   - Record a deliberate exemption where `module-decomposition.md` says to.
9. **Extract a custom hook per cohesive state cluster** ([react.dev: reusing logic with custom
   hooks](https://react.dev/learn/reusing-logic-with-custom-hooks)). Start with the clusters no
   reset hub touches (see the map below). A cluster that `invalidateTallyResults` or
   `clearSelectedCompanyScope` resets needs a single `reset()` exposed by the hook. The hub then calls
   `reset()` instead of five setters. Otherwise the extraction adds an interface without removing any
   coupling.
10. **Use `useReducer` only for state that changes together in one transition.** Don't use it as a
    blanket replacement for `useState` ([Kent C. Dodds](https://kentcdodds.com/blog/should-i-usestate-or-usereducer),
    one author's guidance). The snapshot job cluster (six states, five of them reset together) is the candidate.
11. **Compute derived values during render; don't mirror them into state with an Effect**
    ([react.dev: you might not need an effect](https://react.dev/learn/you-might-not-need-an-effect)).
12. **Keep `invoke` calls behind one typed function per backend command family, once a family has
    more than one caller.** There are 23 `invoke` calls in `main.tsx` today. Generated bindings
    (for example [tauri-specta](https://github.com/specta-rs/tauri-specta)) are a larger decision
    and are not adopted here.
13. **Don't abstract similar code whose reasons to change differ** ([AHA
    programming](https://kentcdodds.com/blog/aha-programming), opinion). Six screens format command
    errors with small differences. Whether all of them should show the backend `remediation` is a
    product decision to make first. A shared helper written before that decision would encode one
    side of it.
14. **In jsdom tests, wait for timer-scheduled effects before asserting call order, and give a
    measured-heavy test an explicit budget.** Examples: #452 (discovery `setTimeout` raced a
    synchronous invoke) and #460 (render cost scaled past vitest's 5 s default under load).
    Lap-timestamp the test to tell the two apart before choosing a fix.

## Decision checklist

Answer these in the PR body before moving frontend code:

1. Is any of it a duplicate of code in another file? Remove the copy first.
2. Which pinned files lose code, and does each destination meet the pin criterion?
3. Which `scripts/` tests read the source files as text, and what do they assert on the moved code?
4. Is the new module named for a subject, with only imported names exported?
5. Does the proof show moved text equal to master, remaining items unchanged, and imports as the
   only additions?
6. Do `tsc --noEmit`, `pnpm build` and `pnpm test` (node, vitest and Playwright) pass after
   committing?
7. Were retargeted source-text tests mutation-checked?

## The current map: `src/main.tsx`

Measured on master `32ac7d79` (2,594 lines) with the TypeScript AST. This counts the top-level
statements of `App()` and which of them name which others. Matching is by identifier, so a
same-named local would add a false edge. Outside `App()`, #468 removes 28 shared items (the file
becomes 2,290 lines).

`App()` spans lines 428–2449 and holds 162 top-level statements:

| kind | count |
|---|---|
| `useState` | 65 |
| `useRef` | 15 |
| `useCallback` | 13 |
| `useEffect` | 8 |
| `useMemo` | 2 |
| plain functions | 21 |
| derived constants | 37 |
| JSX return | 1 (816 lines, naming 58 distinct states) |

**The coupling is concentrated in two reset hubs.** `invalidateTallyResults` names 29 distinct
states and `clearSelectedCompanyScope` names 22; together they touch 34. A state and its setter
count once. The setup handlers come next:

| handler | distinct states named |
|---|---|
| `checkTally` | 17 |
| `bootstrapDirectCompany` | 16 |
| `startCoreSnapshot` | 13 |
| `enrollWriteFixture` | 11 |
| `saveReviewedTallySetup` | 9 |

**Clusters, by how much a hook extraction would cost:**

| cluster | states | reset by a hub | notes |
|---|---|---|---|
| runtime sessions | `runtimeSessions`, `runtimeError` | none | `refreshRuntime`, `cancelTallyRequest`; no source-text assertions. Smallest first hook. |
| persisted company profiles | 5 + a load-version ref | none | writes `companies` (shared); `ux1-shell.test.mjs` asserts its `useState` and stale-load guard as text |
| evidence drawer | 4 + 5 refs | none | focus lifecycle is covered by `evidence-drawer-focus.test.tsx` |
| GST draft | `gstCompany`, `gstFinancialYear`, `draft`, `busy` | `draft` | shares `dashboardError` with `checkTally` |
| mirror explorer | 2 | both | one loader |
| write fixture | 5 | all 5 | attestation flags feed `enrollWriteFixture` |
| sync evidence and proof preview | 4 + a request-version ref | all 4 | |
| snapshot job | 6 + a selection-version ref | 5 of 6 | a background polling effect; a `useReducer` candidate |

The endpoint, probe-review and company-selection state (`config`, `status`, `passport`, `reviewId`,
`companies`, `selectedCompany`, `liveCompanyKeys` and their neighbours) is the centre that the hubs
protect. Leave it in `App()` until the clusters around it are hooks with their own `reset()`.

**Suggested order:**
1. Deduplicate (#468).
2. Runtime-sessions hook.
3. Persisted-profiles hook, retargeting its `ux1-shell` assertions.
4. Give the hub-reset clusters a `reset()` each, one per PR.
5. Snapshot job as a reducer.
6. Only then split the JSX by view.

## Sources

- [react.dev: Reusing logic with custom hooks](https://react.dev/learn/reusing-logic-with-custom-hooks)
- [react.dev: You might not need an effect](https://react.dev/learn/you-might-not-need-an-effect)
- [Tauri 2: Calling Rust from the frontend](https://v2.tauri.app/develop/calling-rust/)
- [Kent C. Dodds: Should I useState or useReducer?](https://kentcdodds.com/blog/should-i-usestate-or-usereducer)
  and [AHA programming](https://kentcdodds.com/blog/aha-programming) (one author's opinions)
- [Bulletproof React: project structure](https://github.com/alan2207/bulletproof-react/blob/master/docs/project-structure.md)
  (a large-app pattern; feature folders are not adopted at this size)
- [git: `.git-blame-ignore-revs`](https://git-scm.com/docs/git-blame#Documentation/git-blame.txt---ignore-revs-fileltfilegt),
  for keeping blame useful across move commits

None of them cover the compatibility surface or source-text tests, which are the two constraints
that govern decomposition here.
