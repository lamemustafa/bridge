type SnapshotHandle = {
  run_id: string;
  phase: string;
  requires_resume: boolean;
};

// The coordinator marks historical, unattached workers requires_resume=true,
// and includes every tracked worker even when it falls outside durable recency.
export function recoverSnapshotJob<T extends SnapshotHandle>(
  current: T | null,
  runs: readonly T[],
  knownRunId: string | null,
): T | null {
  if (current) return runs.find((run) => run.run_id === current.run_id) ?? current;
  const active = runs.filter((run) => !run.requires_resume
    && !["completed", "partial", "failed", "cancelled"].includes(run.phase));
  if (knownRunId !== null) return active.find((run) => run.run_id === knownRunId) ?? null;
  return active.length === 1 ? active[0] : null;
}
