import React from "react";
import { formatIdentifier } from "./display-format";
import { classifyTallyError } from "./tally-error-copy";

type TallyCommandErrorEnvelope = {
  code: string;
  category: string;
  message: string;
  retry: "safe" | "after_change" | "not_recommended";
  local_state_changed: boolean;
  tally_state_may_have_changed: boolean;
  remediation: string;
};

export type OperatorError = string | TallyCommandErrorEnvelope;

export function TallyErrorNotice({ message }: { message: OperatorError }) {
  const guidance = classifyTallyError(typeof message === "string" ? { message } : message);
  const displayMessage = typeof message === "string" ? message : message.message;
  return (
    <div className="error-banner" role="alert">
      <strong>{guidance.category}</strong>
      <span>{guidance.action}</span>
      <details>
        <summary>{typeof message === "string" ? "Details" : "Technical details"}</summary>
        {typeof message !== "string" && (
          <>
            <small>Code <code>{message.code}</code> · Retry {formatIdentifier(message.retry)} · Local state {message.local_state_changed ? "changed" : "unchanged"} · Tally state {message.tally_state_may_have_changed ? "may have changed" : "unchanged by this read-only action"}</small>
            <small>Next step: {message.remediation}</small>
          </>
        )}
        <small>{displayMessage}</small>
      </details>
    </div>
  );
}

function isTallyCommandErrorEnvelope(error: unknown): error is TallyCommandErrorEnvelope {
  if (!error || typeof error !== "object") return false;
  const value = error as Record<string, unknown>;
  return typeof value.code === "string"
    && typeof value.category === "string"
    && typeof value.message === "string"
    && ["safe", "after_change", "not_recommended"].includes(String(value.retry))
    && typeof value.local_state_changed === "boolean"
    && typeof value.tally_state_may_have_changed === "boolean"
    && typeof value.remediation === "string";
}

export function toOperatorError(error: unknown): OperatorError {
  if (isTallyCommandErrorEnvelope(error)) return error;
  return error instanceof Error ? error.message : String(error);
}

export function toErrorMessage(error: unknown): string {
  const normalized = toOperatorError(error);
  return typeof normalized === "string"
    ? normalized
    : `${normalized.category}: ${normalized.message} [${normalized.code}]. ${normalized.remediation}`;
}

/// A backend command error, narrowed loosely: only `message` is required.
/// This is deliberately weaker than `TallyCommandErrorEnvelope` above, which
/// demands every field. Several command families only ever send a subset --
/// `SourceDraftCommandError` (`src-tauri/src/source_draft/types.rs`) carries
/// `code`, `message` and `remediation` but never `category` or `retry`, and
/// `DesktopJournalError` carries the same three. `isTallyCommandErrorEnvelope`
/// would reject both and fall through to `String(error)`, which is why the
/// full-envelope helpers above are not reused here.
type LooseCommandErrorLike = {
  message?: unknown;
  code?: unknown;
  remediation?: unknown;
};

/// The one formatter behind every screen's command-error text (issue #471).
/// Six screens each hand-rolled this, and three of them dropped the
/// backend's `remediation` -- the text telling the operator what to do next.
/// The owner's decision (#471): always show remediation, and always show the
/// code, everywhere, once a cause narrows to `{ message: string, ... }`.
///
/// `fallback` is deliberately per-caller rather than a single canned string:
/// each screen's existing wording (or, for `TrialBalanceScreen`, its
/// `instanceof Error` rule) is kept verbatim for the causes that are not a
/// string and not an object carrying a `message`, per the issue's explicit
/// "keep each screen's existing fallback wording" requirement.
export function formatCommandErrorMessage(
  cause: unknown,
  fallback: string | ((cause: unknown) => string),
): string {
  if (cause && typeof cause === "object") {
    const value = cause as LooseCommandErrorLike;
    if (typeof value.message === "string") {
      const code = typeof value.code === "string" && value.code !== "" ? `[${value.code}]` : "";
      const remediation = typeof value.remediation === "string" && value.remediation !== "" ? value.remediation : "";
      return [value.message, code, remediation].filter(Boolean).join(" ");
    }
  }
  if (typeof cause === "string") return cause;
  return typeof fallback === "function" ? fallback(cause) : fallback;
}
