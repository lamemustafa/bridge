// @vitest-environment jsdom
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { MirrorProofScreen } from "../src/MirrorProofScreen";

type Props = React.ComponentProps<typeof MirrorProofScreen>;
type SnapshotJob = NonNullable<Props["snapshotJob"]>;

function run(run_id: string, phase: SnapshotJob["phase"], requires_resume = false): SnapshotJob {
  return {
    run_id,
    phase,
    requires_resume,
    resume_available: requires_resume,
    mirror_company_id: "company",
    pack_id: "core_accounting",
    requested_from_yyyymmdd: null,
    requested_to_yyyymmdd: null,
    active_window_id: null,
    completed_windows: 0,
    total_windows: 1,
    verification: null,
    proof_id: null,
    proof_sha256: null,
    gap_codes: [],
    warning_codes: [],
    failure_code: null,
  };
}

function props(
  refreshRecentSnapshots: Props["refreshRecentSnapshots"],
  snapshotOutcomeUnknownRunId: string | null = null,
  snapshotSelectionVersion = { current: 1 },
) {
  const clear = vi.fn();
  const result: Props = {
    config: { host: "127.0.0.1", port: 9001 },
    status: null,
    passport: null,
    tallyAction: null,
    selectedCompanyRecord: undefined,
    selectedCompanyLive: false,
    savedCompanyPicker: null,
    companyError: null,
    fixtureControls: null,
    voucherFrom: "",
    setVoucherFrom: vi.fn(),
    voucherTo: "",
    setVoucherTo: vi.fn(),
    syncEvidence: null,
    syncEvidenceError: null,
    refreshSyncEvidence: async () => {},
    latestProof: undefined,
    mirrorTruthState: "unknown",
    snapshotJob: null,
    setInspectedJob: vi.fn(),
    snapshotSelectionVersion,
    snapshotActive: false,
    snapshotError: null,
    snapshotStartOutcomeUnknown: true,
    snapshotOutcomeUnknownRunId,
    liveReadActionsLocked: false,
    setSnapshotStartOutcomeUnknown: clear,
    setSnapshotOutcomeUnknownRunId: vi.fn(),
    startCoreSnapshot: async () => {},
    cancelCoreSnapshot: async () => {},
    resumeCoreSnapshot: async () => {},
    selectedRecentSnapshotRuns: [],
    refreshRecentSnapshots,
    inspectedJob: null,
    activeGapCodes: [],
    activeWarningCodes: [],
    mirrorExplorer: null,
    mirrorExplorerError: null,
    loadMirrorExplorerPage: async () => {},
    proofPreview: null,
    proofPreviewSelection: null,
    previewRedactedProof: async () => {},
    runtimeSessions: [],
    runtimeError: null,
    refreshRuntime: async () => {},
    cancelTallyRequest: async () => {},
  };
  return { clear, result };
}

async function renderAcknowledgment(input: Props) {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<MirrorProofScreen {...input} />));
  const button = [...host.querySelectorAll("button")].find((candidate) =>
    candidate.textContent?.includes("Refresh and confirm"),
  );
  expect(button).toBeDefined();
  return {
    button: button!,
    cleanup: async () => {
      await act(async () => root.unmount());
      host.remove();
    },
  };
}

async function acknowledge(
  runs: SnapshotJob[],
  knownRunId: string | null = null,
  epoch = { current: 1 },
) {
  const fixture = props(async () => runs, knownRunId, epoch);
  const screen = await renderAcknowledgment(fixture.result);
  await act(async () => screen.button.click());
  await screen.cleanup();
  return fixture.clear;
}

test("acknowledgment keeps the posting block for active, missing, and failed refreshes", async () => {
  expect(await acknowledge([run("old", "completed"), run("new", "extract")])).not.toHaveBeenCalled();
  expect(await acknowledge([run("old", "completed")], "missing")).not.toHaveBeenCalled();

  const failed = props(async () => null);
  const screen = await renderAcknowledgment(failed.result);
  await act(async () => screen.button.click());
  expect(failed.clear).not.toHaveBeenCalled();
  await screen.cleanup();
});

test("a stale response retains the block and re-enables acknowledgment", async () => {
  let resolveRuns: (runs: SnapshotJob[]) => void;
  const refresh = new Promise<SnapshotJob[]>((resolve) => {
    resolveRuns = resolve;
  });
  const epoch = { current: 1 };
  const fixture = props(async () => refresh, null, epoch);
  const screen = await renderAcknowledgment(fixture.result);

  act(() => screen.button.click());
  expect(screen.button.disabled).toBe(true);
  epoch.current = 2;
  await act(async () => resolveRuns!([]));

  expect(fixture.clear).not.toHaveBeenCalled();
  expect(screen.button.disabled).toBe(false);
  await screen.cleanup();
});

test("acknowledgment clears only after no active run, including a detached known resume", async () => {
  expect(await acknowledge([run("old", "completed")])).toHaveBeenCalledWith(false);
  expect(await acknowledge([run("resume", "extract", true)], "resume")).toHaveBeenCalledWith(false);
});

test("inspecting a terminal run leaves the active worker state untouched", async () => {
  const active = run("active", "extract");
  const terminal = run("terminal", "completed");
  const fixture = props(async () => [active, terminal]);
  fixture.result.snapshotJob = active;
  fixture.result.snapshotActive = true;
  fixture.result.selectedRecentSnapshotRuns = [active, terminal];
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);

  await act(async () => root.render(<MirrorProofScreen {...fixture.result} />));
  const inspect = [...host.querySelectorAll("button")].filter((candidate) => candidate.textContent === "Inspect").at(-1);
  expect(inspect).toBeDefined();
  await act(async () => inspect!.click());

  expect(fixture.result.setInspectedJob).toHaveBeenCalledWith(terminal);
  expect(fixture.result.snapshotJob).toBe(active);
  expect(fixture.result.snapshotSelectionVersion.current).toBe(1);
  root.unmount();
  host.remove();
});
