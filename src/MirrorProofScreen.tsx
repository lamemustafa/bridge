import React from "react";
import { CircleHelp, Database, FileText, Play, RefreshCw } from "lucide-react";
import { classifyTallyError } from "./tally-error-copy";

// Owns: the presentational markup of the Accounting mirror and proof view
// (view === "mirror") -- the saved-company picker, the truth-state hero,
// the requested-period controls, the Gap Map, the local mirror explorer,
// the hash-linked proof ledger and its redacted-preview export, the
// synthetic write-fixture safety gate, and the Tally runtime/queue panel.
//
// Deliberately does NOT own any of the state or handlers behind that
// markup. Every one of them turned out to be needed outside this view too,
// so they all stay in App() and are passed down read-only (plus the
// handful of setters/callbacks this view triggers):
//
//   - `syncEvidence`, `snapshotJob`, and `recentSnapshotRuns` feed the
//     "Verified baseline" / "Latest attempt" operator-summary strip, which
//     App() also renders while `view === "dashboard"` -- not just here.
//   - App() runs a background effect that keeps polling an active
//     snapshot run's status (and refreshes sync evidence when it finishes)
//     regardless of which view is on screen, so a running read is not lost
//     by navigating away. That effect needs `snapshotJob`,
//     `refreshSyncEvidence`, and `refreshRecentSnapshots` to live in App().
//   - `snapshotJob`/`snapshotActive`/`snapshotStartOutcomeUnknown` also
//     drive `savedCompanySelectionLocked` in App(), which gates company
//     selection everywhere (including the Connect Tally view) because
//     changing the selected company while a snapshot could still be
//     mutating that scope is unsafe. That guard's invariant is unchanged
//     by this extraction: the state it reads still lives in App().
//   - `proofPreview`, `mirrorExplorer`, and the write-fixture attestations
//     are cleared by App()'s `clearSelectedCompanyScope`, which runs from
//     several other places (probing, bootstrapping a company, switching
//     the saved company) that are not guaranteed to happen while this view
//     is unmounted -- so that state has to stay where the clearing code
//     already lives.
//   - `runtimeSessions`/`refreshRuntime` are shared with the endpoint
//     probe, discovery, and bootstrap handlers on other views, and with a
//     separate App()-level poll keyed on `tallyAction`/`snapshotActive`.
//
// Because of that, this file receives its data and every handler as props
// instead of owning local state -- a deliberately "thin" extraction per the
// task's own fallback: presentational markup only, state and behaviour left
// exactly where they were.

type TallyConfig = {
  host: string;
  port: number;
};

type ConnectionStatus = {
  reachable: boolean;
  compatible: boolean;
  server_text: string;
  product: "TallyPrime" | "Tally ERP 9" | "Unknown";
  error?: string;
};

type TallyCompany = {
  name: string;
  guid?: string;
  company_number?: string;
  books_from_yyyymmdd?: string;
  guid_observed?: boolean;
  mirror_company_id?: string;
  correlation_key?: string;
};

type TallyCommandErrorEnvelope = {
  code: string;
  category: string;
  message: string;
  retry: "safe" | "after_change" | "not_recommended";
  local_state_changed: boolean;
  tally_state_may_have_changed: boolean;
  remediation: string;
};

type OperatorError = string | TallyCommandErrorEnvelope;

type CapabilityEvidence = {
  state: "supported" | "unsupported" | "unknown" | "not_configured";
  confidence: "documented" | "observed" | "inferred" | "unknown";
  safe_reason_code?: string;
};

type CapabilityProfile = {
  profile_version: number;
  product: string;
  release?: string;
  mode?: string;
  transports: Record<string, CapabilityEvidence>;
  features: Record<string, CapabilityEvidence>;
  packs: Record<string, CapabilityEvidence>;
};

type TallyProofSummary = {
  integrity_state: "entry_hash_valid";
  run_id: string;
  selection_token: string;
  proof_sha256: string;
  pack_id: string;
  outcome: "completed" | "failed" | "cancelled" | "outcome_unknown";
  verification_state: "verified" | "partial" | "unverified";
  started_at_unix_ms: number;
  completed_at_unix_ms?: number;
  accepted_records: number;
  rejected_records: number;
  provenance_unavailable_records: number;
  gap_codes: string[];
  warning_codes: string[];
};

type TallySyncEvidence = {
  latest_proofs: TallyProofSummary[];
  latest_reconciliation_mismatches: Array<{
    reason_code: string;
    record_aliases: string[];
  }>;
  incremental: {
    execution_enabled: boolean;
    establishment_receipts: number;
    active_checkpoint_heads: number;
    state: string;
  };
  core_accounting_freshness: {
    state: "fresh" | "stale" | "never_verified";
    verified_at_unix_ms?: number;
    checkpoint_present: boolean;
    proof_present: boolean;
  };
};

type RedactedProofPreview = {
  json: string;
  payload_sha256: string;
};

type MirrorExplorerPage = {
  offset: number;
  limit: number;
  total_records: number;
  records: Array<{
    local_alias: string;
    object_type: string;
    identity_confidence: string;
    last_batch_state: string;
    tombstoned: boolean;
  }>;
};

type SnapshotPhase = "prepare" | "capability_check" | "company_identity_check" | "plan_windows" | "extract" | "normalize" | "validate" | "stage" | "reconcile" | "commit_pending" | "emit_proof" | "completed" | "partial" | "failed" | "cancelled";

type SnapshotJobStatus = {
  run_id: string;
  mirror_company_id: string | null;
  pack_id: string | null;
  requested_from_yyyymmdd: string | null;
  requested_to_yyyymmdd: string | null;
  phase: SnapshotPhase;
  active_window_id: string | null;
  completed_windows: number;
  total_windows: number;
  verification: "verified" | "partial" | "unverified" | null;
  proof_id: string | null;
  proof_sha256: string | null;
  gap_codes: string[];
  warning_codes: string[];
  failure_code: string | null;
  requires_resume: boolean;
  resume_available: boolean;
};

type TallyRuntimeSnapshot = {
  session_id: string;
  canonical_endpoint: string;
  issued_requests: number;
  active_requests: number;
  active_request_ids: string[];
  consecutive_failures: number;
  circuit_state: "closed" | "open" | "half_open";
  circuit_retry_after_unix_ms?: number;
  last_success_unix_ms?: number;
  last_failure_unix_ms?: number;
  cached_capability_observed_at_unix_ms?: number;
};

type TallyAction = "probe" | "discover" | "bootstrap" | "save" | "fixture_enroll" | "fixture_revoke" | "evidence" | "explorer" | "start" | "resume" | "cancel";

const PACK_LABELS: Record<string, string> = {
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

function formatIdentifier(value: string): string {
  const words = value.replace(/_/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function formatRuntimeTime(value?: number): string {
  if (value === undefined || !Number.isFinite(value)) {
    return "Not observed";
  }
  return new Date(value).toLocaleString();
}

function formatDuration(startedAt: number, completedAt?: number): string {
  if (!Number.isFinite(startedAt) || completedAt === undefined || completedAt < startedAt) return "Duration unavailable";
  const seconds = Math.round((completedAt - startedAt) / 1000);
  return `Duration ${seconds}s`;
}

function formatTallyDate(value?: string): string {
  if (!value || value.length !== 8) {
    return value || "-";
  }

  return `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6, 8)}`;
}

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

function CapabilityRows({
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

function TallyErrorNotice({ message }: { message: OperatorError }) {
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

function CopyTokenButton({ value, label }: { value: string; label: string }) {
  const [copyState, setCopyState] = React.useState<"idle" | "copied" | "failed">("idle");
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopyState("copied");
      window.setTimeout(() => setCopyState("idle"), 1500);
    } catch {
      setCopyState("failed");
    }
  }
  return (
    <span className="copy-control">
      <button className="copy-token" type="button" onClick={() => void copy()} aria-label={`Copy ${label}`}>
        {copyState === "copied" ? "Copied" : "Copy"}
      </button>
      <span className={`copy-status ${copyState === "failed" ? "copy-failed" : ""}`} role="status" aria-live="polite">
        {copyState === "failed" ? `Copy failed; select the ${label} text manually.` : copyState === "copied" ? `${label} copied.` : ""}
      </span>
      {copyState === "failed" && (
        <input
          className="copy-fallback"
          aria-label={`Selectable full ${label}`}
          readOnly
          value={value}
          onFocus={(event) => event.currentTarget.select()}
        />
      )}
    </span>
  );
}

type GapGuidance = {
  title: string;
  action: string;
  retry: "after_change" | "not_useful";
};

const GAP_GUIDANCE: Record<string, GapGuidance> = {
  source_cut_atomicity_unavailable: {
    title: "Atomic source cut is unavailable",
    action: "No operator action can close this gap in the current Tally profile. The run may still be useful, but it must remain Partial.",
    retry: "not_useful",
  },
  period_report_profile_unobserved: {
    title: "Ledger-balance profile is not validated",
    action: "Validate the exact release, mode, report configuration, scenario, optional-voucher behavior, and receipt/delivery-note tracking effects with a synthetic company before enabling this custom cross-view.",
    retry: "not_useful",
  },
  voucher_header_entry_total_unavailable: {
    title: "Voucher header totals are unavailable",
    action: "Do not infer header totals from balanced entries. Extend the capability pack and validate the source fields first.",
    retry: "not_useful",
  },
  voucher_entry_applicability_unavailable: {
    title: "Voucher applicability is incomplete",
    action: "Classify the voucher type and its book-effect semantics before treating missing entries as an error.",
    retry: "not_useful",
  },
  record_provenance_unavailable: {
    title: "Raw-record provenance is unavailable",
    action: "Use a connector path that binds each canonical record to a source-fragment hash, then run a new evidence read.",
    retry: "after_change",
  },
  report_tie_out_unavailable: {
    title: "Ledger-balance cross-view did not complete",
    action: "Check that Tally is responsive and the custom read-only report is supported, then run a new evidence read.",
    retry: "after_change",
  },
  capability_profile_changed_during_run: {
    title: "Capability profile changed during the run",
    action: "Stabilize the Tally release, mode, loaded company, and endpoint configuration before retrying.",
    retry: "after_change",
  },
  source_changed_during_run: {
    title: "Source data changed during the run",
    action: "Run again during a controlled quiet period. A stable reread still does not prove atomic isolation.",
    retry: "after_change",
  },
  minimum_window_response_too_large: {
    title: "One Tally day exceeds the bounded response limit",
    action: "Bridge cannot split below one calendar day. Reduce that day's source density or use a future qualified collection filter before starting a new run; retrying unchanged will fail again.",
    retry: "after_change",
  },
  adaptive_window_limit_reached: {
    title: "Adaptive window safety limit reached",
    action: "Start a new run for a shorter requested period. Bridge stopped before growing the durable split graph beyond its reviewed bound.",
    retry: "after_change",
  },
};

function guidanceForGap(code: string): GapGuidance {
  return GAP_GUIDANCE[code] ?? {
    title: formatIdentifier(code),
    action: "Inspect the local Proof of Sync and support artifact. Do not retry unchanged until this gap's cause is understood.",
    retry: "not_useful",
  };
}

function GapMap({ codes, available }: { codes: string[]; available: boolean }) {
  const uniqueCodes = Array.from(new Set(codes)).sort();
  return (
    <div className="gap-map">
      {!available ? (
        <div className="empty-state compact">
          <strong>No inspected attempt; Gap Map unavailable</strong>
          <span>Load evidence or inspect a durable run before interpreting gaps.</span>
        </div>
      ) : uniqueCodes.length === 0 ? (
        <div className="empty-state compact">
          <strong>No declared gaps in this attempt</strong>
          <span>This does not establish accuracy unless the attempt is explicitly Verified.</span>
        </div>
      ) : uniqueCodes.map((code) => {
        const guidance = guidanceForGap(code);
        return (
          <article className="gap-item" key={code}>
            <div>
              <strong>{guidance.title}</strong>
              <code>{code}</code>
            </div>
            <p>{guidance.action}</p>
            <span className={`retry-guidance retry-${guidance.retry}`}>
              {guidance.retry === "after_change" ? "Retry only after the stated change" : "Retrying unchanged is not useful"}
            </span>
          </article>
        );
      })}
    </div>
  );
}

type Props = {
  // Cross-view Tally connection/session state -- also read by the
  // dashboard, the company-context bar, and the Connect Tally view.
  config: TallyConfig;
  status: ConnectionStatus | null;
  passport: CapabilityProfile | null;
  tallyAction: TallyAction | null;
  selectedCompanyRecord: TallyCompany | undefined;
  selectedCompanyLive: boolean;
  // The "Choose/change a saved company" panel, rendered by App() and
  // passed down as a slot. `scripts/tally-setup-safety.test.mjs` asserts
  // (against the full src/main.tsx text) on this panel's copy and on the
  // exact `onClick`/`disabled` expressions of its buttons, so this markup
  // -- and `savedCompanyList`/`savedCompanySelectionLocked`/
  // `selectSavedCompany` behind it -- must stay in main.tsx. See the file
  // header.
  savedCompanyPicker: React.ReactNode;
  companyError: OperatorError | null;
  // The entire "Synthetic write fixture (advanced)" <details> panel,
  // rendered by App() and passed down as a slot rather than owned here.
  // `scripts/tally-setup-safety.test.mjs` asserts (against the full
  // src/main.tsx text) that its heading and its enroll/revoke handlers'
  // exact `onClick` expressions are present in that file, so this markup
  // -- and the write-fixture state and handlers behind it -- must stay in
  // main.tsx rather than move here. See the file header.
  fixtureControls: React.ReactNode;

  // Requested accounting period. `startCoreSnapshot` (owned by App(), see
  // below) reads it too, so it is not duplicated as view-local state.
  voucherFrom: string;
  setVoucherFrom: (value: string) => void;
  voucherTo: string;
  setVoucherTo: (value: string) => void;

  // Sync evidence, Core Accounting snapshot runs, the mirror explorer, and
  // the redacted proof preview -- all owned by App() (see the file header).
  syncEvidence: TallySyncEvidence | null;
  syncEvidenceError: OperatorError | null;
  refreshSyncEvidence: (announce?: boolean) => Promise<void>;
  latestProof: TallyProofSummary | undefined;
  mirrorTruthState: string;

  snapshotJob: SnapshotJobStatus | null;
  setSnapshotJob: (job: SnapshotJobStatus) => void;
  snapshotSelectionVersion: React.MutableRefObject<number>;
  snapshotActive: boolean;
  snapshotError: OperatorError | null;
  snapshotStartOutcomeUnknown: boolean;
  setSnapshotStartOutcomeUnknown: (value: boolean) => void;
  startCoreSnapshot: () => Promise<void>;
  cancelCoreSnapshot: () => Promise<void>;
  resumeCoreSnapshot: (runId: string) => Promise<void>;
  selectedRecentSnapshotRuns: SnapshotJobStatus[];
  refreshRecentSnapshots: () => Promise<void>;
  inspectedJob: SnapshotJobStatus | null;
  activeGapCodes: string[];
  activeWarningCodes: string[];

  mirrorExplorer: MirrorExplorerPage | null;
  mirrorExplorerError: OperatorError | null;
  loadMirrorExplorerPage: (offset: number) => Promise<void>;

  proofPreview: RedactedProofPreview | null;
  proofPreviewSelection: { proofId: string; runId: string } | null;
  previewRedactedProof: (proof: TallyProofSummary) => Promise<void>;

  // Shared per-endpoint runtime/queue evidence -- also updated by the
  // probe/discover/bootstrap handlers on other views and by a separate
  // App()-level poll keyed on `tallyAction`/`snapshotActive`.
  runtimeSessions: TallyRuntimeSnapshot[];
  runtimeError: OperatorError | null;
  refreshRuntime: () => Promise<void>;
  cancelTallyRequest: (requestId: string) => Promise<void>;
};

export function MirrorProofScreen({
  config,
  status,
  passport,
  tallyAction,
  selectedCompanyRecord,
  selectedCompanyLive,
  savedCompanyPicker,
  companyError,
  fixtureControls,
  voucherFrom,
  setVoucherFrom,
  voucherTo,
  setVoucherTo,
  syncEvidence,
  syncEvidenceError,
  refreshSyncEvidence,
  latestProof,
  mirrorTruthState,
  snapshotJob,
  setSnapshotJob,
  snapshotSelectionVersion,
  snapshotActive,
  snapshotError,
  snapshotStartOutcomeUnknown,
  setSnapshotStartOutcomeUnknown,
  startCoreSnapshot,
  cancelCoreSnapshot,
  resumeCoreSnapshot,
  selectedRecentSnapshotRuns,
  refreshRecentSnapshots,
  inspectedJob,
  activeGapCodes,
  activeWarningCodes,
  mirrorExplorer,
  mirrorExplorerError,
  loadMirrorExplorerPage,
  proofPreview,
  proofPreviewSelection,
  previewRedactedProof,
  runtimeSessions,
  runtimeError,
  refreshRuntime,
  cancelTallyRequest,
}: Props) {
  return (
    <>
      {savedCompanyPicker}
      <article className="panel wide mirror-hero">
        <div>
          <p className="eyebrow">Truth state</p>
          <h2>{latestProof ? `${formatIdentifier(latestProof.outcome)} · ${formatIdentifier(latestProof.verification_state)} ${formatIdentifier(latestProof.pack_id)} attempt` : "No durable Core Accounting run receipt yet"}</h2>
          <p>
            {latestProof
              ? `Within this run's declared Core Accounting scope, Bridge persisted ${latestProof.accepted_records} provenance-backed accepted canonical rows, ${latestProof.provenance_unavailable_records} canonical rows with an explicit provenance-unavailable gap, and ${latestProof.rejected_records} rejected rows. These are not Tally source-total counts. ${latestProof.gap_codes.length} declared gap(s) and ${latestProof.warning_codes.length} warning(s).`
              : "Endpoint reachability and fetched preview rows do not establish a Verified accounting state."}
          </p>
        </div>
        <div>
          <span className={`truth-state state-${mirrorTruthState === "verified" ? "supported" : "unknown"}`}>
            <CircleHelp size={18} /> {formatIdentifier(mirrorTruthState)}
          </span>
          <div className="snapshot-scope" aria-label="Requested accounting period">
            <label>From<input disabled={tallyAction !== null || snapshotActive} type="date" value={voucherFrom} onChange={(event) => setVoucherFrom(event.target.value)} /></label>
            <label>To<input disabled={tallyAction !== null || snapshotActive} type="date" value={voucherTo} onChange={(event) => setVoucherTo(event.target.value)} /></label>
          </div>
          {snapshotJob?.requested_from_yyyymmdd && snapshotJob.requested_to_yyyymmdd && (
            <span className="section-note">
              Selected run period: {formatTallyDate(snapshotJob.requested_from_yyyymmdd)} to {formatTallyDate(snapshotJob.requested_to_yyyymmdd)}
            </span>
          )}
          <button className="secondary-action" onClick={() => void refreshSyncEvidence(true)} disabled={!selectedCompanyRecord?.mirror_company_id || tallyAction !== null}>
            <RefreshCw size={16} /> {tallyAction === "evidence" ? "Refreshing..." : "Refresh evidence"}
          </button>
          <button className="secondary-action" onClick={() => void startCoreSnapshot()} disabled={!selectedCompanyRecord?.mirror_company_id || !selectedCompanyLive || snapshotActive || snapshotStartOutcomeUnknown || tallyAction !== null}>
            <Play size={16} /> {tallyAction === "start" ? "Starting..." : "Run read-only Core Accounting evidence read"}
          </button>
          {snapshotJob?.resume_available && (
            <button className="secondary-action" onClick={() => void resumeCoreSnapshot(snapshotJob.run_id)} disabled={tallyAction !== null}>
              <Play size={16} /> {tallyAction === "resume" ? "Resuming..." : "Resume interrupted run"}
            </button>
          )}
          {snapshotActive && (
            <button className="secondary-action" onClick={() => void cancelCoreSnapshot()} disabled={tallyAction !== null}>{tallyAction === "cancel" ? "Cancelling..." : "Cancel active run"}</button>
          )}
        </div>
      </article>
      <p className="section-note">Reads Bridge's declared Core Accounting v3 scope for this period. It is not a native Trial Balance, a complete-books guarantee, or an atomic Tally snapshot.</p>

      {syncEvidenceError && <TallyErrorNotice message={syncEvidenceError} />}
      {snapshotError && <TallyErrorNotice message={snapshotError} />}
      {companyError && <TallyErrorNotice message={companyError} />}
      {snapshotStartOutcomeUnknown && (
        <section className="status-strip" role="alert">
          <span>A previous start outcome is unknown. Inspect the refreshed durable runs before allowing another start.</span>
          <button className="secondary-action" type="button" onClick={() => setSnapshotStartOutcomeUnknown(false)}>I reviewed the runs; allow a new start</button>
        </section>
      )}

      {snapshotJob && (
        <section className="status-strip" role="status" aria-live="polite">
          <span className="run-token">Run <code>{snapshotJob.run_id}</code> <CopyTokenButton value={snapshotJob.run_id} label="run ID" /></span>
          <span>Phase: {formatIdentifier(snapshotJob.phase)}</span>
          <span>Completed executable windows: {snapshotJob.completed_windows}/{snapshotJob.total_windows}</span>
          <span>{snapshotJob.verification ? `Result: ${formatIdentifier(snapshotJob.verification)}` : "No verification claim yet"}</span>
          {snapshotJob.failure_code && <span>Failure: {formatIdentifier(snapshotJob.failure_code)}</span>}
          {snapshotJob.requires_resume && (
            <span>{snapshotJob.resume_available ? "Worker detached: explicit resume required" : "Detached legacy state: inspect only"}</span>
          )}
        </section>
      )}

      {selectedRecentSnapshotRuns.length > 0 && (
        <article className="panel wide">
          <div className="panel-heading">
            <div>
              <h2>Recent durable Core Accounting runs</h2>
              <p className="panel-description">Recovery status comes from hash-checked encrypted state, including runs discovered after an app restart.</p>
            </div>
            <button className="secondary-action" onClick={() => void refreshRecentSnapshots()}>
              <RefreshCw size={16} /> Refresh runs
            </button>
          </div>
          <div className="table-wrap" role="region" aria-label="Recent durable Core Accounting runs" tabIndex={0}>
            <table>
              <caption>Showing up to 10 of {selectedRecentSnapshotRuns.length} loaded runs for {selectedCompanyRecord?.name}</caption>
              <thead><tr><th>Run</th><th>Pack</th><th>Phase</th><th>Executable windows</th><th>Worker</th><th>Action</th></tr></thead>
              <tbody>
                {selectedRecentSnapshotRuns.slice(0, 10).map((run) => (
                  <tr key={run.run_id}>
                    <td><code>{run.run_id}</code> <CopyTokenButton value={run.run_id} label="run ID" /></td>
                    <td>{formatIdentifier(run.pack_id ?? "unknown")}</td>
                    <td>{formatIdentifier(run.phase)}</td>
                    <td>{run.completed_windows}/{run.total_windows}</td>
                    <td>{run.resume_available ? "Resume available" : run.requires_resume ? "Inspect only" : run.phase === "completed" || run.phase === "partial" || run.phase === "failed" || run.phase === "cancelled" ? "Terminal" : "Active"}</td>
                    <td><button className="secondary-action" disabled={tallyAction !== null} onClick={() => { snapshotSelectionVersion.current += 1; setSnapshotJob(run); setSnapshotStartOutcomeUnknown(false); }}>Inspect</button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </article>
      )}

      <section className="truth-grid">
        <article className="truth-card">
          <span>Endpoint evidence</span>
          <strong>{status ? (status.compatible ? "Compatible status observed" : status.reachable ? "Reachable; compatibility unknown" : "Not reachable") : "Not checked"}</strong>
          <small>{status ? `${config.host}:${config.port}` : "Run Check Tally Endpoint to collect a current probe."}</small>
        </article>
        <article className="truth-card">
          <span>Company pin</span>
          <strong>{selectedCompanyRecord?.mirror_company_id ? "Observed GUID persisted" : "Not established"}</strong>
          <small>{selectedCompanyRecord?.guid || selectedCompanyRecord?.guid_observed ? "GUID value is stored locally and hidden in this view." : "Select and probe a GUID-bearing company."}</small>
        </article>
        <article className="truth-card">
          <span>Last verified</span>
          <strong>{formatRuntimeTime(syncEvidence?.core_accounting_freshness.verified_at_unix_ms)}</strong>
          <small>{syncEvidence ? formatIdentifier(syncEvidence.core_accounting_freshness.state) : "Evidence not loaded"}</small>
        </article>
        <article className="truth-card">
          <span>Local verified checkpoint</span>
          <strong>{syncEvidence?.core_accounting_freshness.checkpoint_present ? "Bridge receipt committed" : "None"}</strong>
          <small>{syncEvidence?.core_accounting_freshness.proof_present ? "Bridge committed this local receipt atomically; it is not a Tally source watermark or source-isolation guarantee." : "Partial and failed runs never advance freshness."}</small>
        </article>
        <article className="truth-card">
          <span>Incremental execution</span>
          <strong>{syncEvidence?.incremental.execution_enabled ? "Enabled" : "Incremental disabled; use a new full planned read"}</strong>
          <small>
            {syncEvidence
              ? `${formatIdentifier(syncEvidence.incremental.state)} · ${syncEvidence.incremental.establishment_receipts} receipt(s), ${syncEvidence.incremental.active_checkpoint_heads} head(s)`
              : "No exact-scope incremental evidence loaded. A full planned read does not imply source completeness or atomicity."}
          </small>
        </article>
      </section>

      {fixtureControls}

      <article className="panel wide gap-panel">
        <div className="panel-heading">
          <div>
            <h2>Gap Map</h2>
            <p className="panel-description">Declared limits for the inspected attempt, with remediation and retry guidance. An empty map is not a Verified claim.</p>
          </div>
          <span>{activeGapCodes.length} gap{activeGapCodes.length === 1 ? "" : "s"}</span>
        </div>
        <GapMap codes={activeGapCodes} available={!!inspectedJob || !!latestProof} />
        {inspectedJob && <p className="section-note">Gap Map scope: inspected run <code>{inspectedJob.run_id}</code>. This does not replace the separate latest-attempt summary.</p>}
        {activeWarningCodes.length > 0 && (
          <div className="warning-list">
            <strong>Warnings</strong>
            <ul>{activeWarningCodes.map((code) => <li key={code}><code>{code}</code> — {formatIdentifier(code)}</li>)}</ul>
          </div>
        )}
      </article>

      <article className="panel wide mirror-explorer">
        <div className="panel-heading">
          <div>
            <h2>Local mirror explorer</h2>
            <p className="panel-description">Paged, privacy-preserving metadata for the selected company and Core Accounting pack. Names, amounts, source IDs, and payloads are not returned to this view.</p>
            <p className="section-note">Totals describe rows currently held in Bridge's local mirror for the selected pack/run state. They are not Tally source counts and may reflect a Partial attempt. Aliases are page-local and may shift after later runs.</p>
          </div>
          <button className="secondary-action" onClick={() => void loadMirrorExplorerPage(0)} disabled={!selectedCompanyRecord?.mirror_company_id || tallyAction !== null}>
            <Database size={16} /> {tallyAction === "explorer" ? "Loading..." : "Load mirror page"}
          </button>
        </div>
        {mirrorExplorerError && <TallyErrorNotice message={mirrorExplorerError} />}
        {!mirrorExplorer ? (
          <div className="empty-state compact"><strong>Mirror page not loaded</strong><span>This local read does not contact Tally and remains available for persisted company pins.</span></div>
        ) : mirrorExplorer.records.length === 0 ? (
          <div className="empty-state compact"><strong>No local mirror rows in this selected pack scope</strong><span>The local query completed for this company and pack. This says nothing about records outside that scope.</span></div>
        ) : (
          <>
            <div className="table-wrap" role="region" aria-label="Paged local mirror records" tabIndex={0}>
              <table>
                <caption>Showing {mirrorExplorer.offset + 1}-{Math.min(mirrorExplorer.offset + mirrorExplorer.records.length, mirrorExplorer.total_records)} of {mirrorExplorer.total_records} local records. Absence on this page is not absence from the mirror.</caption>
                <thead><tr><th>Local alias</th><th>Object</th><th>Identity confidence</th><th>Last batch</th><th>Lifecycle</th></tr></thead>
                <tbody>{mirrorExplorer.records.map((record) => (
                  <tr key={record.local_alias}>
                    <td>{record.local_alias}</td>
                    <td>{formatIdentifier(record.object_type)}</td>
                    <td>{formatIdentifier(record.identity_confidence)}</td>
                    <td>{formatIdentifier(record.last_batch_state)}</td>
                    <td>{record.tombstoned ? "Tombstoned" : "Present in local mirror"}</td>
                  </tr>
                ))}</tbody>
              </table>
            </div>
            <div className="pagination" aria-label="Mirror explorer pagination">
              <button className="secondary-action" disabled={mirrorExplorer.offset === 0 || tallyAction !== null} onClick={() => void loadMirrorExplorerPage(Math.max(0, mirrorExplorer.offset - mirrorExplorer.limit))}>Previous page</button>
              <span>Page {Math.floor(mirrorExplorer.offset / mirrorExplorer.limit) + 1}</span>
              <button className="secondary-action" disabled={mirrorExplorer.offset + mirrorExplorer.records.length >= mirrorExplorer.total_records || tallyAction !== null} onClick={() => void loadMirrorExplorerPage(mirrorExplorer.offset + mirrorExplorer.limit)}>Next page</button>
            </div>
          </>
        )}
      </article>

      <article className="panel wide">
        <div className="panel-heading">
          <div>
            <h2>Hash-linked local proof ledger</h2>
            <p className="panel-description">Append-only under Bridge's local controls. Hash checks detect inconsistency; this is not a signature, a tamper-proof audit log, or proof that the responder was genuine Tally.</p>
          </div>
          <span>Latest {syncEvidence?.latest_proofs.length ?? 0} loaded · 20-row API limit</span>
        </div>
        {!latestProof ? (
          <div className="empty-state compact">
            <strong>No proof entries for this company</strong>
            <span>A production Core Accounting attempt will append its outcome, gaps, returned-row counts, and local proof hash here.</span>
          </div>
        ) : (
          <div className="table-wrap" role="region" aria-label="Hash-linked local proof ledger" tabIndex={0}>
            <table>
              <caption>Loaded Proof of Sync attempt summaries; accepted/rejected values are returned run-scope rows, not source-completeness counts; older history may not be loaded</caption>
              <thead><tr><th>Completed</th><th>Run</th><th>Pack</th><th>Result</th><th>Accepted / rejected returned rows</th><th>Proof hash</th><th>Gaps</th><th>Warnings</th><th>Support export</th></tr></thead>
              <tbody>
                {syncEvidence?.latest_proofs.map((proof) => (
                  <tr key={proof.selection_token}>
                    <td>{formatRuntimeTime(proof.completed_at_unix_ms)}<small>{formatDuration(proof.started_at_unix_ms, proof.completed_at_unix_ms)}</small></td>
                    <td><code>{proof.run_id}</code> <CopyTokenButton value={proof.run_id} label="run ID" /></td>
                    <td>{formatIdentifier(proof.pack_id)}</td>
                    <td>{formatIdentifier(proof.outcome)} · {formatIdentifier(proof.verification_state)} · Local hash check: {proof.integrity_state === "entry_hash_valid" ? "passed" : formatIdentifier(proof.integrity_state)}</td>
                    <td>{proof.accepted_records} / {proof.rejected_records}</td>
                    <td><code title="Local consistency commitment; not authenticity">{proof.proof_sha256.slice(0, 12)}...</code> <CopyTokenButton value={proof.proof_sha256} label="local proof hash" /></td>
                    <td>{proof.gap_codes.length ? proof.gap_codes.map(formatIdentifier).join(", ") : "None declared"}</td>
                    <td>{proof.warning_codes.length ? proof.warning_codes.map(formatIdentifier).join(", ") : "None declared"}</td>
                    <td><button className="secondary-action" disabled={proofPreviewSelection?.proofId === proof.selection_token} onClick={() => void previewRedactedProof(proof)}>{proofPreviewSelection?.proofId === proof.selection_token ? "Loading/selected" : "Preview"}</button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {proofPreview && (
          <section className="proof-export-preview" aria-label="Exact redacted Proof of Sync preview">
            <div className="panel-heading">
              <div>
                <h3>Exact redacted support artifact for run {proofPreviewSelection?.runId ?? "unknown"}</h3>
                <p className="panel-description">Review these exact local-only bytes before saving. This is a checksum-backed local consistency record, not a signature or proof that the responder was genuine Tally.</p>
              </div>
              <a
                className="secondary-action"
                download={`bridge-tally-proof-${proofPreview.payload_sha256.slice(0, 12)}.json`}
                href={`data:application/json;charset=utf-8,${encodeURIComponent(proofPreview.json)}`}
              >
                <FileText size={16} /> Save reviewed JSON
              </a>
            </div>
            <small>Payload checksum: <code>{proofPreview.payload_sha256}</code> <CopyTokenButton value={proofPreview.payload_sha256} label="support artifact checksum" /></small>
            <pre>{proofPreview.json}</pre>
          </section>
        )}
        {!!syncEvidence?.latest_reconciliation_mismatches.length && (
          <section className="proof-export-preview" aria-label="Local reconciliation drill-down">
            <h3>Local reconciliation drill-down</h3>
            <p className="panel-description">Session-local aliases identify repeated affected records without exposing Tally IDs or book contents. They are deliberately excluded from the public support export.</p>
            <ul className="verification-list">
              {syncEvidence.latest_reconciliation_mismatches.map((mismatch) => (
                <li key={`${mismatch.reason_code}:${mismatch.record_aliases.join(":")}`}>
                  <strong>{formatIdentifier(mismatch.reason_code)}</strong>: {mismatch.record_aliases.join(", ") || "No record alias available"}
                </li>
              ))}
            </ul>
          </section>
        )}
      </article>

      <section className="grid mirror-details">
        <article className="panel">
          <div className="panel-heading">
            <div>
              <h2>Pack readiness</h2>
              <p className="panel-description">Supported means the declared pack contract was observed for this exact profile; it does not mean complete books or a Verified run.</p>
            </div>
          </div>
          <CapabilityRows capabilities={passport?.packs} labels={PACK_LABELS} />
        </article>

        <article className="panel">
          <h2>What “Verified” will require</h2>
          <ul className="verification-list">
            <li>Every requested scope and window completes.</li>
            <li>Tally application status and payload validation pass.</li>
            <li>The company identity matches the pinned source.</li>
            <li>A product-supported atomic source cut or equally strong isolation mechanism is evidenced.</li>
            <li>Declared reconciliation checks pass.</li>
          </ul>
          <p className="section-note">
            Until those results are reported, Bridge will not present previews, counts, or absence of errors as accounting accuracy.
          </p>
          {passport?.mode?.toLowerCase().includes("education") && (
            <p className="section-note">The currently observed Education profile does not provide atomic source-cut evidence, so current Core Accounting runs remain Partial.</p>
          )}
        </article>
      </section>

      <article className="panel wide runtime-panel">
        <div className="panel-heading">
          <div>
            <h2>Tally runtime</h2>
            <p className="panel-description">
              Per-endpoint queue and health evidence. A closed circuit means requests are allowed; it is not proof that a pack is complete.
            </p>
          </div>
          <button className="secondary-action" onClick={() => void refreshRuntime()}>
            <RefreshCw size={16} /> Refresh
          </button>
        </div>
        {runtimeError && <TallyErrorNotice message={runtimeError} />}
        {runtimeSessions.length === 0 ? (
          <div className="empty-state compact">
            <strong>No endpoint session yet</strong>
            <span>Run a Tally endpoint check to create one shared runtime session.</span>
          </div>
        ) : (
          <div className="runtime-list">
            {runtimeSessions.map((session) => (
              <section className="runtime-session" key={session.session_id}>
                <div className="runtime-session-heading">
                  <div>
                    <strong>{session.canonical_endpoint}</strong>
                    <span>{formatIdentifier(session.circuit_state)} circuit · {session.active_requests} active · {session.issued_requests} issued</span>
                  </div>
                  <span className={`truth-state state-${session.circuit_state === "closed" ? "supported" : "unknown"}`}>
                    {formatIdentifier(session.circuit_state)}
                  </span>
                </div>
                <dl className="runtime-health">
                  <div><dt>Consecutive failures</dt><dd>{session.consecutive_failures}</dd></div>
                  <div><dt>Last success</dt><dd>{formatRuntimeTime(session.last_success_unix_ms)}</dd></div>
                  <div><dt>Last failure</dt><dd>{formatRuntimeTime(session.last_failure_unix_ms)}</dd></div>
                  <div><dt>Capability observed</dt><dd>{formatRuntimeTime(session.cached_capability_observed_at_unix_ms)}</dd></div>
                </dl>
                {session.circuit_retry_after_unix_ms && (
                  <p className="section-note">Retry after {formatRuntimeTime(session.circuit_retry_after_unix_ms)}.</p>
                )}
                {session.active_request_ids.length > 0 && (
                  <div className="active-requests">
                    {session.active_request_ids.map((requestId) => (
                      <span className="active-request" key={requestId}>
                        <code>{requestId}</code>
                        <CopyTokenButton value={requestId} label="request ID" />
                        <button onClick={() => void cancelTallyRequest(requestId)}>Cancel request</button>
                      </span>
                    ))}
                  </div>
                )}
              </section>
            ))}
          </div>
        )}
      </article>
    </>
  );
}
