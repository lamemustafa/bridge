// @vitest-environment jsdom

import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, test, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));

import { useTallyRuntimeSessions } from "../src/tally-runtime-sessions";

type Sessions = ReturnType<typeof useTallyRuntimeSessions>;

async function mountHook() {
  const latest: { current: Sessions | null } = { current: null };
  function Probe() {
    latest.current = useTallyRuntimeSessions();
    return null;
  }
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<Probe />));
  return { latest, unmount: () => act(async () => root.unmount()) };
}

const session = {
  session_id: "session-1",
  canonical_endpoint: "http://127.0.0.1:9000",
  issued_requests: 3,
  active_requests: 1,
  active_request_ids: ["request-1"],
  consecutive_failures: 0,
  circuit_state: "closed",
};

const envelope = {
  code: "endpoint_unreachable",
  category: "Endpoint configuration",
  message: "The local Tally endpoint is unreachable.",
  retry: "safe",
  local_state_changed: false,
  tally_state_may_have_changed: false,
  remediation: "Start Tally, then refresh.",
};

afterEach(() => {
  document.body.replaceChildren();
  mocks.invoke.mockReset();
});

test("refresh loads sessions and clears an earlier error", async () => {
  mocks.invoke.mockRejectedValueOnce(envelope).mockResolvedValueOnce([session]);
  const { latest, unmount } = await mountHook();
  await act(async () => { await latest.current!.refreshRuntime(); });
  expect(latest.current!.runtimeError).toEqual(envelope);
  await act(async () => { await latest.current!.refreshRuntime(); });
  expect(latest.current!.runtimeSessions).toEqual([session]);
  expect(latest.current!.runtimeError).toBeNull();
  expect(mocks.invoke.mock.calls).toEqual([["tally_runtime_snapshots"], ["tally_runtime_snapshots"]]);
  await unmount();
});

test("a failed refresh keeps the last sessions and normalises a thrown Error to its message", async () => {
  mocks.invoke.mockResolvedValueOnce([session]).mockRejectedValueOnce(new Error("socket closed"));
  const { latest, unmount } = await mountHook();
  await act(async () => { await latest.current!.refreshRuntime(); });
  await act(async () => { await latest.current!.refreshRuntime(); });
  expect(latest.current!.runtimeSessions).toEqual([session]);
  expect(latest.current!.runtimeError).toBe("socket closed");
  await unmount();
});

test("cancelling a request that already finished says so, then refreshes", async () => {
  let finishRefresh!: (sessions: unknown) => void;
  mocks.invoke.mockImplementation((command: string) =>
    command === "cancel_tally_request" ? Promise.resolve(false) : new Promise((resolve) => { finishRefresh = resolve; }));
  const { latest, unmount } = await mountHook();
  await act(async () => { await latest.current!.cancelTallyRequest("request-1"); });
  expect(mocks.invoke.mock.calls).toEqual([
    ["cancel_tally_request", { requestId: "request-1" }],
    ["tally_runtime_snapshots"],
  ]);
  expect(latest.current!.runtimeError).toBe("The request had already completed or was not found.");
  await act(async () => { finishRefresh([session]); });
  expect(latest.current!.runtimeSessions).toEqual([session]);
  expect(latest.current!.runtimeError).toBeNull();
  await unmount();
});

test("a cancel that fails keeps its error when the following refresh also fails", async () => {
  mocks.invoke.mockRejectedValue(envelope);
  const { latest, unmount } = await mountHook();
  await act(async () => { await latest.current!.cancelTallyRequest("request-1"); });
  await act(async () => {});
  expect(mocks.invoke.mock.calls.map(([command]) => command)).toEqual(["cancel_tally_request", "tally_runtime_snapshots"]);
  expect(latest.current!.runtimeError).toEqual(envelope);
  await unmount();
});
