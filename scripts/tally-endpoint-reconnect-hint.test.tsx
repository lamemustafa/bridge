// @vitest-environment jsdom
// SPDX-License-Identifier: Apache-2.0

import React, { act } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

const endpointHintKey = "bridge.tally.endpoint-reconnect-hint.v1";
const actEnvironment = globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean };

actEnvironment.IS_REACT_ACT_ENVIRONMENT = true;

function button(root: HTMLElement, text: string): HTMLButtonElement {
  const matched = [...root.querySelectorAll<HTMLButtonElement>("button")].find((candidate) =>
    candidate.textContent?.includes(text),
  );
  expect(matched).toBeDefined();
  return matched!;
}

beforeEach(() => vi.resetModules());

afterEach(async () => {
  const root = document.getElementById("root") as (HTMLElement & {
    bridgeRoot?: { unmount(): void };
  }) | null;
  await act(async () => root?.bridgeRoot?.unmount());
  document.body.replaceChildren();
  window.localStorage.clear();
  mocks.invoke.mockReset();
});

test("restart restores only the endpoint hint and waits for an explicit Check Tally", async () => {
  window.localStorage.setItem(endpointHintKey, JSON.stringify({
    host: "127.1.2.3",
    port: 9001,
    selectedCompany: "forged-company",
    passport: { profile_version: 99 },
    canonical_origin: "http://forged.example:9001",
  }));
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "tally_persisted_company_profiles") {
      return Promise.resolve({ profiles: [], total_profiles: 0, limit: 100, truncated: false });
    }
    return Promise.resolve(undefined);
  });
  const root = document.createElement("div");
  root.id = "root";
  document.body.append(root);

  await act(async () => {
    await import("../src/main.tsx");
  });
  await act(async () => {
    button(root, "Settings").click();
  });

  const fields = root.querySelectorAll<HTMLInputElement>("input");
  expect(fields[0]?.value).toBe("127.1.2.3");
  expect(fields[1]?.value).toBe("9001");
  expect(root.textContent).toContain("Enter the address where Tally is running, then check the connection.");
  expect(root.textContent).not.toContain("Choose a company");
  expect(mocks.invoke).not.toHaveBeenCalledWith("probe_tally", expect.anything());

  await act(async () => {
    button(root, "Check Tally").click();
  });
  expect(mocks.invoke).toHaveBeenCalledWith("probe_tally", { config: { host: "127.1.2.3", port: 9001 } });
});

test.each([
  [false, "supported", "observed", true],
  [true, "unknown", "observed", false],
  [true, "supported", "inferred", false],
] as const)("persists only observed XML success (status compatible=%s, XML=%s/%s)", async (compatible, state, confidence, shouldSave) => {
  mocks.invoke.mockImplementation((command: string) => {
    if (command === "tally_persisted_company_profiles") {
      return Promise.resolve({ profiles: [], total_profiles: 0, limit: 100, truncated: false });
    }
    if (command === "probe_tally") {
      return Promise.resolve({
        connection: { reachable: true, compatible, product: "Unknown", server_text: "", error: "status_heuristic_unavailable" },
        companies: [], canonical_origin: "http://localhost:9000", observed_at_unix_ms: 1,
        review_id: "current-review", profile_sha256: "current-profile", review_commitment_sha256: "current-commitment",
        profile: { profile_version: 1, product: "TallyPrime", transports: { xml_http: { state, confidence } }, features: {}, packs: {} },
      });
    }
    return Promise.resolve(undefined);
  });
  const root = document.createElement("div");
  root.id = "root";
  document.body.append(root);
  await act(async () => { await import("../src/main.tsx"); });
  await act(async () => { button(root, "Settings").click(); });
  await act(async () => { button(root, "Check Tally").click(); });
  expect(window.localStorage.getItem(endpointHintKey)).toBe(shouldSave ? JSON.stringify({ host: "localhost", port: 9000 }) : null);
});
