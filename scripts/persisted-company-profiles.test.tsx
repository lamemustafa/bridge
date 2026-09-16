// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { usePersistedCompanyProfiles } from "../src/persisted-company-profiles";

type Profiles = ReturnType<typeof usePersistedCompanyProfiles>;

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

async function mountHook(mergeProfiles: (profiles: unknown[]) => void) {
  const latest: { current: Profiles | null } = { current: null };
  function Probe() {
    latest.current = usePersistedCompanyProfiles(mergeProfiles);
    return null;
  }
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<Probe />));
  return { latest, unmount: () => act(async () => root.unmount()) };
}

const profile = (name: string) => ({ name, mirror_company_id: `mirror-${name}` });
const page = (names: string[], total = names.length, truncated = false) => ({
  profiles: names.map(profile),
  total_profiles: total,
  limit: 50,
  truncated,
});

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("a load merges the profiles and reports the page counts", async () => {
  const merge = vi.fn();
  mocks.invoke.mockResolvedValueOnce(page(["Alpha", "Beta"], 7, true));
  const { latest, unmount } = await mountHook(merge);
  await act(async () => { await latest.current!.refreshPersistedCompanyProfiles(); });
  expect(mocks.invoke).toHaveBeenCalledWith("tally_persisted_company_profiles");
  expect(merge).toHaveBeenCalledWith([profile("Alpha"), profile("Beta")]);
  expect(latest.current).toMatchObject({
    persistedCompanyProfileTotal: 7,
    persistedCompanyProfilesLoaded: 2,
    persistedCompanyProfilesTruncated: true,
    persistedCompanyProfilesLoading: false,
    persistedCompanyProfileError: null,
  });
  await unmount();
});

test("an older load that settles after a newer one writes nothing and does not end loading", async () => {
  const merge = vi.fn();
  const older = deferred<unknown>();
  const newer = deferred<unknown>();
  mocks.invoke.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
  const { latest, unmount } = await mountHook(merge);
  let olderLoad!: Promise<void>;
  let newerLoad!: Promise<void>;
  await act(async () => { olderLoad = latest.current!.refreshPersistedCompanyProfiles(); });
  await act(async () => { newerLoad = latest.current!.refreshPersistedCompanyProfiles(); });
  await act(async () => { older.resolve(page(["Stale"], 1)); await olderLoad; });
  expect(merge).not.toHaveBeenCalled();
  expect(latest.current!.persistedCompanyProfileTotal).toBe(0);
  expect(latest.current!.persistedCompanyProfilesLoading).toBe(true);
  await act(async () => { newer.resolve(page(["Fresh"], 1)); await newerLoad; });
  expect(merge).toHaveBeenCalledTimes(1);
  expect(merge).toHaveBeenCalledWith([profile("Fresh")]);
  expect(latest.current!.persistedCompanyProfilesLoading).toBe(false);
  await unmount();
});

test("an older load that fails after a newer one started reports no error", async () => {
  const older = deferred<unknown>();
  const newer = deferred<unknown>();
  mocks.invoke.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
  const { latest, unmount } = await mountHook(vi.fn());
  let olderLoad!: Promise<void>;
  let newerLoad!: Promise<void>;
  await act(async () => { olderLoad = latest.current!.refreshPersistedCompanyProfiles(); });
  await act(async () => { newerLoad = latest.current!.refreshPersistedCompanyProfiles(); });
  await act(async () => { older.reject(new Error("mirror locked")); await olderLoad; });
  expect(latest.current!.persistedCompanyProfileError).toBeNull();
  await act(async () => { newer.resolve(page([])); await newerLoad; });
  expect(latest.current!.persistedCompanyProfilesLoading).toBe(false);
  await unmount();
});

test("a failed load keeps the earlier counts and exposes the error until the next load starts", async () => {
  const merge = vi.fn();
  const envelope = {
    code: "mirror_unavailable",
    category: "Local mirror",
    message: "The encrypted mirror could not be opened.",
    retry: "safe",
    local_state_changed: false,
    tally_state_may_have_changed: false,
    remediation: "Unlock Bridge, then reopen the client list.",
  };
  const next = deferred<unknown>();
  mocks.invoke.mockResolvedValueOnce(page(["Alpha"], 3)).mockRejectedValueOnce(envelope).mockReturnValueOnce(next.promise);
  const { latest, unmount } = await mountHook(merge);
  await act(async () => { await latest.current!.refreshPersistedCompanyProfiles(); });
  await act(async () => { await latest.current!.refreshPersistedCompanyProfiles(); });
  expect(latest.current!.persistedCompanyProfileError).toEqual(envelope);
  expect(latest.current!.persistedCompanyProfileTotal).toBe(3);
  expect(latest.current!.persistedCompanyProfilesLoading).toBe(false);
  expect(merge).toHaveBeenCalledTimes(1);
  let load!: Promise<void>;
  await act(async () => { load = latest.current!.refreshPersistedCompanyProfiles(); });
  expect(latest.current!.persistedCompanyProfileError).toBeNull();
  expect(latest.current!.persistedCompanyProfilesLoading).toBe(true);
  await act(async () => { next.resolve(page(["Alpha"], 3)); await load; });
  await unmount();
});
