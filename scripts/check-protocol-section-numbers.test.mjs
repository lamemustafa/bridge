#!/usr/bin/env node
// Contract tests for scripts/check-protocol-section-numbers.mjs.
//
// The gate's two failure modes are not symmetric. A false positive is loud and
// gets fixed within the hour — PR #296 hit one and it cost a reorder. A false
// negative is silent: the gate says "unique" and a renumbering lands, and the
// citation it broke is not discovered until someone follows `see §9.7` months
// later. Four of the five review findings against this script were false
// negatives, and none was reachable without a base revision to compare against.
//
// So each case here builds a real two-repository setup — an `upstream` holding
// the *merged* reference and a clone holding the edit — because "merged" is the
// whole premise of the never-renumber rule and cannot be faked in one tree.
//
// Run: node scripts/check-protocol-section-numbers.test.mjs
import { mkdtempSync, rmSync, mkdirSync, writeFileSync, readFileSync, copyFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const GATE = join(here, "check-protocol-section-numbers.mjs");

// A minimal reference exercising every heading form the gate parses: ATX, Setext,
// a pair sharing a title under different numbers, and the two grandfathered 1.2
// headings, whose exact text the gate's KNOWN_DUPLICATES entry names.
const BASE_DOC = `# Reference

## 9.7 Operation support matrix

body

77.77 Stable title
==================

body

### 77.71 Repeated heading

x

### 77.72 Repeated heading

y

## 10 Alpha

a

## 11 Alpha

b

## 20 Beta

c

## 21 Beta

d

## 1.2 Request charset controls response charset

z

## 1.2 A modal error dialog in Tally's UI blocks the gateway until a human clicks OK — **P0 operationally**

w
`;

const git = (cwd, ...args) => {
  const out = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (out.status !== 0) throw new Error(`git ${args.join(" ")}: ${out.stderr}`);
  return out.stdout;
};

const root = mkdtempSync(join(tmpdir(), "section-gate-"));
const upstream = join(root, "upstream");
const work = join(root, "work");
const DOC = "docs/tally/TALLY_PROTOCOL_REFERENCE.md";

mkdirSync(join(upstream, "scripts"), { recursive: true });
mkdirSync(join(upstream, "docs/tally"), { recursive: true });
git(upstream, "init", "-q", ".");
git(upstream, "config", "user.email", "test@example.invalid");
git(upstream, "config", "user.name", "test");
writeFileSync(join(upstream, DOC), BASE_DOC);
copyFileSync(GATE, join(upstream, "scripts", "check-protocol-section-numbers.mjs"));
git(upstream, "add", "-A");
git(upstream, "commit", "-qm", "base");
git(upstream, "branch", "-M", "master");
git(root, "clone", "-q", upstream, "work");

// The gate resolves its repository from its own module URL and reads
// `origin/master` there, so it must be run from inside the clone.
const runGate = () => spawnSync("node", ["scripts/check-protocol-section-numbers.mjs"], {
  cwd: work,
  encoding: "utf8",
});

let failed = 0;
// `expect` is either "passes" or the distinctive fragment of the diagnostic the
// case must produce. Asserting the *fragment* rather than merely "it failed" is
// the point: three of these cases used to fail for an unrelated reason, and a
// test that only checks the exit code calls that a pass.
const check = (name, edit, expect, about) => {
  writeFileSync(join(work, DOC), edit(BASE_DOC));
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  let ok;
  let why = expect;
  if (expect === "passes") {
    ok = out.status === 0;
  } else {
    // A substring alone is too weak, and several cases share one. `absent
    // here` would still match if the gate started failing for an unrelated
    // section while the mutation under test went undetected — the test stays
    // green on a broken gate, which is the failure these tests exist to catch.
    //
    // So three assertions, not one: the rule that fired, the **section number**
    // the case is about, and that exactly one problem was reported. Together
    // those pin down which rule fired on which input.
    const oneProblem = text.includes("(1 problem(s))");
    const mentions = about === undefined || text.includes(about);
    ok = out.status !== 0 && text.includes(expect) && oneProblem && mentions;
    why = `${expect}${about === undefined ? "" : ` — about ${about}`} — exactly 1 problem`;
  }
  if (!ok) {
    failed += 1;
    console.error(`FAIL ${name}\n  expected: ${why}\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  } else {
    console.log(`ok   ${name}`);
  }
};

const swap = (doc, from, to) => doc.replace(from, to);

// --- permitted ---------------------------------------------------------------
check("an untouched reference passes", (d) => d, "passes");
check(
  "retitling a merged section is allowed — the number is what is cited",
  (d) => swap(d, "## 9.7 Operation support matrix", "## 9.7 Rewritten title, same number"),
  "passes",
);
check(
  "retitling is still allowed when another section shares the old title",
  (d) => swap(d, "### 77.71 Repeated heading", "### 77.71 A different heading now"),
  "passes",
);
// Both twins, deliberately. Whether a naive map keeps the *first* or the *last*
// occurrence of a repeated title, exactly one of these two becomes a false
// "was 77.7x, now 77.7y" — so testing one twin leaves the other ordering open,
// and a mutation that drops the uniqueness guard survives a suite that checks
// only one.
check(
  "retitling the second of two same-titled sections is also allowed",
  (d) => swap(d, "### 77.72 Repeated heading", "### 77.72 A different heading now"),
  "passes",
);
check(
  "changing a heading's level is formatting, not renumbering",
  (d) => swap(d, "## 9.7 Operation support matrix", "### 9.7 Operation support matrix"),
  "passes",
);
check("adding a new number is the normal case", (d) => `${d}\n### 9.9 Brand new section\n\nnew\n`, "passes");
// A new section may legitimately carry a title an existing section already has
// — the reference has two sections titled "Repeated heading" today. The swap
// check must not read that as a move: the number it appears at is *new*, so no
// merged number changed hands. Without the base-allocated test this is a false
// positive, and it is the only case that distinguishes the two.
check(
  "a new number may carry a title an existing section already has",
  (d) => `${d}\n## 30 Alpha\n\nnew\n`,
  "passes",
);

// --- refused -----------------------------------------------------------------
check(
  "renumbering a Setext heading is caught (its title carries no '#')",
  (d) => swap(d, "77.77 Stable title", "77.78 Stable title"),
  "absent here",
  "77.77",
);
check(
  "renumbering while also retitling is caught (no title to match on)",
  (d) => swap(d, "## 9.7 Operation support matrix", "## 77.79 Completely new wording"),
  "absent here",
  "9.7",
);
check(
  "deleting a merged section is caught — it breaks citations just as a move does",
  (d) => d.replace(/## 9\.7 Operation support matrix\n\nbody\n/, ""),
  "absent here",
  "9.7",
);
check(
  "swapping two numbers is caught, though both numbers still exist",
  (d) =>
    swap(
      swap(d, "## 9.7 Operation support matrix", "## 77.77 Operation support matrix"),
      "77.77 Stable title",
      "9.7 Stable title",
    ),
  "a section number the base gave to something else",
);
// The swap above has unique titles. When *both* swapped titles also appear on
// another section — `10 Alpha` / `11 Alpha` and `20 Beta` / `21 Beta` — an
// implementation that protects itself from repeated titles by ignoring them
// lets the exchange through, and the presence check cannot help because both
// numbers are still there. Review found exactly this, and it is why the check
// compares each title's *set* of numbers rather than a single one.
check(
  "swapping two numbers is caught even when both titles are repeated elsewhere",
  (d) => swap(swap(d, "## 10 Alpha", "## 10 Beta"), "## 20 Beta", "## 20 Alpha"),
  "a section number the base gave to something else",
  "Alpha",
);

// Retitling a section **to a title another section already has** puts that
// title at two numbers without either number moving. An exchange vacates as
// well as occupies, so requiring a departure is what tells the two apart —
// without it the set comparison reintroduces the false positive it replaced.
check(
  "retitling a section to a title another section already has is allowed",
  (d) => swap(d, "## 20 Beta", "## 20 Alpha"),
  "passes",
);

// ...and retitling one of a repeated pair is still allowed, which is the
// property the discarded-titles approach was protecting.
check(
  "retitling one of a repeated pair is allowed",
  (d) => swap(d, "## 11 Alpha", "## 11 Something else"),
  "passes",
);

check(
  "a new heading under a grandfathered duplicate number is refused",
  (d) => `${d}\n## 1.2 A third one sneaking in\n\nq\n`,
  "excused only for its grandfathered headings",
  "1.2",
);
check(
  "a plain duplicate number is refused",
  (d) => `${d}\n## 9.7 Second claimant\n\nq\n`,
  "is used 2 times",
  "9.7",
);

// A failure list reports whichever rules fired, so the umbrella line must not
// name only one of them: a renumber reported as "duplicate section numbers"
// sends the author hunting for a collision that does not exist.
writeFileSync(join(work, DOC), swap(BASE_DOC, "77.77 Stable title", "77.78 Stable title"));
const umbrella = runGate();
if (/duplicate protocol-reference section numbers/.test(`${umbrella.stdout}${umbrella.stderr}`)) {
  failed += 1;
  console.error("FAIL a renumber must not be reported as a duplicate");
} else {
  console.log("ok   a renumber is not reported as a duplicate");
}

// The real document must satisfy its own gate, and the fixture above is not
// evidence of that — it shares none of the real headings, so it exercises the
// rules but not the *parser* against 2,000 lines of fences, tables and Setext.
//
// It has to go into `upstream` as well. Dropping it into the clone alone makes
// every fixture number look deleted, which is a true report about a nonsense
// comparison. Base and head must both be the real document; then the only way
// to fail is a genuine defect in parsing or uniqueness.
const realDoc = readFileSync(join(here, "..", DOC), "utf8");
writeFileSync(join(upstream, DOC), realDoc);
git(upstream, "commit", "-qam", "the real reference");
// The clone still carries the last case's edit; discard it before fetching.
git(work, "checkout", "-q", "--", DOC);
git(work, "pull", "-q");
writeFileSync(join(work, DOC), realDoc);
const real = runGate();
if (real.status !== 0) {
  failed += 1;
  console.error(`FAIL the committed reference does not pass its own gate: ${real.stderr}`);
} else {
  console.log(`ok   the committed reference passes (${realDoc.split("\n").length} lines)`);
}

rmSync(root, { recursive: true, force: true });
if (failed) {
  console.error(`\n${failed} failing contract(s)`);
  process.exit(1);
}
console.log("\nall section-gate contracts hold");
