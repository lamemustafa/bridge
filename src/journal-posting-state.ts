export type JournalReviewFlags = {
  dispatched: boolean;
  responseRecorded: boolean;
};

export type JournalActionSnapshot = {
  state: string | null;
  attemptRecorded?: boolean;
  hasError: boolean;
};

export function deriveJournalActionState(
  review: JournalReviewFlags,
  action: JournalActionSnapshot | null,
  uncertainAttempt: boolean,
) {
  const noAttemptRecorded = action?.state === "not_dispatched" || action?.attemptRecorded === false;
  const admissionRefused = action?.state === "admission_refused";
  const reconciliationRequired = Boolean(
    review.dispatched ||
      review.responseRecorded ||
      action?.state === "reconciliation_required" ||
      uncertainAttempt ||
      (action?.hasError && !noAttemptRecorded && !admissionRefused),
  );
  const verified = action?.state === "posted_verified" || action?.state === "previous_attempt_reconciled";

  return {
    reconciliationRequired,
    verified,
    canPost: !verified && !reconciliationRequired && !admissionRefused,
    canReconcile: !verified && reconciliationRequired,
    canChooseAnother: verified || !reconciliationRequired,
  };
}
