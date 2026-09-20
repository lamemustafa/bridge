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
import { relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../", import.meta.url));
const canonicalReference = fileURLToPath(
  new URL("../docs/tally/TALLY_PROTOCOL_REFERENCE.md", import.meta.url),
);
const compatibilitySurface = fileURLToPath(
  new URL("../docs/tally/compatibility/compatibility-surface.json", import.meta.url),
);
const PARTS_MARKER = /^<!-- protocol-reference-parts:\s*(.*?)\s*-->$/m;
const LEGACY_ANCHOR_LINE = /^ {0,3}<a\s+id="([^"]+)"\s*><\/a>\s*$/;
const ATX_HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*)|[ \t]*)$/;
const FENCE = /^ {0,3}(`{3,}|~{3,})/;

// Bound both per-item diagnostics and inventories before parsing any
// input-derived path. Besides keeping CI evidence retrievable, the name limit
// prevents a valid-looking generated token from reaching readFileSync and
// leaking an unbounded native filesystem error.
const MAX_REPORTED_NUMBERS = 20;
const MAX_REPORTED_OCCURRENCES = 8;
const MAX_HEADING_CHARS = 160;
const MAX_DECLARED_PARTS = 64;
const MAX_PART_NAME_CHARS = 128;
const MAX_HTML_TAG_CHARS = 1024;
const MAX_HTML_TAG_LINES = 16;

function short(value) {
  const text = String(value);
  return text.length <= MAX_HEADING_CHARS ? text : `${text.slice(0, MAX_HEADING_CHARS)}…`;
}

function atxHeading(line) {
  const found = ATX_HEADING.exec(line);
  if (!found) return null;
  let title = (found[2] ?? "").trimEnd();
  // A closing ATX delimiter needs whitespace before it. Preserve literal
  // terminal hashes such as C# and A##, including before a real delimiter.
  title = title.replace(/[ \t]+#+[ \t]*$/, "").trimEnd();
  return { level: found[1].length, title };
}

// Split-route headings are deliberately admitted only at the top level. A
// heading nested in a quote or list has a GitHub fragment too, but this gate
// cannot safely derive its document structure without becoming a Markdown
// parser. Refuse it rather than silently omitting it from number and route
// checks. Container-scoped fences are refused too: their implicit Markdown
// closure rules would otherwise make a later bare heading disappear.
function stripContainerPrefix(line) {
  let remainder = line;
  let found = false;
  while (true) {
    const prefix = /^ {0,3}(?:> ?|(?:[-*+]|\d{1,9}[.)])[ \t]+)/.exec(remainder);
    if (!prefix) return found ? remainder : null;
    found = true;
    remainder = remainder.slice(prefix[0].length);
  }
}

function containerAtxHeading(line) {
  const remainder = stripContainerPrefix(line);
  return remainder === null ? null : atxHeading(remainder);
}

function fenceRail(line) {
  const direct = FENCE.exec(line);
  if (!direct) return null;
  const delimiterOffset = line.indexOf(direct[1], direct.index);
  return { delimiter: direct[1], after: line.slice(delimiterOffset + direct[1].length) };
}

function containerFence(line) {
  const remainder = stripContainerPrefix(line);
  if (remainder === null) return null;
  return FENCE.exec(remainder);
}

// The split checker admits an indented fence only when its matching closer
// arrives before the first nonblank dedent below the opener. That conservative
// structural rule covers list continuation without tracking Markdown list or
// lazy-paragraph state, and prevents an implicit container close from hiding a
// later top-level heading. Top-level fences remain governed by the existing
// fence state machine.
function leadingSpaces(line) {
  return /^ */.exec(line)[0].length;
}

function indentedFenceClosesBeforeDedent(lines, openerIndex, opening) {
  const openerIndent = leadingSpaces(lines[openerIndex]);
  for (let index = openerIndex + 1; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() && leadingSpaces(line) < openerIndent) return false;
    const rail = fenceRail(line);
    if (
      rail &&
      rail.delimiter[0] === opening.delimiter[0] &&
      rail.delimiter.length >= opening.delimiter.length &&
      rail.after.trim() === ""
    ) {
      return true;
    }
  }
  return false;
}

// The inventory is a top-level HTML-comment control, not a prose convention.
// A monolithic historical reference has none; a split reference has exactly
// one. Marker-looking examples are allowed only inside a fenced code block.
function partInventoryMarkers(indexText) {
  const markers = [];
  let fence = null;
  for (const [index, line] of indexText.split("\n").entries()) {
    const rail = fenceRail(line);
    if (fence !== null) {
      if (
        rail &&
        rail.delimiter[0] === fence[0] &&
        rail.delimiter.length >= fence.length &&
        rail.after.trim() === ""
      ) {
        fence = null;
      }
      continue;
    }
    if (rail) {
      if (rail.delimiter[0] !== "`" || !rail.after.includes("`")) fence = rail.delimiter;
      continue;
    }
    const marker = PARTS_MARKER.exec(line);
    if (marker) markers.push({ value: marker[1], line: index + 1 });
  }
  return markers;
}

// The canonical file stays at the historic path. It and the sibling parts it
// names share one number allocation; a pre-split base has no marker and is
// scanned as the single original document. Keeping this inventory in the
// canonical index makes the split explicit and prevents either the index or a
// new part from being silently outside the duplicate/renumber gate.
function referencePaths(indexText, origin) {
  const markers = partInventoryMarkers(indexText);
  if (!markers.length) return [canonicalReference];
  if (markers.length !== 1) {
    throw new Error(
      `multiple protocol-reference part inventories (${markers.length}) in ${short(origin)}`,
    );
  }
  const marker = markers[0];
  let declaredCount = 1;
  for (const character of marker.value) {
    if (character === "|") declaredCount += 1;
    if (declaredCount > MAX_DECLARED_PARTS) {
      throw new Error(
        `too many protocol-reference parts (${declaredCount}; maximum ${MAX_DECLARED_PARTS}) in ${short(origin)}`,
      );
    }
  }
  const parts = marker.value.split("|").map((part) => part.trim());
  if (!parts.length || parts.some((part) => !part) || new Set(parts).size !== parts.length) {
    throw new Error(`invalid protocol-reference part inventory in ${short(origin)}`);
  }
  return [canonicalReference, ...parts.map((part) => {
    if (
      part.length > MAX_PART_NAME_CHARS ||
      !/^TALLY_PROTOCOL_REFERENCE_[A-Z0-9_]+\.md$/.test(part)
    ) {
      throw new Error(
        `invalid protocol-reference part ${short(JSON.stringify(part))} in ${short(origin)}`,
      );
    }
    return resolve(canonicalReference, "..", part);
  })];
}

const relPath = (path) => relative(repository, path).split(sep).join("/");

function compatibilitySurfacePaths() {
  let text;
  try {
    text = readFileSync(compatibilitySurface, "utf8");
  } catch {
    throw new Error(`compatibility surface is unreadable: ${relPath(compatibilitySurface)}`);
  }

  let manifest;
  try {
    manifest = JSON.parse(text);
  } catch {
    throw new Error(`invalid compatibility surface JSON in ${relPath(compatibilitySurface)}`);
  }
  if (
    manifest === null ||
    typeof manifest !== "object" ||
    Array.isArray(manifest) ||
    manifest.schema_version !== 1 ||
    !Array.isArray(manifest.files) ||
    typeof manifest.manifest_sha256 !== "string" ||
    !/^[0-9a-f]{64}$/.test(manifest.manifest_sha256)
  ) {
    throw new Error(`invalid compatibility surface schema in ${relPath(compatibilitySurface)}`);
  }
  return new Set(manifest.files.map((file, index) => {
    if (
      file === null ||
      typeof file !== "object" ||
      Array.isArray(file) ||
      typeof file.path !== "string" ||
      file.path.length === 0 ||
      typeof file.sha256 !== "string" ||
      !/^[0-9a-f]{64}$/.test(file.sha256)
    ) {
      throw new Error(
        `invalid compatibility surface file row ${index + 1} in ${relPath(compatibilitySurface)}`,
      );
    }
    return file.path;
  }));
}

function showAt(ref, path) {
  return spawnSync("git", ["show", `${ref}:${relPath(path)}`], {
    cwd: repository,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
}

function referencesAt(ref) {
  const index = showAt(ref, canonicalReference);
  if (index.status !== 0) return null;
  const paths = referencePaths(index.stdout, `${short(ref)}:${relPath(canonicalReference)}`);
  return paths.map((path) => {
    const shown = path === canonicalReference ? index : showAt(ref, path);
    if (shown.status !== 0) {
      throw new Error(
        `protocol-reference part ${short(relPath(path))} declared by ${short(ref)} is unreadable`,
      );
    }
    return { path, text: shown.stdout };
  });
}

const canonicalText = readFileSync(canonicalReference, "utf8");
const references = referencePaths(canonicalText, relPath(canonicalReference)).map((path) => {
  try {
    return { path, text: readFileSync(path, "utf8") };
  } catch {
    throw new Error(
      `protocol-reference part ${short(relPath(path))} declared by ` +
        `${relPath(canonicalReference)} is unreadable`,
    );
  }
});

// The split index participates in section-number allocation, but its headings
// are navigation and never generated legacy routes. Before the split, the
// canonical document is both the number source and the route source.
function routeReferences(referenceSet, indexText) {
  if (!partInventoryMarkers(indexText).length) return referenceSet;
  return referenceSet.filter(({ path }) => path !== canonicalReference);
}

// `1.2`, `9.12a` and `12a.4` are all section numbers this document uses.
//
// Levels 2-6, not 2-4: Markdown allows six, a child of an existing `####`
// section naturally needs a fifth, and a pattern that stopped at four would let
// such a heading duplicate an existing number while this gate reported success.
// ATX headings may carry up to three leading spaces and still be headings;
// four or more make an indented code block. A `##` inside a fenced block is not
// a heading at all, and counting one there fails CI over an example.
const SECTION_NUMBER = /^((?:\d+[a-z]?)(?:\.\d+[a-z]?)*)(?=[\s.:—-]|$)/;
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

function isEscaped(text, index) {
  let slashes = 0;
  while (index > slashes && text[index - slashes - 1] === "\\") slashes += 1;
  return slashes % 2 === 1;
}

function closingBacktickRun(text, start, length) {
  for (let index = start; index < text.length;) {
    if (text[index] === "\n") return -1;
    if (text[index] !== "`") {
      index += 1;
      continue;
    }
    let run = 1;
    while (text[index + run] === "`") run += 1;
    if (run === length) return index;
    index += run;
  }
  return -1;
}

function textOutsidePairedCode(lines, visibleEntries) {
  const material = lines.map(() => "");
  for (const { line, index } of visibleEntries) material[index] = line;
  const text = material.join("\n");
  // String offsets below are UTF-16 code units, including before astral text.
  const outside = text.split("");
  for (let index = 0; index < text.length;) {
    if (text[index] !== "`") {
      index += 1;
      continue;
    }
    let run = 1;
    while (text[index + run] === "`") run += 1;
    if (isEscaped(text, index)) {
      index += run;
      continue;
    }
    const close = closingBacktickRun(text, index + run, run);
    if (close === -1) {
      index += run;
      continue;
    }
    for (let hidden = index; hidden < close + run; hidden += 1) {
      if (outside[hidden] !== "\n") outside[hidden] = " ";
    }
    index = close + run;
  }
  return outside.join("").split("\n");
}

function closingHtmlTag(text) {
  let quote = null;
  for (let index = 0; index < text.length; index += 1) {
    if (quote !== null) {
      if (text[index] === quote) quote = null;
      continue;
    }
    if (text[index] === '"' || text[index] === "'") quote = text[index];
    else if (text[index] === ">") return index;
  }
  return -1;
}

function hasUnsupportedHtml(lines, index) {
  const unquote = (line) => line.replace(/^ {0,3}(?:> ?)+/, "");
  const line = unquote(lines[index]);
  if (/^ {0,3}</.test(line)) return true;
  for (const opener of line.matchAll(/<[A-Za-z!/]/g)) {
    let tag = line.slice(opener.index);
    for (let offset = 1; closingHtmlTag(tag) === -1 && offset < MAX_HTML_TAG_LINES; offset += 1) {
      const continuation = lines[index + offset] === undefined
        ? undefined
        : unquote(lines[index + offset]);
      if (continuation === undefined || !continuation.trim()) break;
      tag += `\n${continuation}`;
      if (tag.length > MAX_HTML_TAG_CHARS) return true;
    }
    const close = closingHtmlTag(tag);
    if (close === -1) return true;
    if (/\bid\s*=/i.test(tag.slice(0, close))) return true;
  }
  return false;
}

// Keep one visibility state machine for number scanning, route-heading
// validation, and canonical redirect extraction. It excludes fenced blocks and
// HTML comments before any consumer can mistake examples for rendered content.
// Mixed comment/content lines are outside the admitted protocol grammar and
// fail closed rather than asking this gate to become a general Markdown parser.
function* visibleMarkdownLines(lines, origin, htmlMode = "allow") {
  const outsideCode = textOutsidePairedCode(
    lines,
    lines.map((line, index) => ({ line, index })),
  );
  const visibleEntries = [];
  let fence = null;
  let comment = false;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const rail = fenceRail(line);
    if (fence !== null) {
      if (!rail) continue;
      if (
        rail.delimiter[0] === fence[0] &&
        rail.delimiter.length >= fence.length &&
        rail.after.trim() === ""
      ) {
        fence = null;
      }
      continue;
    }
    if (comment) {
      const end = line.indexOf("-->");
      if (end !== -1) {
        if (line.slice(end + 3).trim()) {
          throw new Error(`unsupported content after an HTML comment in ${short(origin)}:${index + 1}`);
        }
        comment = false;
      }
      continue;
    }
    const opensFence = rail && (rail.delimiter[0] !== "`" || !rail.after.includes("`"));
    if (
      htmlMode !== "allow" &&
      opensFence &&
      leadingSpaces(line) > 0 &&
      !indentedFenceClosesBeforeDedent(lines, index, rail)
    ) {
      throw new Error(
        `indented fenced code block must close before a visible dedent in split protocol ${htmlMode}: ` +
          `${short(origin)}:${index + 1}`,
      );
    }
    if (rail) {
      // A backtick fence's info string may not contain a backtick. Such a line
      // is neither an opener nor a route/heading source.
      if (opensFence) fence = rail.delimiter;
      continue;
    }
    // A same-line paired code span is visible text, but its literal comment
    // delimiters do not begin an HTML comment. Use the offset-preserving mask
    // only to locate an opener; once one is real, HTML owns its raw closing
    // delimiter and the existing mixed-content refusal still applies.
    const start = outsideCode[index].indexOf("<!--");
    if (start !== -1) {
      if (line.slice(0, start).trim()) {
        throw new Error(`unsupported content before an HTML comment in ${short(origin)}:${index + 1}`);
      }
      const end = line.indexOf("-->", start + 4);
      if (end === -1) comment = true;
      else if (line.slice(end + 3).trim()) {
        throw new Error(`unsupported content after an HTML comment in ${short(origin)}:${index + 1}`);
      }
      continue;
    }
    visibleEntries.push({ line, index });
  }
  if (comment) {
    throw new Error(`unclosed HTML comment in ${short(origin)}`);
  }
  if (htmlMode !== "allow") {
    const outsideCode = textOutsidePairedCode(lines, visibleEntries);
    for (const { line, index } of visibleEntries) {
      const admittedAnchor = htmlMode === "index" && LEGACY_ANCHOR_LINE.test(line);
      if (!admittedAnchor && hasUnsupportedHtml(outsideCode, index)) {
        throw new Error(
          `unsupported raw HTML block in split protocol ${htmlMode}: ${short(origin)}:${index + 1}`,
        );
      }
    }
  }
  yield* visibleEntries;
}

function setextHeadingAt(lines, index) {
  if (
    index === 0 ||
    !SETEXT_UNDERLINE.test(lines[index]) ||
    !lines[index - 1].trim()
  ) return null;
  let first = index - 1;
  while (
    first > 0 &&
    lines[first - 1].trim() &&
    !BLOCK_BOUNDARY.test(lines[first - 1]) &&
    !NOT_A_PARAGRAPH.test(lines[first - 1])
  ) {
    first -= 1;
  }
  const text = lines[first];
  if (!text.trim() || BLOCK_BOUNDARY.test(text) || NOT_A_PARAGRAPH.test(text)) return null;
  return { line: first + 1, text };
}

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
          "clicks OK — P0 operationally",
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
  const heading = atxHeading(line);
  const title = visibleHeadingTitle(heading ? heading.title : line.trim());
  return title.replace(SECTION_NUMBER, "").trim();
}

// Scanning one document's lines into number -> occurrences. A function rather
// than a loop, because the base revision has to be scanned the same way.
function scan(lines, file, occurrences = new Map()) {
  const record = (number, line, text) => {
    if (!occurrences.has(number)) occurrences.set(number, []);
    occurrences.get(number).push({ file, line, text: text.trim() });
  };

  const visible = [...visibleMarkdownLines(lines, file)];
  const visibleLines = lines.map(() => "");
  for (const { line, index } of visible) visibleLines[index] = line;
  for (const { line, index } of visible) {
    // Setext: this underlines the paragraph above it, and such a heading may
    // span several lines — the number is on the *first* of them, not on the
    // line immediately above the underline.
    const setext = setextHeadingAt(visibleLines, index);
    if (setext) {
      // Matched against the raw line, not a trimmed copy: SETEXT_NUMBER's own
      // {0,3} indentation limit is what rejects a four-space-indented code
      // line beginning with a number, and trimming first threw that away.
      const numbered = SETEXT_NUMBER.exec(setext.text);
      if (numbered) record(numbered[1], setext.line, setext.text);
      continue;
    }

    const heading = atxHeading(line);
    if (!heading || heading.level === 1 || !heading.title) continue;
    const numbered = SECTION_NUMBER.exec(visibleHeadingTitle(heading.title));
    if (numbered) record(numbered[1], index + 1, line);
  }
  return occurrences;
}

// Rule 2 of the register — a merged section is never renumbered, because other
// documents and code cite these numbers. Uniqueness alone cannot see that:
// renumbering a unique heading leaves it unique. Only the base knows which
// numbers were already allocated.
function baseNumbers() {
  const candidates = [
    process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : null,
    "origin/master",
    "master",
  ].filter(Boolean);
  for (const ref of candidates) {
    const baseReferences = referencesAt(ref);
    if (!baseReferences) continue;
    const numbers = new Map();
    for (const { path, text } of baseReferences) {
      scan(text.split("\n"), relPath(path), numbers);
    }
    return { ref, numbers, references: baseReferences, indexText: showAt(ref, canonicalReference).stdout };
  }
  return null;
}

const occurrences = new Map();
let totalLines = 0;
for (const { path, text } of references) {
  totalLines += text.split("\n").length;
  scan(text.split("\n"), relPath(path), occurrences);
}

if (!occurrences.size) {
  const shown = references.slice(0, MAX_REPORTED_NUMBERS).map(({ path }) => short(relPath(path)));
  throw new Error(
    `no numbered headings found in ${shown.join(", ")}` +
      (references.length > shown.length ? `, … and ${references.length - shown.length} more` : "") +
      " — " +
      "the heading pattern no longer matches, so this gate is checking nothing",
  );
}

function describe(number, found) {
  const shown = found.slice(0, MAX_REPORTED_OCCURRENCES);
  const lines = shown.map(
    (one) => `    ${one.file}:${one.line}: ${short(one.text)}`,
  );
  if (found.length > shown.length) {
    lines.push(`    ... and ${found.length - shown.length} more`);
  }
  return `section ${short(number)} is used ${found.length} times:\n${lines.join("\n")}`;
}

const failures = [];
const surfacePaths = compatibilitySurfacePaths();
const unpinnedReferences = references
  .map(({ path }) => relPath(path))
  .filter((path) => !surfacePaths.has(path));
if (unpinnedReferences.length) {
  failures.push(
    `protocol-reference file(s) absent from the compatibility surface:\n` +
      unpinnedReferences
        .slice(0, MAX_REPORTED_NUMBERS)
        .map((path) => `    ${short(path)}`)
        .join("\n") +
      (unpinnedReferences.length > MAX_REPORTED_NUMBERS
        ? `\n    ... and ${unpinnedReferences.length - MAX_REPORTED_NUMBERS} more`
        : ""),
  );
}
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
  // there, so nothing fires. A swap is *not* caught — both numbers survive an
  // exchange, so presence sees nothing; see the block below for why the gate no
  // longer tries.
  const missing = [...base.numbers.keys()].filter((number) => !occurrences.has(number));
  if (missing.length) {
    failures.push(
      `section number(s) present on ${short(base.ref)} and absent here. A merged ` +
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

  // **A pure swap is not detected, and this gate no longer tries.**
  //
  // Exchange two numbers and both are still present, so the check above sees
  // nothing. Catching it needs a section to be recognisable apart from its
  // number, and the only candidate is its title — which is mutable, and that
  // is fatal rather than merely awkward: a title moving from one number to
  // another is *exactly* what a swap and a legitimate **retitle chain** both
  // look like. Rename `10 Alpha` to `10 Beta` and `20 Beta` to `20 Gamma`, and
  // `Beta` has vacated 20 and occupied 10 without anything moving.
  //
  // Three attempts, three false positives on legitimate edits, each found by
  // review rather than by the attempt before it:
  //
  //   1. "is this title still at its number" — fired on any retitle;
  //   2. the same, restricted to titles unique on both sides — blind to a swap
  //      of two sections whose titles each appear twice, with no other check
  //      behind it;
  //   3. comparing each title's *set* of numbers, requiring one vacated — fires
  //      on the retitle chain above.
  //
  // The information is not there. A gate that blocks legitimate documentation
  // edits gets bypassed or switched off, which costs more than the gap it was
  // closing, so this is a **documented residual** rather than a fourth attempt:
  // `SECTION-REGISTER.md` names it alongside the other two things no file in
  // the repository can see. Rule 2 there tells authors not to exchange numbers;
  // nothing enforces it.
} else {
  // Say so rather than passing quietly: a check that cannot run is not a check
  // that passed.
  console.warn(
    "note: no base revision reachable (shallow clone?), so the never-renumber " +
      "rule was NOT checked — only uniqueness was.",
  );
}

// The canonical index retains every legacy GitHub fragment after the old
// single document is split. Each visible redirect must name the source heading
// and point to its part-local GitHub fragment. Fences are excluded: a literal
// `##` in an example is never a heading or a redirect destination.
function visibleHeadingTitle(title) {
  // This is deliberately a bounded admitted grammar, not a general Markdown
  // renderer. The 94 current route headings use plain ASCII plus `§`/`—`,
  // balanced strong spans, and single-backtick code spans. Reject every other
  // rendering construct before deriving a fragment that could disagree with
  // GitHub's visible heading text.
  if (title.includes("``")) {
    throw new Error(`unsupported split protocol heading markup (multi-backtick code span): ${short(title)}`);
  }
  let visible = "";
  let strong = false;
  let code = false;
  let codeText = "";
  for (let index = 0; index < title.length; index += 1) {
    const character = title[index];
    if (character.codePointAt(0) > 0x7f && character !== "§" && character !== "—") {
      throw new Error(
        `unsupported Unicode character in split protocol heading (only § and — are admitted): ${short(title)}`,
      );
    }
    if (code) {
      if (character === "`") {
        if (!codeText || /^\s|\s$/.test(codeText)) {
          throw new Error(
            `unsupported split protocol heading markup (empty or padded code span): ${short(title)}`,
          );
        }
        visible += codeText;
        codeText = "";
        code = false;
      } else {
        codeText += character;
      }
      continue;
    }
    if (title.startsWith("**", index)) {
      strong = !strong;
      index += 1;
      continue;
    }
    if (character === "`") {
      code = true;
      continue;
    }
    if (
      character === "*" ||
      character === "_" ||
      character === "[" ||
      character === "]" ||
      character === "<" ||
      character === ">" ||
      character === "\\" ||
      character === "~" ||
      (character === "&" && /^&(?:#[0-9]+|#x[0-9a-f]+|[a-z][a-z0-9]+);/i.test(title.slice(index)))
    ) {
      throw new Error(`unsupported split protocol heading markup: ${short(title)}`);
    }
    visible += character;
  }
  if (strong || code) {
    throw new Error(`unsupported split protocol heading markup (unbalanced delimiter): ${short(title)}`);
  }
  return visible;
}

function anchorSeed(title) {
  return visibleHeadingTitle(title)
    .toLowerCase()
    .replace(/[^a-z0-9 _-]/g, "")
    .replace(/ /g, "-");
}

// Locate the link label's closing bracket with the same admitted single-
// backtick state as headings. A `]` inside code is visible label text, not the
// end of the Markdown link.
function parseLegacyLinkLine(line) {
  const opener = /^ {0,3}\[/.exec(line);
  if (!opener) return null;
  const start = opener[0].length;
  let code = false;
  let close = -1;
  for (let index = start; index < line.length; index += 1) {
    if (line[index] === "`") code = !code;
    else if (line[index] === "]" && !code) {
      close = index;
      break;
    }
  }
  if (close === -1) return null;
  const destination = /^\(\.\/([^#)]+)#([^)]+)\)\s*$/.exec(line.slice(close + 1));
  if (!destination) return null;
  const title = line.slice(start, close);
  visibleHeadingTitle(title);
  return { title, path: destination[1], target: destination[2] };
}

function headingRoutes(referenceSet) {
  const globalSeeds = new Set();
  const routes = [];
  for (const { path, text } of referenceSet) {
    const lines = text.split("\n");
    const origin = relPath(path);
    const visible = [...visibleMarkdownLines(lines, origin, "part")];
    const visibleLines = lines.map(() => "");
    for (const { line, index } of visible) visibleLines[index] = line;
    const localSeeds = new Set();
    for (const { line, index } of visible) {
      const setext = setextHeadingAt(visibleLines, index);
      if (setext) {
        throw new Error(
          `Setext headings are unsupported in split protocol routes: ${short(origin)}:${setext.line}`,
        );
      }
      if (containerFence(line)) {
        throw new Error(
          `container-prefixed fenced code block is unsupported in split protocol routes: ` +
            `${short(origin)}:${index + 1}`,
        );
      }
      const container = containerAtxHeading(line);
      if (container) {
        throw new Error(
          `container-prefixed ATX heading is unsupported in split protocol routes: ` +
            `${short(origin)}:${index + 1}`,
        );
      }
      const found = atxHeading(line);
      if (!found || !found.title) continue;
      const title = found.title;
      const seed = anchorSeed(title);
      if (localSeeds.has(seed)) {
        throw new Error(`ambiguous duplicate file-local heading fragment ${short(seed)} in ${short(origin)}`);
      }
      localSeeds.add(seed);
      // H1 participates in the file-local GitHub fragment namespace but was
      // never a section in the monolithic reference, so it reserves only the
      // part-local target ID and does not generate a legacy redirect.
      if (found.level === 1) continue;
      if (globalSeeds.has(seed)) {
        throw new Error(`ambiguous duplicate split-route heading fragment ${short(seed)}`);
      }
      routes.push({
        legacy: seed,
        target: seed,
        title,
        path: relPath(path),
      });
      globalSeeds.add(seed);
    }
  }
  return routes;
}

function indexedContents(indexText, origin = relPath(canonicalReference)) {
  const lines = indexText.split("\n");
  const visibleEntries = [...visibleMarkdownLines(lines, origin, "index")];
  const visible = new Map(visibleEntries.map(({ line, index }) => [index, line]));

  // Every visible canonical heading also creates a GitHub fragment, including
  // H1 navigation headings. Validate those titles with the same admitted
  // grammar as part headings and reserve their generated IDs before accepting
  // explicit legacy redirects. Otherwise a new `## Method note` can capture
  // `#method-note` before the later `<a id="method-note">` is reached.
  const visibleLines = lines.map((_, index) => visible.get(index) ?? "");
  const headingSeeds = [];
  const seenHeadingSeeds = new Set();
  for (const [index, line] of visible) {
    const setext = setextHeadingAt(visibleLines, index);
    if (setext) {
      throw new Error(
        `Setext headings are unsupported in split protocol index: ` +
          `${short(origin)}:${setext.line}`,
      );
    }
    if (containerFence(line)) {
      throw new Error(
        `container-prefixed fenced code block is unsupported in split protocol index: ` +
          `${short(origin)}:${index + 1}`,
      );
    }
    if (containerAtxHeading(line)) {
      throw new Error(
        `container-prefixed ATX heading is unsupported in split protocol index: ` +
          `${short(origin)}:${index + 1}`,
      );
    }
    const heading = atxHeading(line);
    if (!heading || !heading.title) continue;
    const seed = anchorSeed(heading.title);
    if (seenHeadingSeeds.has(seed)) {
      throw new Error(`ambiguous duplicate canonical-index heading fragment ${short(seed)}`);
    }
    headingSeeds.push({ seed, line: index + 1 });
    seenHeadingSeeds.add(seed);
  }

  const routes = [];
  const anchorOccurrences = [];
  const incompleteAnchors = [];
  for (const [index, line] of visible) {
    const anchor = LEGACY_ANCHOR_LINE.exec(line);
    if (anchor) {
      anchorOccurrences.push({ id: anchor[1], line: index + 1 });
      const blank = visible.get(index + 1);
      const link = parseLegacyLinkLine(visible.get(index + 2) ?? "");
      if (blank?.trim() === "" && link) {
        routes.push({
          legacy: anchor[1],
          title: link.title,
          path: `docs/tally/${link.path}`,
          target: link.target,
        });
      } else {
        incompleteAnchors.push({ id: anchor[1], line: index + 1 });
      }
      continue;
    }
  }
  return { routes, headingSeeds, anchorOccurrences, incompleteAnchors };
}

function legacyIndexFailures(contents, origin) {
  const messages = [];
  const counts = new Map();
  for (const { id } of contents.anchorOccurrences) {
    counts.set(id, (counts.get(id) ?? 0) + 1);
  }
  const duplicates = [...counts].filter(([, count]) => count > 1);
  if (duplicates.length) {
    const duplicateOccurrences = duplicates.reduce((total, [, count]) => total + count - 1, 0);
    const shown = duplicates.slice(0, MAX_REPORTED_NUMBERS);
    messages.push(
      `${duplicateOccurrences} duplicate legacy-anchor occurrence(s) in ${short(origin)}:\n` +
        shown.map(([anchor, count]) => `    ${short(anchor)} (${count} occurrences)`).join("\n") +
        (duplicates.length > shown.length
          ? `\n    ... and ${duplicates.length - shown.length} more duplicated legacy anchor(s)`
          : ""),
    );
  }
  if (contents.incompleteAnchors.length) {
    const shown = contents.incompleteAnchors.slice(0, MAX_REPORTED_NUMBERS);
    messages.push(
      `${contents.incompleteAnchors.length} incomplete legacy anchor block(s) in ${short(origin)}; ` +
        "each anchor must be followed by one blank line and one standalone redirect link:\n" +
        shown.map(({ id, line }) => `    ${short(id)} (line ${line})`).join("\n") +
        (contents.incompleteAnchors.length > shown.length
          ? `\n    ... and ${contents.incompleteAnchors.length - shown.length} more`
          : ""),
    );
  }
  return messages;
}

if (partInventoryMarkers(canonicalText).length) {
  const indexed = new Map();
  const currentIndex = indexedContents(canonicalText);
  failures.push(...legacyIndexFailures(currentIndex, relPath(canonicalReference)));
  for (const route of currentIndex.routes) {
    indexed.set(route.legacy, route);
  }
  const capturedByHeadings = currentIndex.headingSeeds.filter(({ seed }) => indexed.has(seed));
  if (capturedByHeadings.length) {
    failures.push(
      `canonical index heading fragment(s) collide with required legacy anchors:\n` +
        capturedByHeadings
          .slice(0, MAX_REPORTED_NUMBERS)
          .map(({ seed, line }) => `    ${short(seed)} (line ${line})`)
          .join("\n") +
        (capturedByHeadings.length > MAX_REPORTED_NUMBERS
          ? `\n    ... and ${capturedByHeadings.length - MAX_REPORTED_NUMBERS} more`
          : ""),
    );
  }
  const currentRoutes = headingRoutes(routeReferences(references, canonicalText));
  const required = new Map(currentRoutes.map((route) => [route.legacy, route]));
  const aliases = [];
  const newlyCreatedAliases = [];
  const inheritedAliases = [];
  if (base) {
    const baseRoutes = headingRoutes(routeReferences(base.references, base.indexText));
    const baseRequired = new Map(baseRoutes.map((route) => [route.legacy, route]));
    const baseOrigin = `${short(base.ref)}:${relPath(canonicalReference)}`;
    const baseIndex = indexedContents(base.indexText, baseOrigin);
    failures.push(...legacyIndexFailures(baseIndex, baseOrigin));
    for (const route of baseIndex.routes) {
      const normal = baseRequired.get(route.legacy);
      if (
        !normal ||
        normal.title !== route.title ||
        normal.path !== route.path ||
        normal.target !== route.target
      ) {
        inheritedAliases.push(route);
      }
    }

    const reusedAliases = inheritedAliases.filter((route) => required.has(route.legacy));
    if (reusedAliases.length) {
      failures.push(
        `inherited legacy anchor(s) reused by current headings; these fragments already ` +
          `identify earlier headings:\n` +
          reusedAliases
            .slice(0, MAX_REPORTED_NUMBERS)
            .map((route) => `    ${short(route.legacy)}`)
            .join("\n") +
          (reusedAliases.length > MAX_REPORTED_NUMBERS
            ? `\n    ... and ${reusedAliases.length - MAX_REPORTED_NUMBERS} more`
            : ""),
      );
    }

    for (const route of baseRoutes) {
      if (!required.has(route.legacy)) {
        aliases.push(route);
        newlyCreatedAliases.push(route);
      }
    }
    // A prior split base may already contain aliases for headings retitled
    // before this change. Keep those historic fragments alive too, reserving
    // their IDs before a new current heading can claim them.
    for (const route of inheritedAliases) {
      if (!aliases.some((alias) => alias.legacy === route.legacy)) {
        aliases.push(route);
      }
    }
  }
  const missing = [...required.keys(), ...aliases.map((route) => route.legacy)].filter((anchor) => !indexed.has(anchor));
  if (missing.length) {
    failures.push(`legacy section anchor(s) missing from ${relPath(canonicalReference)}:\n` + missing.slice(0, MAX_REPORTED_NUMBERS).map((anchor) => `    ${short(anchor)}`).join("\n"));
  }
  const misrouted = currentRoutes.filter((route) => {
    const actual = indexed.get(route.legacy);
    return actual && (actual.title !== route.title || actual.path !== route.path || actual.target !== route.target);
  });
  const destinations = new Set(currentRoutes.map((route) => `${route.path}#${route.target}`));
  const sectionNumberForRoute = (route) => SECTION_NUMBER.exec(visibleHeadingTitle(route.title))?.[1] ?? null;
  const newlyCreatedRetargeted = newlyCreatedAliases.filter((route) => {
    const actual = indexed.get(route.legacy);
    if (!actual || !destinations.has(`${actual.path}#${actual.target}`)) return false;
    const number = sectionNumberForRoute(route);
    if (!number) return true;
    const destinationsForNumber = currentRoutes.filter(
      (current) => sectionNumberForRoute(current) === number,
    );
    if (destinationsForNumber.length !== 1) return true;
    return actual.path !== destinationsForNumber[0].path || actual.target !== destinationsForNumber[0].target;
  });
  if (newlyCreatedRetargeted.length) {
    failures.push(
      "newly-created legacy anchor(s) do not resolve to their original numbered section; " +
        "unnumbered route sources require an explicit reviewed mapping:\n" +
        newlyCreatedRetargeted
          .slice(0, MAX_REPORTED_NUMBERS)
          .map((route) => `    ${short(route.legacy)}`)
          .join("\n") +
        (newlyCreatedRetargeted.length > MAX_REPORTED_NUMBERS
          ? `\n    ... and ${newlyCreatedRetargeted.length - MAX_REPORTED_NUMBERS} more`
          : ""),
    );
  }
  // When the prior destination was retitled again, its current canonical
  // redirect is the next link. Resolve until reaching a live part heading.
  const resolveInheritedDestination = (route) => {
    let next = route;
    const seen = new Set();
    while (true) {
      const destination = `${next.path}#${next.target}`;
      if (destinations.has(destination)) return destination;
      if (seen.has(next.target)) return null;
      seen.add(next.target);
      next = indexed.get(next.target);
      if (!next) return null;
    }
  };
  const retargetedInheritedAliases = inheritedAliases.filter((route) => {
    const expectedDestination = resolveInheritedDestination(route);
    const actual = indexed.get(route.legacy);
    const actualDestination = actual && `${actual.path}#${actual.target}`;
    return (
      !required.has(route.legacy) &&
      expectedDestination &&
      actualDestination &&
      destinations.has(actualDestination) &&
      actualDestination !== expectedDestination
    );
  });
  if (retargetedInheritedAliases.length) {
    failures.push(
      `inherited legacy anchor(s) changed their still-live destination or retitle chain:\n` +
        retargetedInheritedAliases
          .slice(0, MAX_REPORTED_NUMBERS)
          .map((route) => `    ${short(route.legacy)}`)
          .join("\n") +
        (retargetedInheritedAliases.length > MAX_REPORTED_NUMBERS
          ? `\n    ... and ${retargetedInheritedAliases.length - MAX_REPORTED_NUMBERS} more`
          : ""),
    );
  }
  const brokenAliases = aliases.filter((route) => {
    const actual = indexed.get(route.legacy);
    return actual && !destinations.has(`${actual.path}#${actual.target}`);
  });
  if (misrouted.length || brokenAliases.length) {
    const bad = [...misrouted, ...brokenAliases];
    failures.push(`legacy section redirect(s) do not resolve to a current moved heading:\n` + bad.slice(0, MAX_REPORTED_NUMBERS).map((route) => `    ${short(route.legacy)}`).join("\n"));
  }

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
  `Protocol section numbers are unique across ${references.length} file(s) ` +
    `(${occurrences.size} numbers, ${totalLines} lines; known duplicates excused: ${excused}).`,
);
