import React from "react";
import { FileCheck2, FileText, RotateCcw, ShieldCheck } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { deriveJournalActionState } from "./journal-posting-state";

type TallyConfig = { host: string; port: number };

type JournalReview = {
  batchId: string;
  sha256: string;
  company: { name: string; guid: string; companyNumber: string; booksFrom: string };
  builtAt: string;
  dispatched: boolean;
  responseRecorded: boolean;
  preview: string;
};

type JournalActionResponse = {
  batchId: string;
  result: {
    result?: {
      dispatch?: { state?: string; resent?: boolean };
      attempt_recorded?: boolean;
      error?: { code?: string; message?: string; remediation?: string };
    };
  };
};

type Action = "pick" | "post" | "reconcile" | null;

function errorMessage(error: unknown) {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const envelope = error as { message?: unknown; remediation?: unknown };
    const message = typeof envelope.message === "string" ? envelope.message : "Journal action failed.";
    return typeof envelope.remediation === "string" ? `${message} ${envelope.remediation}` : message;
  }
  return "Journal action failed.";
}

function outcomeOf(action: JournalActionResponse | null) {
  return action?.result?.result?.dispatch?.state ?? null;
}

function actionErrorOf(action: JournalActionResponse | null) {
  return action?.result?.result?.error?.message ?? null;
}

export function JournalPostingScreen({ config }: { config: TallyConfig }) {
  const [review, setReview] = React.useState<JournalReview | null>(null);
  const [actionResult, setActionResult] = React.useState<JournalActionResponse | null>(null);
  const [action, setAction] = React.useState<Action>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [reviewConfig, setReviewConfig] = React.useState<TallyConfig | null>(null);
  const [uncertainAttempt, setUncertainAttempt] = React.useState(false);
  const actionRef = React.useRef<Action>(null);

  async function chooseJournal() {
    if (actionRef.current !== null) return;
    actionRef.current = "pick";
    setAction("pick");
    setError(null);
    setActionResult(null);
    setUncertainAttempt(false);
    try {
      const selected = await invoke<JournalReview | null>("desktop_pick_journal_for_review", { config });
      if (selected) {
        setReview(selected);
        setReviewConfig(config);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      setAction(null);
    }
  }

  async function runAction(kind: "post" | "reconcile") {
    if (!review || actionRef.current !== null) return;
    const requestedConfig = reviewConfig ?? config;
    actionRef.current = kind;
    setAction(kind);
    setError(null);
    setActionResult(null);
    try {
      const command = kind === "post" ? "desktop_post_reviewed_journal" : "desktop_reconcile_reviewed_journal";
      const result = await invoke<JournalActionResponse>(command, {
        request: {
          batchId: review.batchId,
          sha256: review.sha256,
          companyGuid: review.company.guid,
          config: requestedConfig,
        },
      });
      setActionResult(result);
      setUncertainAttempt(false);
    } catch (cause) {
      setError(errorMessage(cause));
      if (kind === "post") setUncertainAttempt(true);
    } finally {
      actionRef.current = null;
      setAction(null);
    }
  }

  const outcome = outcomeOf(actionResult);
  const journalState = deriveJournalActionState(
    { dispatched: Boolean(review?.dispatched), responseRecorded: Boolean(review?.responseRecorded) },
    actionResult
      ? {
          state: outcome,
          attemptRecorded: actionResult.result?.result?.attempt_recorded,
          hasError: Boolean(actionErrorOf(actionResult)),
        }
      : null,
    uncertainAttempt,
  );
  const { reconciliationRequired, verified } = journalState;

  return (
    <section className="panel wide journal-review" aria-labelledby="journal-review-heading" aria-busy={action !== null}>
      <div className="panel-heading">
        <div>
          <h2 id="journal-review-heading">Review a Bridge Journal</h2>
          <p className="panel-description">Choose the original XML file Bridge generated. Bridge checks it against the saved local batch before showing one Journal for review.</p>
        </div>
        <FileText size={24} aria-hidden="true" />
      </div>

      {error && <div className="error-banner" role="alert"><strong>Journal action failed</strong><span>{error}</span></div>}

      {!review ? (
        <div className="journal-empty-state">
          <ShieldCheck size={28} aria-hidden="true" />
          <p>No Journal is open for review.</p>
          <button className="primary" type="button" onClick={() => void chooseJournal()} disabled={action !== null}>
            <FileText size={18} aria-hidden="true" />
            {action === "pick" ? "Opening file picker…" : "Choose Journal file"}
          </button>
        </div>
      ) : (
        <>
          <dl className="journal-review-details">
            <div><dt>Company</dt><dd>{review.company.name}</dd></div>
            <div><dt>Company number</dt><dd>{review.company.companyNumber}</dd></div>
            <div><dt>Books from</dt><dd>{review.company.booksFrom}</dd></div>
            <div><dt>Saved batch</dt><dd>{review.batchId}</dd></div>
            <div><dt>Review endpoint</dt><dd>{reviewConfig?.host}:{reviewConfig?.port}</dd></div>
          </dl>
          <div className="journal-preview" aria-label="Journal details">
            <h3>Journal details</h3>
            <pre>{review.preview}</pre>
          </div>
          {verified && <p className="journal-status" role="status"><FileCheck2 size={18} aria-hidden="true" /> Bridge confirmed the original Journal and its saved batch.</p>}
          {reconciliationRequired && !verified && <p className="journal-status journal-status-warning" role="alert">The original batch needs reconciliation. Bridge will use this same review and will not rebuild or resend it.</p>}
          {actionErrorOf(actionResult) && <p className="journal-status journal-status-warning" role="alert">{actionErrorOf(actionResult)}</p>}
          {!verified && !reconciliationRequired && <p className="journal-action-note">Review the approval dialog; Bridge then checks and posts this saved batch.</p>}
          <div className="journal-actions">
            {journalState.canReconcile ? (
              <button className="primary" type="button" onClick={() => void runAction("reconcile")} disabled={action !== null}>
                <RotateCcw size={18} aria-hidden="true" />
                {action === "reconcile" ? "Reconciling original batch…" : "Reconcile original batch"}
              </button>
            ) : journalState.canPost ? (
              <button className="primary" type="button" onClick={() => void runAction("post")} disabled={action !== null}>
                <ShieldCheck size={18} aria-hidden="true" />
                {action === "post" ? "Review approval dialog…" : "Post Journal"}
              </button>
            ) : null}
            {journalState.canChooseAnother && <button className="secondary-action" type="button" onClick={() => void chooseJournal()} disabled={action !== null}>Choose another file</button>}
          </div>
        </>
      )}
    </section>
  );
}
