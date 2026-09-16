import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { type OperatorError, toOperatorError } from "./tally-command-error";
import type { TallyRuntimeSnapshot } from "./tally-mirror-contract";

// The Tally runtime sessions panel: the endpoint sessions Bridge holds and the
// cancellation of an in-flight request. No reset hub in App() clears this state.
export function useTallyRuntimeSessions() {
  const [runtimeSessions, setRuntimeSessions] = React.useState<TallyRuntimeSnapshot[]>([]);
  const [runtimeError, setRuntimeError] = React.useState<OperatorError | null>(null);

  const refreshRuntime = React.useCallback(async () => {
    try {
      const snapshots = await invoke<TallyRuntimeSnapshot[]>("tally_runtime_snapshots");
      setRuntimeSessions(snapshots);
      setRuntimeError(null);
    } catch (error) {
      setRuntimeError(toOperatorError(error));
    }
  }, []);

  async function cancelTallyRequest(requestId: string) {
    try {
      const cancelled = await invoke<boolean>("cancel_tally_request", { requestId });
      if (!cancelled) {
        setRuntimeError("The request had already completed or was not found.");
      }
    } catch (error) {
      setRuntimeError(toOperatorError(error));
    } finally {
      void refreshRuntime();
    }
  }

  return { runtimeSessions, runtimeError, refreshRuntime, cancelTallyRequest };
}
