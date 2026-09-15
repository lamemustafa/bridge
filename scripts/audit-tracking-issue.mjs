// SPDX-License-Identifier: Apache-2.0
//
// Keeps exactly one open tracking issue in step with what `cargo audit` finds
// on the default branch, for the scheduled audit in
// `.github/workflows/dependency-security-scheduled.yml`.
//
// Why a scheduled audit needs this at all: on 2026-09-15 RUSTSEC-2026-0285 was
// published against `rustls` and nobody knew until the next person to open a
// pull request found the pipeline frozen. An advisory published against
// unchanged code is not a pull request's fault and cannot be discovered by
// anything that only runs on pull requests -- it needs a job that looks at the
// default branch on a clock. That job then has to put its finding somewhere a
// person will see, which is what this script does.
//
// The hard part is not opening an issue, it is not opening one every day. An
// advisory commonly stays unresolved for days while an upgrade is
// investigated, so "file a finding" naively becomes a daily duplicate. This
// script therefore reconciles rather than reports: one open issue per
// repository, found by a hidden marker, updated only when the finding SET
// changes, and closed when the findings are gone. A run that changes nothing
// is the normal case and must stay silent.
//
// Split deliberately into a pure planner and a thin applier: the planner is
// what the tests exercise, so the reconciliation rules are provable without a
// network, a GitHub token, or a Rust toolchain -- `pnpm test` runs every
// scripts/*.test.mjs in the "Frontend build" CI job, which has none of those.

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

// A hidden marker rather than a title match: titles get edited by people, and
// a tracking issue whose title someone improved must still be found.
export const MARKER = "<!-- dependency-security-tracker -->";

// The label already exists in this repository. A new one would have to be
// created as a side effect of the first run, which is a repository
// configuration change this script has no business making on its own.
const LABEL = "area:security";

const TITLE = "Dependency advisories on master";

// Mirrors `idsAndDetails` in check-advisory-delta.mjs deliberately rather than
// importing it: that module runs `main()` at import time, so importing it here
// would run a full audit as a side effect. Keep the two in step -- especially
// the synthetic key, which exists because a yanked-crate warning carries no
// advisory at all and would otherwise be invisible to a set comparison.
export function findingsFromReport(report) {
  const findings = new Map();
  const record = (id, detail) => {
    if (!findings.has(id)) findings.set(id, detail);
  };
  for (const vulnerability of report.vulnerabilities?.list ?? []) {
    record(vulnerability.advisory.id, {
      id: vulnerability.advisory.id,
      package: `${vulnerability.package.name}@${vulnerability.package.version}`,
      summary: vulnerability.advisory.title,
      kind: "vulnerability",
    });
  }
  for (const kind of Object.keys(report.warnings ?? {})) {
    for (const warning of report.warnings[kind] ?? []) {
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
  return [...findings.values()].sort((a, b) => a.id.localeCompare(b.id));
}

// The recorded set is written into the issue body on its own line so a later
// run can read back exactly what the last run saw. Comparing against the
// rendered prose instead would make an editorial change to the issue look
// like a change in the findings.
const RECORDED_PREFIX = "Tracked advisory ids:";

export function recordedIds(body) {
  const line = (body ?? "")
    .split("\n")
    .map((entry) => entry.trim())
    .find((entry) => entry.startsWith(RECORDED_PREFIX));
  if (!line) return [];
  return line
    .slice(RECORDED_PREFIX.length)
    .split(",")
    .map((id) => id.trim().replace(/^`|`$/g, ""))
    .filter(Boolean)
    .sort();
}

/// The whole contract, in one pure function.
///
/// `none` and `unchanged` are distinct on purpose: both do nothing, but the
/// first means "clean, nothing tracked" and the second means "still broken,
/// already tracked". Collapsing them would hide a standing advisory behind the
/// same silence as a clean run.
export function planAction(findings, existing) {
  const ids = findings.map((finding) => finding.id).sort();
  if (ids.length === 0) {
    return existing ? { action: "close", issue: existing.number } : { action: "none" };
  }
  if (!existing) return { action: "create", ids, findings };
  const recorded = [...(existing.ids ?? [])].sort();
  const same = recorded.length === ids.length && recorded.every((id, i) => id === ids[i]);
  if (same) return { action: "unchanged", issue: existing.number, ids };
  return {
    action: "update",
    issue: existing.number,
    ids,
    findings,
    added: ids.filter((id) => !recorded.includes(id)),
    removed: recorded.filter((id) => !ids.includes(id)),
  };
}

export function issueBody(findings) {
  const lines = findings.map(
    (finding) => `- \`${finding.id}\` (${finding.kind}) ${finding.package} -- ${finding.summary}`,
  );
  return [
    MARKER,
    "",
    "`cargo audit` reports the following on the default branch, after",
    "`.cargo/audit.toml`'s ignore list is applied.",
    "",
    ...lines,
    "",
    "This issue is maintained by the scheduled dependency audit. It is updated",
    "when the set of findings changes and closed automatically when the",
    "findings are gone -- edit the title or add comments freely, but leave the",
    "marker and the recorded-ids line intact or the next run will open a",
    "duplicate.",
    "",
    `${RECORDED_PREFIX} ${findings.map((finding) => `\`${finding.id}\``).join(", ")}`,
  ].join("\n");
}

function gh(args) {
  const result = spawnSync("gh", args, { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 });
  if (result.status !== 0) {
    throw new Error(`audit-tracking-issue: gh ${args.join(" ")} failed: ${result.stderr?.trim()}`);
  }
  return result.stdout;
}

// `gh issue list` rather than `gh api .../issues`: the repository's
// `check-gh-api-pagination` gate exempts gh's own list subcommands because
// their paging is gh-owned, and an explicit --limit says what this one expects.
function findExisting() {
  const issues = JSON.parse(
    gh([
      "issue",
      "list",
      "--state",
      "open",
      "--label",
      LABEL,
      "--limit",
      "100",
      "--json",
      "number,body",
    ]),
  );
  const match = issues.find((issue) => (issue.body ?? "").includes(MARKER));
  return match ? { number: match.number, ids: recordedIds(match.body) } : null;
}

function apply(plan) {
  if (plan.action === "none" || plan.action === "unchanged") return;
  if (plan.action === "create") {
    gh(["issue", "create", "--title", TITLE, "--label", LABEL, "--body", issueBody(plan.findings)]);
    return;
  }
  if (plan.action === "update") {
    gh(["issue", "edit", String(plan.issue), "--body", issueBody(plan.findings)]);
    const changed = [
      plan.added.length ? `newly reported: ${plan.added.join(", ")}` : null,
      plan.removed.length ? `no longer reported: ${plan.removed.join(", ")}` : null,
    ]
      .filter(Boolean)
      .join("; ");
    gh(["issue", "comment", String(plan.issue), "--body", `Scheduled audit update -- ${changed}.`]);
    return;
  }
  gh([
    "issue",
    "comment",
    String(plan.issue),
    "--body",
    "Scheduled audit reports no findings on the default branch. Closing.",
  ]);
  gh(["issue", "close", String(plan.issue)]);
}

function main() {
  const argv = process.argv.slice(2);
  let reportPath = null;
  let shouldApply = false;
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--report") reportPath = argv[++i];
    else if (argv[i] === "--apply") shouldApply = true;
    else throw new Error(`audit-tracking-issue: unknown argument: ${argv[i]}`);
  }
  if (!reportPath) throw new Error("audit-tracking-issue: --report <path> is required");

  const findings = findingsFromReport(JSON.parse(readFileSync(reportPath, "utf8")));
  const plan = planAction(findings, shouldApply ? findExisting() : null);
  process.stdout.write(`${JSON.stringify(plan, null, 2)}\n`);
  if (shouldApply) apply(plan);
}

if (process.argv[1] && process.argv[1].endsWith("audit-tracking-issue.mjs")) main();
