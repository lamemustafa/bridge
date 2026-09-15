// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("prepared Documents state survives the AXAL navigation round trip", async () => {
  const [app, documents] = await Promise.all([
    readFile(new URL("../src/main.tsx", import.meta.url), "utf8"),
    readFile(new URL("../src/DocumentsScreen.tsx", import.meta.url), "utf8"),
  ]);

  assert.match(
    app,
    /const \[documentsWorkspace, setDocumentsWorkspace\] = React\.useState\(createDocumentsWorkspaceState\);/,
  );
  assert.match(app, /workspaceState=\{documentsWorkspace\}/);
  assert.match(app, /setWorkspaceState=\{setDocumentsWorkspace\}/);
  // This is an ownership regression check, matching the repository's existing
  // source-boundary tests: it fails if the workflow state moves back into the
  // conditionally mounted screen without forbidding unrelated local UI state.
  assert.doesNotMatch(documents, /useState<SelectedDocumentPath/);
  assert.doesNotMatch(documents, /useState<ScanDocumentsResponse/);
  assert.doesNotMatch(documents, /useState<SyncDocumentsResponse/);
  assert.match(documents, /documentScan: null/);
  assert.match(documents, /documentAction: null/);
  assert.match(documents, /scanSessionId required for sync/);
});
