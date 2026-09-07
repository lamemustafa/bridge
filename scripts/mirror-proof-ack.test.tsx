// @vitest-environment jsdom
import React, { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, test, vi } from "vitest";
import { MirrorProofScreen } from "../src/MirrorProofScreen";

const run = (run_id: string, phase: string, requires_resume = false) => ({ run_id, phase, requires_resume, resume_available: requires_resume, mirror_company_id: "company", pack_id: "core_accounting", requested_from_yyyymmdd: null, requested_to_yyyymmdd: null, active_window_id: null, completed_windows: 0, total_windows: 1, verification: null, proof_id: null, proof_sha256: null, gap_codes: [], warning_codes: [], failure_code: null });

async function acknowledge(runs: unknown, knownRunId: string | null = null, epoch = { current: 1 }) {
  const clear = vi.fn();
  const host = document.createElement("div"); document.body.append(host);
  const root = createRoot(host);
  await act(async () => root.render(<MirrorProofScreen {...({ config: { host: "127.0.0.1", port: 9001 }, status: null, passport: null, tallyAction: null, selectedCompanyRecord: undefined, selectedCompanyLive: false, savedCompanyPicker: null, companyError: null, fixtureControls: null, voucherFrom: "", setVoucherFrom: vi.fn(), voucherTo: "", setVoucherTo: vi.fn(), syncEvidence: null, syncEvidenceError: null, refreshSyncEvidence: async () => {}, latestProof: undefined, mirrorTruthState: "unknown", snapshotJob: null, setSnapshotJob: vi.fn(), snapshotSelectionVersion: epoch, snapshotActive: false, snapshotError: null, snapshotStartOutcomeUnknown: true, snapshotOutcomeUnknownRunId: knownRunId, liveReadActionsLocked: false, setSnapshotStartOutcomeUnknown: clear, setSnapshotOutcomeUnknownRunId: vi.fn(), startCoreSnapshot: async () => {}, cancelCoreSnapshot: async () => {}, resumeCoreSnapshot: async () => {}, selectedRecentSnapshotRuns: [], refreshRecentSnapshots: async () => typeof runs === "function" ? runs() : runs, inspectedJob: null, activeGapCodes: [], activeWarningCodes: [], mirrorExplorer: null, mirrorExplorerError: null, loadMirrorExplorerPage: async () => {}, proofPreview: null, proofPreviewSelection: null, previewRedactedProof: async () => {}, runtimeSessions: [], runtimeError: null, refreshRuntime: async () => {}, cancelTallyRequest: async () => {} } as any)} />));
  await act(async () => { [...host.querySelectorAll("button")].find((button) => button.textContent?.includes("Refresh and confirm"))!.click(); });
  root.unmount(); host.remove(); return clear;
}

test("acknowledgment keeps the posting block for active, missing, failed-refresh, and stale selections", async () => {
  expect((await acknowledge([run("old", "completed"), run("new", "extract")]))).not.toHaveBeenCalled();
  expect((await acknowledge([run("old", "completed")], "missing"))).not.toHaveBeenCalled();
  expect((await acknowledge(null))).not.toHaveBeenCalled();
  const epoch = { current: 1 }; const staleRuns = () => { epoch.current = 2; return []; };
  expect((await acknowledge(staleRuns, null, epoch))).not.toHaveBeenCalled();
});

test("acknowledgment clears only after no active run, including a detached known resume", async () => {
  expect((await acknowledge([run("old", "completed")]))).toHaveBeenCalledWith(false);
  expect((await acknowledge([run("resume", "extract", true)], "resume"))).toHaveBeenCalledWith(false);
});
