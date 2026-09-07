import assert from "node:assert/strict";
import test from "node:test";

import { assertPreviewAdmission, previewVersion } from "./check-mcpb-preview-admission.mjs";

const current = {
  releaseTag: "mcp-preview-0.2.0",
  sourceRef: "refs/heads/master",
  defaultBranch: "master",
  applicationVersion: "0.2.0",
  mcpbVersion: "0.2.0",
};

test("preview admission binds the tag version to both source manifests on the default branch", () => {
  assert.equal(previewVersion(current.releaseTag), "0.2.0");
  assert.doesNotThrow(() => assertPreviewAdmission(current));
});

test("preview admission rejects a mismatched manifest version and an unreviewed branch", () => {
  assert.throws(
    () => assertPreviewAdmission({ ...current, releaseTag: "mcp-preview-0.2.1" }),
    /must match application and MCPB manifest versions/,
  );
  assert.throws(
    () => assertPreviewAdmission({ ...current, sourceRef: "refs/heads/preview-only" }),
    /must be dispatched from the default branch/,
  );
});
