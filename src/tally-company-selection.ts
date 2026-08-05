// SPDX-License-Identifier: Apache-2.0

export type CompanyScopeCleanup = {
  clearQualifiedReadReview: () => void;
  clearPassportSnapshot: () => void;
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

export type TallyReadinessState = {
  companyReady: boolean;
  companyNeedsRecheck: boolean;
  showCheck: boolean;
  showCompanyLink: boolean;
};

export type ProbeSelectionEffects = {
  clearDroppedCompanyScope: () => void;
  installProbeState: () => void;
};

export type TallyCompanyIdentity = {
  name: string;
  guid?: string;
  correlation_key?: string;
  mirror_company_id?: string;
};

export function tallyCompanyKey(company: TallyCompanyIdentity): string {
  if (company.correlation_key) return `correlation:${company.correlation_key}`;
  if (company.guid) return `guid:${company.guid.toLocaleLowerCase()}`;
  if (company.mirror_company_id) return `mirror:${company.mirror_company_id}`;
  return `unverified-name:${company.name}`;
}

export function currentProbeCompanies<T extends TallyCompanyIdentity>(
  companies: readonly T[],
  liveCompanyKeys: readonly string[],
): T[] {
  return companies.filter((company) => liveCompanyKeys.includes(tallyCompanyKey(company)));
}

export function canReuseCurrentProbeReview({
  reviewAvailable,
  setupSaved,
}: {
  reviewAvailable: boolean;
  setupSaved: boolean;
}): boolean {
  return reviewAvailable && !setupSaved;
}

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

export function tallyReadinessState({
  endpointComplete,
  companySelected,
  companyCurrent,
  companySaved,
}: {
  endpointComplete: boolean;
  companySelected: boolean;
  companyCurrent: boolean;
  companySaved: boolean;
}): TallyReadinessState {
  const companyReady = companySelected && companyCurrent && companySaved;
  return {
    companyReady,
    companyNeedsRecheck: companySelected && !companyCurrent,
    showCheck: !companyReady,
    showCompanyLink: endpointComplete && !companyReady,
  };
}

export function clearCompanyScopedState(cleanup: CompanyScopeCleanup) {
  cleanup.clearQualifiedReadReview();
  cleanup.clearPassportSnapshot();
  cleanup.clearSyncEvidence();
  cleanup.clearProofPreview();
  cleanup.clearMirrorExplorer();
  cleanup.clearSnapshotState();
  cleanup.invalidateTallyResults();
}
