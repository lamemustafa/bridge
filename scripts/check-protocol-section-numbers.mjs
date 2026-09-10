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
// It cannot stop two open PRs from *choosing* the same number — nothing in the
// repository can see across unmerged branches. It does guarantee the duplicate
// never reaches master: whichever PR rebases second picks up the first one's
// heading and fails here, before merging.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const reference = fileURLToPath(
  new URL("../docs/tally/TALLY_PROTOCOL_REFERENCE.md", import.meta.url),
);

// `1.2`, `9.12a`, `12a.4` are all section numbers this document uses.
const HEADING = /^(#{2,4})\s+((?:\d+[a-z]?)(?:\.\d+[a-z]?)*)(?=[\s.:—-]|$)/;

// Duplicates that predate this check. Rule 3 of the register says a merged
// section is never renumbered — other documents and commit messages cite these
// numbers — so an existing collision is debt to be cited, not silently fixed.
// Nothing may be added here to get a new duplicate through: move the later
// arrival instead.
const KNOWN_DUPLICATES = new Map([
  [
    "1.2",
    "present on master before this gate existed; renumbering either would break " +
      "citations that already point at them",
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

const failures = [];
for (const [number, found] of occurrences) {
  if (found.length === 1) continue;
  if (KNOWN_DUPLICATES.has(number)) continue;
  failures.push(
    `section ${number} is used ${found.length} times:\n` +
      found.map((one) => `    line ${one.line}: ${one.text}`).join("\n"),
  );
}

// A known duplicate that has been resolved should stop being excused, or the
// exemption outlives the problem and quietly covers the next collision.
for (const [number, reason] of KNOWN_DUPLICATES) {
  const found = occurrences.get(number);
  if (!found || found.length > 1) continue;
  failures.push(
    `section ${number} is no longer duplicated (${reason}) — remove it from ` +
      "KNOWN_DUPLICATES so the exemption cannot cover a future collision",
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
