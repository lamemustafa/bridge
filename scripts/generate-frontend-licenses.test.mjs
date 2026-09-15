// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  buildGroups,
  normalizeLicenseText,
  normalizeRepositoryUrl,
  renderInventory,
} from "./generate-frontend-licenses.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const reportPath = fileURLToPath(new URL("../THIRD_PARTY_LICENSES.txt", import.meta.url));

test("repository URL normalization matches every form package.json ships", () => {
  assert.equal(
    normalizeRepositoryUrl({ repository: { url: "git+https://github.com/tauri-apps/tauri.git" } }),
    "https://github.com/tauri-apps/tauri",
  );
  assert.equal(
    normalizeRepositoryUrl({ repository: { url: "https://github.com/lucide-icons/lucide.git", directory: "packages/lucide-react" } }),
    "https://github.com/lucide-icons/lucide",
  );
  assert.equal(
    normalizeRepositoryUrl({ repository: "git://github.com/owner/repo.git" }),
    "https://github.com/owner/repo",
  );
  assert.equal(
    normalizeRepositoryUrl({ repository: "git@github.com:owner/repo.git" }),
    "https://github.com/owner/repo",
  );
  assert.equal(
    normalizeRepositoryUrl({ repository: "github:owner/repo" }),
    "https://github.com/owner/repo",
  );
  assert.equal(
    normalizeRepositoryUrl({ repository: { url: "git+ssh://git@github.com/owner/repo.git" } }),
    "https://github.com/owner/repo",
  );
  assert.equal(
    normalizeRepositoryUrl({ homepage: "https://example.dev/pkg#readme" }),
    "https://example.dev/pkg",
  );
  assert.equal(normalizeRepositoryUrl({}), null);
});

test("license text normalization strips trailing whitespace and CRLF without touching internal blank lines", () => {
  const raw = "MIT License\r\n\r\nCopyright (c) X   \r\n\r\nSome body.  \n\ntrailing blank\n\n\n";
  assert.equal(
    normalizeLicenseText(raw),
    "MIT License\n\nCopyright (c) X\n\nSome body.\n\ntrailing blank",
  );
});

test("buildGroups merges byte-identical license text and keeps the shared SPDX expression", () => {
  const groups = buildGroups([
    { name: "react", version: "19.2.8", license: "MIT", source: "https://github.com/react/react", text: "SHARED" },
    { name: "scheduler", version: "0.27.0", license: "MIT", source: "https://github.com/facebook/react", text: "SHARED" },
    { name: "react-dom", version: "19.2.8", license: "MIT", source: "https://github.com/react/react", text: "SHARED" },
  ]);
  assert.equal(groups.length, 1);
  assert.deepEqual(groups[0].members.map((m) => m.name), ["react", "react-dom", "scheduler"]);
  assert.equal(groups[0].license, "MIT");
  // First member alphabetically wins when a merged group's sources disagree.
  assert.equal(groups[0].source, "https://github.com/react/react");
});

test("buildGroups orders distinct entries alphabetically by their first member's name", () => {
  const groups = buildGroups([
    { name: "zeta", version: "1.0.0", license: "MIT", source: "https://example.com/zeta", text: "Z" },
    { name: "@scope/alpha", version: "1.0.0", license: "MIT", source: "https://example.com/alpha", text: "A" },
  ]);
  assert.deepEqual(groups.map((g) => g.members[0].name), ["@scope/alpha", "zeta"]);
});

test("buildGroups refuses to merge identical text under disagreeing SPDX expressions", () => {
  assert.throws(
    () => buildGroups([
      { name: "a", version: "1.0.0", license: "MIT", source: "https://example.com/a", text: "SAME" },
      { name: "b", version: "1.0.0", license: "ISC", source: "https://example.com/b", text: "SAME" },
    ]),
    /disagree on their SPDX expression/,
  );
});

test("buildGroups requires a resolvable source", () => {
  assert.throws(
    () => buildGroups([{ name: "a", version: "1.0.0", license: "MIT", source: null, text: "TEXT" }]),
    /no repository or homepage/,
  );
});

test("renderInventory reproduces the header/underline/license-block format the checker expects", () => {
  const groups = buildGroups([
    { name: "only-pkg", version: "2.0.0", license: "0BSD", source: "https://example.com/only", text: "License body." },
  ]);
  const rendered = renderInventory(groups);
  assert.equal(
    rendered,
    "Bridge frontend third-party licenses\n" +
      "====================================\n\n" +
      "The native Rust dependency inventory and full license texts are provided in\n" +
      "THIRD_PARTY_LICENSES_RUST.txt. This file covers the production frontend\n" +
      "dependency graph from pnpm-lock.yaml.\n\n" +
      "only-pkg 2.0.0\n" +
      "--------------\n" +
      "License: 0BSD\n" +
      "Source: https://example.com/only\n\n" +
      "License body.\n",
  );
});

test("the committed file's `name version` tokens are exactly what the generator (re-run) produces", async (t) => {
  // Exercises the real generator end to end against the actual lockfile/node_modules,
  // then restores whatever THIRD_PARTY_LICENSES.txt looked like before the test ran --
  // this test intentionally writes the real file, so it must always leave it as found.
  const before = await readFile(reportPath, "utf8");
  t.after(async () => writeFile(reportPath, before, "utf8"));

  // `check-dependency-inventory.mjs --frontend` (and, by convention, this
  // generator) refuses to run unless npm_execpath is set, which only happens
  // when a pnpm script invokes it -- so drive both through `pnpm run`,
  // exactly as `pnpm run license:generate:frontend` / `pnpm run license:check`
  // do for a real user.
  const runScript = (script) => execFileSync("corepack", ["pnpm", "run", script], {
    cwd: root,
    encoding: "utf8",
  });

  runScript("license:generate:frontend");
  const first = await readFile(reportPath, "utf8");
  runScript("license:generate:frontend");
  const second = await readFile(reportPath, "utf8");
  assert.equal(first, second, "generator output must be deterministic across consecutive runs");

  runScript("license:check");
});
