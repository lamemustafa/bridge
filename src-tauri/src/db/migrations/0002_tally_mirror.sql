CREATE TABLE IF NOT EXISTS tally_schema_migrations (
  version INTEGER PRIMARY KEY,
  description TEXT NOT NULL,
  applied_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tally_endpoints (
  id TEXT PRIMARY KEY,
  canonical_origin TEXT NOT NULL UNIQUE,
  created_at_unix_ms INTEGER NOT NULL,
  last_observed_at_unix_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tally_capability_snapshots (
  id TEXT PRIMARY KEY,
  endpoint_id TEXT NOT NULL,
  observed_at_unix_ms INTEGER NOT NULL,
  profile_version INTEGER NOT NULL CHECK (profile_version > 0),
  product TEXT NOT NULL,
  release TEXT,
  mode TEXT,
  mode_confidence TEXT NOT NULL CHECK (
    mode_confidence IN ('documented', 'observed', 'inferred', 'unknown')
  ),
  FOREIGN KEY (endpoint_id) REFERENCES tally_endpoints(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_capability_snapshots_endpoint_observed
  ON tally_capability_snapshots(endpoint_id, observed_at_unix_ms DESC);

CREATE TABLE IF NOT EXISTS tally_capability_items (
  snapshot_id TEXT NOT NULL,
  capability_kind TEXT NOT NULL CHECK (
    capability_kind IN ('transport', 'pack', 'feature')
  ),
  capability_key TEXT NOT NULL,
  capability_state TEXT NOT NULL CHECK (
    capability_state IN ('supported', 'unsupported', 'unknown', 'not_configured')
  ),
  confidence TEXT NOT NULL CHECK (
    confidence IN ('documented', 'observed', 'inferred', 'unknown')
  ),
  safe_reason_code TEXT,
  PRIMARY KEY (snapshot_id, capability_kind, capability_key),
  FOREIGN KEY (snapshot_id) REFERENCES tally_capability_snapshots(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_companies (
  id TEXT PRIMARY KEY,
  endpoint_id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  company_guid TEXT,
  remote_id TEXT,
  master_id TEXT,
  fallback_fingerprint TEXT,
  identity_confidence TEXT NOT NULL CHECK (
    identity_confidence IN ('documented', 'observed', 'inferred', 'unknown')
  ),
  first_observed_at_unix_ms INTEGER NOT NULL,
  last_observed_at_unix_ms INTEGER NOT NULL,
  CHECK (
    company_guid IS NOT NULL OR remote_id IS NOT NULL OR
    master_id IS NOT NULL OR fallback_fingerprint IS NOT NULL
  ),
  FOREIGN KEY (endpoint_id) REFERENCES tally_endpoints(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_companies_guid
  ON tally_companies(endpoint_id, company_guid) WHERE company_guid IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_companies_remote_id
  ON tally_companies(endpoint_id, remote_id) WHERE remote_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_companies_master_id
  ON tally_companies(endpoint_id, master_id) WHERE master_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_companies_fallback
  ON tally_companies(endpoint_id, fallback_fingerprint)
  WHERE fallback_fingerprint IS NOT NULL;

CREATE TABLE IF NOT EXISTS tally_observation_batches (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  capability_snapshot_id TEXT NOT NULL,
  company_id TEXT NOT NULL,
  pack_id TEXT NOT NULL,
  pack_schema_major INTEGER NOT NULL CHECK (pack_schema_major >= 0),
  pack_schema_minor INTEGER NOT NULL CHECK (pack_schema_minor >= 0),
  source_transport TEXT NOT NULL,
  source_release TEXT,
  requested_from_yyyymmdd TEXT,
  requested_to_yyyymmdd TEXT,
  started_at_unix_ms INTEGER NOT NULL,
  completed_at_unix_ms INTEGER,
  state TEXT NOT NULL CHECK (state IN ('staging', 'verified', 'partial', 'failed')),
  snapshot_sha256 TEXT,
  accepted_records INTEGER NOT NULL DEFAULT 0 CHECK (accepted_records >= 0),
  rejected_records INTEGER NOT NULL DEFAULT 0 CHECK (rejected_records >= 0),
  UNIQUE (run_id, pack_id),
  FOREIGN KEY (capability_snapshot_id) REFERENCES tally_capability_snapshots(id) ON DELETE RESTRICT,
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_batches_company_pack_started
  ON tally_observation_batches(company_id, pack_id, started_at_unix_ms DESC);

CREATE TABLE IF NOT EXISTS tally_source_records (
  id TEXT PRIMARY KEY,
  company_id TEXT NOT NULL,
  object_type TEXT NOT NULL,
  display_name TEXT,
  source_guid TEXT,
  remote_id TEXT,
  master_id TEXT,
  fallback_fingerprint TEXT,
  identity_confidence TEXT NOT NULL CHECK (
    identity_confidence IN ('documented', 'observed', 'inferred', 'unknown')
  ),
  first_seen_batch_id TEXT NOT NULL,
  last_seen_batch_id TEXT NOT NULL,
  tombstoned_at_unix_ms INTEGER,
  CHECK (
    source_guid IS NOT NULL OR remote_id IS NOT NULL OR
    master_id IS NOT NULL OR fallback_fingerprint IS NOT NULL
  ),
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT,
  FOREIGN KEY (first_seen_batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT,
  FOREIGN KEY (last_seen_batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_records_guid
  ON tally_source_records(company_id, object_type, source_guid)
  WHERE source_guid IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_records_remote_id
  ON tally_source_records(company_id, object_type, remote_id)
  WHERE remote_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_records_master_id
  ON tally_source_records(company_id, object_type, master_id)
  WHERE master_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_records_fallback
  ON tally_source_records(company_id, object_type, fallback_fingerprint)
  WHERE fallback_fingerprint IS NOT NULL;

CREATE TABLE IF NOT EXISTS tally_record_observations (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  source_record_id TEXT NOT NULL,
  observed_at_unix_ms INTEGER NOT NULL,
  raw_source_sha256 TEXT NOT NULL,
  canonical_sha256 TEXT,
  canonical_payload_json TEXT,
  exact_decimals_json TEXT NOT NULL DEFAULT '{}',
  observed_alter_id TEXT,
  validation_status TEXT NOT NULL CHECK (validation_status IN ('accepted', 'rejected')),
  safe_rejection_code TEXT,
  UNIQUE (batch_id, source_record_id),
  CHECK (
    (validation_status = 'accepted' AND canonical_sha256 IS NOT NULL AND
      canonical_payload_json IS NOT NULL AND safe_rejection_code IS NULL) OR
    (validation_status = 'rejected' AND canonical_payload_json IS NULL AND
      safe_rejection_code IS NOT NULL)
  ),
  FOREIGN KEY (batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT,
  FOREIGN KEY (source_record_id) REFERENCES tally_source_records(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_observations_batch
  ON tally_record_observations(batch_id, validation_status);

CREATE TABLE IF NOT EXISTS tally_proof_ledger (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  proof_contract_version INTEGER NOT NULL CHECK (proof_contract_version > 0),
  previous_entry_sha256 TEXT,
  entry_sha256 TEXT NOT NULL UNIQUE,
  run_id TEXT NOT NULL,
  batch_id TEXT NOT NULL UNIQUE,
  capability_snapshot_id TEXT NOT NULL,
  company_id TEXT NOT NULL,
  pack_id TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK (
    outcome IN ('completed', 'failed', 'cancelled', 'outcome_unknown')
  ),
  verification_state TEXT NOT NULL CHECK (
    verification_state IN ('verified', 'partial', 'unverified')
  ),
  started_at_unix_ms INTEGER NOT NULL,
  completed_at_unix_ms INTEGER,
  accepted_records INTEGER NOT NULL CHECK (accepted_records >= 0),
  rejected_records INTEGER NOT NULL CHECK (rejected_records >= 0),
  snapshot_sha256 TEXT,
  checkpoint_before TEXT,
  checkpoint_after TEXT,
  gap_codes_json TEXT NOT NULL DEFAULT '[]',
  warning_codes_json TEXT NOT NULL DEFAULT '[]',
  created_at_unix_ms INTEGER NOT NULL,
  FOREIGN KEY (batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT,
  FOREIGN KEY (capability_snapshot_id) REFERENCES tally_capability_snapshots(id) ON DELETE RESTRICT,
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_checkpoints (
  company_id TEXT NOT NULL,
  pack_id TEXT NOT NULL,
  checkpoint_token TEXT NOT NULL,
  run_id TEXT NOT NULL,
  proof_id TEXT NOT NULL,
  snapshot_sha256 TEXT NOT NULL,
  verified_at_unix_ms INTEGER NOT NULL,
  freshness_target_seconds INTEGER NOT NULL CHECK (freshness_target_seconds > 0),
  generation INTEGER NOT NULL CHECK (generation > 0),
  PRIMARY KEY (company_id, pack_id),
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT,
  FOREIGN KEY (proof_id) REFERENCES tally_proof_ledger(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_capability_snapshots_no_update
BEFORE UPDATE ON tally_capability_snapshots
BEGIN
  SELECT RAISE(ABORT, 'capability snapshots are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_capability_snapshots_no_delete
BEFORE DELETE ON tally_capability_snapshots
BEGIN
  SELECT RAISE(ABORT, 'capability snapshots are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_record_observations_no_update
BEFORE UPDATE ON tally_record_observations
BEGIN
  SELECT RAISE(ABORT, 'record observations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_record_observations_no_delete
BEFORE DELETE ON tally_record_observations
BEGIN
  SELECT RAISE(ABORT, 'record observations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_proof_ledger_no_update
BEFORE UPDATE ON tally_proof_ledger
BEGIN
  SELECT RAISE(ABORT, 'proof ledger entries are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_proof_ledger_no_delete
BEFORE DELETE ON tally_proof_ledger
BEGIN
  SELECT RAISE(ABORT, 'proof ledger entries are immutable');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (2, 'encrypted Tally mirror and proof ledger', 0);
