// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import {
  clearCompanyScopedState,
  reconcileProbeCompanySelection,
} from "../src/tally-company-selection.ts";

test("an automatic probe drop clears company-scoped data and invalidates every request token", () => {
  const state = {
    selectedReadScope: { company: "old-company" },
    passportSnapshotId: "passport-old",
    diagnostics: ["ledger-old"],
    syncEvidence: { company: "old-company" },
    syncEvidenceError: "old evidence error",
    proofPreview: { proof: "old-proof" },
    proofPreviewSelection: { proofId: "old-proof" },
    mirrorExplorer: { page: "old-page" },
    mirrorExplorerError: "old mirror error",
    snapshotJob: { runId: "old-run" },
    snapshotError: "old snapshot error",
    snapshotStartOutcomeUnknown: true,
    diagnosticsRequestVersion: 4,
    proofPreviewRequestVersion: 7,
    snapshotSelectionVersion: 11,
    tallyResultsVersion: 13,
  };

  const transition = reconcileProbeCompanySelection("old-company", ["new-company"]);
  assert.deepEqual(transition, { selectedCompany: "", dropped: true });

  if (transition.dropped) {
    clearCompanyScopedState({
      clearQualifiedReadReview: () => { state.selectedReadScope = null; },
      clearPassportSnapshot: () => { state.passportSnapshotId = null; },
      clearSensitiveDiagnostics: () => {
        state.diagnostics = [];
        state.diagnosticsRequestVersion += 1;
      },
      clearSyncEvidence: () => {
        state.syncEvidence = null;
        state.syncEvidenceError = null;
      },
      clearProofPreview: () => {
        state.proofPreview = null;
        state.proofPreviewSelection = null;
        state.proofPreviewRequestVersion += 1;
      },
      clearMirrorExplorer: () => {
        state.mirrorExplorer = null;
        state.mirrorExplorerError = null;
      },
      clearSnapshotState: () => {
        state.snapshotJob = null;
        state.snapshotError = null;
        state.snapshotStartOutcomeUnknown = false;
        state.snapshotSelectionVersion += 1;
      },
      invalidateTallyResults: () => { state.tallyResultsVersion += 1; },
    });
  }

  assert.deepEqual(state, {
    selectedReadScope: null,
    passportSnapshotId: null,
    diagnostics: [],
    syncEvidence: null,
    syncEvidenceError: null,
    proofPreview: null,
    proofPreviewSelection: null,
    mirrorExplorer: null,
    mirrorExplorerError: null,
    snapshotJob: null,
    snapshotError: null,
    snapshotStartOutcomeUnknown: false,
    diagnosticsRequestVersion: 5,
    proofPreviewRequestVersion: 8,
    snapshotSelectionVersion: 12,
    tallyResultsVersion: 14,
  });
});

test("a selected company retained by the probe is not reported as dropped", () => {
  assert.deepEqual(
    reconcileProbeCompanySelection("retained-company", ["other-company", "retained-company"]),
    { selectedCompany: "retained-company", dropped: false },
  );
  assert.deepEqual(
    reconcileProbeCompanySelection("", ["new-company"]),
    { selectedCompany: "", dropped: false },
  );
});
