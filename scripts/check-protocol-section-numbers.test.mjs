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
const SURFACE = "docs/tally/compatibility/compatibility-surface.json";
const TEST_SHA256 = "0".repeat(64);
const writeSurface = (repository, paths) => {
  mkdirSync(dirname(join(repository, SURFACE)), { recursive: true });
  writeFileSync(
    join(repository, SURFACE),
    JSON.stringify({
      schema_version: 1,
      files: paths.map((path) => ({ path, sha256: TEST_SHA256 })),
      manifest_sha256: TEST_SHA256,
    }),
  );
};

mkdirSync(join(upstream, "scripts"), { recursive: true });
mkdirSync(join(upstream, "docs/tally"), { recursive: true });
git(upstream, "init", "-q", ".");
git(upstream, "config", "user.email", "test@example.invalid");
git(upstream, "config", "user.name", "test");
writeFileSync(join(upstream, DOC), BASE_DOC);
writeSurface(upstream, [DOC]);
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
check(
  "a grandfathered formatted heading remains recognized after conversion to Setext",
  (d) => d.replace(
    "## 1.2 A modal error dialog in Tally's UI blocks the gateway until a human clicks OK — **P0 operationally**",
    "1.2 A modal error dialog in Tally's UI blocks the gateway until a human clicks OK — **P0 operationally**\n---",
  ),
  "passes",
);
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
// Titles move between numbers for entirely legitimate reasons, which is why
// this gate no longer tries to detect a swap. Each of the three cases below was
// a false positive in one of the three attempts, and each must now pass — the
// last one is the residual that buys the other two.
check(
  "a retitle chain is allowed — Beta leaves 20 and arrives at 10, nothing moved",
  (d) => swap(swap(d, "## 10 Alpha", "## 10 Beta"), "## 20 Beta", "## 20 Gamma"),
  "passes",
);
check(
  "exchanging two numbers is NOT detected — a documented residual, not a pass",
  (d) =>
    swap(
      swap(d, "## 9.7 Operation support matrix", "## 77.77 Operation support matrix"),
      "77.77 Stable title",
      "9.7 Stable title",
    ),
  "passes",
);


// Retitling a section to a title another section already has puts that title at
// two numbers without either number moving.
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

// --- split inventory and legacy anchors -------------------------------------
// A pre-split base has one document. The split index names the files that now
// allocate one shared number namespace; the gate must catch a duplicate in a
// second part, permit a whole section moving there, and retain the old anchor.
// Keep BASE_DOC's Setext and repeated-title cases for the monolithic parser
// contracts above. Split route sources deliberately use the narrower admitted
// grammar: ATX headings with unique generated fragments.
const SPLIT_BASE_DOC = BASE_DOC
  .replace("77.77 Stable title\n==================", "## 77.77 Stable title")
  .replace("### 77.72 Repeated heading", "### 77.72 Repeated heading second");
const PART_A = "docs/tally/TALLY_PROTOCOL_REFERENCE_PART_A.md";
const PART_B = "docs/tally/TALLY_PROTOCOL_REFERENCE_PART_B.md";
const PART_C = "docs/tally/TALLY_PROTOCOL_REFERENCE_PART_C.md";
const anchorsFor = (documents) => {
  const globalCounts = new Map();
  return documents.flatMap((document, index) => {
    const localCounts = new Map();
    return document.split("\n").flatMap((line) => {
      const found = /^ {0,3}#{2,6}\s+(.*)$/.exec(line);
      if (!found) return [];
      const title = found[1].trimEnd().replace(/[ \t]+#+[ \t]*$/, "").trimEnd();
      const seed = title.toLowerCase().replace(/[^a-z0-9 _-]/g, "").replace(/ /g, "-");
      const global = globalCounts.get(seed) ?? 0;
      const local = localCounts.get(seed) ?? 0;
      globalCounts.set(seed, global + 1);
      localCounts.set(seed, local + 1);
      return [{ anchor: global === 0 ? seed : `${seed}-${global}`, title, path: index === 0 ? PART_A : PART_B, target: local === 0 ? seed : `${seed}-${local}` }];
    });
  });
};
const routeBlock = ({ anchor, title, path, target }) =>
  `<a id="${anchor}"></a>\n\n[${title}](./${path.split("/").pop()}#${target})`;
const splitIndex = (documents, omitted = new Set()) => `# Reference index

` +
  `<!-- protocol-reference-parts: TALLY_PROTOCOL_REFERENCE_PART_A.md | TALLY_PROTOCOL_REFERENCE_PART_B.md -->

` +
  anchorsFor(documents).filter(({ anchor }) => !omitted.has(anchor)).map(routeBlock).join("\n") +
  "\n";
const runSplitGate = (first, second, omitted) => {
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(join(work, DOC), splitIndex([first, second], omitted));
  writeSurface(work, [DOC, PART_A, PART_B]);
  return runGate();
};

{
  const out = runSplitGate(SPLIT_BASE_DOC, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("Setext headings are unsupported in split protocol routes")) {
    console.log("ok   a pre-split Setext heading is refused before claiming preserved split routes");
  } else {
    failed += 1;
    console.error(`FAIL a pre-split Setext route source must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

writeFileSync(join(upstream, DOC), SPLIT_BASE_DOC);
git(upstream, "add", DOC);
git(upstream, "commit", "-qm", "use the admitted split-heading grammar");
git(work, "fetch", "-q", "origin");

{
  const second = "# Part B\n\n## 9.7 Second claimant in another part\n\nq\n";
  const out = runSplitGate(SPLIT_BASE_DOC, second);
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("is used 2 times") && text.includes("9.7") && text.includes("(1 problem(s))")) {
    console.log("ok   a number duplicated across declared parts is caught");
  } else {
    failed += 1;
    console.error(`FAIL a cross-part duplicate must be caught\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n90 Setext route\n================\n\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("Setext headings are unsupported in split protocol routes")) {
    console.log("ok   a current split part cannot claim complete routes while using Setext headings");
  } else {
    failed += 1;
    console.error(`FAIL a current Setext route source must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n## Reference\n\nbody\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("ambiguous duplicate file-local heading fragment") &&
    text.includes("reference")
  ) {
    console.log("ok   a part H1 reserves its file-local fragment before H2-H6 routes");
  } else {
    failed += 1;
    console.error(`FAIL a part H1 must reserve its rendered fragment\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n<!--\n## 90 Hidden example heading\n-->\n`;
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(join(work, DOC), splitIndex([SPLIT_BASE_DOC, second]));
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  if (out.status === 0) {
    console.log("ok   a heading inside a part HTML comment does not create a route or number");
  } else {
    failed += 1;
    console.error(`FAIL commented part headings must stay invisible\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

for (const [name, title] of [
  ["a non-ASCII letter", "90 Café"],
  ["a combining mark", `90 Cafe${"\u0301"}`],
]) {
  const first = `${SPLIT_BASE_DOC}\n## ${title}\n\nbody\n`;
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  // Keep malformed markup out of the generated fixture route itself so the
  // refusal is proved at the heading boundary, before route completeness.
  writeFileSync(join(work, DOC), splitIndex([SPLIT_BASE_DOC, second]));
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("unsupported Unicode character in split protocol heading")
  ) {
    console.log(`ok   ${name} is refused before an incorrect legacy fragment is accepted`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must be refused by the route grammar\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

for (const [name, title] of [
  ["a Markdown link", "90 [Linked](./target)"],
  ["raw inline HTML", "90 <TYPE>"],
  ["single-star emphasis", "90 *emphasis*"],
  ["underscore emphasis", "90 _emphasis_"],
  ["strikethrough", "90 ~~strike~~"],
  ["a backslash escape", String.raw`90 escaped \*star`],
  ["an entity", "90 Copy &copy;"],
  ["unbalanced strong markup", "90 **unbalanced"],
  ["a padded code span", "90 ` padded `"],
  ["a multi-backtick code span", "90 ``multi``"],
]) {
  const first = `${SPLIT_BASE_DOC}\n## ${title}\n\nbody\n`;
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  // Keep malformed markup out of the generated fixture route itself so the
  // refusal is proved at the heading boundary, before route completeness.
  writeFileSync(join(work, DOC), splitIndex([SPLIT_BASE_DOC, second]));
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("unsupported split protocol heading markup")) {
    console.log(`ok   ${name} is refused by the bounded split-heading grammar`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must be refused by the route grammar\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const out = runSplitGate(
    `${SPLIT_BASE_DOC}\n## 90 **Strong \`CODE\`** — §\n\nbody\n`,
    "# Part B\n",
  );
  if (out.status === 0) {
    console.log("ok   the real protocol's strong, code, §, and — heading grammar remains supported");
  } else {
    failed += 1;
    console.error(`FAIL supported protocol-heading markup must pass\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n## 90 \`A]B\`\n\nbody\n`;
  const out = runSplitGate(first, "# Part B\n");
  if (out.status === 0) {
    console.log("ok   a closing bracket inside an admitted code span remains part of the redirect label");
  } else {
    failed += 1;
    console.error(`FAIL code-span brackets must not close redirect labels\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n## 90 C#\n\nbody\n\n## 91 A##\n\nbody\n\n## 92 C# ###\n\nbody\n`;
  const out = runSplitGate(first, "# Part B\n");
  if (out.status === 0) {
    console.log("ok   literal terminal hashes survive while a whitespace-delimited ATX closer is removed");
  } else {
    failed += 1;
    console.error(`FAIL ATX closing hashes must not consume literal title hashes\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n## **9.7** Duplicate claimant\n\nbody\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("section 9.7 is used 2 times") &&
    text.includes("**9.7** Duplicate claimant") &&
    text.includes("(1 problem(s))")
  ) {
    console.log("ok   a formatted section prefix participates in number allocation");
  } else {
    failed += 1;
    console.error(`FAIL formatted section numbers must not bypass allocation\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n<div>\n## 90 Hidden heading\n</div>\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("unsupported raw HTML block in split protocol part")) {
    console.log("ok   a heading hidden inside a part raw-HTML block cannot become a route target");
  } else {
    failed += 1;
    console.error(`FAIL part raw-HTML blocks must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

for (const [name, heading] of [
  ["a blockquote heading", "> ## 9.7 Second claimant inside a quote\n"],
  ["a nested blockquote/list heading", "> - ## 9.7 Second claimant inside a list\n"],
]) {
  const first = `${SPLIT_BASE_DOC}\n${heading}`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("container-prefixed ATX heading") && text.includes(PART_A)) {
    console.log(`ok   ${name} is refused before it can evade split-route validation`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n> \`\`\`md\n> ## 9.7 example only\n> \`\`\`\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("container-prefixed fenced code block") && text.includes(PART_A)) {
    console.log("ok   a container-fenced heading example is refused instead of changing fence scope");
  } else {
    failed += 1;
    console.error(`FAIL a container-fenced heading example must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n  \`\`\`md\n  ## 9.7 example only\n  \`\`\`\n\n## 9.7 live duplicate\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("section 9.7 is used 2 times") && text.includes("live duplicate")) {
    console.log("ok   an indented fence hides its example while the following live heading is checked");
  } else {
    failed += 1;
    console.error(`FAIL an indented fence must close before a following live heading\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  const anchor = '<a id="97-operation-support-matrix"></a>';
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second]).replace(anchor, `paragraph ${anchor}\n\n${anchor}`),
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("unsupported raw HTML block in split protocol index")) {
    console.log("ok   an inline canonical HTML ID cannot shadow a complete legacy redirect");
  } else {
    failed += 1;
    console.error(`FAIL inline canonical HTML IDs must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

for (const [name, shadow] of [
  [
    "an HTML ID after a heading interrupts an unmatched code span",
    'Example ` text\n# Extra navigation\ntext <span id="97-operation-support-matrix"></span>\n`\n\n',
  ],
  [
    "a multiline inline-code HTML example outside the admitted format",
    'Example ` text\n<span id="97-operation-support-matrix"></span>\n`\n\n',
  ],
  [
    "an indented HTML ID continuing a paragraph",
    'Example paragraph\n    <span id="97-operation-support-matrix"></span>\n\n',
  ],
  [
    "an HTML ID after astral text and a paired code span",
    'Example \u{1f600}\u{1f600} `safe` <span id="97-operation-support-matrix"></span>\n\n',
  ],
  [
    "an unmatched backtick before an HTML ID",
    'Example ` text <span id="97-operation-support-matrix"></span>\n\n',
  ],
  [
    "a multiline HTML tag carrying an ID",
    'Example <span\nid="97-operation-support-matrix"></span>\n\n',
  ],
  [
    "an HTML ID whose attribute name is split from its assignment",
    'Example <a\nid\n="97-operation-support-matrix"></a>\n\n',
  ],
  [
    "a quoted multiline HTML ID",
    '> Example <a\n> id\n> ="97-operation-support-matrix"></a>\n\n',
  ],
  [
    "an HTML ID between mismatched backtick runs",
    'Example ` one ``` <a id="97-operation-support-matrix"></a> ``\n\n',
  ],
  [
    "an HTML ID between escaped backticks",
    'Example \\` <a id="97-operation-support-matrix"></a> \\`\n\n',
  ],
  [
    "an HTML ID after a backslash-prefixed code closer",
    'Example `safe \\` <a id="97-operation-support-matrix"></a> `\n\n',
  ],
  [
    "an HTML ID after a quoted greater-than attribute",
    'Example <span title=">" id="97-operation-support-matrix"></span>\n\n',
  ],
]) {
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  const anchor = '<a id="97-operation-support-matrix"></a>';
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(join(work, DOC), splitIndex([first, second]).replace(anchor, shadow + anchor));
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("unsupported raw HTML block in split protocol index")) {
    console.log(`ok   ${name} cannot shadow a complete legacy redirect`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n## 90 id=value\n\nbody\n`;
  const out = runSplitGate(first, "# Part B\n");
  if (out.status === 0) {
    console.log("ok   ordinary id=value heading text is not mistaken for raw HTML");
  } else {
    failed += 1;
    console.error(`FAIL ordinary heading text must remain supported\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\nparagraph <a id="90-inline-id"></a>\n\n## 90 Inline id\n\nbody\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("unsupported raw HTML block in split protocol part")) {
    console.log("ok   an inline part HTML ID cannot shadow a heading fragment");
  } else {
    failed += 1;
    console.error(`FAIL inline part HTML IDs must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

for (const [name, example] of [
  ["an inline-code HTML ID example", '`<a id="example-only"></a>`'],
  ["an inline-code HTML ID example ending in a literal backslash", '`<a id="example-only"></a> \\`'],
  ["an inline-code HTML comment example", '`<!-- example only -->`'],
]) {
  const first = `${SPLIT_BASE_DOC}\nparagraph ${example}\n`;
  const out = runSplitGate(first, "# Part B\n");
  if (out.status === 0) {
    console.log(`ok   ${name} remains non-rendered content`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must remain allowed\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n## Method note\n\nbody\n`;
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), "# Part B\n");
  writeFileSync(
    join(work, DOC),
    splitIndex([first, "# Part B\n"]).replace(
      "# Reference index\n\n",
      "# Reference index\n\n## Method note\n\n",
    ),
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("canonical index heading fragment(s) collide with required legacy anchors") &&
    text.includes("method-note")
  ) {
    console.log("ok   a canonical heading cannot capture a required legacy anchor");
  } else {
    failed += 1;
    console.error(`FAIL canonical headings must not capture legacy redirects\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second]).replace(
      "# Reference index\n\n",
      "# Reference index\n\nSetext navigation\n-----------------\n\n",
    ),
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("Setext headings are unsupported in split protocol index")) {
    console.log("ok   a canonical Setext heading is refused before claiming preserved fragments");
  } else {
    failed += 1;
    console.error(`FAIL canonical Setext headings must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

// The canonical marker owns the protocol inventory. Every declared file must
// also be pinned by the compatibility surface, without adding a second static
// list to this gate.
{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  const third = "# Part C\n";
  const indexWithThirdPart = splitIndex([first, second]).replace(
    "TALLY_PROTOCOL_REFERENCE_PART_B.md -->",
    "TALLY_PROTOCOL_REFERENCE_PART_B.md | TALLY_PROTOCOL_REFERENCE_PART_C.md -->",
  );
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(join(work, PART_C), third);
  writeFileSync(join(work, DOC), indexWithThirdPart);
  writeSurface(work, [DOC, PART_A, PART_B]);
  let out = runGate();
  let text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("protocol-reference file(s) absent from the compatibility surface") &&
    text.includes(PART_C) &&
    text.includes("(1 problem(s))")
  ) {
    console.log("ok   a declared protocol part absent from the compatibility surface is refused");
  } else {
    failed += 1;
    console.error(`FAIL an unpinned declared part must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }

  writeFileSync(
    join(work, SURFACE),
    JSON.stringify({
      schema_version: 1,
      files: [{ path: DOC }],
      manifest_sha256: TEST_SHA256,
    }),
  );
  out = runGate();
  text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("invalid compatibility surface file row 1")) {
    console.log("ok   a malformed compatibility surface row is refused");
  } else {
    failed += 1;
    console.error(`FAIL a malformed compatibility surface must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }

  rmSync(join(work, SURFACE));
  out = runGate();
  text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("compatibility surface is unreadable")) {
    console.log("ok   a missing compatibility surface is refused");
  } else {
    failed += 1;
    console.error(`FAIL a missing compatibility surface must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }

  writeSurface(work, [DOC, PART_A, PART_B, PART_C]);
  out = runGate();
  if (out.status === 0) {
    console.log("ok   an additionally declared protocol part passes once pinned");
  } else {
    failed += 1;
    console.error(`FAIL a pinned declared part must pass\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  const markerLookingExample =
    "\n\`\`\`md\n<!-- protocol-reference-parts: TALLY_PROTOCOL_REFERENCE_ABSENT.md -->\n\`\`\`\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second]) +
      "\n<!-- protocol-reference-parts: TALLY_PROTOCOL_REFERENCE_ABSENT.md -->\n",
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  let out = runGate();
  let text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("multiple protocol-reference part inventories")) {
    console.log("ok   a second protocol inventory is refused even when it names an absent part");
  } else {
    failed += 1;
    console.error(`FAIL a second protocol inventory must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }

  writeFileSync(join(work, DOC), splitIndex([first, second]) + markerLookingExample);
  out = runGate();
  text = `${out.stdout}${out.stderr}`;
  if (out.status === 0) {
    console.log("ok   a fenced marker-looking example does not declare a second protocol inventory");
  } else {
    failed += 1;
    console.error(`FAIL a fenced marker-looking example must remain inert\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

// Inventory input is user-authored and reaches the gate before ordinary
// failure aggregation. Reject oversized names and inventories before any
// filesystem call, and keep the resulting native Node diagnostic bounded.
for (const [name, inventory, diagnostic] of [
  ["an invalid long part token", `bad-${"x".repeat(10_000)}`, "invalid protocol-reference part"],
  [
    "a valid-looking long part token",
    `TALLY_PROTOCOL_REFERENCE_${"A".repeat(10_000)}.md`,
    "invalid protocol-reference part",
  ],
  [
    "an oversized declared-part inventory",
    Array.from({ length: 65 }, (_, index) =>
      `TALLY_PROTOCOL_REFERENCE_EXTRA_${String(index).padStart(2, "0")}.md`).join(" | "),
    "too many protocol-reference parts",
  ],
]) {
  const index = splitIndex([SPLIT_BASE_DOC, "# Part B\n"]).replace(
    /<!-- protocol-reference-parts:.*?-->/,
    `<!-- protocol-reference-parts: ${inventory} -->`,
  );
  writeFileSync(join(work, DOC), index);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes(diagnostic) &&
    text.length < 3_000 &&
    !text.includes("ENAMETOOLONG") &&
    !text.includes("x".repeat(1_000)) &&
    !text.includes("A".repeat(1_000))
  ) {
    console.log(`ok   ${name} is refused with a bounded diagnostic`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must fail before native path handling with bounded output\n  exit ${out.status}; bytes ${text.length}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second]) + "\n## 9.7 Numbered index heading\n\n",
  );
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("is used 2 times") &&
    text.includes("9.7") &&
    text.includes("(1 problem(s))")
  ) {
    console.log("ok   a number duplicated between the canonical index and a part is caught");
  } else {
    failed += 1;
    console.error(`FAIL an index-part duplicate must be caught\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  const anchor = '<a id="97-operation-support-matrix"></a>';
  const orphan = `${anchor}\n\norphaned anchor body\n\n`;
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(join(work, DOC), splitIndex([first, second]).replace(anchor, orphan + anchor));
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("1 duplicate legacy-anchor occurrence(s)") &&
    text.includes("1 incomplete legacy anchor block(s)") &&
    text.includes("97-operation-support-matrix") &&
    text.includes("(2 problem(s))")
  ) {
    console.log("ok   an incomplete anchor is refused and still participates in duplicate-ID accounting");
  } else {
    failed += 1;
    console.error(`FAIL incomplete anchors must be refused and counted\n  exit ${out.status}: ${text.split("\n").slice(0, 8).join("\n  ")}`);
  }
}

{
  const moved = "## 9.7 Operation support matrix\n\nbody\n";
  const first = SPLIT_BASE_DOC.replace(moved, "");
  const out = runSplitGate(first, `# Part B\n\n${moved}`);
  const text = `${out.stdout}${out.stderr}`;
  if (out.status === 0) {
    console.log("ok   moving a base section into a declared part is not flagged as deleted");
  } else {
    failed += 1;
    console.error(`FAIL a genuine section move must pass\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

// Generated indexes can repeat many different anchors. The refusal must name
// this fault and its total count without allowing the diagnostic to scale with
// every repeated ID.
{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  const repeats = Array.from({ length: 80 }, (_, index) => {
    const anchor = `duplicate-route-${String(index).padStart(2, "0")}-${"x".repeat(300)}`;
    const route = routeBlock({
      anchor,
      title: "extra",
      path: PART_A,
      target: "10-alpha",
    });
    return `${route}\n${route}`;
  }).join("\n");
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(join(work, DOC), `${splitIndex([first, second])}${repeats}\n`);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  const shownDuplicates = text.match(/^    duplicate-route-/gm)?.length ?? 0;
  if (
    out.status !== 0 &&
    text.includes("80 duplicate legacy-anchor occurrence(s)") &&
    text.includes("(1 problem(s))") &&
    shownDuplicates === 20 &&
    text.length < 8_000
  ) {
    console.log("ok   duplicate-anchor diagnostics report the fault and count within a fixed bound");
  } else {
    failed += 1;
    console.error(
      `FAIL duplicate-anchor diagnostics must be bounded and specific\n` +
        `  exit ${out.status}; shown ${shownDuplicates}; bytes ${text.length}: ${text.split("\n").slice(0, 6).join("\n  ")}`,
    );
  }
}

{
  const first = SPLIT_BASE_DOC.replace("## 9.7 Operation support matrix", "## 9.8 Renumbered section");
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  // Keep the former fragment itself so this case isolates the base-number rule
  // rather than failing first for the independently enforced anchor redirect.
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second]),
  );
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("absent here") && text.includes("9.7")) {
    console.log("ok   renumbering a section inside a declared part is caught against the base");
  } else {
    failed += 1;
    console.error(`FAIL a split-part renumber must be caught\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const moved = "## 9.7 Operation support matrix\n\nbody\n";
  const first = SPLIT_BASE_DOC.replace(moved, "");
  const out = runSplitGate(first, `# Part B\n\n${moved}`, new Set(["97-operation-support-matrix"]));
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("legacy section anchor(s) missing") && text.includes("97-operation-support-matrix") && text.includes("(1 problem(s))")) {
    console.log("ok   removing a legacy anchor is caught");
  } else {
    failed += 1;
    console.error(`FAIL a missing legacy anchor must be caught\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const moved = "## 9.7 Operation support matrix\n\nbody\n";
  const first = SPLIT_BASE_DOC.replace(moved, "");
  const second = `# Part B\n\n${moved}`;
  const fakeRoute = routeBlock({
    anchor: "97-operation-support-matrix",
    title: "9.7 Operation support matrix",
    path: PART_B,
    target: "97-operation-support-matrix",
  });
  const sameLineRoute = '<a id="97-operation-support-matrix"></a> [9.7 Operation support matrix](./TALLY_PROTOCOL_REFERENCE_PART_B.md#97-operation-support-matrix)';
  const quotedRoute = fakeRoute.split("\n").map((line) => line ? `> ${line}` : ">").join("\n");
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second], new Set(["97-operation-support-matrix"])) +
      `\n\`\`\`md\n${fakeRoute}\n\`\`\`\n${quotedRoute}\n\`${sameLineRoute}\`\n` +
      `<!--\n${fakeRoute}\n-->\n`,
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("unsupported raw HTML block in split protocol index")
  ) {
    console.log("ok   quoted HTML IDs are refused while fenced, inline-code, and commented routes stay non-rendered");
  } else {
    failed += 1;
    console.error(`FAIL code examples must not satisfy a missing legacy route\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

{
  const moved = "## 9.7 Operation support matrix\n\nbody\n";
  const first = SPLIT_BASE_DOC.replace(moved, "");
  const second = `# Part B\n\n${moved}`;
  const sameLineRoute = '<a id="97-operation-support-matrix"></a> [9.7 Operation support matrix](./TALLY_PROTOCOL_REFERENCE_PART_B.md#97-operation-support-matrix)';
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second], new Set(["97-operation-support-matrix"])) +
      `\n\`\n${sameLineRoute}\n\`\n`,
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("unsupported raw HTML block in split protocol index") &&
    text.includes("docs/tally/TALLY_PROTOCOL_REFERENCE.md:")
  ) {
    console.log("ok   a full route inside multiline inline code is refused");
  } else {
    failed += 1;
    console.error(`FAIL multiline inline code must not satisfy a legacy route\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}


{
  const moved = "## 9.7 Operation support matrix\n\nbody\n";
  const first = SPLIT_BASE_DOC.replace(moved, "");
  const second = `# Part B\n\n${moved}`;
  const fakeRoute = routeBlock({
    anchor: "97-operation-support-matrix",
    title: "9.7 Operation support matrix",
    path: PART_B,
    target: "97-operation-support-matrix",
  });
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second], new Set(["97-operation-support-matrix"])) +
      `\n<div>\n${fakeRoute}\n</div>\n`,
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (out.status !== 0 && text.includes("unsupported raw HTML block in split protocol index")) {
    console.log("ok   a raw HTML block cannot hide a route example from Markdown rendering");
  } else {
    failed += 1;
    console.error(`FAIL raw HTML route blocks must be refused\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
  }
}

// Run the entire gate with Windows-style relative paths while retaining the
// host filesystem resolver. This exercises both Git tree lookups and Markdown
// route comparison on any CI host; it is not a Windows-host qualification.
{
  runSplitGate(SPLIT_BASE_DOC, "# Part B\n");
  const gatePath = join(work, "scripts/check-protocol-section-numbers.mjs");
  const original = readFileSync(gatePath, "utf8");
  const importLine = 'import { relative, resolve, sep } from "node:path";';
  if (!original.includes(importLine)) throw new Error("Windows path control needs its import seam updated");
  const windowsPaths = original.replace(importLine, String.raw`import { relative as hostRelative, resolve } from "node:path";
const relative = (...args) => hostRelative(...args).replaceAll("/", "\\");
const sep = "\\";`);
  try {
    writeFileSync(gatePath, windowsPaths);
    const out = runGate();
    const text = `${out.stdout}${out.stderr}`;
    if (out.status === 0 && !/rule was not checked/i.test(text)) {
      console.log("ok   Windows-style paths preserve Git base reads and current section routes");
    } else {
      failed += 1;
      console.error(`FAIL Windows-style paths must retain base and route checks\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
    }
  } finally {
    writeFileSync(gatePath, original);
  }
}

// Retitling keeps a section number allocated. Its old URL becomes an alias to
// the new heading, including on later PRs whose base is already split/retitled.
{
  const first = SPLIT_BASE_DOC.replace("## 10 Alpha", "## 10 Alpha revised");
  const second = "# Part B\n";
  const index = splitIndex([first, second]);
  const alias = routeBlock({
    anchor: "10-alpha",
    title: "10 Alpha",
    path: PART_A,
    target: "10-alpha-revised",
  }) + "\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  const expectRoute = (name, content, diagnostic, anchor = "10-alpha") => {
    writeFileSync(join(work, DOC), content);
    const out = runGate();
    const text = `${out.stdout}${out.stderr}`;
    const ok = diagnostic === undefined
      ? out.status === 0
      : out.status !== 0 && text.includes(diagnostic) && text.includes(anchor) && text.includes("(1 problem(s))");
    if (ok) console.log(`ok   ${name}`);
    else {
      failed += 1;
      console.error(`FAIL ${name}\n  exit ${out.status}: ${text.split("\n").slice(0, 6).join("\n  ")}`);
    }
  };
  expectRoute("split retitle preserves the old fragment as a working alias", index + alias);
  expectRoute(
    "a first-generation retitle alias cannot target an unrelated current section",
    index + alias.replace("#10-alpha-revised)", "#20-beta)"),
    "legacy anchor",
  );

  const unnumberedBase = `${SPLIT_BASE_DOC}\n## Method note\n\nmethod\n`;
  writeFileSync(join(upstream, DOC), unnumberedBase);
  git(upstream, "add", DOC);
  git(upstream, "commit", "-qm", "add an unnumbered route source");
  git(work, "fetch", "-q", "origin");
  expectRoute(
    "a first-generation unnumbered alias is refused instead of guessing a destination",
    index + alias + routeBlock({
      anchor: "method-note",
      title: "Method note",
      path: PART_A,
      target: "10-alpha-revised",
    }) + "\n",
    "legacy anchor",
    "method-note",
  );
  writeFileSync(join(upstream, DOC), SPLIT_BASE_DOC);
  git(upstream, "add", DOC);
  git(upstream, "commit", "-qm", "restore the numbered route fixture");
  git(work, "fetch", "-q", "origin");
  expectRoute("legacy aliases cannot target a nonexistent heading",
    index + alias.replace("#10-alpha-revised)", "#missing-heading)"), "do not resolve");

  const splitBaseNewAliasFirst = `${SPLIT_BASE_DOC}\n## 30 Plain source\n\nbody\n`;
  const splitBaseNewAliasIndex = splitIndex([splitBaseNewAliasFirst, second]);
  writeFileSync(join(upstream, PART_A), splitBaseNewAliasFirst);
  writeFileSync(join(upstream, PART_B), second);
  writeFileSync(join(upstream, DOC), splitBaseNewAliasIndex);
  git(upstream, "add", "-A");
  git(upstream, "commit", "-qm", "split reference before a new retitle");
  git(work, "fetch", "-q", "origin");
  const splitRetitledFirst = splitBaseNewAliasFirst.replace("## 30 Plain source", "## 30 Retitled source");
  writeFileSync(join(work, PART_A), splitRetitledFirst);
  writeFileSync(join(work, PART_B), second);
  expectRoute(
    "a split-base retitle cannot redirect its newly created alias to an unrelated live section",
    splitIndex([splitRetitledFirst, second]) + routeBlock({
      anchor: "30-plain-source",
      title: "30 Plain source",
      path: PART_A,
      target: "20-beta",
    }) + "\n",
    "legacy anchor",
    "30-plain-source",
  );

  const splitBaseFirst = `${first}\n## 30 Method note revised\n\nmethod\n`;
  const splitBaseIndex = splitIndex([splitBaseFirst, second]);
  const methodAlias = routeBlock({
    anchor: "method-note",
    title: "Method note",
    path: PART_A,
    target: "30-method-note-revised",
  }) + "\n";
  const splitBaseContent = splitBaseIndex + alias + methodAlias;
  writeFileSync(join(work, PART_A), splitBaseFirst);
  writeFileSync(join(upstream, PART_A), splitBaseFirst);
  writeFileSync(join(upstream, PART_B), second);
  const fencedBaseExample = `\n\`\`\`md\n${routeBlock({
    anchor: "example-only",
    title: "example",
    path: PART_A,
      target: "30-method-note-revised",
  })}\n\`\`\`\n`;
  writeFileSync(join(upstream, DOC), splitBaseContent + fencedBaseExample);
  git(upstream, "add", "-A");
  git(upstream, "commit", "-qm", "split reference with a fenced route example");
  git(work, "fetch", "-q", "origin");
  expectRoute("a fenced route example in the base does not become an inherited alias", splitBaseContent);

  writeFileSync(join(upstream, DOC), splitBaseContent);
  git(upstream, "add", DOC);
  git(upstream, "commit", "-qm", "retain only rendered split routes");
  git(work, "fetch", "-q", "origin");
  expectRoute("an inherited alias remains valid on a later split-base edit", splitBaseContent);
  expectRoute("a later split-base edit cannot drop its inherited alias", splitBaseIndex, "legacy section anchor(s) missing");
  expectRoute("an inherited alias cannot retain a dead destination",
    splitBaseIndex + alias.replace("#10-alpha-revised)", "#missing-heading)") + methodAlias, "do not resolve");
  expectRoute(
    "an inherited alias cannot change its still-live destination",
    splitBaseIndex + alias + methodAlias.replace("#30-method-note-revised)", "#10-alpha-revised)"),
    "changed their still-live destination",
    "method-note",
  );

  const retitledAgainFirst = splitBaseFirst.replace("## 30 Method note revised", "## 30 Method note final");
  const retitledAgainIndex = splitIndex([retitledAgainFirst, second]);
  const revisedAlias = routeBlock({
    anchor: "30-method-note-revised",
    title: "30 Method note revised",
    path: PART_A,
    target: "30-method-note-final",
  }) + "\n";
  writeFileSync(join(work, PART_A), retitledAgainFirst);
  expectRoute(
    "an inherited alias may follow a legitimate second retitle",
    retitledAgainIndex + alias + revisedAlias + methodAlias.replace("#30-method-note-revised)", "#30-method-note-final)"),
  );
  expectRoute(
    "an inherited alias cannot leave a legitimate second-retitle chain",
    retitledAgainIndex + alias + revisedAlias + methodAlias.replace("#30-method-note-revised)", "#10-alpha-revised)"),
    "changed their still-live destination or retitle chain",
    "method-note",
  );
  const duplicateFirst = splitBaseFirst.replace(
    "## 30 Method note revised",
    "## 30 Method note revised\n\nnew earlier section\n\n## 30 Method note revised",
  );
  writeFileSync(join(work, PART_A), duplicateFirst);
  writeFileSync(join(work, DOC), splitIndex([duplicateFirst, second]) + alias + methodAlias);
  const duplicateOut = runGate();
  const duplicateText = `${duplicateOut.stdout}${duplicateOut.stderr}`;
  if (
    duplicateOut.status !== 0 &&
    duplicateText.includes("ambiguous duplicate file-local heading fragment") &&
    duplicateText.includes("30-method-note-revised")
  ) {
    console.log("ok   a duplicate heading insertion cannot steal an ordinary base fragment");
  } else {
    failed += 1;
    console.error(`FAIL a duplicate heading insertion must be refused\n  exit ${duplicateOut.status}: ${duplicateText.split("\n").slice(0, 6).join("\n  ")}`);
  }

  const collisionFirst = `${splitBaseFirst}\n## Method note\n\nnew section\n`;
  writeFileSync(join(work, PART_A), collisionFirst);
  expectRoute(
    "a new unnumbered heading cannot steal an inherited alias",
    splitIndex([collisionFirst, second]) + alias,
    "inherited legacy anchor(s) reused by current headings",
    "method-note",
  );

  // A numbered heading in a split index is part of the shared number
  // namespace, but it is navigation rather than a legacy route source.
  writeFileSync(join(work, PART_A), splitBaseFirst);
  const numberedBaseContent = `${splitBaseContent}\n## 88 Index allocation\n\nindex body\n`;
  writeFileSync(join(upstream, DOC), numberedBaseContent);
  git(upstream, "add", DOC);
  git(upstream, "commit", "-qm", "allocate a number in the split index");
  git(work, "fetch", "-q", "origin");
  expectRoute("an unchanged numbered split index passes without becoming a legacy route", numberedBaseContent);
  expectRoute(
    "deleting a base index number is caught",
    splitBaseContent,
    "absent here",
    "88",
  );
  expectRoute(
    "renumbering a base index heading is caught",
    numberedBaseContent.replace("## 88 Index allocation", "## 89 Index allocation"),
    "absent here",
    "88",
  );
  // The real-tree case below pulls a new fixture base. Only this disposable
  // clone is restored; all candidate files remain untouched.
  git(work, "checkout", "-q", "--", DOC);
  rmSync(join(work, PART_A));
  rmSync(join(work, PART_B));
}

// --- #533 container visibility gaps -----------------------------------------
// Reset the disposable two-repository fixture to one split base. These controls
// exercise the actual base comparison so a convenient fixture-only failure
// cannot stand in for the gate's refusal.
{
  const first = SPLIT_BASE_DOC;
  const second = "# Part B\n";
  writeFileSync(join(upstream, PART_A), first);
  writeFileSync(join(upstream, PART_B), second);
  writeFileSync(join(upstream, DOC), splitIndex([first, second]));
  writeSurface(upstream, [DOC, PART_A, PART_B]);
  git(upstream, "add", "-A");
  git(upstream, "commit", "-qm", "reset split fixture for container controls");
  git(work, "fetch", "-q", "origin");

  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  writeFileSync(
    join(work, DOC),
    splitIndex([first, second]).replace(
      "# Reference index\n\n",
      "# Reference index\n\n> ## Method note\n\n",
    ),
  );
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("container-prefixed ATX heading is unsupported in split protocol index") &&
    text.includes(DOC)
  ) {
    console.log("ok   a blockquote index heading cannot shadow a legacy fragment");
  } else {
    failed += 1;
    console.error(`FAIL a blockquote index heading must be refused before it shadows a legacy fragment\n  exit ${out.status}: ${text.split("\\n").slice(0, 6).join("\\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n- continuation context\n  \`\`\`md\n  ## 9.7 example only\n\n## 9.7 live duplicate\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("list-continuation fenced code block is unsupported in split protocol part") &&
    text.includes(PART_A)
  ) {
    console.log("ok   an unclosed list-continuation fence cannot mask a dedented live duplicate");
  } else {
    failed += 1;
    console.error(`FAIL an unclosed list-continuation fence must be refused before it masks a live duplicate\n  exit ${out.status}: ${text.split("\\n").slice(0, 6).join("\\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n- continuation context\n  \`\`\`md\n  ## 9.7 safe example\n  \`\`\`\n`;
  const second = "# Part B\n";
  writeFileSync(join(work, PART_A), first);
  writeFileSync(join(work, PART_B), second);
  // The index deliberately lists only rendered headings. The fenced example
  // must stay inert while the matching closer makes the list fence safe.
  writeFileSync(join(work, DOC), splitIndex([SPLIT_BASE_DOC, second]));
  writeSurface(work, [DOC, PART_A, PART_B]);
  const out = runGate();
  if (out.status === 0) {
    console.log("ok   a closed list-continuation fence remains an ordinary safe example");
  } else {
    failed += 1;
    console.error(`FAIL a closed list-continuation fence must remain allowed\n  exit ${out.status}: ${`${out.stdout}${out.stderr}`.split("\\n").slice(0, 6).join("\\n  ")}`);
  }
}

for (const [name, continuation] of [
  ["a blank line", "\n\n"],
  ["ordinary indented continuation text", "\n  ordinary continuation text\n"],
]) {
  const first = `${SPLIT_BASE_DOC}\n- continuation context${continuation}  \`\`\`md\n  ## 9.7 example only\n\n## 9.7 live duplicate\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("list-continuation fenced code block is unsupported in split protocol part") &&
    text.includes(PART_A)
  ) {
    console.log(`ok   ${name} cannot leave a list-continuation fence masking a dedented live duplicate`);
  } else {
    failed += 1;
    console.error(`FAIL ${name} must not let a list-continuation fence mask a live duplicate\n  exit ${out.status}: ${text.split("\\n").slice(0, 6).join("\\n  ")}`);
  }
}

{
  const first = `${SPLIT_BASE_DOC}\n- continuation context\n  \`\`\`md\n  ## 9.7 example only\n\`\`\`\n## 9.7 live duplicate\n`;
  const out = runSplitGate(first, "# Part B\n");
  const text = `${out.stdout}${out.stderr}`;
  if (
    out.status !== 0 &&
    text.includes("list-continuation fenced code block is unsupported in split protocol part") &&
    text.includes(PART_A)
  ) {
    console.log("ok   a dedented matching rail cannot close a list-continuation fence");
  } else {
    failed += 1;
    console.error(`FAIL a dedented matching rail must refuse the list-continuation opener\n  exit ${out.status}: ${text.split("\\n").slice(0, 8).join("\\n  ")}`);
  }
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
const realPartsMarker = /<!-- protocol-reference-parts:\s*(.*?)\s*-->/.exec(realDoc);
if (!realPartsMarker) throw new Error("the real reference must declare its split parts");
const realPartPaths = realPartsMarker[1].split("|").map((part) => `docs/tally/${part.trim()}`);
writeFileSync(join(upstream, DOC), realDoc);
for (const part of realPartPaths) {
  writeFileSync(join(upstream, part), readFileSync(join(here, "..", part), "utf8"));
}
writeFileSync(join(upstream, SURFACE), readFileSync(join(here, "..", SURFACE), "utf8"));
git(upstream, "add", "-A");
git(upstream, "commit", "-qm", "the real reference");
// The clone still carries the last case's edits; discard temporary split parts
// before fetching the committed reference and its declared files.
git(work, "checkout", "-q", "--", DOC, SURFACE);
git(work, "clean", "-qfd");
git(work, "pull", "-q");
const real = runGate();
if (real.status !== 0) {
  failed += 1;
  console.error(`FAIL the committed reference does not pass its own gate: ${real.stderr}`);
} else {
  const totalLines = [realDoc, ...realPartPaths.map((part) => readFileSync(join(here, "..", part), "utf8"))]
    .reduce((count, document) => count + document.split("\n").length, 0);
  console.log(`ok   the committed reference passes (${totalLines} lines across index and parts)`);
}

rmSync(root, { recursive: true, force: true });
if (failed) {
  console.error(`\n${failed} failing contract(s)`);
  process.exit(1);
}
console.log("\nall section-gate contracts hold");
