// SPDX-License-Identifier: Apache-2.0
//
// Contract tests for scripts/audit-tracking-issue.mjs.
//
// The failure this file exists to prevent is not "the issue is wrong". It is
// the scheduled audit filing a fresh issue every single day an advisory stays
// unresolved, which is what a reconciler does the moment it can no longer read
// back what its previous run wrote. So the load-bearing test here is the body
// round trip: `recordedIds(issueBody(findings))` must return exactly the ids
// that went in. Reformat the issue body without keeping that true and this
// suite fails rather than the repository quietly accumulating duplicates.
//
// Everything is pure -- no gh, no network, no Rust toolchain. `pnpm test`
// globs scripts/*.test.mjs in the "Frontend build" CI job, which has none.

import assert from "node:assert/strict";
import test from "node:test";

import {
  MARKER,
  findingsFromReport,
  issueBody,
  planAction,
  recordedIds,
} from "./audit-tracking-issue.mjs";

const vuln = (id, name, version, title) => ({
  advisory: { id, title },
  package: { name, version },
});

const REPORT = {
  vulnerabilities: {
    found: true,
    list: [vuln("RUSTSEC-2026-0285", "rustls", "0.23.43", "TLS 1.3 handshake messages")],
  },
  warnings: {
    unmaintained: [
      { advisory: { id: "RUSTSEC-2024-0001", title: "unmaintained crate" }, package: { name: "old", version: "1.0.0" } },
    ],
    // A yanked crate carries no advisory at all. Without a synthetic key it
    // would be invisible to a set comparison, so a yank appearing or being
    // resolved would never move the tracking issue.
    yanked: [{ package: { name: "chacha20", version: "0.9.0" } }],
  },
};

test("a report becomes a sorted, deduplicated finding set", () => {
  const findings = findingsFromReport(REPORT);
  assert.deepEqual(
    findings.map((finding) => finding.id),
    ["RUSTSEC-2024-0001", "RUSTSEC-2026-0285", "yanked:chacha20@0.9.0"],
  );
  assert.equal(findings.find((f) => f.id === "RUSTSEC-2026-0285").kind, "vulnerability");
});

test("a warning with no advisory still gets a stable synthetic id", () => {
  const findings = findingsFromReport({ vulnerabilities: { list: [] }, warnings: { yanked: [{ package: { name: "c", version: "1.2.3" } }] } });
  assert.deepEqual(findings.map((f) => f.id), ["yanked:c@1.2.3"]);
});

test("an empty report yields no findings", () => {
  assert.deepEqual(findingsFromReport({ vulnerabilities: { found: false, list: [] } }), []);
});

// The one that matters: a body this script wrote must be readable by the next
// run. If this breaks, every scheduled run files a duplicate issue.
test("the issue body round-trips its recorded ids", () => {
  const findings = findingsFromReport(REPORT);
  const body = issueBody(findings);
  assert.ok(body.includes(MARKER), "the body must carry the marker used to find it again");
  assert.deepEqual(
    recordedIds(body),
    findings.map((f) => f.id).sort(),
  );
});

test("a body with no recorded-ids line reads as no ids, not a crash", () => {
  assert.deepEqual(recordedIds("nothing here"), []);
  assert.deepEqual(recordedIds(undefined), []);
});

test("clean run with nothing tracked does nothing", () => {
  assert.deepEqual(planAction([], null), { action: "none" });
});

test("clean run with something tracked closes it", () => {
  assert.deepEqual(planAction([], { number: 7, ids: ["RUSTSEC-2026-0285"] }), {
    action: "close",
    issue: 7,
  });
});

test("first finding opens an issue", () => {
  const findings = findingsFromReport(REPORT);
  const plan = planAction(findings, null);
  assert.equal(plan.action, "create");
  assert.deepEqual(plan.ids, findings.map((f) => f.id).sort());
});

test("an unchanged finding set is silent rather than a daily duplicate", () => {
  const findings = findingsFromReport(REPORT);
  const plan = planAction(findings, { number: 7, ids: findings.map((f) => f.id) });
  assert.equal(plan.action, "unchanged");
  assert.equal(plan.issue, 7);
});

test("ordering alone is not a change", () => {
  const findings = findingsFromReport(REPORT);
  const reversed = findings.map((f) => f.id).reverse();
  assert.equal(planAction(findings, { number: 7, ids: reversed }).action, "unchanged");
});

test("a changed finding set updates and names what moved", () => {
  const findings = findingsFromReport(REPORT);
  const plan = planAction(findings, { number: 7, ids: ["RUSTSEC-2024-0001", "RUSTSEC-1999-0000"] });
  assert.equal(plan.action, "update");
  assert.deepEqual(plan.added, ["RUSTSEC-2026-0285", "yanked:chacha20@0.9.0"]);
  assert.deepEqual(plan.removed, ["RUSTSEC-1999-0000"]);
});

test("a tracked issue whose ids cannot be read is treated as changed, not as clean", () => {
  const findings = findingsFromReport(REPORT);
  const plan = planAction(findings, { number: 7, ids: [] });
  assert.equal(plan.action, "update", "an unreadable body must not silently look up to date");
});
