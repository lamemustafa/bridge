CREATE TABLE IF NOT EXISTS tally_write_mapping_versions (
  id TEXT PRIMARY KEY,
  company_id TEXT NOT NULL,
  object_type TEXT NOT NULL,
  mapping_key TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version > 0),
  mapping_sha256 TEXT NOT NULL CHECK (
    length(mapping_sha256) = 64 AND mapping_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  supersedes_id TEXT,
  created_at_unix_ms INTEGER NOT NULL,
  UNIQUE (company_id, object_type, mapping_key, version),
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT,
  FOREIGN KEY (supersedes_id) REFERENCES tally_write_mapping_versions(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_write_mapping_heads (
  company_id TEXT NOT NULL,
  object_type TEXT NOT NULL,
  mapping_key TEXT NOT NULL,
  mapping_version_id TEXT NOT NULL UNIQUE,
  activated_at_unix_ms INTEGER NOT NULL,
  PRIMARY KEY (company_id, object_type, mapping_key),
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT,
  FOREIGN KEY (mapping_version_id) REFERENCES tally_write_mapping_versions(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_mapping_heads_scope_insert
BEFORE INSERT ON tally_write_mapping_heads
WHEN NOT EXISTS (
  SELECT 1 FROM tally_write_mapping_versions AS version
  WHERE version.id = NEW.mapping_version_id AND version.company_id = NEW.company_id AND
    version.object_type = NEW.object_type AND version.mapping_key = NEW.mapping_key
)
BEGIN
  SELECT RAISE(ABORT, 'mapping head scope mismatch');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_mapping_heads_scope_update
BEFORE UPDATE ON tally_write_mapping_heads
WHEN NOT EXISTS (
  SELECT 1 FROM tally_write_mapping_versions AS version
  WHERE version.id = NEW.mapping_version_id AND version.company_id = NEW.company_id AND
    version.object_type = NEW.object_type AND version.mapping_key = NEW.mapping_key
)
BEGIN
  SELECT RAISE(ABORT, 'mapping head scope mismatch');
END;

CREATE TABLE IF NOT EXISTS tally_import_outbox_jobs (
  id TEXT PRIMARY KEY,
  company_id TEXT NOT NULL,
  mapping_version_id TEXT NOT NULL,
  request_id TEXT NOT NULL UNIQUE,
  payload_sha256 TEXT NOT NULL CHECK (
    length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  diff_sha256 TEXT NOT NULL CHECK (
    length(diff_sha256) = 64 AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  approval_digest TEXT CHECK (
    approval_digest IS NULL OR
    (length(approval_digest) = 64 AND approval_digest NOT GLOB '*[^0-9a-f]*')
  ),
  state TEXT NOT NULL CHECK (state IN (
    'prepared', 'approved', 'ready_to_send', 'send_started',
    'confirmed_success', 'confirmed_failure', 'outcome_unknown',
    'recovered_success', 'recovered_not_applied', 'failed_pre_send', 'cancelled'
  )),
  dispatch_attempts INTEGER NOT NULL DEFAULT 0 CHECK (dispatch_attempts IN (0, 1)),
  created_at_unix_ms INTEGER NOT NULL,
  approved_at_unix_ms INTEGER,
  send_started_at_unix_ms INTEGER,
  completed_at_unix_ms INTEGER,
  CHECK (
    (state = 'prepared' AND approval_digest IS NULL AND approved_at_unix_ms IS NULL) OR
    (state <> 'prepared' AND state <> 'cancelled' AND state <> 'failed_pre_send' AND
      approval_digest IS NOT NULL AND approved_at_unix_ms IS NOT NULL) OR
    (state IN ('cancelled', 'failed_pre_send'))
  ),
  CHECK (
    (state IN ('prepared', 'approved', 'ready_to_send', 'cancelled', 'failed_pre_send') AND
      dispatch_attempts = 0 AND send_started_at_unix_ms IS NULL) OR
    (state IN ('send_started', 'confirmed_success', 'confirmed_failure', 'outcome_unknown',
      'recovered_success', 'recovered_not_applied') AND dispatch_attempts = 1 AND
      send_started_at_unix_ms IS NOT NULL)
  ),
  CHECK (
    (state IN ('confirmed_success', 'confirmed_failure', 'recovered_success',
      'recovered_not_applied', 'failed_pre_send', 'cancelled') AND
      completed_at_unix_ms IS NOT NULL) OR
    (state NOT IN ('confirmed_success', 'confirmed_failure', 'recovered_success',
      'recovered_not_applied', 'failed_pre_send', 'cancelled') AND
      completed_at_unix_ms IS NULL)
  ),
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT,
  FOREIGN KEY (mapping_version_id) REFERENCES tally_write_mapping_versions(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_import_jobs_company_state
  ON tally_import_outbox_jobs(company_id, state, created_at_unix_ms);

CREATE TABLE IF NOT EXISTS tally_import_outbox_items (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
  object_type TEXT NOT NULL,
  operation TEXT NOT NULL CHECK (operation IN ('create', 'alter', 'delete')),
  source_identity_sha256 TEXT NOT NULL CHECK (
    length(source_identity_sha256) = 64 AND source_identity_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  payload_sha256 TEXT NOT NULL CHECK (
    length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  diff_sha256 TEXT NOT NULL CHECK (
    length(diff_sha256) = 64 AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  expected_before_sha256 TEXT CHECK (
    expected_before_sha256 IS NULL OR
    (length(expected_before_sha256) = 64 AND expected_before_sha256 NOT GLOB '*[^0-9a-f]*')
  ),
  CHECK (
    (operation = 'create' AND expected_before_sha256 IS NULL) OR
    (operation IN ('alter', 'delete') AND expected_before_sha256 IS NOT NULL)
  ),
  UNIQUE (job_id, ordinal),
  UNIQUE (job_id, source_identity_sha256, operation),
  FOREIGN KEY (job_id) REFERENCES tally_import_outbox_jobs(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_import_idempotency_state (
  idempotency_key_sha256 TEXT PRIMARY KEY CHECK (
    length(idempotency_key_sha256) = 64 AND
    idempotency_key_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  job_id TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL CHECK (state IN (
    'reserved', 'send_started', 'outcome_unknown', 'terminal', 'abandoned_before_send'
  )),
  reserved_at_unix_ms INTEGER NOT NULL,
  send_started_at_unix_ms INTEGER,
  terminal_at_unix_ms INTEGER,
  CHECK (
    (state = 'reserved' AND send_started_at_unix_ms IS NULL AND terminal_at_unix_ms IS NULL) OR
    (state IN ('send_started', 'outcome_unknown') AND send_started_at_unix_ms IS NOT NULL AND
      terminal_at_unix_ms IS NULL) OR
    (state = 'terminal' AND send_started_at_unix_ms IS NOT NULL AND terminal_at_unix_ms IS NOT NULL) OR
    (state = 'abandoned_before_send' AND send_started_at_unix_ms IS NULL AND
      terminal_at_unix_ms IS NOT NULL)
  ),
  FOREIGN KEY (job_id) REFERENCES tally_import_outbox_jobs(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_import_conflicts (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL,
  source_identity_sha256 TEXT NOT NULL CHECK (
    length(source_identity_sha256) = 64 AND source_identity_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  diff_sha256 TEXT NOT NULL CHECK (
    length(diff_sha256) = 64 AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  conflict_code TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('open', 'resolved', 'rejected')),
  resolution_code TEXT,
  resolution_digest TEXT CHECK (
    resolution_digest IS NULL OR
    (length(resolution_digest) = 64 AND resolution_digest NOT GLOB '*[^0-9a-f]*')
  ),
  created_at_unix_ms INTEGER NOT NULL,
  resolved_at_unix_ms INTEGER,
  CHECK (
    (state = 'open' AND resolution_code IS NULL AND resolution_digest IS NULL AND
      resolved_at_unix_ms IS NULL) OR
    (state IN ('resolved', 'rejected') AND resolution_code IS NOT NULL AND
      resolution_digest IS NOT NULL AND resolved_at_unix_ms IS NOT NULL)
  ),
  UNIQUE (job_id, source_identity_sha256, conflict_code),
  FOREIGN KEY (job_id) REFERENCES tally_import_outbox_jobs(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_import_results (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  job_id TEXT NOT NULL,
  phase TEXT NOT NULL CHECK (phase IN ('initial', 'recovery')),
  verification_id TEXT NOT NULL UNIQUE,
  outcome TEXT NOT NULL CHECK (outcome IN (
    'confirmed_success', 'confirmed_failure', 'outcome_unknown',
    'recovered_success', 'recovered_not_applied', 'recovery_inconclusive'
  )),
  result_sha256 TEXT NOT NULL CHECK (
    length(result_sha256) = 64 AND result_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  intended_payload_sha256 TEXT CHECK (
    intended_payload_sha256 IS NULL OR
    (length(intended_payload_sha256) = 64 AND intended_payload_sha256 NOT GLOB '*[^0-9a-f]*')
  ),
  observed_payload_sha256 TEXT CHECK (
    observed_payload_sha256 IS NULL OR
    (length(observed_payload_sha256) = 64 AND observed_payload_sha256 NOT GLOB '*[^0-9a-f]*')
  ),
  identity_coverage_sha256 TEXT CHECK (
    identity_coverage_sha256 IS NULL OR
    (length(identity_coverage_sha256) = 64 AND identity_coverage_sha256 NOT GLOB '*[^0-9a-f]*')
  ),
  observed_version_digest TEXT CHECK (
    observed_version_digest IS NULL OR
    (length(observed_version_digest) = 64 AND observed_version_digest NOT GLOB '*[^0-9a-f]*')
  ),
  safe_result_code TEXT NOT NULL,
  counters_observed INTEGER NOT NULL CHECK (counters_observed IN (0, 1)),
  created_count INTEGER CHECK (created_count IS NULL OR created_count >= 0),
  altered_count INTEGER CHECK (altered_count IS NULL OR altered_count >= 0),
  deleted_count INTEGER CHECK (deleted_count IS NULL OR deleted_count >= 0),
  ignored_count INTEGER CHECK (ignored_count IS NULL OR ignored_count >= 0),
  error_count INTEGER CHECK (error_count IS NULL OR error_count >= 0),
  cancelled_count INTEGER CHECK (cancelled_count IS NULL OR cancelled_count >= 0),
  exception_count INTEGER CHECK (exception_count IS NULL OR exception_count >= 0),
  line_error_count INTEGER CHECK (line_error_count IS NULL OR line_error_count >= 0),
  observed_at_unix_ms INTEGER NOT NULL,
  CHECK (length(safe_result_code) > 0 AND length(safe_result_code) <= 200),
  CHECK (
    (phase = 'initial' AND outcome IN
      ('confirmed_success', 'confirmed_failure', 'outcome_unknown')) OR
    (phase = 'recovery' AND outcome IN
      ('recovered_success', 'recovered_not_applied', 'recovery_inconclusive'))
  ),
  CHECK (
    (phase = 'initial' AND intended_payload_sha256 IS NULL AND
      observed_payload_sha256 IS NULL AND identity_coverage_sha256 IS NULL AND
      observed_version_digest IS NULL) OR
    (phase = 'recovery' AND intended_payload_sha256 IS NOT NULL AND
      observed_payload_sha256 IS NOT NULL AND identity_coverage_sha256 IS NOT NULL AND
      observed_version_digest IS NOT NULL)
  ),
  CHECK (
    (counters_observed = 0 AND created_count IS NULL AND altered_count IS NULL AND
      deleted_count IS NULL AND ignored_count IS NULL AND error_count IS NULL AND
      cancelled_count IS NULL AND exception_count IS NULL AND line_error_count IS NULL) OR
    (counters_observed = 1 AND created_count IS NOT NULL AND altered_count IS NOT NULL AND
      deleted_count IS NOT NULL AND ignored_count IS NOT NULL AND error_count IS NOT NULL AND
      cancelled_count IS NOT NULL AND exception_count IS NOT NULL AND
      line_error_count IS NOT NULL)
  ),
  UNIQUE (job_id, phase, verification_id),
  FOREIGN KEY (job_id) REFERENCES tally_import_outbox_jobs(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_import_job_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  job_id TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT NOT NULL CHECK (to_state IN (
    'prepared', 'approved', 'ready_to_send', 'send_started',
    'confirmed_success', 'confirmed_failure', 'outcome_unknown',
    'recovered_success', 'recovered_not_applied', 'failed_pre_send', 'cancelled'
  )),
  request_id TEXT NOT NULL,
  verification_id TEXT,
  safe_reason_code TEXT,
  evidence_sha256 TEXT NOT NULL CHECK (
    length(evidence_sha256) = 64 AND evidence_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  observed_at_unix_ms INTEGER NOT NULL,
  CHECK (from_state IS NULL OR from_state IN (
    'prepared', 'approved', 'ready_to_send', 'send_started',
    'confirmed_success', 'confirmed_failure', 'outcome_unknown',
    'recovered_success', 'recovered_not_applied', 'failed_pre_send', 'cancelled'
  )),
  CHECK (
    (from_state IS NULL AND to_state = 'prepared') OR
    (from_state = 'prepared' AND to_state IN ('approved', 'cancelled')) OR
    (from_state = 'approved' AND to_state IN ('ready_to_send', 'cancelled')) OR
    (from_state = 'ready_to_send' AND to_state IN ('send_started', 'failed_pre_send', 'cancelled')) OR
    (from_state = 'send_started' AND to_state IN
      ('confirmed_success', 'confirmed_failure', 'outcome_unknown')) OR
    (from_state = 'outcome_unknown' AND to_state IN
      ('outcome_unknown', 'recovered_success', 'recovered_not_applied'))
  ),
  FOREIGN KEY (job_id) REFERENCES tally_import_outbox_jobs(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_mapping_versions_no_update
BEFORE UPDATE ON tally_write_mapping_versions
BEGIN
  SELECT RAISE(ABORT, 'write mapping versions are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_mapping_versions_no_delete
BEFORE DELETE ON tally_write_mapping_versions
BEGIN
  SELECT RAISE(ABORT, 'write mapping versions are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_items_no_update
BEFORE UPDATE ON tally_import_outbox_items
BEGIN
  SELECT RAISE(ABORT, 'import outbox items are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_items_no_delete
BEFORE DELETE ON tally_import_outbox_items
BEGIN
  SELECT RAISE(ABORT, 'import outbox items are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_results_no_update
BEFORE UPDATE ON tally_import_results
BEGIN
  SELECT RAISE(ABORT, 'import results are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_results_no_delete
BEFORE DELETE ON tally_import_results
BEGIN
  SELECT RAISE(ABORT, 'import results are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_events_no_update
BEFORE UPDATE ON tally_import_job_events
BEGIN
  SELECT RAISE(ABORT, 'import job events are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_events_no_delete
BEFORE DELETE ON tally_import_job_events
BEGIN
  SELECT RAISE(ABORT, 'import job events are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_jobs_state_machine
BEFORE UPDATE OF state ON tally_import_outbox_jobs
WHEN NOT (
  (OLD.state = 'prepared' AND NEW.state IN ('approved', 'cancelled')) OR
  (OLD.state = 'approved' AND NEW.state IN ('ready_to_send', 'cancelled')) OR
  (OLD.state = 'ready_to_send' AND NEW.state IN ('send_started', 'failed_pre_send', 'cancelled')) OR
  (OLD.state = 'send_started' AND NEW.state IN
    ('confirmed_success', 'confirmed_failure', 'outcome_unknown')) OR
  (OLD.state = 'outcome_unknown' AND NEW.state IN
    ('recovered_success', 'recovered_not_applied'))
)
BEGIN
  SELECT RAISE(ABORT, 'invalid import job state transition');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_jobs_evidence_immutable
BEFORE UPDATE ON tally_import_outbox_jobs
WHEN OLD.id IS NOT NEW.id OR
  OLD.company_id IS NOT NEW.company_id OR
  OLD.mapping_version_id IS NOT NEW.mapping_version_id OR
  OLD.request_id IS NOT NEW.request_id OR
  OLD.payload_sha256 IS NOT NEW.payload_sha256 OR
  OLD.diff_sha256 IS NOT NEW.diff_sha256 OR
  OLD.created_at_unix_ms IS NOT NEW.created_at_unix_ms OR
  (OLD.approval_digest IS NOT NEW.approval_digest AND OLD.state <> 'prepared') OR
  (OLD.approved_at_unix_ms IS NOT NEW.approved_at_unix_ms AND OLD.state <> 'prepared') OR
  (OLD.dispatch_attempts IS NOT NEW.dispatch_attempts AND OLD.state <> 'ready_to_send') OR
  (OLD.send_started_at_unix_ms IS NOT NEW.send_started_at_unix_ms AND
    OLD.state <> 'ready_to_send') OR
  (OLD.completed_at_unix_ms IS NOT NEW.completed_at_unix_ms AND
    OLD.completed_at_unix_ms IS NOT NULL)
BEGIN
  SELECT RAISE(ABORT, 'import job evidence is immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_jobs_no_delete
BEFORE DELETE ON tally_import_outbox_jobs
BEGIN
  SELECT RAISE(ABORT, 'import jobs cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_idempotency_state_machine
BEFORE UPDATE OF state ON tally_import_idempotency_state
WHEN NOT (
  (OLD.state = 'reserved' AND NEW.state IN ('send_started', 'abandoned_before_send')) OR
  (OLD.state = 'send_started' AND NEW.state IN ('outcome_unknown', 'terminal')) OR
  (OLD.state = 'outcome_unknown' AND NEW.state = 'terminal')
)
BEGIN
  SELECT RAISE(ABORT, 'invalid import idempotency transition');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_idempotency_evidence_immutable
BEFORE UPDATE ON tally_import_idempotency_state
WHEN OLD.idempotency_key_sha256 IS NOT NEW.idempotency_key_sha256 OR
  OLD.job_id IS NOT NEW.job_id OR
  OLD.reserved_at_unix_ms IS NOT NEW.reserved_at_unix_ms OR
  (OLD.send_started_at_unix_ms IS NOT NEW.send_started_at_unix_ms AND
    OLD.state <> 'reserved') OR
  (OLD.terminal_at_unix_ms IS NOT NEW.terminal_at_unix_ms AND
    OLD.state NOT IN ('reserved', 'send_started', 'outcome_unknown'))
BEGIN
  SELECT RAISE(ABORT, 'import idempotency evidence is immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_idempotency_no_delete
BEFORE DELETE ON tally_import_idempotency_state
BEGIN
  SELECT RAISE(ABORT, 'import idempotency state cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_conflicts_state_machine
BEFORE UPDATE OF state ON tally_import_conflicts
WHEN NOT (OLD.state = 'open' AND NEW.state IN ('resolved', 'rejected'))
BEGIN
  SELECT RAISE(ABORT, 'invalid import conflict state transition');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_conflicts_prepared_only
BEFORE INSERT ON tally_import_conflicts
WHEN (SELECT state FROM tally_import_outbox_jobs WHERE id = NEW.job_id) <> 'prepared'
BEGIN
  SELECT RAISE(ABORT, 'conflicts can only be recorded before approval');
END;

CREATE TRIGGER IF NOT EXISTS tally_import_conflicts_no_delete
BEFORE DELETE ON tally_import_conflicts
BEGIN
  SELECT RAISE(ABORT, 'import conflicts cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (3, 'durable safe Tally write outbox and recovery evidence', 0);
