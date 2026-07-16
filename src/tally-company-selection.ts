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

export type ProbeSelectionEffects = {
  clearDroppedCompanyScope: () => void;
  installProbeState: () => void;
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

export function applyProbeCompanySelectionTransition(
  selectedCompany: string,
  liveCompanyKeys: readonly string[],
  effects: ProbeSelectionEffects,
): ProbeSelectionTransition {
  const transition = reconcileProbeCompanySelection(selectedCompany, liveCompanyKeys);
  if (transition.dropped) effects.clearDroppedCompanyScope();
  effects.installProbeState();
  return transition;
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
