import assert from "node:assert/strict";
import test from "node:test";
import { recoverSnapshotJob } from "../src/snapshot-job-recovery.ts";

const active = { run_id: "active", phase: "extract", requires_resume: false };
const historical = { run_id: "historical", phase: "extract", requires_resume: true };
const completed = { run_id: "completed", phase: "completed", requires_resume: false };

test("a lost start can reattach only one current-process active worker", () => {
  assert.equal(recoverSnapshotJob(null, [completed, historical, active], null), active);
  assert.equal(recoverSnapshotJob(null, [completed, historical], null), null);
  assert.equal(recoverSnapshotJob(null, [active, { ...active, run_id: "other" }], null), null);
});

test("a lost resume uses its exact run and never substitutes another active worker", () => {
  assert.equal(recoverSnapshotJob(null, [active, { ...active, run_id: "other" }], "active"), active);
  assert.equal(recoverSnapshotJob(null, [active, historical], "historical"), null);
  assert.equal(recoverSnapshotJob(null, [active, completed], "missing"), null);
});

test("refresh preserves the current polling handle and applies its terminal observation", () => {
  const terminal = { ...active, phase: "cancelled" };
  assert.equal(recoverSnapshotJob(active, [completed, historical], null), active);
  assert.equal(recoverSnapshotJob(active, [terminal, completed], null), terminal);
});
