// SPDX-License-Identifier: Apache-2.0

export type CompanyScopeCleanup = {
  clearQualifiedReadReview: () => void;
  clearPassportSnapshot: () => void;
  clearSensitiveDiagnostics: () => void;
  clearSyncEvidence: () => void;
  clearProofPreview: () => void;
  clearMirrorExplorer: () => void;
  clearSnapshotState: () => void;
  invalidateTallyResults: () => void;
};

export type ProbeSelectionTransition = {
  selectedCompany: string;
  dropped: boolean;
};

export function reconcileProbeCompanySelection(
  selectedCompany: string,
  liveCompanyKeys: readonly string[],
): ProbeSelectionTransition {
  const dropped = selectedCompany !== "" && !liveCompanyKeys.includes(selectedCompany);
  return {
    selectedCompany: dropped ? "" : selectedCompany,
    dropped,
  };
}

export function clearCompanyScopedState(cleanup: CompanyScopeCleanup) {
  cleanup.clearQualifiedReadReview();
  cleanup.clearPassportSnapshot();
  cleanup.clearSensitiveDiagnostics();
  cleanup.clearSyncEvidence();
  cleanup.clearProofPreview();
  cleanup.clearMirrorExplorer();
  cleanup.clearSnapshotState();
  cleanup.invalidateTallyResults();
}
