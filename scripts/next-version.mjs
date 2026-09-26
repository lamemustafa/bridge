#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// Proposes the next release version from the labels of the pull requests
// merged since the last release, and, with --apply, writes it to every file
// that must carry it. See "Choosing the version" in docs/release-process.md.
//
//   node scripts/next-version.mjs                 propose only (read-only)
//   node scripts/next-version.mjs --since TAG     compare against TAG instead
//   node scripts/next-version.mjs --to REF        compare up to REF (default HEAD)
//   node scripts/next-version.mjs --level minor   override the proposed level
//   node scripts/next-version.mjs --apply         write the proposed version
//
// A pull request is classified by its own labels, else by the labels of the
// issues it closes. Any pull request left unclassified makes the proposal
// refuse, naming each one, unless --level is given: a guessed level would be
// silent, and the level is the one judgement this script cannot make.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(import.meta.dirname, "..");

// Highest kind wins. Labels are the repository's existing ones, plus
// `breaking`, which a maintainer adds to a pull request that removes or
// changes behaviour an existing user relies on.
export const KINDS = [
  ["breaking", ["breaking"]],
  ["feature", ["type:feature", "enhancement"]],
  ["fix", ["type:bug", "type:rectify", "bug"]],
  ["maintenance", ["type:chore", "dependencies", "github_actions", "infra"]],
  ["docs", ["documentation"]],
];

export function classify(labels) {
  for (const [kind, names] of KINDS) {
    if (labels.some((label) => names.includes(label))) return kind;
  }
  return null;
}

// SemVer 2.0.0 defines MAJOR/MINOR/PATCH only for 1.0.0 and later; under
// 0.y.z "anything MAY change at any time" (rule 4), and its FAQ suggests
// incrementing the minor version for each release. This project's own
// convention, within that freedom: before 1.0.0 a breaking change or a
// feature bumps the minor version and a fix-only release the patch; from
// 1.0.0 a breaking change bumps the major version, as rule 8 requires.
export function levelFor(kinds, current) {
  const has = (kind) => kinds.includes(kind);
  const [major] = parse(current);
  if (has("breaking")) return major === 0 ? "minor" : "major";
  if (has("feature")) return "minor";
  if (has("fix") || has("maintenance")) return "patch";
  return null;
}

export function parse(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error(`not a plain MAJOR.MINOR.PATCH version: ${version}`);
  return match.slice(1).map(Number);
}

export function bump(version, level) {
  const [major, minor, patch] = parse(version);
  if (level === "major") return `${major + 1}.0.0`;
  if (level === "minor") return `${major}.${minor + 1}.0`;
  if (level === "patch") return `${major}.${minor}.${patch + 1}`;
  throw new Error(`unknown level: ${level}`);
}

// Every file that must carry the release version. The preview admission check
// reads the first two; the bundle smoke check compares the server's
// CARGO_PKG_VERSION (Cargo.toml) with the manifest; check-license-metadata
// requires all five to agree.
export const VERSION_FILES = {
  "package.json": {
    read: (text) => JSON.parse(text).version,
    pattern: /^(  "version": ")([^"]+)(",)$/m,
  },
  "packaging/mcpb/manifest.json": {
    read: (text) => JSON.parse(text).version,
    pattern: /^(  "version": ")([^"]+)(",)$/m,
  },
  "src-tauri/Cargo.toml": {
    read: (text) => /^version = "([^"]+)"$/m.exec(text)?.[1],
    pattern: /^(version = ")([^"]+)(")$/m,
  },
  "src-tauri/tauri.conf.json": {
    read: (text) => JSON.parse(text).version,
    pattern: /^(  "version": ")([^"]+)(",)$/m,
  },
  "src-tauri/Cargo.lock": {
    read: (text) => /\[\[package\]\]\nname = "bridge"\nversion = "([^"]+)"\n/.exec(text)?.[1],
    pattern: /(\[\[package\]\]\nname = "bridge"\nversion = ")([^"]+)("\n)/,
  },
};

export function readVersions(base = root) {
  return Object.fromEntries(
    Object.entries(VERSION_FILES).map(([file, spec]) => [file, spec.read(readFileSync(resolve(base, file), "utf8")) ?? null]),
  );
}

export function writeVersions(next, base = root) {
  // Check every file before writing any, so a refusal leaves none half-bumped.
  const updates = Object.entries(VERSION_FILES).map(([file, spec]) => {
    const path = resolve(base, file);
    const text = readFileSync(path, "utf8");
    const count = (text.match(new RegExp(spec.pattern.source, `${spec.pattern.flags}g`)) ?? []).length;
    if (count !== 1) throw new Error(`${file}: expected exactly one version line, found ${count}`);
    return [path, text.replace(spec.pattern, (_, before, _old, after) => `${before}${next}${after}`)];
  });
  const readme = resolve(base, "README.md");
  const readmeText = readFileSync(readme, "utf8");
  const sentence = /(current development source is version `)([^`]+)(`)/;
  const sentences = (readmeText.match(new RegExp(sentence.source, "g")) ?? []).length;
  if (sentences !== 1) throw new Error(`README.md: expected exactly one current-version sentence, found ${sentences}`);
  updates.push([readme, readmeText.replace(sentence, (_, a, _old, b) => `${a}${next}${b}`)]);
  for (const [path, text] of updates) writeFileSync(path, text);
}

export function propose({ current, pulls, level }) {
  const classified = pulls.map((pull) => ({
    ...pull,
    kind: classify(pull.labels) ?? classify(pull.issueLabels ?? []),
  }));
  const unclassified = classified.filter((pull) => !pull.kind);
  const kinds = [...new Set(classified.map((pull) => pull.kind).filter(Boolean))];
  const proposedLevel = levelFor(kinds, current);
  if (!level && unclassified.length) {
    return { ok: false, current, classified, unclassified, reason: "unclassified pull requests; label them or pass --level" };
  }
  const chosen = level ?? proposedLevel;
  if (!chosen) return { ok: false, current, classified, unclassified, reason: "nothing to release: documentation only, or no merged pull requests" };
  return { ok: true, current, next: bump(current, chosen), level: chosen, proposedLevel, overridden: Boolean(level), classified, unclassified };
}

export function draftNotes(classified) {
  const titles = { breaking: "Breaking changes", feature: "New and changed", fix: "Fixes", maintenance: "Maintenance", docs: "Documentation", null: "Unclassified" };
  const lines = ["<!-- Draft from pull request titles. Rewrite it in plain words (docs/release-process.md). -->"];
  for (const kind of ["breaking", "feature", "fix", "maintenance", "docs", null]) {
    const group = classified.filter((pull) => pull.kind === kind);
    if (!group.length) continue;
    lines.push("", `**${titles[kind]}**`, "");
    for (const pull of group) lines.push(`- ${pull.title} (#${pull.number})`);
  }
  return lines.join("\n");
}

function run(command, args) {
  return execFileSync(command, args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }).trim();
}

export function latestReleaseTag(tags) {
  const versioned = tags
    .map((tag) => ({ tag, match: /^(?:mcp-preview-|v)(\d+\.\d+\.\d+)$/.exec(tag) }))
    .filter(({ match }) => match)
    .map(({ tag, match }) => ({ tag, version: parse(match[1]) }));
  versioned.sort((a, b) => a.version[0] - b.version[0] || a.version[1] - b.version[1] || a.version[2] - b.version[2]);
  return versioned.at(-1)?.tag ?? null;
}

// Squash merges end each subject with "(#N)", the pull request; a subject
// that carries an issue number first, "... (#626) (#708)", ends with the pull
// request, so the last number is the one taken.
export function pullNumbers(subjects) {
  const numbers = [];
  const missing = [];
  for (const subject of subjects.filter(Boolean)) {
    const match = /\(#(\d+)\)\s*$/.exec(subject);
    if (match) numbers.push(Number(match[1]));
    else missing.push(subject);
  }
  return { numbers, missing };
}

function pullsSince(tag, to) {
  const { numbers, missing } = pullNumbers(run("git", ["log", "--format=%s", `${tag}..${to}`]).split("\n"));
  const pulls = [];
  for (const number of numbers) {
    const pull = JSON.parse(run("gh", ["pr", "view", String(number), "--json", "number,title,labels,closingIssuesReferences"]));
    const issueLabels = [];
    for (const issue of pull.closingIssuesReferences ?? []) {
      issueLabels.push(...JSON.parse(run("gh", ["issue", "view", String(issue.number), "--json", "labels"])).labels.map((label) => label.name));
    }
    pulls.push({ number: pull.number, title: pull.title, labels: pull.labels.map((label) => label.name), issueLabels });
  }
  // A commit without a pull request number (a direct push) cannot be
  // classified by label; it is reported as unclassified rather than dropped.
  for (const subject of missing) pulls.push({ number: "?", title: subject, labels: [], issueLabels: [] });
  return pulls;
}

function argument(name) {
  const index = process.argv.indexOf(name);
  return index === -1 ? undefined : process.argv[index + 1];
}

async function main() {
  const versions = readVersions();
  const distinct = [...new Set(Object.values(versions))];
  if (distinct.length !== 1 || !distinct[0]) {
    throw new Error(`version files disagree: ${JSON.stringify(versions)}`);
  }
  const current = distinct[0];
  const level = argument("--level");
  if (level && !["major", "minor", "patch"].includes(level)) throw new Error("--level must be major, minor or patch");
  const to = argument("--to") ?? "HEAD";
  const since = argument("--since") ?? latestReleaseTag(run("git", ["tag", "--list"]).split("\n"));
  if (!since) throw new Error("no release tag found; run git fetch --tags origin, or pass --since TAG");
  if (!argument("--since")) {
    // A stale clone would silently compare against an older release.
    const remote = latestReleaseTag(run("git", ["ls-remote", "--tags", "--refs", "origin"]).split("\n").map((line) => line.split("refs/tags/")[1] ?? ""));
    if (remote && remote !== since) throw new Error(`the newest release tag on origin is ${remote}, but the local one is ${since}; run git fetch --tags origin`);
  }
  // git log A..B does not fail when A is not an ancestor of B; it silently
  // returns a different set of commits.
  try {
    execFileSync("git", ["merge-base", "--is-ancestor", since, to], { cwd: root, stdio: "ignore" });
  } catch {
    throw new Error(`${since} is not an ancestor of ${to}; the pull requests since it cannot be listed`);
  }
  const result = propose({ current, pulls: pullsSince(since, to), level });

  console.log(`current version ${current}; last release tag ${since}; compared up to ${to}`);
  console.log(`${result.classified.length} commits since then`);
  if (result.unclassified.length) {
    console.log(`unclassified (${result.unclassified.length}):`);
    for (const pull of result.unclassified) console.log(`  #${pull.number} ${pull.title}`);
  }
  if (!result.ok) {
    console.error(result.reason);
    process.exitCode = 1;
    return;
  }
  console.log(`proposed level ${result.proposedLevel ?? "none"}${result.overridden ? `, overridden to ${result.level}` : ""}: ${current} -> ${result.next}`);
  console.log(`\n${draftNotes(result.classified)}\n`);
  if (process.argv.includes("--apply")) {
    writeVersions(result.next);
    console.log(`wrote ${result.next} to ${Object.keys(VERSION_FILES).join(", ")} and README.md`);
    console.log("package.json, src-tauri/Cargo.toml and src-tauri/Cargo.lock are pinned: run scripts/reseal.sh, then commit.");
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main();
