// SPDX-License-Identifier: Apache-2.0
//
// The protocol reference numbers its sections sequentially and several branches
// extend it at once. A number is invisible to another branch until it merges,
// so two branches can claim the same one and neither notices — it has happened,
// and `1.2` is used twice on master today.
//
// This gate reads the numbers out of the reference itself. That matters: a
// register a contributor has to remember to update is not enforcement, and a
// register listing only the numbers people remembered to claim cannot answer
// "is 9.11c taken?" for the sixty-odd numbers already in the document. The
// headings are the allocation, so they are what gets checked.
//
// On a `pull_request` event `actions/checkout` takes `refs/pull/N/merge`, so
// this runs against the PR *merged into the base as of that run* — not against
// the branch alone. Two PRs choosing the same number are therefore caught as
// soon as one of them merges and the other re-runs.
//
// The residual is a green check that has gone stale: if the base moves after
// the run and the repository permits merging a branch that is not up to date,
// the old result stands. Closing that is a repository setting — "require
// branches to be up to date" or a merge queue — not something a script can do.
// The `push: master` run is the backstop, and it fails loudly on master.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const reference = fileURLToPath(
  new URL("../docs/tally/TALLY_PROTOCOL_REFERENCE.md", import.meta.url),
);

// `1.2`, `9.12a` and `12a.4` are all section numbers this document uses.
//
// Levels 2-6, not 2-4: Markdown allows six, a child of an existing `####`
// section naturally needs a fifth, and a pattern that stopped at four would let
// such a heading duplicate an existing number while this gate reported success.
const HEADING = /^(#{2,6})\s+((?:\d+[a-z]?)(?:\.\d+[a-z]?)*)(?=[\s.:—-]|$)/;

// Diagnostics are bounded. A malformed or generated reference can carry very
// many duplicates, or one very long heading, and CI evidence that does not fit
// is unreadable exactly when the gate has something to say.
const MAX_REPORTED_NUMBERS = 20;
const MAX_REPORTED_OCCURRENCES = 8;
const MAX_HEADING_CHARS = 160;

// Duplicates that predate this check. Rule 3 of the register says a merged
// section is never renumbered — other documents and commit messages cite these
// numbers — so an existing collision is debt to be cited, not silently fixed.
// Nothing may be added here to get a new duplicate through: move the later
// arrival instead.
//
// The *count* is part of the exemption. Excusing the number alone would let a
// contributor add a third `1.2` and still pass, turning the one piece of
// recorded debt into a hole.
const KNOWN_DUPLICATES = new Map([
  [
    "1.2",
    {
      occurrences: 2,
      reason:
        "present on master before this gate existed; renumbering either would " +
        "break citations that already point at them",
    },
  ],
]);

const lines = readFileSync(reference, "utf8").split("\n");
const occurrences = new Map();
lines.forEach((line, index) => {
  const found = HEADING.exec(line);
  if (!found) return;
  const number = found[2];
  if (!occurrences.has(number)) occurrences.set(number, []);
  occurrences.get(number).push({ line: index + 1, text: line.trim() });
});

if (!occurrences.size) {
  throw new Error(
    `no numbered headings found in ${reference} — the heading pattern no longer ` +
      "matches the document, so this gate is checking nothing",
  );
}

function describe(number, found) {
  const shown = found.slice(0, MAX_REPORTED_OCCURRENCES);
  const lines = shown.map(
    (one) => `    line ${one.line}: ${one.text.slice(0, MAX_HEADING_CHARS)}`,
  );
  if (found.length > shown.length) {
    lines.push(`    ... and ${found.length - shown.length} more`);
  }
  return `section ${number} is used ${found.length} times:\n${lines.join("\n")}`;
}

const failures = [];
let omitted = 0;
for (const [number, found] of occurrences) {
  const allowed = KNOWN_DUPLICATES.get(number)?.occurrences ?? 1;
  if (found.length <= allowed) continue;
  if (failures.length >= MAX_REPORTED_NUMBERS) {
    omitted += 1;
    continue;
  }
  failures.push(describe(number, found));
}
if (omitted) {
  failures.push(`... and ${omitted} further duplicated section number(s)`);
}

// A known duplicate that has been resolved should stop being excused, or the
// exemption outlives the problem and quietly covers the next collision.
for (const [number, { occurrences: allowed, reason }] of KNOWN_DUPLICATES) {
  const found = occurrences.get(number) ?? [];
  if (found.length >= allowed) continue;
  failures.push(
    `section ${number} now appears ${found.length} time(s), fewer than the ` +
      `${allowed} this gate excuses (${reason}) — update or remove its ` +
      "KNOWN_DUPLICATES entry so the exemption cannot cover a future collision",
  );
}

if (failures.length) {
  throw new Error(
    "duplicate protocol-reference section numbers:\n" +
      failures.join("\n") +
      "\n\nThe later arrival moves — see docs/tally/SECTION-REGISTER.md.",
  );
}

const excused = [...KNOWN_DUPLICATES.keys()].join(", ");
console.log(
  `Protocol section numbers are unique (${occurrences.size} numbers, ` +
    `${lines.length} lines; known duplicates excused: ${excused}).`,
);
