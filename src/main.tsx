import React from "react";
import ReactDOM from "react-dom/client";
import { Activity, Building2, Cable, Check, Cloud, Database, FileText, FolderOpen, KeyRound, Play, ShieldCheck } from "lucide-react";
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
import { AllClientsScreen } from "./AllClientsScreen";
import {
  automaticOutstandingsAsOf,
  millisecondsUntilNextLocalMidnight,
  operatorSelectedOutstandingsAsOf,
  refreshAutomaticOutstandingsAsOf,
} from "./outstandings-as-of";
import { GstScreen } from "./GstScreen";
import { DscScreen } from "./DscScreen";
import { DocumentsScreen } from "./DocumentsScreen";
import { AxalScreen } from "./AxalScreen";
import { MirrorProofScreen } from "./MirrorProofScreen";
import { ErrorBoundary } from "./ErrorBoundary";
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

export type GstReturnDraft = {
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

type AxalIntegration = "tally" | "documents" | "dsc";

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

type View = "dashboard" | "clients" | "outstandings" | "companies" | "gst" | "mirror" | "dsc" | "documents" | "axal";
type TallyAction = "probe" | "discover" | "bootstrap" | "save" | "fixture_enroll" | "fixture_revoke" | "evidence" | "explorer" | "start" | "resume" | "cancel";

const TABLE_PREVIEW_LIMIT = 100;
const MIRROR_PAGE_LIMIT = 25;

const VIEW_TITLES: Record<View, string> = {
  dashboard: "Tally evidence dashboard",
  clients: "All clients",
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
  // Every company Tally reports as open. Kept separate from
  // `untrustedDiscoveredCompanies`, which is deliberately cleared once a
  // company verifies -- clearing that list is what made the other open books
  // disappear from the UI the moment one was chosen.
  const [openCompanyNames, setOpenCompanyNames] = React.useState<string[]>([]);
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
  // Owned by App() and shared with the DSC, Documents, and AXAL views --
  // AxalScreen both reads and writes these two (see its Props comment).
  const [axalSession, setAxalSession] = React.useState<{ id: string; integration: AxalIntegration } | null>(null);
  const [axalConnection, setAxalConnection] = React.useState<AxalConnectionStatus | null>(null);
  const [view, setView] = React.useState<View>("dashboard");
  const [outstandingsAsOfSelection, setOutstandingsAsOfSelection] = React.useState(
    () => automaticOutstandingsAsOf(),
  );
  const [busy, setBusy] = React.useState(false);
  const [tallyAction, setTallyAction] = React.useState<TallyAction | null>(null);
  const tallyResultsVersion = React.useRef(0);
  const proofPreviewRequestVersion = React.useRef(0);
  const snapshotSelectionVersion = React.useRef(0);
  const mainContentRef = React.useRef<HTMLElement>(null);

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

  // Both of these are backed by the encrypted mirror, and touching the mirror
  // resolves its key from the OS keychain -- which prompts. Running them on
  // mount meant every launch prompted before the operator had done anything,
  // and it defeated the lazy mirror initialisation in the Rust layer entirely.
  // Fetch each only once a view that actually needs it is open.
  React.useEffect(() => {
    // Mirror only. The app opens on the dashboard, so including it here would
    // have kept the boot-time keychain prompt exactly as it was. The dashboard's
    // "latest attempt" line derives from `snapshotJob`, which a user action
    // sets -- it does not need the recent-runs list to render.
    if (view !== "mirror") return;
    void refreshRecentSnapshots();
  }, [view, refreshRecentSnapshots]);

  React.useEffect(() => {
    if (view !== "companies" && view !== "outstandings" && view !== "clients") return;
    void refreshPersistedCompanyProfiles();
  }, [view, refreshPersistedCompanyProfiles]);

  React.useEffect(() => {
    if (view !== "outstandings" && view !== "clients") return;

    let timer: number | undefined;
    const refreshAutomaticDate = () => {
      setOutstandingsAsOfSelection((current) => refreshAutomaticOutstandingsAsOf(current));
    };
    const scheduleLocalMidnightRefresh = () => {
      timer = window.setTimeout(() => {
        refreshAutomaticDate();
        scheduleLocalMidnightRefresh();
      }, millisecondsUntilNextLocalMidnight());
    };

    // Entering the workflow must not reuse a date from a previous local day;
    // the helper leaves a deliberate operator choice untouched.
    refreshAutomaticDate();
    scheduleLocalMidnightRefresh();
    return () => {
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [view]);

  const changeOutstandingsAsOf = React.useCallback((value: string) => {
    setOutstandingsAsOfSelection(operatorSelectedOutstandingsAsOf(value));
  }, []);

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
        setOpenCompanyNames(discovered.map((candidate) => candidate.name));
              setOpenCompanyNames(discovered.map((candidate) => candidate.name));
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
        setOpenCompanyNames(discovered.map((candidate) => candidate.name));
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
            // MERGE, never replace. This probe is scoped to one company via
            // SVCURRENTCOMPANY, so it returns exactly one row by design --
            // replacing the live set with it made Bridge forget every other
            // book open in Tally the moment a company was chosen, which is
            // what reduced the all-clients read to "1 of 1 book".
            setLiveCompanyKeys((current) =>
              Array.from(new Set([...current, ...nextLiveCompanyKeys])));
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

  const gstDraftComplete = draft !== null && draft.missing_fields.length === 0;
  const selectedCompanyRecord = companies.find((company) => tallyCompanyKey(company) === selectedCompany);
  const selectedCompanyLive = !!selectedCompanyRecord && liveCompanyKeys.includes(tallyCompanyKey(selectedCompanyRecord));
  const currentProbeCompanyList = currentProbeCompanies(companies, liveCompanyKeys);
  // Open in Tally but not already offered as a verified choice. Compared by
  // name because a discovered candidate has no GUID until it is verified --
  // establishing that GUID is exactly what choosing it does.
  const verifiedCompanyNames = new Set(currentProbeCompanyList.map((company) => company.name));
  const otherOpenCompanies = openCompanyNames
    .filter((name) => !verifiedCompanyNames.has(name))
    .map((name) => ({ name }));
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
            aria-current={["outstandings", "companies", "clients"].includes(view) ? "page" : undefined}
            className={["outstandings", "companies", "clients"].includes(view) ? "active" : ""}
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
            {view !== "companies" && (
              <p className="eyebrow">
                {view === "outstandings"
                  ? "Receivables and payables"
                  : view === "clients"
                    ? "Every book open in Tally"
                    : "Tally Truth Layer"}
              </p>
            )}
            <h1 id="active-view-title">{VIEW_TITLES[view]}</h1>
          </div>
          {!["outstandings", "companies", "clients"].includes(view) && (
            <button className="primary" onClick={checkTally} disabled={tallyAction !== null}>
              <Cable size={18} />
              {tallyAction === "probe" ? "Checking endpoint..." : "Check Tally Endpoint"}
            </button>
          )}
        </header>

        {!["outstandings", "companies", "clients"].includes(view) && (
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
          <ErrorBoundary key="dashboard" label="Tally evidence dashboard">
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
          </ErrorBoundary>
        )}

        {view === "clients" && (
          <ErrorBoundary key="clients" label="All clients">
          <AllClientsScreen
            config={config}
            asOf={outstandingsAsOfSelection.value}
            /* Every company Tally reports open, GUID-verified by the probe --
               NOT only the ones already saved. Requiring a save first was
               correct while company discovery could never be trusted
               (CompanyListV1 returned no HEADER/STATUS); with CompanyListV2
               the probe verifies every open book on every run, and a firm
               holding ten client books will not bless each one before a
               cross-client screen works. */
            companies={currentProbeCompanyList
              .filter((company) => company.guid)
              .map((company) => ({ name: company.name, guid: company.guid as string }))}
            onOpenCompany={(company) => {
              setSelectedCompany(tallyCompanyKey({ name: company.name, guid: company.guid }));
              setView("outstandings");
            }}
            onBack={() => setView("outstandings")}
          />
          </ErrorBoundary>
        )}

        {view === "outstandings" && (
          <ErrorBoundary key="outstandings" label="Aged outstandings">
          <OutstandingsScreen
            config={config}
            company={selectedCompanyReady && selectedCompanyRecord?.guid ? { name: selectedCompanyRecord.name, guid: selectedCompanyRecord.guid } : undefined}
            onChangeSetup={() => setView("companies")}
            onViewAllClients={() => setView("clients")}
            openBookCount={currentProbeCompanyList.filter((entry) => entry.guid).length}
            asOf={outstandingsAsOfSelection.value}
            onAsOfChange={changeOutstandingsAsOf}
          />
          </ErrorBoundary>
        )}

        {view === "companies" && (
          <ErrorBoundary key="companies" label="Connect Tally">
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
                {/* Switching to another client's book must not require noticing
                    that "Check Tally again" repopulates a hidden list.
                    Bridge's company report carries no HEADER/STATUS, so the
                    probe can never mark it trusted; the verified picker above
                    therefore only ever lists companies already SAVED, and
                    every other open book was unreachable once one was saved.
                    These stay a visually separate, clearly-unverified group --
                    choosing one runs the scoped bootstrap that verifies it. */}
                {currentProbeCompanyList.length > 0 && otherOpenCompanies.length > 0 && (
                  <div className="company-more">
                    <h3>Other companies open in Tally</h3>
                    <p>Bridge verifies a company&rsquo;s identity when you choose it.</p>
                    <div className="company-options" role="list" aria-label="Other companies open in Tally">
                      {otherOpenCompanies.slice(0, TABLE_PREVIEW_LIMIT).map((company, index) => (
                        <button
                          className="company-option"
                          type="button"
                          key={`other-${company.name}-${index}`}
                          onClick={() => void bootstrapDirectCompany(company.name)}
                          disabled={snapshotActive || tallyAction !== null}
                        >
                          <Building2 size={20} />
                          <span>{company.name}</span>
                          <small>{tallyAction === "bootstrap" ? "Checking…" : "Switch to this company"}</small>
                        </button>
                      ))}
                    </div>
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
          </ErrorBoundary>
        )}

        {view === "gst" && (
          <ErrorBoundary key="gst" label="GST return readiness">
          <GstScreen draft={draft} />
          </ErrorBoundary>
        )}

        {view === "mirror" && (
          <ErrorBoundary key="mirror" label="Accounting mirror and proof">
          <MirrorProofScreen
            config={config}
            status={status}
            passport={passport}
            tallyAction={tallyAction}
            selectedCompanyRecord={selectedCompanyRecord}
            selectedCompanyLive={selectedCompanyLive}
            savedCompanyPicker={savedCompanyList.length > 0 && (
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
            companyError={companyError}
            fixtureControls={selectedCompanyRecord?.mirror_company_id && (
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
            voucherFrom={voucherFrom}
            setVoucherFrom={setVoucherFrom}
            voucherTo={voucherTo}
            setVoucherTo={setVoucherTo}
            syncEvidence={syncEvidence}
            syncEvidenceError={syncEvidenceError}
            refreshSyncEvidence={refreshSyncEvidence}
            latestProof={latestProof}
            mirrorTruthState={mirrorTruthState}
            snapshotJob={snapshotJob}
            setSnapshotJob={setSnapshotJob}
            snapshotSelectionVersion={snapshotSelectionVersion}
            snapshotActive={snapshotActive}
            snapshotError={snapshotError}
            snapshotStartOutcomeUnknown={snapshotStartOutcomeUnknown}
            setSnapshotStartOutcomeUnknown={setSnapshotStartOutcomeUnknown}
            startCoreSnapshot={startCoreSnapshot}
            cancelCoreSnapshot={cancelCoreSnapshot}
            resumeCoreSnapshot={resumeCoreSnapshot}
            selectedRecentSnapshotRuns={selectedRecentSnapshotRuns}
            refreshRecentSnapshots={refreshRecentSnapshots}
            inspectedJob={inspectedJob}
            activeGapCodes={activeGapCodes}
            activeWarningCodes={activeWarningCodes}
            mirrorExplorer={mirrorExplorer}
            mirrorExplorerError={mirrorExplorerError}
            loadMirrorExplorerPage={loadMirrorExplorerPage}
            proofPreview={proofPreview}
            proofPreviewSelection={proofPreviewSelection}
            previewRedactedProof={previewRedactedProof}
            runtimeSessions={runtimeSessions}
            runtimeError={runtimeError}
            refreshRuntime={refreshRuntime}
            cancelTallyRequest={cancelTallyRequest}
          />
          </ErrorBoundary>
        )}

        {view === "dsc" && (
          <ErrorBoundary key="dsc" label="DSC token">
          <DscScreen busy={busy} setBusy={setBusy} axalConnection={axalConnection} axalSession={axalSession} />
          </ErrorBoundary>
        )}

        {view === "documents" && (
          <ErrorBoundary key="documents" label="Documents">
          <DocumentsScreen busy={busy} setBusy={setBusy} axalConnection={axalConnection} axalSession={axalSession} />
          </ErrorBoundary>
        )}

        {view === "axal" && (
          <ErrorBoundary key="axal" label="AXAL backend">
          <AxalScreen
            busy={busy}
            setBusy={setBusy}
            axalConnection={axalConnection}
            axalSession={axalSession}
            setAxalSession={setAxalSession}
            setAxalConnection={setAxalConnection}
          />
          </ErrorBoundary>
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

function toTallyDate(value: string): string {
  return value.replace(/-/g, "");
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
