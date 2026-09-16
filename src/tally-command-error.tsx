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
