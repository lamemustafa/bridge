export type TallyConfig = {
  host: string;
  port: number;
};

export type ConnectionStatus = {
  reachable: boolean;
  compatible: boolean;
  server_text: string;
  product: "TallyPrime" | "Tally ERP 9" | "Unknown";
  error?: string;
};

export type TallyCompany = {
  name: string;
  guid?: string;
  company_number?: string;
  books_from_yyyymmdd?: string;
  guid_observed?: boolean;
  mirror_company_id?: string;
  correlation_key?: string;
  identity_confidence?: "observed" | "unknown";
  canonical_endpoint?: string;
  last_observed_at_unix_ms?: number;
};

export type TallyProofSummary = {
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

export type TallySyncEvidence = {
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

export type RedactedProofPreview = {
  json: string;
  payload_sha256: string;
};

export type MirrorExplorerPage = {
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

export type SnapshotJobStatus = {
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

export type TallyRuntimeSnapshot = {
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

export type TallyAction = "probe" | "discover" | "bootstrap" | "save" | "fixture_enroll" | "fixture_revoke" | "evidence" | "explorer" | "start" | "resume" | "cancel";
