import type { EvidenceDrawerEntry } from "./evidence-drawer-entry";

export function isLocalEvidenceReadSuppressed(
  evidenceDrawerOpen: boolean,
  evidenceDrawerEntry: EvidenceDrawerEntry,
): boolean {
  return evidenceDrawerOpen && evidenceDrawerEntry.kind === "local-only";
}
