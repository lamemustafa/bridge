// SPDX-License-Identifier: Apache-2.0

import assert from "node:assert/strict";
import test from "node:test";

import {
  applyProbeCompanySelectionTransition,
  clearCompanyScopedState,
  reconcileProbeCompanySelection,
} from "../src/tally-company-selection.ts";

function companyScopedState() {
  return {
    passport: { profile_version: 1, company: "old-company" },
    profileSha256: "profile-old",
    reviewId: "review-old",
    reviewCommitmentSha256: "commitment-old",
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
}

function cleanupFor(state) {
  return {
    clearQualifiedReadReview: () => {
      state.passport = null;
      state.profileSha256 = null;
      state.reviewId = null;
      state.reviewCommitmentSha256 = null;
      state.selectedReadScope = null;
    },
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
  };
}

test("an automatic probe drop clears old company state before installing a usable fresh review", () => {
  const state = companyScopedState();
  const freshProbe = {
    passport: { profile_version: 2, company: "new-company" },
    profileSha256: "profile-new",
    reviewId: "review-new",
    reviewCommitmentSha256: "commitment-new",
    selectedReadScope: null,
    passportSnapshotId: null,
  };

  const transition = applyProbeCompanySelectionTransition(
    "old-company",
    ["new-company"],
    {
      clearDroppedCompanyScope: () => clearCompanyScopedState(cleanupFor(state)),
      installProbeState: () => Object.assign(state, freshProbe),
    },
  );
  assert.deepEqual(transition, { selectedCompany: "", dropped: true });
  assert.deepEqual(state, {
    ...freshProbe,
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
  assert.equal(
    Boolean(state.passport && state.reviewId && state.reviewCommitmentSha256),
    true,
    "the fresh probe review remains available after selecting a returned company",
  );
});

test("a manual company selection clears the existing review and all company-scoped state", () => {
  const state = companyScopedState();

  clearCompanyScopedState(cleanupFor(state));

  assert.equal(state.passport, null);
  assert.equal(state.profileSha256, null);
  assert.equal(state.reviewId, null);
  assert.equal(state.reviewCommitmentSha256, null);
  assert.equal(state.selectedReadScope, null);
  assert.equal(state.passportSnapshotId, null);
  assert.deepEqual(state.diagnostics, []);
  assert.equal(state.syncEvidence, null);
  assert.equal(state.proofPreview, null);
  assert.equal(state.mirrorExplorer, null);
  assert.equal(state.snapshotJob, null);
  assert.equal(state.snapshotStartOutcomeUnknown, false);
  assert.equal(state.diagnosticsRequestVersion, 5);
  assert.equal(state.proofPreviewRequestVersion, 8);
  assert.equal(state.snapshotSelectionVersion, 12);
  assert.equal(state.tallyResultsVersion, 14);
});

test("a manual company selection clears an unqualified probe review", () => {
  const state = companyScopedState();
  state.selectedReadScope = null;

  clearCompanyScopedState(cleanupFor(state));

  assert.equal(state.passport, null);
  assert.equal(state.profileSha256, null);
  assert.equal(state.reviewId, null);
  assert.equal(state.reviewCommitmentSha256, null);
  assert.equal(state.selectedReadScope, null);
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
