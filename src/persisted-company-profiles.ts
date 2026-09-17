import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { type OperatorError, toOperatorError } from "./tally-command-error";
import type { TallyCompany } from "./tally-mirror-contract";

type PersistedCompanyProfilePage = {
  profiles: TallyCompany[];
  total_profiles: number;
  limit: number;
  truncated: boolean;
};

// Loads the saved company profiles Bridge keeps locally. A newer load
// supersedes an older one: only the latest load may write results, report an
// error or clear the loading flag. Merging the profiles into App's company list
// stays with App, which owns that list and its identity rules.
export function usePersistedCompanyProfiles(mergeProfiles: (profiles: TallyCompany[]) => void) {
  const [persistedCompanyProfileTotal, setPersistedCompanyProfileTotal] = React.useState(0);
  const [persistedCompanyProfilesLoaded, setPersistedCompanyProfilesLoaded] = React.useState(0);
  const [persistedCompanyProfilesTruncated, setPersistedCompanyProfilesTruncated] = React.useState(false);
  const [persistedCompanyProfilesLoading, setPersistedCompanyProfilesLoading] = React.useState(false);
  const [persistedCompanyProfileError, setPersistedCompanyProfileError] = React.useState<OperatorError | null>(null);
  const persistedCompanyProfileLoadVersion = React.useRef(0);

  const refreshPersistedCompanyProfiles = React.useCallback(async () => {
    const loadVersion = persistedCompanyProfileLoadVersion.current + 1;
    persistedCompanyProfileLoadVersion.current = loadVersion;
    setPersistedCompanyProfilesLoading(true);
    setPersistedCompanyProfileError(null);
    try {
      const page = await invoke<PersistedCompanyProfilePage>("tally_persisted_company_profiles");
      if (loadVersion !== persistedCompanyProfileLoadVersion.current) return;
      mergeProfiles(page.profiles);
      setPersistedCompanyProfileTotal(page.total_profiles);
      setPersistedCompanyProfilesLoaded(page.profiles.length);
      setPersistedCompanyProfilesTruncated(page.truncated);
      setPersistedCompanyProfileError(null);
    } catch (error) {
      if (loadVersion !== persistedCompanyProfileLoadVersion.current) return;
      const operatorError = toOperatorError(error);
      setPersistedCompanyProfileError(operatorError);
    } finally {
      if (loadVersion === persistedCompanyProfileLoadVersion.current) {
        setPersistedCompanyProfilesLoading(false);
      }
    }
  }, [mergeProfiles]);

  return {
    persistedCompanyProfileTotal,
    persistedCompanyProfilesLoaded,
    persistedCompanyProfilesTruncated,
    persistedCompanyProfilesLoading,
    persistedCompanyProfileError,
    refreshPersistedCompanyProfiles,
  };
}
