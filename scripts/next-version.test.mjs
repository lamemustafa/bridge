// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";

import {
  VERSION_FILES, bump, classify, draftNotes, latestReleaseTag, levelFor, propose, pullNumbers, readVersions, writeVersions,
} from "./next-version.mjs";

const repository = resolve(import.meta.dirname, "..");
const pull = (number, labels, issueLabels = []) => ({ number, title: `change ${number}`, labels, issueLabels });

test("the highest kind among a pull request's labels wins", () => {
  assert.equal(classify(["type:bug", "breaking"]), "breaking");
  assert.equal(classify(["area:tally", "type:feature"]), "feature");
  assert.equal(classify(["type:rectify"]), "fix");
  assert.equal(classify(["dependencies"]), "maintenance");
  assert.equal(classify(["documentation"]), "docs");
  assert.equal(classify(["area:tally", "severity:p2"]), null);
});

test("before 1.0.0 a breaking change or a feature bumps the minor version", () => {
  assert.equal(levelFor(["breaking"], "0.3.0"), "minor");
  assert.equal(levelFor(["feature", "fix"], "0.3.0"), "minor");
  assert.equal(levelFor(["fix"], "0.3.0"), "patch");
  assert.equal(levelFor(["maintenance"], "0.3.0"), "patch");
  assert.equal(levelFor(["docs"], "0.3.0"), null);
});

test("from 1.0.0 a breaking change bumps the major version", () => {
  assert.equal(levelFor(["breaking"], "1.2.3"), "major");
  assert.equal(levelFor(["feature"], "1.2.3"), "minor");
  assert.equal(bump("1.2.3", "major"), "2.0.0");
  assert.equal(bump("0.3.0", "minor"), "0.4.0");
  assert.equal(bump("0.3.0", "patch"), "0.3.1");
  assert.throws(() => bump("0.3", "patch"), /not a plain/);
});

test("an unclassified pull request refuses the proposal unless a level is given", () => {
  const pulls = [pull(1, ["type:feature"]), pull(2, [])];
  const refused = propose({ current: "0.3.0", pulls });
  assert.equal(refused.ok, false);
  assert.deepEqual(refused.unclassified.map((p) => p.number), [2]);
  const overridden = propose({ current: "0.3.0", pulls, level: "patch" });
  assert.equal(overridden.ok, true);
  assert.equal(overridden.next, "0.3.1");
  assert.equal(overridden.proposedLevel, "minor");
  assert.equal(overridden.overridden, true);
});

test("a pull request with no label of its own takes the closed issue's", () => {
  const result = propose({ current: "0.3.0", pulls: [pull(3, [], ["area:tally", "type:bug"])] });
  assert.equal(result.ok, true);
  assert.equal(result.next, "0.3.1");
});

test("documentation alone proposes no release", () => {
  const result = propose({ current: "0.3.0", pulls: [pull(4, ["documentation"])] });
  assert.equal(result.ok, false);
  assert.match(result.reason, /nothing to release/);
});

test("the pull request number is the last one in a squash subject, and a direct push is kept", () => {
  const { numbers, missing } = pullNumbers([
    "Name a ledger stored with a trailing CR LF (#626) (#708)",
    "E2b: port high_value_register (#713)",
    "A direct push with no number",
    "",
  ]);
  assert.deepEqual(numbers, [708, 713]);
  assert.deepEqual(missing, ["A direct push with no number"]);
});

test("the latest release tag is chosen by version, not by name order", () => {
  assert.equal(latestReleaseTag(["v0.1.0", "mcp-preview-0.2.0", "mcp-preview-0.10.0", "mcp-preview-0.9.1", "other"]), "mcp-preview-0.10.0");
  assert.equal(latestReleaseTag(["other"]), null);
});

test("draft notes group titles by kind and mark themselves as a draft", () => {
  const notes = draftNotes(propose({ current: "0.3.0", pulls: [pull(5, ["type:feature"]), pull(6, ["type:bug"])] }).classified);
  assert.match(notes, /^<!-- Draft/);
  assert.ok(notes.indexOf("**New and changed**") < notes.indexOf("**Fixes**"));
  assert.match(notes, /- change 5 \(#5\)/);
});

test("writing a version changes exactly the five version files and the README sentence", () => {
  const base = mkdtempSync(join(tmpdir(), "next-version-"));
  try {
    for (const file of [...Object.keys(VERSION_FILES), "README.md"]) {
      mkdirSync(dirname(join(base, file)), { recursive: true });
      copyFileSync(join(repository, file), join(base, file));
    }
    const before = readVersions(base);
    assert.equal(new Set(Object.values(before)).size, 1, "the repository's own version files agree");
    const next = bump(Object.values(before)[0], "minor");
    writeVersions(next, base);
    assert.deepEqual(Object.values(readVersions(base)), Object.keys(VERSION_FILES).map(() => next));
    assert.match(readFileSync(join(base, "README.md"), "utf8"), new RegExp(`current development source is version \`${next.replaceAll(".", "\\.")}\``));
    const old = Object.values(before)[0];
    const original = readFileSync(join(repository, "src-tauri/Cargo.lock"), "utf8");
    const expected = original.replace(`name = "bridge"\nversion = "${old}"\n`, `name = "bridge"\nversion = "${next}"\n`);
    assert.notEqual(expected, original);
    assert.equal(readFileSync(join(base, "src-tauri/Cargo.lock"), "utf8"), expected, "only the bridge entry's version changed");
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("a file with two candidate version lines is refused, not half-written", () => {
  const base = mkdtempSync(join(tmpdir(), "next-version-"));
  try {
    for (const file of [...Object.keys(VERSION_FILES), "README.md"]) {
      mkdirSync(dirname(join(base, file)), { recursive: true });
      copyFileSync(join(repository, file), join(base, file));
    }
    const cargo = join(base, "src-tauri/Cargo.toml");
    writeFileSync(cargo, `${readFileSync(cargo, "utf8")}\nversion = "9.9.9"\n`);
    const packageBefore = readFileSync(join(base, "package.json"), "utf8");
    assert.throws(() => writeVersions("0.9.0", base), /Cargo\.toml: expected exactly one version line, found 2/);
    assert.equal(readFileSync(join(base, "package.json"), "utf8"), packageBefore, "no file was written");
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});
