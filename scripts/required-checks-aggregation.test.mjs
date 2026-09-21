import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflowPath = join(repoRoot, ".github", "workflows", "ci.yml");

function requiredChecksCommand() {
  const lines = readFileSync(workflowPath, "utf8").split("\n");
  const stepStart = lines.findIndex((line) => line === "      - name: Verify no required job failed");
  assert.notEqual(stepStart, -1, "required-checks aggregation step is missing");

  const stepEndOffset = lines.slice(stepStart + 1).findIndex(
    (line) => /^      - name:/.test(line) || /^  [a-zA-Z0-9_-]+:/.test(line),
  );
  const stepEnd = stepEndOffset === -1 ? lines.length : stepStart + 1 + stepEndOffset;
  const runStart = lines.findIndex(
    (line, index) => index > stepStart && index < stepEnd && line === "        run: |",
  );
  assert.notEqual(runStart, -1, "required-checks aggregation command is missing");

  const commandLines = [];
  for (let index = runStart + 1; index < stepEnd; index += 1) {
    const line = lines[index];
    if (line === "") {
      commandLines.push("");
      continue;
    }
    assert.match(line, /^ {10}/, "required-checks command has unexpected indentation");
    commandLines.push(line.slice(10));
  }
  const command = commandLines.join("\n").trimEnd();
  assert.match(command, /NEEDS_JSON/, "required-checks command must consume NEEDS_JSON");
  return command;
}

const baseNeeds = {
  changes: { result: "success" },
  frontend: { result: "success" },
  "rust-format": { result: "success" },
  "workflow-consistency": { result: "success" },
  "tally-portable": { result: "success" },
  native: { result: "skipped" },
  "bundle-smoke": { result: "skipped" },
  "compiler-cache-retention": { result: "skipped" },
};

function runRequiredChecks(needs) {
  return spawnSync("bash", ["-c", requiredChecksCommand()], {
    cwd: repoRoot,
    encoding: "utf8",
    env: { ...process.env, NEEDS_JSON: JSON.stringify(needs) },
  });
}

test("required checks accept workflow consistency success and legitimate conditional skips", () => {
  const result = runRequiredChecks(baseNeeds);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /All required jobs succeeded or were legitimately skipped/);
});

for (const resultName of ["skipped", "failure", "cancelled", "neutral"]) {
  test(`required checks reject workflow consistency result ${resultName}`, () => {
    const result = runRequiredChecks({
      ...baseNeeds,
      "workflow-consistency": { result: resultName },
    });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stdout, /Required jobs did not pass: .*workflow-consistency/);
  });
}

test("required checks reject a missing workflow consistency result", () => {
  const needs = { ...baseNeeds };
  delete needs["workflow-consistency"];
  const result = runRequiredChecks(needs);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /Required jobs did not pass: .*workflow-consistency/);
});

for (const [job, resultName] of [["frontend", "failure"], ["rust-format", "cancelled"]]) {
  test(`required checks retain normal ${resultName} propagation for ${job}`, () => {
    const result = runRequiredChecks({
      ...baseNeeds,
      [job]: { result: resultName },
    });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stdout, new RegExp(`Required jobs did not pass: .*${job}`));
  });
}
