import React from "react";
import { formatIdentifier } from "./display-format";

type CapabilityEvidence = {
  state: "supported" | "unsupported" | "unknown" | "not_configured";
  confidence: "documented" | "observed" | "inferred" | "unknown";
  safe_reason_code?: string;
};

export type CapabilityProfile = {
  profile_version: number;
  product: string;
  release?: string;
  mode?: string;
  transports: Record<string, CapabilityEvidence>;
  features: Record<string, CapabilityEvidence>;
  packs: Record<string, CapabilityEvidence>;
};

export const PACK_LABELS: Record<string, string> = {
  core_accounting: "Core accounting",
  india_tax: "India tax",
  bills_and_payments: "Bills and payments",
  inventory: "Inventory",
};

const CAPABILITY_REASON_LABELS: Record<string, string> = {
  xml_export_probe_failed: "The safe XML export probe did not complete.",
  tally_status_not_recognized: "The endpoint response was not recognized as a compatible Tally status.",
  release_not_observed: "The Tally release was not observed, so this transport was not tested.",
  configuration_not_observed: "Bridge did not inspect this optional transport's configuration.",
  company_identity_invalid: "The company result contained an invalid or unsafe identity field.",
  company_identity_ambiguous: "Two or more returned companies shared the same complete observed identity.",
  company_identity_display_scope_ambiguous: "Two same-GUID books differ only by name casing or surrounding whitespace, so Tally cannot safely scope the selected book. Rename one book, then probe again.",
  direct_company_report_untrusted: "Tally returned a direct company report without the normal success wrapper. Its names remain unverified until separately checked.",
  standard_ledger_identity_profile_observed: "A strict, scoped standard ledger collection observed one local company identity. It does not establish completeness, sync eligibility, or write support.",
  scoped_standard_identity_observed: "A strict, scoped local company identity was observed. Responder authenticity and accounting completeness remain unestablished.",
  practical_limit_not_measured: "No live workload has established a practical response limit for this endpoint.",
  selected_read_probe_not_run: "This selected read was not run by the connection probe.",
  selected_ledger_read_empty_observed: "The exact selected ledger profile returned a valid empty response; source emptiness is not claimed.",
  selected_ledger_read_non_empty_observed: "The exact selected ledger profile returned validated identified rows, which were discarded.",
  selected_voucher_window_empty_observed: "The exact request-bound voucher window returned a valid empty response; source completeness is not claimed.",
  selected_voucher_window_non_empty_observed: "The exact request-bound voucher window returned validated identified rows, which were discarded.",
  qualification_prerequisite_failed: "Voucher qualification was skipped because the ledger prerequisite did not pass.",
  selected_voucher_date_outside_window: "A returned voucher fell outside the exact reviewed date window.",
  selected_read_identity_unavailable: "The selected response did not prove stable unique row identity.",
  selected_read_schema_rejected: "The selected response did not match the exact reviewed schema and structure.",
  selected_read_transport_or_validation_failed: "The selected read failed transport, decoding, or strict validation and remains unknown.",
  write_probe_not_run: "No write probe was run. Bridge never infers write support from read access.",
  verified_snapshot_not_run: "No profile-scoped capability run has established this pack's declared contract.",
};

function formatCapabilityState(state: CapabilityEvidence["state"]): string {
  switch (state) {
    case "supported":
      return "Supported";
    case "unsupported":
      return "Unsupported";
    case "not_configured":
      return "Not configured";
    default:
      return "Unknown";
  }
}

function formatConfidence(confidence: CapabilityEvidence["confidence"]): string {
  switch (confidence) {
    case "documented":
      return "Documented evidence";
    case "observed":
      return "Observed by this probe";
    case "inferred":
      return "Inferred, not directly observed";
    default:
      return "Evidence confidence unknown";
  }
}

function formatCapabilityReason(reason?: string): string {
  if (!reason) {
    return "No reason code was returned.";
  }

  return CAPABILITY_REASON_LABELS[reason] || `Reason: ${formatIdentifier(reason)}.`;
}

function CapabilityBadge({ evidence }: { evidence?: CapabilityEvidence }) {
  if (!evidence) {
    return <span className="capability-badge state-unobserved">Not observed</span>;
  }

  return (
    <span className={`capability-badge state-${evidence.state}`}>
      {formatCapabilityState(evidence.state)}
    </span>
  );
}

export function CapabilityRows({
  capabilities,
  labels,
}: {
  capabilities?: Record<string, CapabilityEvidence>;
  labels: Record<string, string>;
}) {
  const keys = Array.from(new Set([...Object.keys(labels), ...Object.keys(capabilities || {})]));

  return (
    <div className="capability-list">
      {keys.map((key) => {
        const evidence = capabilities?.[key];
        return (
          <div className="capability-row" key={key}>
            <div>
              <strong>{labels[key] || formatIdentifier(key)}</strong>
              <span>
                {evidence
                  ? `${formatConfidence(evidence.confidence)}. ${formatCapabilityReason(evidence.safe_reason_code)}`
                  : "This endpoint has not been probed in the current configuration."}
              </span>
            </div>
            <CapabilityBadge evidence={evidence} />
          </div>
        );
      })}
    </div>
  );
}
