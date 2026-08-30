import type { OutstandingsEvidence } from "./OutstandingsScreen";

export type EvidenceDrawerEntry =
  | {
      kind: "local-only";
    }
  | {
      kind: "report-bound";
      evidence: OutstandingsEvidence;
    }
  | {
      kind: "report-not-read";
    }
  | {
      kind: "report-read-failed";
      message: string;
    };

export function reportEvidenceDrawerEntry(
  evidence: OutstandingsEvidence | null,
  failure: string | null,
): EvidenceDrawerEntry {
  if (evidence) return { kind: "report-bound", evidence };
  if (failure) return { kind: "report-read-failed", message: failure };
  return { kind: "report-not-read" };
}
