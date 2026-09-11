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
const check = (name, edit, expect) => {
  writeFileSync(join(work, DOC), edit(BASE_DOC));
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  const ok = expect === "passes"
    ? out.status === 0
    : out.status !== 0 && text.includes(expect);
  if (!ok) {
    failed += 1;
    console.error(`FAIL ${name}\n  expected: ${expect}\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
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

// --- refused -----------------------------------------------------------------
check(
  "renumbering a Setext heading is caught (its title carries no '#')",
  (d) => swap(d, "77.77 Stable title", "77.78 Stable title"),
  "absent here",
);
check(
  "renumbering while also retitling is caught (no title to match on)",
  (d) => swap(d, "## 9.7 Operation support matrix", "## 77.79 Completely new wording"),
  "absent here",
);
check(
  "deleting a merged section is caught — it breaks citations just as a move does",
  (d) => d.replace(/## 9\.7 Operation support matrix\n\nbody\n/, ""),
  "absent here",
);
check(
  "swapping two numbers is caught, though both numbers still exist",
  (d) =>
    swap(
      swap(d, "## 9.7 Operation support matrix", "## 77.77 Operation support matrix"),
      "77.77 Stable title",
      "9.7 Stable title",
    ),
  "heading moved to a different number",
);
check(
  "a new heading under a grandfathered duplicate number is refused",
  (d) => `${d}\n## 1.2 A third one sneaking in\n\nq\n`,
  "excused only for its grandfathered headings",
);
check(
  "a plain duplicate number is refused",
  (d) => `${d}\n## 9.7 Second claimant\n\nq\n`,
  "is used 2 times",
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
