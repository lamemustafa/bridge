// SPDX-License-Identifier: Apache-2.0
//
// `cargo audit` (see .cargo/audit.toml) answers "does src-tauri/Cargo.lock,
// as it stands right now, contain an accepted vulnerability?" It cannot
// answer "did THIS CHANGE introduce one" -- that requires comparing two
// states of the lockfile, and cargo-audit has no built-in flag for that. This
// script is the custom scripting that comparison actually needs; say so
// plainly rather than implying `cargo audit` does differential auditing on
// its own.
//
// The distinction matters because of what happened on 2026-09-15:
// RUSTSEC-2026-0285 was published against `rustls` (pulled in transitively
// via `reqwest`), and `cargo audit` on an UNCHANGED master went from a clean
// pass on 2026-09-12 to `error: 1 vulnerability found!` with no code change
// on our side. Because "Dependency security" was a required status check,
// that one external event blocked all 9 then-open PRs at once, including
// ones that had nothing to do with rustls or reqwest. The fix landed and was
// the right call -- but the blocking mechanism was wrong: a PR should fail
// because IT introduced new exposure, not because the outside world changed
// under an unrelated branch.
//
// This script resolves the set of advisory ids for two Cargo.lock states --
// the merge base and HEAD -- using the SAME single advisory-database
// snapshot for both (fetched once, the second scan passes `--no-fetch` to
// reuse it). That is what makes this a differential check rather than two
// independent audits: if an advisory newly applies to a dependency version
// that was ALREADY in the lockfile before this branch touched anything, it
// shows up in both the base and HEAD sets and nets out of the diff, exactly
// like the rustls incident above would have. It only fails on an id present
// in HEAD's set and absent from the base's -- something this branch's own
// Cargo.lock change is responsible for.
//
// `.cargo/audit.toml`'s `ignore` list applies identically to both scans
// (cargo-audit loads `./.cargo/audit.toml` relative to this process's cwd,
// not relative to either scanned lockfile), so an accepted, dated advisory
// never shows up as "new" here either -- this script and that file are a
// matched pair, not two independent gates.
//
// Run: node scripts/check-advisory-delta.mjs [--base <ref>] [--head <ref>]
//        [--lockfile <path>] [--allow-missing-git]
// In CI, run with no arguments: base defaults to the merge base of HEAD and
// the pull request's target branch (GITHUB_BASE_REF, else origin/master).

import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = fileURLToPath(new URL("../", import.meta.url));
const DEFAULT_LOCKFILE = "src-tauri/Cargo.lock";
const MAX_REPORTED_IDS = 30;

function parseArgs(argv) {
  const args = { base: null, head: "HEAD", lockfile: DEFAULT_LOCKFILE };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--base") args.base = requireValue(argv, ++i, "--base");
    else if (arg === "--head") args.head = requireValue(argv, ++i, "--head");
    else if (arg === "--lockfile") args.lockfile = requireValue(argv, ++i, "--lockfile");
    else throw new Error(`check-advisory-delta: unknown argument: ${arg}`);
  }
  return args;
}

function requireValue(argv, index, flag) {
  if (index >= argv.length) throw new Error(`check-advisory-delta: ${flag} requires a value`);
  return argv[index];
}

function git(args) {
  const result = spawnSync("git", args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  return result;
}

// Mirrors the candidate-ref fallback in check-protocol-section-numbers.mjs:
// prefer the PR's actual target branch when CI sets it, then origin/master,
// then a local master (a fork or shallow-clone checkout may lack the
// `origin/` remote-tracking ref).
function resolveBaseCommit(head) {
  const candidates = [
    process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : null,
    "origin/master",
    "master",
  ].filter(Boolean);
  const tried = [];
  for (const branch of candidates) {
    const mergeBase = git(["merge-base", branch, head]);
    if (mergeBase.status === 0) return { commit: mergeBase.stdout.trim(), via: branch };
    tried.push(branch);
  }
  throw new Error(
    "check-advisory-delta: could not compute a merge base against any of " +
      `${tried.join(", ")} -- pass --base <ref> explicitly (a shallow clone ` +
      "often needs this).",
  );
}

// Reads a lockfile out of a specific commit without touching the working
// tree or checking anything out -- HEAD may be a dirty work-in-progress and
// the base commit is not something this script should ever check out.
function lockfileAt(commit, lockfilePath) {
  const show = git(["show", `${commit}:${lockfilePath}`]);
  if (show.status !== 0) {
    throw new Error(
      `check-advisory-delta: 'git show ${commit}:${lockfilePath}' failed -- ` +
        `is ${commit} a valid commit that contains this lockfile?\n${show.stderr}`,
    );
  }
  return show.stdout;
}

function runCargoAudit(lockfileContents, { fetch }) {
  const scratchDir = mkdtempSync(join(tmpdir(), "bridge-advisory-delta-"));
  const scratchLockfile = join(scratchDir, "Cargo.lock");
  writeFileSync(scratchLockfile, lockfileContents);
  try {
    const args = ["audit", "--file", scratchLockfile, "--json"];
    if (!fetch) args.push("--no-fetch");
    const result = spawnSync("cargo", args, {
      cwd: REPO_ROOT, // so ./.cargo/audit.toml resolves the same way it does in CI
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
    let report;
    try {
      report = JSON.parse(result.stdout);
    } catch (cause) {
      throw new Error(
        "check-advisory-delta: 'cargo audit --json' did not produce valid JSON " +
          `(exit ${result.status}). stderr:\n${result.stderr}\nstdout:\n${result.stdout}`,
      );
    }
    return report;
  } finally {
    rmSync(scratchDir, { recursive: true, force: true });
  }
}

// Collects one id per finding: a real RUSTSEC id where the finding has one
// (every `vulnerabilities` entry, and most `warnings` entries), and a
// synthetic `<kind>:<name>@<version>` key for the ones that don't -- a
// yanked-crate warning carries no advisory at all (see .cargo/audit.toml's
// note on `chacha20`), so without a synthetic key it would be invisible to
// this diff entirely, on both sides, forever.
function idsAndDetails(report) {
  const details = new Map();
  const record = (id, detail) => {
    if (!details.has(id)) details.set(id, detail);
  };
  for (const vulnerability of report.vulnerabilities.list) {
    record(vulnerability.advisory.id, {
      id: vulnerability.advisory.id,
      package: `${vulnerability.package.name}@${vulnerability.package.version}`,
      summary: vulnerability.advisory.title,
      kind: "vulnerability",
    });
  }
  for (const kind of Object.keys(report.warnings ?? {})) {
    for (const warning of report.warnings[kind]) {
      const id =
        warning.advisory?.id ?? `${kind}:${warning.package.name}@${warning.package.version}`;
      record(id, {
        id,
        package: `${warning.package.name}@${warning.package.version}`,
        summary: warning.advisory?.title ?? kind,
        kind,
      });
    }
  }
  return details;
}

function formatFinding(detail) {
  return `  - ${detail.id} (${detail.kind}) ${detail.package} -- ${detail.summary}`;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const head = args.head;

  const headCommitCheck = git(["rev-parse", "--verify", head]);
  if (headCommitCheck.status !== 0) {
    throw new Error(`check-advisory-delta: '${head}' does not resolve to a commit`);
  }
  const headCommit = headCommitCheck.stdout.trim();

  const base = args.base ? { commit: args.base, via: "--base" } : resolveBaseCommit(headCommit);
  const baseCommitCheck = git(["rev-parse", "--verify", base.commit]);
  if (baseCommitCheck.status !== 0) {
    throw new Error(
      `check-advisory-delta: base ref '${base.commit}' (via ${base.via}) does not resolve to a commit`,
    );
  }
  const baseCommit = baseCommitCheck.stdout.trim();

  console.log(`check-advisory-delta: base=${baseCommit} (via ${base.via}) head=${headCommit}`);

  const baseLockfile = lockfileAt(baseCommit, args.lockfile);
  const headLockfile = lockfileAt(headCommit, args.lockfile);

  if (baseCommit === headCommit || baseLockfile === headLockfile) {
    console.log(
      "check-advisory-delta: lockfile is byte-identical between base and head -- no delta to check.",
    );
    return;
  }

  // Fetch the advisory database once and reuse it for both scans, so any
  // difference between the two id sets is caused by the Cargo.lock content
  // difference alone, never by the advisory database changing between the
  // two `cargo audit` invocations.
  const baseReport = runCargoAudit(baseLockfile, { fetch: true });
  const headReport = runCargoAudit(headLockfile, { fetch: false });

  const baseDetails = idsAndDetails(baseReport);
  const headDetails = idsAndDetails(headReport);

  const newIds = [...headDetails.keys()].filter((id) => !baseDetails.has(id)).sort();
  const resolvedIds = [...baseDetails.keys()].filter((id) => !headDetails.has(id)).sort();

  if (resolvedIds.length) {
    console.log(
      `check-advisory-delta: ${resolvedIds.length} advisory finding(s) present at the base ` +
        "are no longer present at HEAD:",
    );
    for (const id of resolvedIds.slice(0, MAX_REPORTED_IDS)) {
      console.log(formatFinding(baseDetails.get(id)));
    }
  }

  if (newIds.length) {
    const shown = newIds.slice(0, MAX_REPORTED_IDS);
    const lines = shown.map((id) => formatFinding(headDetails.get(id)));
    if (newIds.length > shown.length) {
      lines.push(`  ... and ${newIds.length - shown.length} more`);
    }
    throw new Error(
      `check-advisory-delta: this change introduces ${newIds.length} new advisory ` +
        `finding(s) not present at the merge base (${baseCommit}):\n${lines.join("\n")}\n\n` +
        "If this is a real vulnerability, fix the dependency rather than ignoring it. If it is " +
        "an accepted risk with no available fix, add a dated, reasoned entry to .cargo/audit.toml " +
        "-- see that file's header for what it can and cannot express.",
    );
  }

  console.log(
    `check-advisory-delta: no new advisory findings (base has ${baseDetails.size}, ` +
      `head has ${headDetails.size}, both filtered through .cargo/audit.toml).`,
  );
}

main();
