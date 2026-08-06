import React from "react";
import ReactDOM from "react-dom/client";
import { Activity, Building2, Cable, Check, CircleHelp, Cloud, Database, FileText, FolderOpen, KeyRound, Play, RefreshCw, ShieldCheck, UploadCloud } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import {
  applyProbeCompanySelectionTransition,
  canReuseCurrentProbeReview,
  clearCompanyScopedState,
  companyDiscoveryPrompt,
  currentProbeCompanies,
  tallyCompanyKey,
  tallyReadinessState,
} from "./tally-company-selection";
import { classifyTallyError } from "./tally-error-copy";
import { TallyReadinessFlow } from "./TallyReadinessFlow";
import { OutstandingsScreen } from "./OutstandingsScreen";
import "./styles.css";

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
  guid_observed?: boolean;
  mirror_company_id?: string;
  correlation_key?: string;
  identity_confidence?: "observed" | "unknown";
  canonical_endpoint?: string;
  last_observed_at_unix_ms?: number;
};

type UntrustedCompanyCandidate = {
  name: string;
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

type PersistedCompanyProfilePage = {
  profiles: TallyCompany[];
  total_profiles: number;
  limit: number;
  truncated: boolean;
};

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

type TallyProbeResult = {
  review_id: string;
  canonical_origin: string;
  observed_at_unix_ms: number;
  connection: ConnectionStatus;
  companies: TallyCompany[];
  profile: CapabilityProfile;
  selected_read_scope?: SelectedReadScope;
  profile_sha256: string;
  review_commitment_sha256: string;
  passport_snapshot_id?: string;
};

type SelectedReadScope = {
  scope_version: number;
  ledger_profile_id: string;
  voucher_profile_id: string;
  voucher_from_yyyymmdd: string;
  voucher_to_yyyymmdd: string;
  scope_commitment_sha256: string;
};

type SavedTallySetup = {
  passport_snapshot_id: string;
  canonical_origin: string;
  observed_at_unix_ms: number;
  company: TallyCompany;
  review_cleanup_warning?: "review_cache_cleanup_failed_after_save";
};

type TallyWriteFixtureEnrollmentStatus = {
  fixture_state: "not_enrolled" | "active" | "revoked";
  enrolled_at_unix_ms?: number;
  revoked_at_unix_ms?: number;
  candidate_gate: "not_enrolled" | "enrolled";
  write_capability: "unknown";
};

type TallyWriteFixtureEnrollmentResponse = TallyWriteFixtureEnrollmentStatus & {
  tally_requests_attempted: number;
  tally_writes_attempted: number;
  review_cleanup_warning?: "review_cache_cleanup_failed_after_fixture_enrollment";
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
    affirmative_exact_capability_receipts: number;
    establishment_receipts: number;
    active_checkpoint_heads: number;
    state: "exact_capability_not_observed" | "verified_establishment_missing" | "execution_not_enabled";
    fallback_warning_code: string;
  };
  core_accounting_freshness: {
    state: "fresh" | "stale" | "never_verified";
    verified_at_unix_ms?: number;
    age_seconds?: number;
    checkpoint_present: boolean;
    proof_present: boolean;
  };
};

type RedactedProofPreview = {
  json: string;
  payload_sha256: string;
};

type MirrorExplorerPage = {
  pack_id: string;
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

type GstReturnDraft = {
  company: string;
  financial_year: string;
  gstr1: {
    b2b_invoice_count: number;
    b2c_invoice_count: number;
    credit_debit_note_count: number;
    hsn_summary_count: number;
  };
  gstr3b: {
    outward_taxable_value: string;
    integrated_tax: string;
    central_tax: string;
    state_tax: string;
    cess: string;
  };
  missing_fields: string[];
};

type DscCertificate = {
  label: string;
  common_name?: string | null;
  organization?: string | null;
  issuer_name?: string | null;
  serial_number?: string | null;
  valid_from?: string | null;
  valid_to?: string | null;
  fingerprint?: string | null;
  parse_error?: string | null;
};

type DscAttempt = {
  token_type: string;
  library_path: string;
  library_exists: boolean;
  loaded: boolean;
  initialized: boolean;
  slot_count: number;
  login_success: boolean;
  certificate_count?: number | null;
  certificates: DscCertificate[];
  error?: string | null;
};

type DscProbeReport = {
  platform: string;
  arch: string;
  force_load: boolean;
  detect_only: boolean;
  attempts: DscAttempt[];
};

type AxalIntegration = "tally" | "documents" | "dsc";

type AxalValidationResponse = {
  valid: boolean;
  status?: string | null;
  last_synced?: string | null;
  error?: string | null;
};

type AxalSessionResponse = {
  credentialSessionId: string;
  validation: AxalValidationResponse;
};

type AxalConnectionStatus = {
  connected: boolean;
  status: string;
  last_synced_at?: string | null;
  workspace: {
    id: string;
    name: string;
    billing_plan: string;
    storage_used: number;
    storage_limit: number;
  };
};

type DscSyncResponse = {
  success: boolean;
  message: string;
  results?: {
    created: number;
    updated: number;
    skipped: number;
    errors: string[];
  } | null;
};

type DocumentFile = {
  scanId: string;
  relativePath: string;
  size: number;
  mtime: number;
  extension?: string | null;
  mimeType: string;
  hash?: string | null;
  contentHash?: string | null;
  serverFileKey?: string | null;
  multipartInfo?: {
    uploadId: string;
    parts: {
      partNumber: number;
      etag: string;
      size: number;
      bytesRead: number;
    }[];
  } | null;
};

type ScanDocumentsResponse = {
  scanSessionId: string;
  files: DocumentFile[];
  totalSize: number;
  skipped: { path: string; reason: string }[];
};

type SyncDocumentsResponse = {
  success: boolean;
  uploadedFiles: DocumentFile[];
  failedFiles: { relativePath: string; error: string }[];
  duplicateCount: number;
  batchIds: string[];
};

type SelectedDocumentPath = {
  selectionId: string;
  displayName: string;
};

type View = "dashboard" | "outstandings" | "companies" | "gst" | "mirror" | "dsc" | "documents" | "axal";
type TallyAction = "probe" | "discover" | "bootstrap" | "save" | "fixture_enroll" | "fixture_revoke" | "evidence" | "explorer" | "start" | "resume" | "cancel";

const TABLE_PREVIEW_LIMIT = 100;
const MIRROR_PAGE_LIMIT = 25;

const VIEW_TITLES: Record<View, string> = {
  dashboard: "Tally evidence dashboard",
  outstandings: "Aged outstandings",
  companies: "Connect Tally",
  gst: "GST return readiness",
  mirror: "Accounting mirror and proof",
  dsc: "DSC token",
  documents: "Documents",
  axal: "AXAL backend",
};

const TRANSPORT_LABELS: Record<string, string> = {
  xml_http: "XML over HTTP",
  json_ex: "JSONEX",
  tdl_companion: "TDL companion",
  odbc: "ODBC",
};

const PACK_LABELS: Record<string, string> = {
  core_accounting: "Core accounting",
  india_tax: "India tax",
  bills_and_payments: "Bills and payments",
  inventory: "Inventory",
};

const FEATURE_LABELS: Record<string, string> = {
  endpoint_reachability: "Endpoint responder reachability",
  loaded_companies: "Loaded companies",
  stable_company_identity: "Stable company identity",
  encoding_behaviour: "Response encoding",
  practical_response_limit: "Practical response limit",
  company_read: "Company enumeration",
  ledger_read: "Ledger read",
  voucher_read: "Voucher read",
  selected_ledger_read: "Selected-company ledger profile",
  selected_voucher_window_read: "Selected voucher-window profile",
  write: "Write capability",
};

const CAPABILITY_REASON_LABELS: Record<string, string> = {
  xml_export_probe_failed: "The safe XML export probe did not complete.",
  tally_status_not_recognized: "The endpoint response was not recognized as a compatible Tally status.",
  release_not_observed: "The Tally release was not observed, so this transport was not tested.",
  configuration_not_observed: "Bridge did not inspect this optional transport's configuration.",
  company_identity_invalid: "The company result contained an invalid or unsafe identity field.",
  company_identity_ambiguous: "Two or more returned companies shared the same normalized GUID.",
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

const DSC_METADATA_RETENTION_MS = 5 * 60 * 1000;

function App() {
  const currentFinancialYear = React.useMemo(() => getCurrentFinancialYear(), []);
  const [config, setConfig] = React.useState<TallyConfig>({ host: "localhost", port: 9000 });
  const [status, setStatus] = React.useState<ConnectionStatus | null>(null);
  const [passport, setPassport] = React.useState<CapabilityProfile | null>(null);
  const [profileSha256, setProfileSha256] = React.useState<string | null>(null);
  const [reviewId, setReviewId] = React.useState<string | null>(null);
  const [reviewCommitmentSha256, setReviewCommitmentSha256] = React.useState<string | null>(null);
  const [selectedReadScope, setSelectedReadScope] = React.useState<SelectedReadScope | null>(null);
  const [passportSnapshotId, setPassportSnapshotId] = React.useState<string | null>(null);
  const [runtimeSessions, setRuntimeSessions] = React.useState<TallyRuntimeSnapshot[]>([]);
  const [runtimeError, setRuntimeError] = React.useState<OperatorError | null>(null);
  const [companies, setCompanies] = React.useState<TallyCompany[]>([]);
  const [untrustedDiscoveredCompanies, setUntrustedDiscoveredCompanies] = React.useState<UntrustedCompanyCandidate[]>([]);
  const [untrustedDiscoveryError, setUntrustedDiscoveryError] = React.useState<OperatorError | null>(null);
  const [selectedCompany, setSelectedCompany] = React.useState("");
  const [liveCompanyKeys, setLiveCompanyKeys] = React.useState<string[]>([]);
  const [persistedCompanyProfileTotal, setPersistedCompanyProfileTotal] = React.useState(0);
  const [persistedCompanyProfilesLoaded, setPersistedCompanyProfilesLoaded] = React.useState(0);
  const [persistedCompanyProfilesTruncated, setPersistedCompanyProfilesTruncated] = React.useState(false);
  const [voucherFrom, setVoucherFrom] = React.useState(currentFinancialYear.from);
  const [voucherTo, setVoucherTo] = React.useState(currentFinancialYear.to);
  const [companyError, setCompanyError] = React.useState<OperatorError | null>(null);
  const [fixtureStatus, setFixtureStatus] = React.useState<TallyWriteFixtureEnrollmentStatus | null>(null);
  const [fixtureStatusError, setFixtureStatusError] = React.useState<string | null>(null);
  const [fixtureDisposableAttested, setFixtureDisposableAttested] = React.useState(false);
  const [fixtureNoCustomerDataAttested, setFixtureNoCustomerDataAttested] = React.useState(false);
  const [fixtureBackupGuidanceAcknowledged, setFixtureBackupGuidanceAcknowledged] = React.useState(false);
  const [syncEvidence, setSyncEvidence] = React.useState<TallySyncEvidence | null>(null);
  const [syncEvidenceError, setSyncEvidenceError] = React.useState<OperatorError | null>(null);
  const [proofPreview, setProofPreview] = React.useState<RedactedProofPreview | null>(null);
  const [proofPreviewSelection, setProofPreviewSelection] = React.useState<{ proofId: string; runId: string } | null>(null);
  const [mirrorExplorer, setMirrorExplorer] = React.useState<MirrorExplorerPage | null>(null);
  const [mirrorExplorerError, setMirrorExplorerError] = React.useState<OperatorError | null>(null);
  const [snapshotJob, setSnapshotJob] = React.useState<SnapshotJobStatus | null>(null);
  const [recentSnapshotRuns, setRecentSnapshotRuns] = React.useState<SnapshotJobStatus[]>([]);
  const [snapshotError, setSnapshotError] = React.useState<OperatorError | null>(null);
  const [snapshotStartOutcomeUnknown, setSnapshotStartOutcomeUnknown] = React.useState(false);
  const [dashboardError, setDashboardError] = React.useState<OperatorError | null>(null);
  const [gstCompany, setGstCompany] = React.useState("");
  const [gstFinancialYear, setGstFinancialYear] = React.useState(currentFinancialYear.label);
  const [draft, setDraft] = React.useState<GstReturnDraft | null>(null);
  const [dscReport, setDscReport] = React.useState<DscProbeReport | null>(null);
  const [dscDetectReport, setDscDetectReport] = React.useState<DscProbeReport | null>(null);
  const [dscPin, setDscPin] = React.useState("");
  const [dscError, setDscError] = React.useState<string | null>(null);
  const [dscAction, setDscAction] = React.useState<"detect" | "extract" | null>(null);
  const [dscSync, setDscSync] = React.useState<DscSyncResponse | null>(null);
  const [dscSyncing, setDscSyncing] = React.useState(false);
  const [axalBaseUrl, setAxalBaseUrl] = React.useState("https://complyeaze.com");
  const [axalIntegration, setAxalIntegration] = React.useState<AxalIntegration>("dsc");
  const [axalApiId, setAxalApiId] = React.useState("");
  const [axalApiKey, setAxalApiKey] = React.useState("");
  const [axalSession, setAxalSession] = React.useState<{ id: string; integration: AxalIntegration } | null>(null);
  const [axalValidation, setAxalValidation] = React.useState<AxalValidationResponse | null>(null);
  const [axalConnection, setAxalConnection] = React.useState<AxalConnectionStatus | null>(null);
  const [axalError, setAxalError] = React.useState<string | null>(null);
  const [axalAction, setAxalAction] = React.useState<"validate" | "status" | null>(null);
  const [documentPaths, setDocumentPaths] = React.useState<SelectedDocumentPath[]>([]);
  const [documentScan, setDocumentScan] = React.useState<ScanDocumentsResponse | null>(null);
  const [documentSync, setDocumentSync] = React.useState<SyncDocumentsResponse | null>(null);
  const [documentError, setDocumentError] = React.useState<string | null>(null);
  const [documentAction, setDocumentAction] = React.useState<"scan" | "sync" | null>(null);
  const [view, setView] = React.useState<View>("dashboard");
  const [busy, setBusy] = React.useState(false);
  const [tallyAction, setTallyAction] = React.useState<TallyAction | null>(null);
  const tallyResultsVersion = React.useRef(0);
  const proofPreviewRequestVersion = React.useRef(0);
  const snapshotSelectionVersion = React.useRef(0);
  const dscRequestVersion = React.useRef(0);
  const mainContentRef = React.useRef<HTMLElement>(null);

  const clearDscSensitiveState = React.useCallback(() => {
    dscRequestVersion.current += 1;
    setDscReport(null);
    setDscDetectReport(null);
    setDscPin("");
    setDscSync(null);
  }, []);

  React.useEffect(() => {
    if (!dscReport && !dscDetectReport && !dscPin && !dscSync) return;
    const expiry = window.setTimeout(clearDscSensitiveState, DSC_METADATA_RETENTION_MS);
    return () => window.clearTimeout(expiry);
  }, [clearDscSensitiveState, dscDetectReport, dscPin, dscReport, dscSync]);

  React.useEffect(() => {
    if (view !== "dsc") clearDscSensitiveState();
  }, [clearDscSensitiveState, view]);

  const refreshRuntime = React.useCallback(async () => {
    try {
      const snapshots = await invoke<TallyRuntimeSnapshot[]>("tally_runtime_snapshots");
      setRuntimeSessions(snapshots);
      setRuntimeError(null);
    } catch (error) {
      setRuntimeError(toOperatorError(error));
    }
  }, []);

  const refreshRecentSnapshots = React.useCallback(async () => {
    try {
      const runs = await invoke<SnapshotJobStatus[]>("tally_recent_snapshot_runs");
      setRecentSnapshotRuns(runs);
      setSnapshotJob((current) => current ? runs.find((run) => run.run_id === current.run_id) ?? current : null);
    } catch (error) {
      setSnapshotError(toOperatorError(error));
    }
  }, []);

  const refreshPersistedCompanyProfiles = React.useCallback(async () => {
    try {
      const page = await invoke<PersistedCompanyProfilePage>("tally_persisted_company_profiles");
      setCompanies((current) => mergeTallyCompanies(page.profiles, current));
      setPersistedCompanyProfileTotal(page.total_profiles);
      setPersistedCompanyProfilesLoaded(page.profiles.length);
      setPersistedCompanyProfilesTruncated(page.truncated);
    } catch (error) {
      setCompanyError(toOperatorError(error));
    }
  }, []);

  React.useEffect(() => {
    void refreshRecentSnapshots();
    void refreshPersistedCompanyProfiles();
  }, [refreshRecentSnapshots, refreshPersistedCompanyProfiles]);

  React.useEffect(() => {
    mainContentRef.current?.focus();
  }, [view]);

  const snapshotActive = !!snapshotJob
    && !snapshotJob.requires_resume
    && !["completed", "partial", "failed", "cancelled"].includes(snapshotJob.phase);
  const savedCompanyMutationPending = tallyAction === "save"
    || tallyAction === "fixture_enroll"
    || tallyAction === "fixture_revoke";
  const savedCompanySelectionLocked = snapshotActive
    || snapshotStartOutcomeUnknown
    || savedCompanyMutationPending
    || tallyAction === "start"
    || tallyAction === "resume";

  React.useEffect(() => {
    if (!tallyAction && !snapshotActive) {
      void refreshRuntime();
      return;
    }
    let stopped = false;
    let timer: number | undefined;
    const poll = async () => {
      await refreshRuntime();
      if (!stopped) timer = window.setTimeout(() => void poll(), 500);
    };
    void poll();
    return () => {
      stopped = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [tallyAction, snapshotActive, refreshRuntime]);

  React.useEffect(() => {
    if (
      !snapshotJob
      || snapshotJob.requires_resume
      || ["completed", "partial", "failed", "cancelled"].includes(snapshotJob.phase)
    ) {
      return;
    }
    let stopped = false;
    let timer: number | undefined;
    let delay = 750;
    const selectionVersion = snapshotSelectionVersion.current;
    const poll = async () => {
      try {
        const status = await invoke<SnapshotJobStatus>("tally_snapshot_status", { runId: snapshotJob.run_id });
        if (stopped || selectionVersion !== snapshotSelectionVersion.current) return;
        setSnapshotJob(status);
        if (["completed", "partial", "failed", "cancelled"].includes(status.phase)) {
          void refreshSyncEvidence();
          void refreshRecentSnapshots();
          return;
        }
        delay = 750;
      } catch (error) {
        if (stopped || selectionVersion !== snapshotSelectionVersion.current) return;
        setSnapshotError(toOperatorError(error));
        delay = Math.min(delay * 2, 10_000);
      }
      if (!stopped) timer = window.setTimeout(() => void poll(), delay);
    };
    void poll();
    return () => {
      stopped = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [snapshotJob?.run_id, snapshotJob?.phase, snapshotJob?.requires_resume, refreshRecentSnapshots]);

  function invalidateTallyResults() {
    tallyResultsVersion.current += 1;
    setStatus(null);
    setPassport(null);
    setProfileSha256(null);
    setReviewId(null);
    setReviewCommitmentSha256(null);
    setSelectedReadScope(null);
    setPassportSnapshotId(null);
    setLiveCompanyKeys([]);
    setUntrustedDiscoveredCompanies([]);
    setUntrustedDiscoveryError(null);
    setDraft(null);
    setCompanyError(null);
    setFixtureStatus(null);
    setFixtureStatusError(null);
    setFixtureDisposableAttested(false);
    setFixtureNoCustomerDataAttested(false);
    setFixtureBackupGuidanceAcknowledged(false);
    setSyncEvidence(null);
    setSyncEvidenceError(null);
    setProofPreview(null);
    setProofPreviewSelection(null);
    proofPreviewRequestVersion.current += 1;
    setMirrorExplorer(null);
    setMirrorExplorerError(null);
    setSnapshotError(null);
    setDashboardError(null);
  }

  function clearSelectedCompanyScope({ preserveCurrentProbeReview = false } = {}) {
    setCompanyError(null);
    setFixtureStatus(null);
    setFixtureStatusError(null);
    setFixtureDisposableAttested(false);
    setFixtureNoCustomerDataAttested(false);
    setFixtureBackupGuidanceAcknowledged(false);
    clearCompanyScopedState({
      clearQualifiedReadReview: () => {
        if (!preserveCurrentProbeReview) {
          setPassport(null);
          setProfileSha256(null);
          setReviewId(null);
          setReviewCommitmentSha256(null);
        }
        setSelectedReadScope(null);
      },
      clearPassportSnapshot: () => setPassportSnapshotId(null),
      clearSyncEvidence: () => {
        setSyncEvidence(null);
        setSyncEvidenceError(null);
      },
      clearProofPreview: () => {
        setProofPreview(null);
        setProofPreviewSelection(null);
        proofPreviewRequestVersion.current += 1;
      },
      clearMirrorExplorer: () => {
        setMirrorExplorer(null);
        setMirrorExplorerError(null);
      },
      clearSnapshotState: () => {
        snapshotSelectionVersion.current += 1;
        setSnapshotJob(null);
        setSnapshotError(null);
        setSnapshotStartOutcomeUnknown(false);
      },
      invalidateTallyResults: () => {
        tallyResultsVersion.current += 1;
      },
    });
  }

  function selectSavedCompany(key: string) {
    if (key === selectedCompany || savedCompanySelectionLocked) return;
    clearSelectedCompanyScope();
    setSelectedCompany(key);
  }

  function updateTallyHost(host: string) {
    setConfig((current) => ({ ...current, host }));
    invalidateTallyResults();
  }

  function updateTallyPort(port: number) {
    setConfig((current) => ({ ...current, port }));
    invalidateTallyResults();
  }

  async function checkTally() {
    const resultsVersion = tallyResultsVersion.current;
    setTallyAction("probe");
    setDashboardError(null);
    try {
      const result = await invoke<TallyProbeResult>("probe_tally", { config });
      if (resultsVersion === tallyResultsVersion.current) {
        const liveCompanies = result.companies.map((company) => ({
          ...company,
          canonical_endpoint: result.canonical_origin,
          last_observed_at_unix_ms: result.observed_at_unix_ms,
        }));
        const nextLiveCompanyKeys = liveCompanies.map(tallyCompanyKey);
        const selection = applyProbeCompanySelectionTransition(
          selectedCompany,
          nextLiveCompanyKeys,
          {
            clearDroppedCompanyScope: clearSelectedCompanyScope,
            installProbeState: () => {
              setStatus(result.connection);
              setPassport(result.profile);
              setProfileSha256(result.profile_sha256);
              setReviewId(result.review_id);
              setReviewCommitmentSha256(result.review_commitment_sha256);
              setSelectedReadScope(result.selected_read_scope ?? null);
              setPassportSnapshotId(result.passport_snapshot_id ?? null);
              setCompanies((current) => mergeTallyCompanies(liveCompanies, current));
              setLiveCompanyKeys(nextLiveCompanyKeys);
            },
          },
        );
        setSelectedCompany(selection.selectedCompany);
        void refreshPersistedCompanyProfiles();
        setUntrustedDiscoveredCompanies([]);
        setUntrustedDiscoveryError(null);
        if (result.profile.transports.xml_http?.safe_reason_code === "direct_company_report_untrusted") {
          const discoveryResultsVersion = tallyResultsVersion.current;
          try {
            const discovered = await invoke<UntrustedCompanyCandidate[]>("fetch_tally_companies", { config });
            if (discoveryResultsVersion === tallyResultsVersion.current) {
              setUntrustedDiscoveredCompanies(discovered);
            }
          } catch (error) {
            if (discoveryResultsVersion === tallyResultsVersion.current) {
              setUntrustedDiscoveryError(toOperatorError(error));
            }
          }
        }
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) {
        setStatus(null);
        setPassport(null);
        setProfileSha256(null);
        setReviewId(null);
        setReviewCommitmentSha256(null);
        setSelectedReadScope(null);
        setPassportSnapshotId(null);
        setLiveCompanyKeys([]);
        setDashboardError(toOperatorError(error));
      }
    } finally {
      setTallyAction(null);
      void refreshRuntime();
    }
  }

  async function discoverUntrustedCompanies() {
    if (currentProbeCompanyList.length > 0) return;
    const resultsVersion = tallyResultsVersion.current;
    setTallyAction("discover");
    setUntrustedDiscoveryError(null);
    setUntrustedDiscoveredCompanies([]);
    try {
      const discovered = await invoke<UntrustedCompanyCandidate[]>("fetch_tally_companies", { config });
      if (resultsVersion === tallyResultsVersion.current) {
        setUntrustedDiscoveredCompanies(discovered);
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) {
        setUntrustedDiscoveryError(toOperatorError(error));
      }
    } finally {
      setTallyAction(null);
      void refreshRuntime();
    }
  }

  async function bootstrapDirectCompany(candidateName: string) {
    const resultsVersion = tallyResultsVersion.current;
    setTallyAction("bootstrap");
    setCompanyError(null);
    try {
      const result = await invoke<TallyProbeResult>("bootstrap_direct_tally_company", {
        request: { config, candidate_name: candidateName },
      });
      if (resultsVersion !== tallyResultsVersion.current) return;
      const liveCompanies = result.companies.map((company) => ({
        ...company,
        canonical_endpoint: result.canonical_origin,
        last_observed_at_unix_ms: result.observed_at_unix_ms,
      }));
      const nextLiveCompanyKeys = liveCompanies.map(tallyCompanyKey);
      const selection = applyProbeCompanySelectionTransition(
        selectedCompany,
        nextLiveCompanyKeys,
        {
          clearDroppedCompanyScope: clearSelectedCompanyScope,
          installProbeState: () => {
            setStatus(result.connection);
            setPassport(result.profile);
            setProfileSha256(result.profile_sha256);
            setReviewId(result.review_id);
            setReviewCommitmentSha256(result.review_commitment_sha256);
            setSelectedReadScope(result.selected_read_scope ?? null);
            setPassportSnapshotId(result.passport_snapshot_id ?? null);
            setCompanies((current) => mergeTallyCompanies(liveCompanies, current));
            setLiveCompanyKeys(nextLiveCompanyKeys);
          },
        },
      );
      const verifiedCompany = liveCompanies.length === 1 && liveCompanies[0].guid
        ? liveCompanies[0]
        : null;
      const verifiedCompanyKey = verifiedCompany
        ? tallyCompanyKey(verifiedCompany)
        : selection.selectedCompany;
      setSelectedCompany(verifiedCompanyKey);
      if (verifiedCompany) {
        setUntrustedDiscoveredCompanies([]);
        setUntrustedDiscoveryError(null);
      }
      void refreshPersistedCompanyProfiles();
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) setCompanyError(toOperatorError(error));
    } finally {
      setTallyAction(null);
      void refreshRuntime();
    }
  }

  async function saveReviewedTallySetup() {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!reviewId || !reviewCommitmentSha256 || !company?.guid || !liveCompanyKeys.includes(tallyCompanyKey(company))) {
      setCompanyError("Probe again and select a GUID-bearing company from the current result before saving.");
      return;
    }
    const resultsVersion = tallyResultsVersion.current;
    const reviewedCompanyKey = tallyCompanyKey(company);
    setTallyAction("save");
    setCompanyError(null);
    try {
      const saved = await invoke<SavedTallySetup>("save_tally_setup", {
        request: {
          config,
          expected_review_id: reviewId,
          expected_review_commitment_sha256: reviewCommitmentSha256,
          selected_company_guid: company.guid,
        },
      });
      if (resultsVersion !== tallyResultsVersion.current) return;
      const persisted = {
        ...saved.company,
        canonical_endpoint: saved.canonical_origin,
        last_observed_at_unix_ms: saved.observed_at_unix_ms,
      };
      setPassportSnapshotId(saved.passport_snapshot_id);
      setCompanies((current) => mergeTallyCompanies(
        [persisted],
        current.filter((candidate) => tallyCompanyKey(candidate) !== reviewedCompanyKey),
      ));
      setSelectedCompany(tallyCompanyKey(persisted));
      setLiveCompanyKeys((current) => Array.from(new Set([
        ...current.filter((key) => key !== reviewedCompanyKey),
        tallyCompanyKey(persisted),
      ])));
      setReviewId(null);
      setReviewCommitmentSha256(null);
      if (saved.review_cleanup_warning) {
        setCompanyError("The reviewed setup was saved, but its one-time in-memory review token could not be cleaned up. Restart Bridge before probing or saving another scope.");
      }
      void refreshPersistedCompanyProfiles();
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) {
        setCompanyError(toOperatorError(error));
      }
    } finally {
      setTallyAction((current) => current === "save" ? null : current);
    }
  }

  async function enrollWriteFixture() {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!reviewId || !reviewCommitmentSha256 || !company?.mirror_company_id || !company.guid || !selectedCompanyLive) {
      setCompanyError("Probe again, select the persisted GUID-bearing company, and review it before locally enrolling a synthetic fixture.");
      return;
    }
    if (!fixtureDisposableAttested || !fixtureNoCustomerDataAttested || !fixtureBackupGuidanceAcknowledged) {
      setCompanyError("Confirm all three safeguards before enrolling the synthetic fixture.");
      return;
    }
    const resultsVersion = tallyResultsVersion.current;
    const reviewedCompanyKey = tallyCompanyKey(company);
    const expectedReviewId = reviewId;
    setTallyAction("fixture_enroll");
    setCompanyError(null);
    try {
      const result = await invoke<TallyWriteFixtureEnrollmentResponse>("enroll_tally_write_fixture", {
        request: {
          config,
          expected_review_id: reviewId,
          expected_review_commitment_sha256: reviewCommitmentSha256,
          mirror_company_id: company.mirror_company_id,
          selected_company_guid: company.guid,
          disposable_company_attested: fixtureDisposableAttested,
          no_customer_data_attested: fixtureNoCustomerDataAttested,
          backup_guidance_acknowledged: fixtureBackupGuidanceAcknowledged,
        },
      });
      if (resultsVersion !== tallyResultsVersion.current || reviewedCompanyKey !== selectedCompany || expectedReviewId !== reviewId) return;
      setFixtureStatus(result);
      setFixtureDisposableAttested(false);
      setFixtureNoCustomerDataAttested(false);
      setFixtureBackupGuidanceAcknowledged(false);
      setReviewId(null);
      setReviewCommitmentSha256(null);
      if (result.review_cleanup_warning) {
        setCompanyError("The local fixture enrollment was saved, but its one-time in-memory review token could not be cleaned up. Restart Bridge before probing or enrolling another fixture.");
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) setCompanyError(toOperatorError(error));
    } finally {
      setTallyAction((current) => current === "fixture_enroll" ? null : current);
    }
  }

  async function revokeWriteFixture() {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!company?.mirror_company_id) {
      setCompanyError("Select a persisted company before revoking its local fixture enrollment.");
      return;
    }
    const resultsVersion = tallyResultsVersion.current;
    const companyKey = tallyCompanyKey(company);
    setTallyAction("fixture_revoke");
    setCompanyError(null);
    try {
      const status = await invoke<TallyWriteFixtureEnrollmentStatus>("revoke_tally_write_fixture_enrollment", {
        request: { mirror_company_id: company.mirror_company_id },
      });
      if (resultsVersion !== tallyResultsVersion.current || companyKey !== selectedCompany) return;
      setFixtureStatus(status);
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) setCompanyError(toOperatorError(error));
    } finally {
      setTallyAction((current) => current === "fixture_revoke" ? null : current);
    }
  }

  async function refreshWriteFixtureStatus(mirrorCompanyId: string) {
    setFixtureStatus(null);
    setFixtureStatusError(null);
    try {
      const status = await invoke<TallyWriteFixtureEnrollmentStatus>("tally_write_fixture_enrollment_status", {
        request: { mirror_company_id: mirrorCompanyId },
      });
      const current = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
      if (current?.mirror_company_id === mirrorCompanyId) setFixtureStatus(status);
    } catch {
      const current = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
      if (current?.mirror_company_id === mirrorCompanyId) {
        setFixtureStatusError("Bridge could not read the local fixture state. Retry before changing this local gate.");
      }
    }
  }

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

  async function refreshSyncEvidence(announce = false) {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!company?.mirror_company_id) {
      setSyncEvidence(null);
      setSyncEvidenceError(
        selectedCompany
          ? "Run Check Tally Endpoint to persist this company GUID before reading mirror evidence."
          : null,
      );
      return;
    }
    const resultsVersion = tallyResultsVersion.current;
    const mirrorCompanyId = company.mirror_company_id;
    proofPreviewRequestVersion.current += 1;
    setProofPreview(null);
    setProofPreviewSelection(null);
    if (announce) setTallyAction("evidence");
    try {
      const evidence = await invoke<TallySyncEvidence>("tally_sync_evidence", {
        request: { mirror_company_id: mirrorCompanyId },
      });
      if (resultsVersion === tallyResultsVersion.current) {
        setSyncEvidence(evidence);
        setProofPreview(null);
        setProofPreviewSelection(null);
        proofPreviewRequestVersion.current += 1;
        setSyncEvidenceError(null);
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) {
        setSyncEvidence(null);
        setSyncEvidenceError(toOperatorError(error));
      }
    } finally {
      if (announce) setTallyAction(null);
    }
  }

  async function previewRedactedProof(proof: TallyProofSummary) {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!company?.mirror_company_id) {
      setSyncEvidenceError("Select a company with an observed stable identity first.");
      return;
    }
    const resultsVersion = tallyResultsVersion.current;
    const requestVersion = ++proofPreviewRequestVersion.current;
    const mirrorCompanyId = company.mirror_company_id;
    setProofPreview(null);
    setProofPreviewSelection({ proofId: proof.selection_token, runId: proof.run_id });
    try {
      const preview = await invoke<RedactedProofPreview>("preview_tally_redacted_proof", {
        request: {
          mirror_company_id: mirrorCompanyId,
          proof_id: proof.selection_token,
        },
      });
      if (resultsVersion === tallyResultsVersion.current && requestVersion === proofPreviewRequestVersion.current) {
        setProofPreview(preview);
        setSyncEvidenceError(null);
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current && requestVersion === proofPreviewRequestVersion.current) {
        setProofPreview(null);
        setProofPreviewSelection(null);
        setSyncEvidenceError(toOperatorError(error));
      }
    }
  }

  async function loadMirrorExplorerPage(offset: number) {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!company?.mirror_company_id) {
      setMirrorExplorerError("Select a persisted company identity before browsing the local mirror.");
      return;
    }
    const resultsVersion = tallyResultsVersion.current;
    const mirrorCompanyId = company.mirror_company_id;
    setTallyAction("explorer");
    try {
      const page = await invoke<MirrorExplorerPage>("tally_mirror_explorer_page", {
        request: {
          mirror_company_id: mirrorCompanyId,
          pack_id: "core_accounting",
          offset,
          limit: MIRROR_PAGE_LIMIT,
        },
      });
      if (resultsVersion === tallyResultsVersion.current) {
        setMirrorExplorer(page);
        setMirrorExplorerError(null);
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) {
        setMirrorExplorerError(toOperatorError(error));
      }
    } finally {
      setTallyAction(null);
    }
  }

  async function startCoreSnapshot() {
    const company = companies.find((candidate) => tallyCompanyKey(candidate) === selectedCompany);
    if (!company?.mirror_company_id) {
      setSnapshotError("Run Check Tally Endpoint and select a company with an observed GUID first.");
      return;
    }
    if (!liveCompanyKeys.includes(tallyCompanyKey(company))) {
      setSnapshotError("The persisted company pin is available for offline evidence review, but it has not been matched by the current endpoint probe. Probe and select the matching live company before starting a Core Accounting read.");
      return;
    }
    if (!voucherFrom || !voucherTo || voucherFrom > voucherTo) {
      setSnapshotError("Choose a valid requested accounting period.");
      return;
    }
    setTallyAction("start");
    setSnapshotError(null);
    const selectionVersion = ++snapshotSelectionVersion.current;
    try {
      const job = await invoke<SnapshotJobStatus>("start_tally_core_snapshot", {
        request: {
          config,
          mirror_company_id: company.mirror_company_id,
          from: toTallyDate(voucherFrom),
          to: toTallyDate(voucherTo),
        },
      });
      if (selectionVersion === snapshotSelectionVersion.current) {
        setSnapshotJob(job);
        setRecentSnapshotRuns((current) => [
          job,
          ...current.filter((run) => run.run_id !== job.run_id),
        ]);
        setSnapshotStartOutcomeUnknown(false);
      }
      void refreshRecentSnapshots();
    } catch (error) {
      await refreshRecentSnapshots();
      setSnapshotStartOutcomeUnknown(true);
      setSnapshotError(`Start outcome was not confirmed. Recent durable runs were refreshed and a new start is locked until you review them. ${toErrorMessage(error)}`);
    } finally {
      setTallyAction(null);
    }
  }

  async function cancelCoreSnapshot() {
    if (!snapshotJob) return;
    const runId = snapshotJob.run_id;
    const selectionVersion = snapshotSelectionVersion.current;
    setTallyAction("cancel");
    try {
      const accepted = await invoke<boolean>("cancel_tally_snapshot", { runId });
      const status = await invoke<SnapshotJobStatus>("tally_snapshot_status", { runId });
      if (selectionVersion === snapshotSelectionVersion.current) setSnapshotJob(status);
      if (!accepted) {
        setSnapshotError("Cancellation was not accepted because the run was already terminal or no longer cancellable. Status was refreshed.");
      }
    } catch (error) {
      await refreshRecentSnapshots();
      setSnapshotError(`Cancellation outcome was not confirmed. Run status was refreshed. ${toErrorMessage(error)}`);
    } finally {
      setTallyAction(null);
    }
  }

  async function resumeCoreSnapshot(runId: string) {
    const selectionVersion = ++snapshotSelectionVersion.current;
    setTallyAction("resume");
    setSnapshotError(null);
    try {
      const job = await invoke<SnapshotJobStatus>("resume_tally_core_snapshot", {
        request: { config, run_id: runId },
      });
      if (selectionVersion === snapshotSelectionVersion.current) setSnapshotJob(job);
      void refreshRecentSnapshots();
    } catch (error) {
      await refreshRecentSnapshots();
      setSnapshotError(`Resume outcome was not confirmed. Run status was refreshed before another resume is allowed. ${toErrorMessage(error)}`);
    } finally {
      setTallyAction(null);
    }
  }

  async function prepareDraft() {
    const company = gstCompany.trim();
    const financialYear = gstFinancialYear.trim();
    if (!company || !/^\d{4}-\d{4}$/.test(financialYear)) {
      setDashboardError("Enter a company and a financial year in YYYY-YYYY format.");
      return;
    }

    const resultsVersion = tallyResultsVersion.current;
    setBusy(true);
    setDashboardError(null);
    try {
      const result = await invoke<GstReturnDraft>("prepare_gst_return_draft", {
        request: {
          company,
          financial_year: financialYear,
        },
      });
      if (resultsVersion === tallyResultsVersion.current) {
        setDraft(result);
      }
    } catch (error) {
      if (resultsVersion === tallyResultsVersion.current) {
        setDraft(null);
        setDashboardError(toOperatorError(error));
      }
    } finally {
      setBusy(false);
    }
  }

  async function runDsc(detectOnly: boolean) {
    const pin = dscPin;
    if (!detectOnly && !pin) {
      setDscError("Enter the DSC token PIN before extracting certificates.");
      return;
    }

    const requestVersion = ++dscRequestVersion.current;
    setBusy(true);
    setDscAction(detectOnly ? "detect" : "extract");
    setDscError(null);
    setDscReport(null);
    setDscDetectReport(null);
    setDscSync(null);
    if (!detectOnly) {
      setDscPin("");
    }
    try {
      const result = detectOnly
        ? await invoke<DscProbeReport>("detect_dsc_token")
        : await invoke<DscProbeReport>("extract_dsc_certificates", { pins: [pin] });
      if (requestVersion === dscRequestVersion.current) {
        if (detectOnly) {
          setDscDetectReport(result);
        } else {
          setDscReport(result);
        }
      }
    } catch (error) {
      if (requestVersion === dscRequestVersion.current) {
        setDscError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      setBusy(false);
      setDscAction(null);
    }
  }

  function axalCredentials() {
    return {
      api_key: axalApiKey,
      api_id: axalApiId,
      integration: axalIntegration,
      base_url: axalBaseUrl,
    };
  }

  function invalidateAxalSession() {
    const sessionId = axalSession?.id;
    setAxalSession(null);
    setAxalConnection(null);
    if (sessionId) {
      void invoke("revoke_axal_credential_session", {
        credentialSessionId: sessionId,
      }).catch(() => undefined);
    }
  }

  async function validateAxal() {
    setBusy(true);
    setAxalAction("validate");
    setAxalError(null);
    try {
      const result = await invoke<AxalSessionResponse>("validate_axal_credentials", {
        credentials: axalCredentials(),
      });
      setAxalValidation(result.validation);
      setAxalSession({ id: result.credentialSessionId, integration: axalIntegration });
      setAxalConnection(null);
    } catch (error) {
      setAxalError(error instanceof Error ? error.message : String(error));
    } finally {
      setAxalApiKey("");
      setBusy(false);
      setAxalAction(null);
    }
  }

  async function checkAxalStatus() {
    if (!axalSession) {
      setAxalError("Validate AXAL credentials before checking connection status.");
      return;
    }
    setBusy(true);
    setAxalAction("status");
    setAxalError(null);
    try {
      const result = await invoke<AxalConnectionStatus>("check_axal_connection_status", {
        credentialSessionId: axalSession.id,
      });
      setAxalConnection(result);
    } catch (error) {
      setAxalError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setAxalAction(null);
    }
  }

  async function syncDscCertificate() {
    if (!primaryCertificate || !successfulDscAttempt || !axalConnection || axalSession?.integration !== "dsc") {
      setDscError("Extract a certificate and check AXAL workspace status before syncing.");
      return;
    }

    setDscSyncing(true);
    setDscError(null);
    try {
      const holderName =
        primaryCertificate.common_name || primaryCertificate.organization || primaryCertificate.label;
      const result = await invoke<DscSyncResponse>("sync_dsc_certificates_to_axal", {
        request: {
          credentialSessionId: axalSession.id,
          workspaceExternalId: axalConnection.workspace.id,
          certificates: [
            {
              holderName,
              provider: primaryCertificate.issuer_name || "Unknown",
              serialNumber: primaryCertificate.serial_number || "",
              tokenType: successfulDscAttempt.token_type,
              class: "Unknown",
              purpose: "Digital Signature",
              issueDate: primaryCertificate.valid_from || "",
              expirationDate: primaryCertificate.valid_to || "",
              clientName: holderName,
              metadata: {
                organization: primaryCertificate.organization,
                issuer: primaryCertificate.issuer_name,
                fingerprint: primaryCertificate.fingerprint,
                tokenType: successfulDscAttempt.token_type,
              },
            },
          ],
        },
      });
      setDscSync(result);
    } catch (error) {
      setDscError(error instanceof Error ? error.message : String(error));
    } finally {
      setDscSyncing(false);
    }
  }

  async function scanDocuments() {
    setBusy(true);
    setDocumentAction("scan");
    setDocumentError(null);
    setDocumentSync(null);
    try {
      const result = await invoke<ScanDocumentsResponse>("scan_document_paths", {
        request: {
          selection_ids: documentPaths.map((path) => path.selectionId),
          use_hash: true,
          exclude_hidden_files: true,
          exclude_zero_byte_files: true,
        },
      });
      setDocumentScan(result);
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setDocumentAction(null);
    }
  }

  async function chooseDocumentFiles() {
    setDocumentError(null);
    try {
      const paths = await invoke<SelectedDocumentPath[]>("select_document_files");
      if (paths.length > 0) {
        setDocumentPaths((current) => [...current, ...paths]);
        setDocumentScan(null);
        setDocumentSync(null);
      }
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    }
  }

  async function chooseDocumentFolder() {
    setDocumentError(null);
    try {
      const paths = await invoke<SelectedDocumentPath[]>("select_document_folder");
      if (paths.length > 0) {
        setDocumentPaths((current) => [...current, ...paths]);
        setDocumentScan(null);
        setDocumentSync(null);
      }
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    }
  }

  function clearDocuments() {
    void invoke("revoke_document_authorizations", {
      selectionIds: documentPaths.map((path) => path.selectionId),
      scanSessionId: documentScan?.scanSessionId ?? null,
    }).catch(() => undefined);
    setDocumentPaths([]);
    setDocumentScan(null);
    setDocumentSync(null);
  }

  async function syncDocuments() {
    if (!documentScan?.files.length || !axalConnection || axalSession?.integration !== "documents") {
      setDocumentError("Scan files and check AXAL workspace status before syncing documents.");
      return;
    }

    setBusy(true);
    setDocumentAction("sync");
    setDocumentError(null);
    try {
      const result = await invoke<SyncDocumentsResponse>("sync_documents_to_axal", {
        request: {
          credentialSessionId: axalSession.id,
          workspaceExternalId: axalConnection.workspace.id,
          scanSessionId: documentScan.scanSessionId,
          files: documentScan.files,
          maxFilesPerBatch: 20,
        },
      });
      setDocumentSync(result);
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setDocumentAction(null);
    }
  }

  const successfulDscAttempt = dscReport?.attempts.find(
    (attempt) => attempt.login_success && attempt.certificates.length > 0,
  );
  const detectedDscAttempt = dscDetectReport?.attempts.find(
    (attempt) => attempt.loaded && attempt.initialized && attempt.slot_count > 0 && !attempt.error,
  );
  const primaryCertificate =
    successfulDscAttempt?.certificates.find((certificate) => certificate.common_name) ??
    successfulDscAttempt?.certificates[0];
  const gstDraftComplete = draft !== null && draft.missing_fields.length === 0;
  const selectedCompanyRecord = companies.find((company) => tallyCompanyKey(company) === selectedCompany);
  const selectedCompanyLive = !!selectedCompanyRecord && liveCompanyKeys.includes(tallyCompanyKey(selectedCompanyRecord));
  const currentProbeCompanyList = currentProbeCompanies(companies, liveCompanyKeys);
  const savedCompanyList = companies.filter((company) => Boolean(company.mirror_company_id));
  const setupConnectionComplete = Boolean(status?.reachable && passport);
  const selectedCompanyReady = tallyReadinessState({
    endpointComplete: setupConnectionComplete,
    companySelected: Boolean(selectedCompanyRecord),
    companyCurrent: selectedCompanyLive,
    companySaved: Boolean(selectedCompanyRecord?.mirror_company_id),
  }).companyReady;
  const discoveredCompanyPrompt = companyDiscoveryPrompt(
    selectedCompany,
    liveCompanyKeys,
    untrustedDiscoveredCompanies.length,
  );
  React.useEffect(() => {
    const mirrorCompanyId = selectedCompanyRecord?.mirror_company_id;
    setFixtureStatus(null);
    setFixtureStatusError(null);
    if (!mirrorCompanyId) return;
    let cancelled = false;
    void invoke<TallyWriteFixtureEnrollmentStatus>("tally_write_fixture_enrollment_status", {
      request: { mirror_company_id: mirrorCompanyId },
    })
      .then((status) => {
        if (!cancelled) setFixtureStatus(status);
      })
      .catch(() => {
        if (!cancelled) setFixtureStatusError("Bridge could not read the local fixture state. Retry before changing this local gate.");
      });
    return () => {
      cancelled = true;
    };
  }, [selectedCompanyRecord?.mirror_company_id]);
  const selectedRecentSnapshotRuns = selectedCompanyRecord?.mirror_company_id
    ? recentSnapshotRuns.filter((run) => run.mirror_company_id === selectedCompanyRecord.mirror_company_id)
    : [];
  const latestProof = syncEvidence?.latest_proofs[0];
  const mirrorTruthState = latestProof?.verification_state ?? "unknown";
  const inspectedJob = snapshotJob?.mirror_company_id === selectedCompanyRecord?.mirror_company_id ? snapshotJob : null;
  const latestDurableJob = inspectedJob
    && !inspectedJob.requires_resume
    && !["completed", "partial", "failed", "cancelled"].includes(inspectedJob.phase)
    ? inspectedJob
    : selectedRecentSnapshotRuns[0] ?? null;
  const activeGapCodes = inspectedJob ? inspectedJob.gap_codes : latestProof?.gap_codes ?? [];
  const activeWarningCodes = inspectedJob ? inspectedJob.warning_codes : latestProof?.warning_codes ?? [];
  const latestGapCodes = latestDurableJob ? latestDurableJob.gap_codes : latestProof?.gap_codes ?? [];
  const latestWarningCodes = latestDurableJob ? latestDurableJob.warning_codes : latestProof?.warning_codes ?? [];
  const inspectingHistoricalRun = !!inspectedJob && !!latestDurableJob && inspectedJob.run_id !== latestDurableJob.run_id;
  const verifiedBaseline = syncEvidence?.core_accounting_freshness.verified_at_unix_ms
    ? `${formatIdentifier(syncEvidence.core_accounting_freshness.state)} · ${formatRuntimeTime(syncEvidence.core_accounting_freshness.verified_at_unix_ms)}`
    : "No verified Core Accounting baseline";
  const latestAttemptSummary = latestDurableJob
    ? `${formatIdentifier(latestDurableJob.phase)}${latestDurableJob.verification ? ` · ${formatIdentifier(latestDurableJob.verification)}` : ""}`
    : latestProof
      ? `${formatIdentifier(latestProof.outcome)} · ${formatIdentifier(latestProof.verification_state)} · ${formatRuntimeTime(latestProof.completed_at_unix_ms)}`
      : "No Core Accounting attempt loaded";
  const latestAttemptNeedsReview = latestDurableJob
    ? !!latestDurableJob.failure_code || latestDurableJob.requires_resume || ["partial", "failed", "cancelled"].includes(latestDurableJob.phase)
    : !!latestProof && (latestProof.outcome !== "completed" || latestProof.verification_state !== "verified");
  const operatorMissing = !selectedCompanyRecord?.mirror_company_id
    ? "A selected company with an observed, persisted GUID"
    : latestAttemptNeedsReview
      ? "Review of the latest non-Verified or interrupted attempt"
      : !status
        ? "A current endpoint and capability probe; offline evidence remains reviewable"
        : latestGapCodes.length || latestWarningCodes.length
          ? `${latestGapCodes.length} gap${latestGapCodes.length === 1 ? "" : "s"} and ${latestWarningCodes.length} warning${latestWarningCodes.length === 1 ? "" : "s"} in the latest attempt`
          : syncEvidence?.core_accounting_freshness.state === "fresh"
            ? "No gaps declared for the loaded Verified scope; unsupported or unrequested scopes are not covered"
            : "A fresh Verified baseline for this company";
  const operatorNext = !selectedCompanyRecord?.mirror_company_id
    ? "Select a GUID-bearing company in Tally"
    : snapshotJob?.resume_available
      ? "Resume the interrupted run"
      : snapshotActive
        ? "Let the active phase finish or cancel explicitly"
        : latestAttemptNeedsReview
          ? latestDurableJob?.failure_code
            ? `Review ${formatIdentifier(latestDurableJob.failure_code)} before relying on the older baseline`
            : "Review the latest non-Verified attempt before relying on the older baseline"
          : !status
            ? "Review offline evidence, then probe before any new live read"
            : latestGapCodes.length
              ? inspectingHistoricalRun ? "Inspect the latest run, then review its Gap Map before retrying" : "Review the latest Gap Map before retrying"
              : latestWarningCodes.length
                ? inspectingHistoricalRun ? "Inspect the latest run, then review its warnings" : "Review warnings before relying on the latest attempt"
                : syncEvidence?.core_accounting_freshness.state === "fresh"
                  ? "No immediate action; monitor freshness and new attempts"
                  : "Run a read-only Core Accounting evidence read";

  return (
    <div className="shell">
      <a className="skip-link" href="#main-content">Skip to active view</a>
      <aside className="sidebar">
        <div className="brand">
          <ShieldCheck size={24} />
          <div>
            <strong>Bridge</strong>
            <span>Tauri Agent</span>
          </div>
        </div>
        <nav aria-label="Bridge operations">
          <button aria-current={view === "dashboard" ? "page" : undefined} className={view === "dashboard" ? "active" : ""} onClick={() => setView("dashboard")}>
            <Activity size={18} /> Dashboard
          </button>
          <button
            aria-current={["outstandings", "companies"].includes(view) ? "page" : undefined}
            className={["outstandings", "companies"].includes(view) ? "active" : ""}
            onClick={() => setView(selectedCompanyReady ? "outstandings" : "companies")}
          >
            <Cable size={18} /> Tally
          </button>
          <button aria-current={view === "gst" ? "page" : undefined} className={view === "gst" ? "active" : ""} onClick={() => setView("gst")}>
            <FileText size={18} /> GST Returns
          </button>
          <button aria-current={view === "mirror" ? "page" : undefined} className={view === "mirror" ? "active" : ""} onClick={() => setView("mirror")}>
            <Database size={18} /> Mirror &amp; Proof
          </button>
          <button aria-current={view === "dsc" ? "page" : undefined} className={view === "dsc" ? "active" : ""} onClick={() => setView("dsc")}>
            <KeyRound size={18} /> DSC Token
          </button>
          <button aria-current={view === "documents" ? "page" : undefined} className={view === "documents" ? "active" : ""} onClick={() => setView("documents")}>
            <FolderOpen size={18} /> Documents
          </button>
          <button aria-current={view === "axal" ? "page" : undefined} className={view === "axal" ? "active" : ""} onClick={() => setView("axal")}>
            <Cloud size={18} /> AXAL Backend
          </button>
        </nav>
      </aside>

      <main className="content" id="main-content" ref={mainContentRef} tabIndex={-1} aria-labelledby="active-view-title">
        <header>
          <div>
            {view !== "companies" && <p className="eyebrow">{view === "outstandings" ? "Receivables and payables" : "Tally Truth Layer"}</p>}
            <h1 id="active-view-title">{VIEW_TITLES[view]}</h1>
          </div>
          {!["outstandings", "companies"].includes(view) && (
            <button className="primary" onClick={checkTally} disabled={tallyAction !== null}>
              <Cable size={18} />
              {tallyAction === "probe" ? "Checking endpoint..." : "Check Tally Endpoint"}
            </button>
          )}
        </header>

        {!["outstandings", "companies"].includes(view) && (
          <section className="company-context-bar" aria-label="Selected Tally company context">
            <div>
              <span>Selected company</span>
              <strong>{selectedCompanyRecord?.name ?? "None selected"}</strong>
            </div>
            <div>
              <span>Identity confidence</span>
              <strong>{selectedCompanyRecord?.mirror_company_id ? formatIdentifier(selectedCompanyRecord.identity_confidence ?? "unknown") : "Not established"}</strong>
            </div>
            <div>
              <span>Pinned evidence endpoint</span>
              <strong>{selectedCompanyRecord?.canonical_endpoint ?? "No persisted endpoint"}</strong>
            </div>
            <div>
              <span>Configured live endpoint</span>
              <strong>{config.host}:{config.port}</strong>
            </div>
            <div>
              <span>Current probe match</span>
              <strong>{selectedCompanyLive ? "Matched" : selectedCompanyRecord ? "Offline evidence only" : "Not selected"}</strong>
            </div>
            <button className="secondary-action" type="button" onClick={() => setView("companies")}>Manage Tally</button>
          </section>
        )}

        {discoveredCompanyPrompt && view !== "companies" && (
          <section className="company-discovery-notice" role="status" aria-live="polite">
            <div>
              <strong>{discoveredCompanyPrompt.heading}</strong>
              <span>{discoveredCompanyPrompt.detail}</span>
            </div>
            <button
              className="primary"
              type="button"
              onClick={() => {
                setView("companies");
              }}
            >
              {discoveredCompanyPrompt.actionLabel}
            </button>
          </section>
        )}

        {["dashboard", "mirror"].includes(view) && (
          <section className="operator-question-grid" aria-label="Tally operator summary">
            <article><span>Verified baseline</span><strong>{verifiedBaseline}</strong></article>
            <article><span>Latest attempt</span><strong>{latestAttemptSummary}</strong></article>
            <article><span>What is missing?</span><strong>{operatorMissing}</strong></article>
            <article><span>What should I do?</span><strong>{operatorNext}</strong></article>
          </section>
        )}

        {view === "dashboard" && (
          <>
            <section className="toolbar">
              <label>
                GST company
                <input
                  value={gstCompany}
                  onChange={(event) => {
                    setGstCompany(event.target.value);
                    setDraft(null);
                    tallyResultsVersion.current += 1;
                  }}
                />
              </label>
              <label>
                Financial year
                <input
                  value={gstFinancialYear}
                  placeholder="YYYY-YYYY"
                  onChange={(event) => {
                    setGstFinancialYear(event.target.value);
                    setDraft(null);
                    tallyResultsVersion.current += 1;
                  }}
                />
              </label>
              <button onClick={prepareDraft} disabled={busy}>
                <Play size={18} />
                Check GST Availability
              </button>
            </section>

            {dashboardError && <TallyErrorNotice message={dashboardError} />}

            <section className="grid">
              <article className="panel">
                <h2>Tally connection</h2>
                <dl>
                  <div><dt>Transport</dt><dd>{status ? (status.reachable ? "Endpoint reachable" : "Endpoint not reachable") : "Not checked"}</dd></div>
                  <div><dt>Compatibility</dt><dd>{status ? (status.compatible ? "Recognized Tally status; data capabilities not verified" : status.reachable ? "Endpoint responded but Tally compatibility was not recognized" : "Unavailable") : "Not checked"}</dd></div>
                  <div><dt>Status heuristic claim</dt><dd>{status?.product ?? "Unknown"}</dd></div>
                  <div><dt>Responder text</dt><dd>{status?.server_text || formatConnectionError(status?.error) || "Waiting for endpoint check"}</dd></div>
                </dl>
              </article>

              <article className="panel">
                <h2>GST preparation</h2>
                <dl>
                  <div><dt>Status</dt><dd>{gstDraftComplete ? "Calculated" : draft ? "Unavailable in this build" : "Not checked"}</dd></div>
                  <div><dt>Company</dt><dd>{draft?.company ?? "No result"}</dd></div>
                  <div><dt>GSTR-1 B2B</dt><dd>{gstDraftComplete ? draft.gstr1.b2b_invoice_count : "Not available"}</dd></div>
                  <div><dt>GSTR-3B taxable</dt><dd>{gstDraftComplete ? draft.gstr3b.outward_taxable_value : "Not available"}</dd></div>
                </dl>
              </article>

              <article className="panel wide passport-panel">
                <div className="panel-heading">
                  <div>
                    <h2>Capability Passport</h2>
                    <p className="panel-description">
                      Evidence from the latest read-only local endpoint probe. This does not establish responder authenticity, record completeness, or write permission.
                    </p>
                  </div>
                  <span>{passport ? `Profile v${passport.profile_version}` : "No current passport"}</span>
                </div>

                <div className="passport-summary">
                  <div>
                    <span>Product</span>
                    <strong>{passport?.product || status?.product || "Unknown"}</strong>
                    <small>{passport ? "Reported by this probe" : "Not observed"}</small>
                  </div>
                  <div>
                    <span>Release</span>
                    <strong>{passport?.release || "Unknown"}</strong>
                    <small>No release is inferred from product text</small>
                  </div>
                  <div>
                    <span>Mode</span>
                    <strong>{passport?.mode || "Unknown"}</strong>
                    <small>Education mode is labelled only when observed</small>
                  </div>
                  <div>
                    <span>Companies returned by current probe</span>
                    <strong>{passport ? liveCompanyKeys.length : "Unknown"}</strong>
                    <small>
                      {passport?.transports.xml_http?.safe_reason_code === "company_not_loaded"
                        ? "XML is active, but Tally reported that no company is loaded"
                        : passport
                          ? "Persisted offline pins are excluded; this is not a source-completeness count"
                          : "Probe the endpoint first"}
                    </small>
                  </div>
                  <div>
                    <span>Local capability observation</span>
                    <strong>{passportSnapshotId ? "Stored" : "Unknown"}</strong>
                    <small>
                      {passportSnapshotId
                        ? `Observation ID ${passportSnapshotId.slice(0, 8)}…`
                        : "No local capability observation stored"}
                    </small>
                  </div>
                  <div>
                    <span>Persisted company pins</span>
                    <strong>{persistedCompanyProfileTotal}</strong>
                    <small>{persistedCompanyProfilesTruncated ? `Newest ${persistedCompanyProfilesLoaded} loaded; local profile list is truncated` : "Available for local evidence review; excluded from current-probe counts"}</small>
                  </div>
                </div>

                <div className="passport-columns">
                  <section>
                    <h3>Transports</h3>
                    <CapabilityRows capabilities={passport?.transports} labels={TRANSPORT_LABELS} />
                  </section>
                  <section>
                    <h3>Capability packs</h3>
                    <p className="section-note">Pack support remains unknown until its declared fields and invariants are observed on this exact profile. Pack support does not establish a Verified accounting state.</p>
                    <CapabilityRows capabilities={passport?.packs} labels={PACK_LABELS} />
                  </section>
                  <section>
                    <h3>Observed features</h3>
                    <p className="section-note">Unknown is intentional when this exact endpoint has not supplied enough evidence. The connection probe never writes to Tally.</p>
                    <CapabilityRows capabilities={passport?.features} labels={FEATURE_LABELS} />
                  </section>
                </div>
              </article>
            </section>

            <section className="status-strip">
              <span>Serial Tally queue: configured, not compatibility proof</span>
              <span>
                Accounting mirror evidence: {passportSnapshotId ? "capability observation stored; record-proof status not loaded" : "no capability observation or proof status loaded"}
              </span>
              <span>DSC: token detection and certificate extraction</span>
            </section>
          </>
        )}

        {view === "outstandings" && (
          <OutstandingsScreen
            config={config}
            company={selectedCompanyReady && selectedCompanyRecord?.guid ? { name: selectedCompanyRecord.name, guid: selectedCompanyRecord.guid } : undefined}
            onChangeSetup={() => setView("companies")}
          />
        )}

        {view === "companies" && (
          <>
            <TallyReadinessFlow
              config={config}
              endpointReachable={Boolean(status?.reachable)}
              passportObserved={Boolean(passport)}
              companyReady={selectedCompanyReady}
              busy={tallyAction !== null}
              settingsLocked={snapshotActive}
              onHostChange={updateTallyHost}
              onPortChange={updateTallyPort}
              onCheck={checkTally}
            />

            {dashboardError && <TallyErrorNotice message={dashboardError} />}
            {companyError && !setupConnectionComplete && <TallyErrorNotice message={companyError} />}

            {setupConnectionComplete && (
              <section className="setup-company" id="company-profile" aria-labelledby="company-profile-heading">
                <div>
                  <h2 id="company-profile-heading">Choose a company</h2>
                  <p>Choose the company that is open in Tally. Bridge only reads from Tally.</p>
                </div>
                {companyError && <TallyErrorNotice message={companyError} />}
                {currentProbeCompanyList.length > 0 ? (
                  <>
                    <div className="company-options" role="list" aria-label="Companies found in Tally">
                      {currentProbeCompanyList.map((company) => {
                        const key = tallyCompanyKey(company);
                        const current = liveCompanyKeys.includes(key);
                        const selected = key === selectedCompany;
                        return (
                          <button
                            className={`company-option${selected ? " selected" : ""}`}
                            type="button"
                            key={key}
                            aria-pressed={selected}
                            disabled={!current || tallyAction !== null || snapshotActive}
                            onClick={() => {
                              if (key === selectedCompany) return;
                              clearSelectedCompanyScope({
                                preserveCurrentProbeReview: canReuseCurrentProbeReview({
                                  reviewAvailable: Boolean(reviewId && reviewCommitmentSha256),
                                  setupSaved: Boolean(passportSnapshotId),
                                }),
                              });
                              setSelectedCompany(key);
                            }}
                          >
                            <Building2 size={20} />
                            <span>{company.name}</span>
                            <small>{current ? "Open in Tally" : "Not open now"}</small>
                          </button>
                        );
                      })}
                    </div>
                  </>
                ) : untrustedDiscoveredCompanies.length > 0 ? (
                  <div className="company-options" role="list" aria-label="Companies to verify">
                    {untrustedDiscoveredCompanies.slice(0, TABLE_PREVIEW_LIMIT).map((company, index) => (
                      <button className="company-option" type="button" key={`${company.name}-${index}`} onClick={() => void bootstrapDirectCompany(company.name)} disabled={snapshotActive || tallyAction !== null}>
                        <Building2 size={20} />
                        <span>{company.name}</span>
                        <small>{tallyAction === "bootstrap" ? "Checking…" : "Use this company"}</small>
                      </button>
                    ))}
                  </div>
                ) : (
                  <div className="setup-empty-state">
                    <Building2 size={28} />
                    <p>{untrustedDiscoveryError ? "Bridge could not list companies from Tally." : "No companies were found."}</p>
                    <button className="secondary-action" type="button" onClick={() => void discoverUntrustedCompanies()} disabled={snapshotActive || tallyAction !== null}>
                      {tallyAction === "discover" ? "Checking Tally…" : "Find companies"}
                    </button>
                  </div>
                )}
                <div className="setup-company-footer">
                  {selectedCompany && !selectedCompanyLive ? <p>Open this company in Tally, then check Tally again.</p> : null}
                  {selectedCompanyLive && !selectedCompanyReady && (
                    <button className="primary" type="button" onClick={() => void saveReviewedTallySetup()} disabled={snapshotActive || tallyAction !== null || !passport || !reviewId || !reviewCommitmentSha256 || !selectedCompanyRecord?.guid}>
                      {tallyAction === "save" ? "Saving company…" : "Use this company"}
                    </button>
                  )}
                  {selectedCompanyReady && (
                    <>
                      <p className="setup-complete" role="status"><Check size={18} /> {selectedCompanyRecord?.name} is ready.</p>
                      <button className="primary" type="button" onClick={() => setView("outstandings")}>Open outstandings</button>
                    </>
                  )}
                </div>
              </section>
            )}

          </>
        )}

        {view === "gst" && (
          !draft || !gstDraftComplete ? (
            <article className="panel wide">
              <h2>GST calculation unavailable</h2>
              <div className="empty-state">
                <FileText size={32} />
                <strong>No verified GST draft</strong>
                <span>
                  {draft
                    ? draft.missing_fields.join(" ")
                    : "Use GST preparation on the dashboard to check availability. Zero values are not assumed."}
                </span>
              </div>
            </article>
          ) : (
            <section className="grid">
              <article className="panel">
                <h2>GSTR-1 draft</h2>
                <dl>
                  <div><dt>B2B invoices</dt><dd>{draft.gstr1.b2b_invoice_count}</dd></div>
                  <div><dt>B2C invoices</dt><dd>{draft.gstr1.b2c_invoice_count}</dd></div>
                  <div><dt>Credit/debit notes</dt><dd>{draft.gstr1.credit_debit_note_count}</dd></div>
                  <div><dt>HSN summaries</dt><dd>{draft.gstr1.hsn_summary_count}</dd></div>
                </dl>
              </article>
              <article className="panel">
                <h2>GSTR-3B draft</h2>
                <dl>
                  <div><dt>Taxable value</dt><dd>{draft.gstr3b.outward_taxable_value}</dd></div>
                  <div><dt>IGST</dt><dd>{draft.gstr3b.integrated_tax}</dd></div>
                  <div><dt>CGST</dt><dd>{draft.gstr3b.central_tax}</dd></div>
                  <div><dt>SGST</dt><dd>{draft.gstr3b.state_tax}</dd></div>
                </dl>
              </article>

            </section>
          )
        )}

        {view === "mirror" && (
          <>
            {savedCompanyList.length > 0 && (
              <section className="panel wide" aria-labelledby="saved-company-heading">
                <h2 id="saved-company-heading">{selectedCompanyRecord?.mirror_company_id ? "Saved company" : "Choose a saved company"}</h2>
                <p className="panel-description">Review local Mirror &amp; Proof evidence without contacting Tally.</p>
                {selectedCompanyRecord?.mirror_company_id ? (
                  <button className="secondary-action" type="button" onClick={() => selectSavedCompany("")} disabled={savedCompanySelectionLocked}>Change saved company</button>
                ) : (
                  <div className="company-options" role="list" aria-label="Saved companies">
                    {savedCompanyList.map((company) => {
                      const key = tallyCompanyKey(company);
                      return (
                        <button className="company-option" type="button" key={key} onClick={() => selectSavedCompany(key)} disabled={savedCompanySelectionLocked}>
                          <Building2 size={20} />
                          <span>{company.name}</span>
                          <small>Saved locally</small>
                        </button>
                      );
                    })}
                  </div>
                )}
              </section>
            )}
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
                <div className="table-shell">
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

            {selectedCompanyRecord?.mirror_company_id && (
              <details className="panel wide fixture-controls" aria-label="Synthetic write fixture">
                <summary>Synthetic write fixture (advanced)</summary>
                <div className="panel-heading">
                  <div>
                    <h2>Local fixture safety gate</h2>
                    <p className="panel-description">A revocable local gate for a future synthetic canary. It does not write to Tally and write capability remains Unknown.</p>
                  </div>
                </div>
                <dl>
                  <div><dt>Local state</dt><dd>{fixtureStatusError ? "Unavailable" : fixtureStatus ? formatIdentifier(fixtureStatus.fixture_state) : "Checking local state"}</dd></div>
                  <div><dt>Candidate gate</dt><dd>{fixtureStatus ? formatIdentifier(fixtureStatus.candidate_gate) : "Not checked"}</dd></div>
                  <div><dt>Write capability</dt><dd>Unknown</dd></div>
                </dl>
                {fixtureStatusError && (
                  <div className="toolbar secondary-toolbar">
                    <p className="privacy-warning" role="note">{fixtureStatusError}</p>
                    <button className="secondary-action" type="button" onClick={() => void refreshWriteFixtureStatus(selectedCompanyRecord.mirror_company_id!)} disabled={tallyAction !== null || snapshotActive}>Retry local fixture status</button>
                  </div>
                )}
                {fixtureStatus?.fixture_state === "active" ? (
                  <div className="toolbar secondary-toolbar">
                    <button className="secondary-action" type="button" onClick={() => void revokeWriteFixture()} disabled={snapshotActive || tallyAction !== null}>Revoke local fixture enrollment</button>
                  </div>
                ) : !reviewId || !reviewCommitmentSha256 || !selectedCompanyLive ? (
                  <div className="toolbar secondary-toolbar">
                    <p className="section-note">Check Tally again with this saved company open before locally enrolling a fixture.</p>
                    <button className="secondary-action" type="button" onClick={() => setView("companies")} disabled={snapshotActive || tallyAction !== null}>Prepare fixture review</button>
                  </div>
                ) : (
                  <>
                    <label><input type="checkbox" checked={fixtureDisposableAttested} disabled={tallyAction !== null || snapshotActive} onChange={(event) => setFixtureDisposableAttested(event.target.checked)} /> This is a dedicated disposable synthetic company.</label>
                    <label><input type="checkbox" checked={fixtureNoCustomerDataAttested} disabled={tallyAction !== null || snapshotActive} onChange={(event) => setFixtureNoCustomerDataAttested(event.target.checked)} /> No customer, personal, or production data will be used.</label>
                    <label><input type="checkbox" checked={fixtureBackupGuidanceAcknowledged} disabled={tallyAction !== null || snapshotActive} onChange={(event) => setFixtureBackupGuidanceAcknowledged(event.target.checked)} /> I have created and checked an offline backup before any later canary.</label>
                    <div className="toolbar secondary-toolbar">
                      <button className="secondary-action" type="button" onClick={() => void enrollWriteFixture()} disabled={snapshotActive || tallyAction !== null || !fixtureStatus || !!fixtureStatusError || !fixtureDisposableAttested || !fixtureNoCustomerDataAttested || !fixtureBackupGuidanceAcknowledged}>
                        {tallyAction === "fixture_enroll" ? "Enrolling locally…" : "Enroll local synthetic fixture"}
                      </button>
                    </div>
                  </>
                )}
              </details>
            )}

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
                <div className="table-wrap">
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
        )}

        {view === "dsc" && (
          <>
            <section className="toolbar">
              <label>
                Token PIN
                <input
                  type="password"
                  value={dscPin}
                  autoComplete="off"
                  onChange={(event) => setDscPin(event.target.value)}
                />
              </label>
              <button onClick={() => runDsc(true)} disabled={busy}>
                <RefreshCw size={18} className={dscAction === "detect" ? "spin" : ""} />
                {dscAction === "detect" ? "Detecting..." : "Detect Token"}
              </button>
              <button onClick={() => runDsc(false)} disabled={busy || !dscPin.trim()}>
                <KeyRound size={18} className={dscAction === "extract" ? "pulse-icon" : ""} />
                {dscAction === "extract" ? "Extracting..." : "Extract Certificates"}
              </button>
            </section>

            {dscError && <div className="error-banner">{dscError}</div>}

            <section className="grid single-panel-grid">
              <article className="panel certificate-panel">
                <h2>Certificate summary</h2>
                {dscAction ? (
                  <div className="empty-state compact">
                    <RefreshCw size={32} className="spin" />
                    <strong>{dscAction === "detect" ? "Detecting token" : "Reading certificate"}</strong>
                    <span>This can take a few seconds while the token library initializes.</span>
                  </div>
                ) : primaryCertificate ? (
                  <dl>
                    <div><dt>Client</dt><dd>{primaryCertificate.common_name || primaryCertificate.organization || primaryCertificate.label}</dd></div>
                    <div><dt>Expiry</dt><dd>{primaryCertificate.valid_to || "Unknown"}</dd></div>
                    <div><dt>Serial</dt><dd>{primaryCertificate.serial_number || "Unknown"}</dd></div>
                    <div><dt>Provider</dt><dd>{successfulDscAttempt?.token_type ?? "Unknown"}</dd></div>
                    <div><dt>Certificates</dt><dd>{successfulDscAttempt?.certificate_count ?? successfulDscAttempt?.certificates.length ?? 0}</dd></div>
                    <div><dt>AXAL sync</dt><dd>{dscSync?.message || "Not synced"}</dd></div>
                  </dl>
                ) : detectedDscAttempt ? (
                  <div className="empty-state compact success-state">
                  <KeyRound size={32} />
                  <strong>Token detected</strong>
                  <span>{detectedDscAttempt.token_type} token is available. Extract certificates to show holder details.</span>
                  </div>
                ) : (
                  <div className="empty-state compact">
                  <KeyRound size={32} />
                  <strong>No certificate loaded</strong>
                  <span>Detect the token or extract certificates to show DSC holder details.</span>
                  </div>
                )}
                {primaryCertificate && (
                  <div className="panel-actions">
                    <button onClick={syncDscCertificate} disabled={busy || dscSyncing || !axalConnection || axalSession?.integration !== "dsc"}>
                      <Cloud size={18} className={dscSyncing ? "pulse-icon" : ""} />
                      {dscSyncing ? "Syncing..." : "Sync Certificate"}
                    </button>
                  </div>
                )}
                {(dscReport || dscDetectReport) && (
                  <div className="panel-actions">
                    <button onClick={clearDscSensitiveState} disabled={busy || dscSyncing}>
                      Clear certificate details
                    </button>
                    <span>Certificate and token details clear automatically after five minutes.</span>
                  </div>
                )}
              </article>
            </section>
          </>
        )}

        {view === "documents" && (
          <>
            <section className="toolbar document-toolbar">
              <button onClick={chooseDocumentFiles} disabled={busy}>
                <FileText size={18} />
                Choose Files
              </button>
              <button onClick={chooseDocumentFolder} disabled={busy}>
                <FolderOpen size={18} />
                Choose Folder
              </button>
              <button onClick={clearDocuments} disabled={busy || (documentPaths.length === 0 && !documentScan)}>
                Clear
              </button>
              <button onClick={scanDocuments} disabled={busy || documentPaths.length === 0}>
                <RefreshCw size={18} className={documentAction === "scan" ? "spin" : ""} />
                {documentAction === "scan" ? "Scanning..." : "Scan"}
              </button>
              <button onClick={syncDocuments} disabled={busy || !documentScan?.files.length || !axalConnection || axalSession?.integration !== "documents"}>
                <UploadCloud size={18} className={documentAction === "sync" ? "pulse-icon" : ""} />
                {documentAction === "sync" ? "Syncing..." : "Sync Documents"}
              </button>
            </section>

            {documentError && <div className="error-banner">{documentError}</div>}

            <article className="panel wide selected-paths">
              <div className="panel-heading">
                <h2>Selected paths</h2>
                <span>{documentPaths.length} selected</span>
              </div>
              {documentPaths.length === 0 ? (
                <div className="empty-state compact">
                  <FolderOpen size={32} />
                  <strong>No paths selected</strong>
                  <span>Choose files or a folder before scanning.</span>
                </div>
              ) : (
                <div className="path-list">
                  {documentPaths.map((path) => (
                    <div key={path.selectionId}>{path.displayName}</div>
                  ))}
                </div>
              )}
            </article>

            <section className="grid">
              <article className="panel">
                <h2>Scan summary</h2>
                {documentAction === "scan" ? (
                  <div className="empty-state compact">
                    <RefreshCw size={32} className="spin" />
                    <strong>Scanning documents</strong>
                    <span>Hashing files and preparing document metadata.</span>
                  </div>
                ) : (
                  <dl>
                    <div><dt>Files</dt><dd>{documentScan?.files.length ?? 0}</dd></div>
                    <div><dt>Total size</dt><dd>{formatBytes(documentScan?.totalSize ?? 0)}</dd></div>
                    <div><dt>Skipped</dt><dd>{documentScan?.skipped.length ?? 0}</dd></div>
                    <div><dt>Workspace</dt><dd>{axalConnection?.workspace.name || "Check AXAL status first"}</dd></div>
                  </dl>
                )}
              </article>

              <article className="panel">
                <h2>Sync summary</h2>
                {documentAction === "sync" ? (
                  <div className="empty-state compact">
                    <UploadCloud size={32} className="pulse-icon" />
                    <strong>Uploading documents</strong>
                    <span>Requesting upload URLs, sending files, and confirming the batch.</span>
                  </div>
                ) : (
                  <dl>
                    <div><dt>Status</dt><dd>{documentSync ? (documentSync.success ? "Complete" : "Partial") : "Not synced"}</dd></div>
                    <div><dt>Uploaded</dt><dd>{documentSync?.uploadedFiles.length ?? 0}</dd></div>
                    <div><dt>Failed</dt><dd>{documentSync?.failedFiles.length ?? 0}</dd></div>
                    <div><dt>Duplicates</dt><dd>{documentSync?.duplicateCount ?? 0}</dd></div>
                  </dl>
                )}
              </article>
            </section>

            <article className="panel wide data-grid">
              <div className="panel-heading">
                <h2>Files</h2>
                <span>{formatPreviewCount(documentScan?.files.length ?? 0, "ready")}</span>
              </div>
              {!documentScan?.files.length ? (
                <div className="empty-state compact">
                  <FolderOpen size={32} />
                  <strong>No files scanned</strong>
                  <span>Enter one or more file/folder paths, then scan.</span>
                </div>
              ) : (
                <div className="table-wrap">
                  <table>
                    <thead>
                      <tr>
                        <th>Path</th>
                        <th>Type</th>
                        <th>Size</th>
                        <th>Hash</th>
                      </tr>
                    </thead>
                    <tbody>
                      {documentScan.files.slice(0, TABLE_PREVIEW_LIMIT).map((file) => (
                        <tr key={file.scanId}>
                          <td>{file.relativePath}</td>
                          <td>{file.mimeType}</td>
                          <td>{formatBytes(file.size)}</td>
                          <td>{file.contentHash ? `${file.contentHash.slice(0, 12)}...` : "-"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </article>
          </>
        )}

        {view === "axal" && (
          <>
            <section className="toolbar">
              <label>
                Base URL
                <input value={axalBaseUrl} onChange={(event) => { setAxalBaseUrl(event.target.value); invalidateAxalSession(); }} />
              </label>
              <label>
                Integration
                <select value={axalIntegration} onChange={(event) => { setAxalIntegration(event.target.value as AxalIntegration); invalidateAxalSession(); }}>
                  <option value="tally">Tally Prime</option>
                  <option value="documents">Document Sync</option>
                  <option value="dsc">DSC Management</option>
                </select>
              </label>
            </section>

            <section className="toolbar secondary-toolbar">
              <label>
                API ID
                <input value={axalApiId} onChange={(event) => { setAxalApiId(event.target.value); invalidateAxalSession(); }} />
              </label>
              <label>
                API Key
                <input type="password" value={axalApiKey} onChange={(event) => { setAxalApiKey(event.target.value); invalidateAxalSession(); }} />
              </label>
              <button onClick={validateAxal} disabled={busy || !axalApiId || !axalApiKey}>
                <RefreshCw size={18} className={axalAction === "validate" ? "spin" : ""} />
                {axalAction === "validate" ? "Validating..." : "Validate"}
              </button>
              <button onClick={checkAxalStatus} disabled={busy || !axalSession}>
                <Cloud size={18} className={axalAction === "status" ? "pulse-icon" : ""} />
                {axalAction === "status" ? "Checking..." : "Check Status"}
              </button>
            </section>

            {axalError && <div className="error-banner">{axalError}</div>}

            <section className="grid">
              <article className="panel">
                <h2>Credential validation</h2>
                {axalAction === "validate" ? (
                  <div className="empty-state compact">
                    <RefreshCw size={32} className="spin" />
                    <strong>Validating credentials</strong>
                    <span>Checking the API key against AXAL.</span>
                  </div>
                ) : (
                  <dl>
                    <div><dt>Status</dt><dd>{axalValidation ? (axalValidation.valid ? "Valid" : "Invalid") : "Not checked"}</dd></div>
                    <div><dt>Server state</dt><dd>{axalValidation?.status || "-"}</dd></div>
                    <div><dt>Last synced</dt><dd>{axalValidation?.last_synced || "-"}</dd></div>
                    <div><dt>Error</dt><dd>{axalValidation?.error || "-"}</dd></div>
                  </dl>
                )}
              </article>

              <article className="panel">
                <h2>Workspace status</h2>
                {axalAction === "status" ? (
                  <div className="empty-state compact">
                    <RefreshCw size={32} className="spin" />
                    <strong>Checking workspace</strong>
                    <span>Fetching integration status and workspace metadata.</span>
                  </div>
                ) : (
                  <dl>
                    <div><dt>Connection</dt><dd>{axalConnection ? (axalConnection.connected ? "Connected" : "Disconnected") : "Not checked"}</dd></div>
                    <div><dt>Status</dt><dd>{axalConnection?.status || "-"}</dd></div>
                    <div><dt>Workspace</dt><dd>{axalConnection?.workspace.name || "-"}</dd></div>
                    <div><dt>Plan</dt><dd>{axalConnection?.workspace.billing_plan || "-"}</dd></div>
                    <div><dt>Storage</dt><dd>{axalConnection ? `${formatBytes(axalConnection.workspace.storage_used)} / ${formatBytes(axalConnection.workspace.storage_limit)}` : "-"}</dd></div>
                    <div><dt>Last synced</dt><dd>{axalConnection?.last_synced_at || "-"}</dd></div>
                  </dl>
                )}
              </article>
            </section>
          </>
        )}
      </main>
    </div>
  );
}

type RootContainer = HTMLElement & {
  bridgeRoot?: ReturnType<typeof ReactDOM.createRoot>;
};

const rootContainer = document.getElementById("root") as RootContainer;
const root = rootContainer.bridgeRoot ?? ReactDOM.createRoot(rootContainer);
rootContainer.bridgeRoot = root;
root.render(<App />);

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

function toTallyDate(value: string): string {
  return value.replace(/-/g, "");
}

function formatTallyDate(value?: string): string {
  if (!value || value.length !== 8) {
    return value || "-";
  }

  return `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6, 8)}`;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatCapabilityEvidence(evidence?: CapabilityEvidence): string {
  if (!evidence) return "Unknown; qualification evidence unavailable";
  const reason = evidence.safe_reason_code
    ? CAPABILITY_REASON_LABELS[evidence.safe_reason_code] ?? formatIdentifier(evidence.safe_reason_code)
    : "No reason supplied";
  return `${formatIdentifier(evidence.state)} / ${formatIdentifier(evidence.confidence)} — ${reason}`;
}

function formatPreviewCount(total: number, label = "loaded"): string {
  return `Showing ${Math.min(total, TABLE_PREVIEW_LIMIT)} of ${total} returned ${label}; source completeness not established`;
}

function mergeTallyCompanies(preferred: TallyCompany[], existing: TallyCompany[]): TallyCompany[] {
  const merged = new Map<string, TallyCompany>();
  for (const company of existing) merged.set(tallyCompanyKey(company), company);
  for (const company of preferred) {
    const key = tallyCompanyKey(company);
    const current = merged.get(key);
    merged.set(key, {
      ...current,
      ...company,
      guid: company.guid ?? current?.guid,
      guid_observed: company.guid_observed ?? current?.guid_observed,
      mirror_company_id: company.mirror_company_id ?? current?.mirror_company_id,
      correlation_key: company.correlation_key ?? current?.correlation_key,
      canonical_endpoint: company.canonical_endpoint ?? current?.canonical_endpoint,
      last_observed_at_unix_ms: company.last_observed_at_unix_ms ?? current?.last_observed_at_unix_ms,
    });
  }
  return Array.from(merged.values()).sort((left, right) => left.name.localeCompare(right.name));
}

function getCurrentFinancialYear(now = new Date()): { label: string; from: string; to: string } {
  const year = now.getFullYear();
  const startYear = now.getMonth() >= 3 ? year : year - 1;
  const endYear = startYear + 1;
  return {
    label: `${startYear}-${endYear}`,
    from: `${startYear}-04-01`,
    to: `${endYear}-03-31`,
  };
}

function toErrorMessage(error: unknown): string {
  const normalized = toOperatorError(error);
  return typeof normalized === "string"
    ? normalized
    : `${normalized.category}: ${normalized.message} [${normalized.code}]. ${normalized.remediation}`;
}

function toOperatorError(error: unknown): OperatorError {
  if (isTallyCommandErrorEnvelope(error)) return error;
  return error instanceof Error ? error.message : String(error);
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

function formatConnectionError(code?: string): string {
  const labels: Record<string, string> = {
    request_cancelled: "The read-only endpoint request was cancelled.",
    endpoint_queue_deadline_exceeded: "The local endpoint queue deadline was exceeded.",
    endpoint_circuit_open: "The local endpoint circuit is temporarily open.",
    response_size_limit_exceeded: "The endpoint response exceeded Bridge's safety limit.",
    response_encoding_invalid: "The endpoint response encoding was invalid.",
    endpoint_unreachable: "The local Tally endpoint is unreachable.",
  };
  return code ? labels[code] ?? "The local Tally endpoint check failed safely." : "";
}
