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

import { spawnSync } from "node:child_process";
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
// ATX headings may carry up to three leading spaces and still be headings;
// four or more make an indented code block. A `##` inside a fenced block is not
// a heading at all, and counting one there fails CI over an example.
const HEADING = /^ {0,3}(#{2,6})\s+((?:\d+[a-z]?)(?:\.\d+[a-z]?)*)(?=[\s.:—-]|$)/;
const FENCE = /^ {0,3}(`{3,}|~{3,})/;
// A Setext heading is a line of text with `===` or `---` under it, and it is a
// heading at levels 1 and 2. Ignoring the form entirely would let
// `9.14 New section` + an underline duplicate an existing number while this
// gate reported success.
const SETEXT_UNDERLINE = /^ {0,3}(=+|-+)\s*$/;
const SETEXT_NUMBER = /^ {0,3}((?:\d+[a-z]?)(?:\.\d+[a-z]?)*)(?=[\s.:—-]|$)/;
// A Setext heading's content is a *paragraph*. A list item, table row or block
// quote is not one, and walking back to the top of a run of non-blank lines will
// land on them — `1. **Write responses to a file**` reads as section `1`
// otherwise, which is a real line in this document. Note `\d{1,9}[.)]\s` matches
// an ordered-list marker (`1. `) and not a section number (`9.14 `), because in
// the latter the dot is followed by a digit.
const NOT_A_PARAGRAPH = /^ {0,3}(?:[-*+]\s|\d{1,9}[.)]\s|>|\||#)/;
// A paragraph also ends at an *unnumbered* ATX heading, a fence, or a thematic
// break. `HEADING` only matches numbered ones, so walking back past a
// `## Unnumbered context` line and then rejecting it as not-a-paragraph lost the
// numbered Setext heading underneath it entirely.
const BLOCK_BOUNDARY = /^ {0,3}(?:#{1,6}\s|`{3,}|~{3,}|(?:[-*_]\s*){3,}$)/;

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
// The exemption names the *headings*, not a count. A count alone has two holes:
// a third `1.2` passes if one of the originals is renumbered in the same
// change, and a collision-cleanup racing a stale section PR can replace a
// grandfathered occurrence with a genuinely new one while the total stays two.
// Naming them means a new `1.2` is a new heading, and a new heading is a
// failure whatever the arithmetic says.
const KNOWN_DUPLICATES = new Map([
  [
    "1.2",
    {
      headings: [
        "Request charset controls response charset",
        "A modal error dialog in Tally's UI blocks the gateway until a human " +
          "clicks OK — **P0 operationally**",
      ],
      reason:
        "present on master before this gate existed; renumbering either would " +
        "break citations that already point at them",
    },
  ],
]);

// Compare heading text, not the whole line: the level (`##` vs `###`) is
// formatting and may legitimately change, while the words identify the section.
//
// Both markers are optional because a **Setext** heading has no `#` at all — it
// is underlined by the line below it. Stripping only an ATX prefix left the
// number inside the "title" of every Setext heading, so a comparison meant to be
// title-against-title was silently number+title against number+title.
function titleOf(line) {
  return line
    .trim()
    .replace(/^#+\s+/, "")
    .replace(/^(?:\d+[a-z]?)(?:\.\d+[a-z]?)*(?=[\s.:\u2014-]|$)\s*/, "")
    .trim();
}

// Scanning one document's lines into number -> occurrences. A function rather
// than a loop, because the base revision has to be scanned the same way.
function scan(lines) {
  const occurrences = new Map();
  let fence = null;
  const record = (number, line, text) => {
    if (!occurrences.has(number)) occurrences.set(number, []);
    occurrences.get(number).push({ line, text: text.trim() });
  };

  lines.forEach((line, index) => {
    const rail = FENCE.exec(line);
    if (rail) {
      if (fence === null) {
        // A backtick fence's info string may not contain a backtick — CommonMark
        // says so, and Markdown does not open a block for one. Treating it as an
        // opener suppressed every heading until the next bare closer, so a
        // duplicate inside that span passed CI unseen. A tilde fence's info
        // string may, so this is backtick-specific.
        const info = line.slice(line.indexOf(rail[1]) + rail[1].length);
        if (rail[1][0] === "`" && info.includes("`")) return;
        fence = rail[1];
        return;
      }
      // A closing fence carries no info string, matches the opener's character,
      // and is at least as long.
      const after = line.slice(line.indexOf(rail[1]) + rail[1].length);
      if (rail[1][0] === fence[0] && rail[1].length >= fence.length && after.trim() === "") {
        fence = null;
      }
      return;
    }
    if (fence !== null) return;

    // Setext: this underlines the paragraph above it, and such a heading may
    // span several lines — the number is on the *first* of them, not on the
    // line immediately above the underline.
    if (SETEXT_UNDERLINE.test(line) && index > 0) {
      let first = index - 1;
      while (
        first > 0 &&
        lines[first - 1].trim() &&
        !BLOCK_BOUNDARY.test(lines[first - 1]) &&
        !NOT_A_PARAGRAPH.test(lines[first - 1])
      ) {
        first -= 1;
      }
      const heading = lines[first];
      // An ATX heading above is a heading in its own right, and `---` under it
      // is a thematic break. A blank line is not a heading at all.
      if (heading.trim() && !BLOCK_BOUNDARY.test(heading) && !NOT_A_PARAGRAPH.test(heading)) {
        // Matched against the raw line, not a trimmed copy: SETEXT_NUMBER's own
        // {0,3} indentation limit is what rejects a four-space-indented code
        // line beginning with a number, and trimming first threw that away.
        const numbered = SETEXT_NUMBER.exec(heading);
        if (numbered) record(numbered[1], first + 1, heading);
      }
      return;
    }

    const found = HEADING.exec(line);
    if (found) record(found[2], index + 1, line);
  });
  return occurrences;
}

// Rule 2 of the register — a merged section is never renumbered, because other
// documents and code cite these numbers. Uniqueness alone cannot see that:
// renumbering a unique heading leaves it unique. Only the base knows which
// numbers were already allocated.
function baseNumbers() {
  const repository = fileURLToPath(new URL("../", import.meta.url));
  const candidates = [
    process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : null,
    "origin/master",
    "master",
  ].filter(Boolean);
  for (const ref of candidates) {
    const show = spawnSync(
      "git",
      ["show", `${ref}:docs/tally/TALLY_PROTOCOL_REFERENCE.md`],
      { cwd: repository, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
    );
    if (show.status === 0) return { ref, numbers: scan(show.stdout.split("\n")) };
  }
  return null;
}

const lines = readFileSync(reference, "utf8").split("\n");
const occurrences = scan(lines);

if (!occurrences.size) {
  throw new Error(
    `no numbered headings found in ${reference} — the heading pattern no longer ` +
      "matches the document, so this gate is checking nothing",
  );
}

// Every value interpolated into a diagnostic goes through this. The heading text
// and the occurrence counts were capped and the *number* was not, so a generated
// reference carrying one very long numeric token still produced output that grew
// with its input — unretrievable exactly when the gate has something to say.
function short(value) {
  const text = String(value);
  return text.length <= MAX_HEADING_CHARS ? text : `${text.slice(0, MAX_HEADING_CHARS)}…`;
}

function describe(number, found) {
  const shown = found.slice(0, MAX_REPORTED_OCCURRENCES);
  const lines = shown.map(
    (one) => `    line ${one.line}: ${short(one.text)}`,
  );
  if (found.length > shown.length) {
    lines.push(`    ... and ${found.length - shown.length} more`);
  }
  return `section ${short(number)} is used ${found.length} times:\n${lines.join("\n")}`;
}

const failures = [];
let omitted = 0;
for (const [number, found] of occurrences) {
  const excused = KNOWN_DUPLICATES.get(number);
  if (!excused && found.length === 1) continue;
  if (excused) {
    const remaining = [...excused.headings];
    const unexcused = found.filter((one) => {
      const at = remaining.indexOf(titleOf(one.text));
      if (at === -1) return true;
      remaining.splice(at, 1);
      return false;
    });
    if (!unexcused.length) continue;
    failures.push(
      `section ${short(number)} is excused only for its grandfathered headings ` +
        `(${excused.reason}); these are new:\n` +
        unexcused
          .slice(0, MAX_REPORTED_OCCURRENCES)
          .map((one) => `    line ${one.line}: ${short(one.text)}`)
          .join("\n"),
    );
    continue;
  }
  if (failures.length >= MAX_REPORTED_NUMBERS) {
    omitted += 1;
    continue;
  }
  failures.push(describe(number, found));
}
if (omitted) {
  failures.push(`... and ${omitted} further duplicated section number(s)`);
}

// A grandfathered heading that has been renumbered or retitled should stop
// being excused, or the exemption outlives the problem and quietly covers the
// next collision under the same number.
for (const [number, { headings, reason }] of KNOWN_DUPLICATES) {
  const present = new Set((occurrences.get(number) ?? []).map((one) => titleOf(one.text)));
  const gone = headings.filter((heading) => !present.has(heading));
  if (!gone.length) continue;
  failures.push(
    `section ${short(number)} no longer carries ${gone.length} of its grandfathered ` +
      `heading(s) (${reason}) — update or remove its KNOWN_DUPLICATES entry so ` +
      "the exemption cannot cover a future collision:\n" +
      gone.map((heading) => `    ${short(heading)}`).join("\n"),
  );
}

// Rule 2: a number already allocated on the base must still be there. Uniqueness
// cannot see this — renumbering a unique heading leaves it unique — and code
// cites these numbers (`src-tauri/src/agent_import.rs` cites 9.8).
const base = baseNumbers();
if (base) {
  // The test is **presence of the number**, not identity of the heading.
  //
  // An earlier version asked "does the heading that had this number still have
  // it?", which needs a section to have an identity independent of its number,
  // and the only candidate was its title. Title-as-identity has three holes and
  // review found all three: a Setext heading's title still contained its number,
  // so renumbering one read as a deletion; renumbering *and* retitling in one
  // change made the old title vanish, which also read as a deletion; and two
  // sections sharing a title made retitling either one look like a move.
  //
  // Numbers need no such proxy. They are what citations point at, they are
  // already parsed, and "9.7 is gone" is exactly the harm the rule exists to
  // prevent — whether it left by being renumbered, retitled into a different
  // number, or deleted outright. All three break `see §9.7` identically.
  //
  // Retitling stays free, which is what PR #296 needed: the number is still
  // there, so nothing fires. A swap is caught, because a swap is two moves and
  // both destinations are new numbers while neither source survives.
  const missing = [...base.numbers.keys()].filter((number) => !occurrences.has(number));
  if (missing.length) {
    failures.push(
      `section number(s) present on ${base.ref} and absent here. A merged ` +
        "section number is never reused for something else, moved, or removed — " +
        "other documents and code cite it. Retitling is fine; renumbering is " +
        "not. Give new material a free number and leave the existing one " +
        "alone. If a section genuinely must go, leave its number in place with " +
        "a line saying where its content went, so the citation still lands:\n" +
        missing
          .slice(0, MAX_REPORTED_NUMBERS)
          .map((number) => `    ${short(number)}`)
          .join("\n") +
        (missing.length > MAX_REPORTED_NUMBERS
          ? `\n    ... and ${missing.length - MAX_REPORTED_NUMBERS} more`
          : ""),
    );
  }

  // Presence alone cannot see a **swap**: exchange two numbers and both are
  // still there, while every citation to either now lands on the other's
  // content. That needs a section to be recognisable independently of its
  // number, and the only available handle is its title — so use it, but only
  // where it is actually a handle.
  //
  // A title identifies a section only when it is unique on **both** revisions.
  // Restricting the check to that is what makes it safe: two sections sharing a
  // title can no longer make retitling one of them look like a move, which was
  // the third review finding here. The other two are already answered — a
  // Setext heading's number is stripped by `titleOf` now, and a renumber that
  // also retitles is caught by the presence check above, which needs no title
  // at all.
  //
  // Because both sides are unique, the destination is exactly one number, so
  // the diagnostic cannot grow with its input the way `now.join(", ")` could.
  const soleNumberOf = (scanned) => {
    const byTitle = new Map();
    for (const [number, found] of scanned) {
      for (const one of found) {
        const title = titleOf(one.text);
        byTitle.set(title, byTitle.has(title) ? null : number);
      }
    }
    return byTitle;
  };
  const wasAt = soleNumberOf(base.numbers);
  const isAt = soleNumberOf(occurrences);
  const moved = [];
  for (const [title, number] of wasAt) {
    if (number === null) continue;
    const now = isAt.get(title);
    if (now === undefined || now === null || now === number) continue;
    moved.push({ title, number, now });
  }
  if (moved.length) {
    failures.push(
      `section(s) whose heading moved to a different number against ${base.ref}. ` +
        "Both numbers still exist, so this is a swap or a re-use: a citation to " +
        "either one now lands on the other's content. Leave merged numbers where " +
        "they are and give new material a free one:\n" +
        moved
          .slice(0, MAX_REPORTED_NUMBERS)
          .map(({ title, number, now }) =>
            `    ${short(title)}: was ${short(number)}, now ${short(now)}`)
          .join("\n"),
    );
  }
} else {
  // Say so rather than passing quietly: a check that cannot run is not a check
  // that passed.
  console.warn(
    "note: no base revision reachable (shallow clone?), so the never-renumber " +
      "rule was NOT checked — only uniqueness was.",
  );
}

if (failures.length) {
  // Not "duplicate section numbers": this gate enforces three rules and reports
  // them through one list, so naming the first one mislabels the other two. A
  // renumber failure read as a duplicate sends the author looking for a
  // collision that is not there.
  throw new Error(
    `protocol-reference section numbering (${failures.length} problem(s)):\n` +
      failures.join("\n") +
      "\n\nSee docs/tally/SECTION-REGISTER.md.",
  );
}

const excused = [...KNOWN_DUPLICATES.keys()].join(", ");
console.log(
  `Protocol section numbers are unique (${occurrences.size} numbers, ` +
    `${lines.length} lines; known duplicates excused: ${excused}).`,
);
