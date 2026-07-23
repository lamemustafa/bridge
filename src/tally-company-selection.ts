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

export type CompanyDiscoveryPrompt = {
  companyCount: number;
  heading: string;
  detail: string;
  actionLabel: string;
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

export function companyDiscoveryPrompt(
  selectedCompany: string,
  liveCompanyKeys: readonly string[],
  untrustedCompanyCount = 0,
): CompanyDiscoveryPrompt | null {
  if (selectedCompany !== "") return null;

  if (liveCompanyKeys.length > 0) {
    const companyCount = liveCompanyKeys.length;
    const companyLabel = companyCount === 1 ? "company" : "companies";
    return {
      companyCount,
      heading: `${companyCount} ${companyLabel} discovered`,
      detail: "Bridge identified the current Tally company list. Choose one explicitly before reading or saving any company-scoped data.",
      actionLabel: "Choose company",
    };
  }

  if (untrustedCompanyCount === 0) return null;

  const companyCount = untrustedCompanyCount;
  const companyLabel = companyCount === 1 ? "company" : "companies";
  return {
    companyCount,
    heading: `${companyCount} ${companyLabel} listed for verification`,
    detail: "Tally returned a compatibility company listing. Verify the intended company before Bridge treats its identity as evidence or enables company-scoped reads.",
    actionLabel: "Verify company",
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
