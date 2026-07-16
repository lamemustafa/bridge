CREATE TABLE IF NOT EXISTS tally_snapshot_window_attempts (
  id TEXT PRIMARY KEY,
  batch_id TEXT NOT NULL,
  window_id TEXT NOT NULL,
  attempt_ordinal INTEGER NOT NULL CHECK (attempt_ordinal > 0),
  state TEXT NOT NULL CHECK (state IN ('open', 'abandoned', 'complete')),
  started_at_unix_ms INTEGER NOT NULL CHECK (started_at_unix_ms > 0),
  completed_at_unix_ms INTEGER,
  receipt_json TEXT CHECK (receipt_json IS NULL OR json_valid(receipt_json)),
  receipt_sha256 TEXT CHECK (
    receipt_sha256 IS NULL OR (
      length(receipt_sha256) = 64 AND receipt_sha256 NOT GLOB '*[^0-9a-f]*'
    )
  ),
  UNIQUE (id, batch_id, window_id),
  UNIQUE (batch_id, window_id, attempt_ordinal),
  CHECK (
    (state = 'open' AND completed_at_unix_ms IS NULL AND receipt_json IS NULL AND
      receipt_sha256 IS NULL) OR
    (state = 'abandoned' AND completed_at_unix_ms IS NOT NULL AND receipt_json IS NULL AND
      receipt_sha256 IS NULL) OR
    (state = 'complete' AND completed_at_unix_ms IS NOT NULL AND receipt_json IS NOT NULL AND
      receipt_sha256 IS NOT NULL)
  ),
  CHECK (completed_at_unix_ms IS NULL OR completed_at_unix_ms >= started_at_unix_ms),
  FOREIGN KEY (batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_tally_snapshot_window_one_open_attempt
  ON tally_snapshot_window_attempts(batch_id, window_id) WHERE state = 'open';

CREATE INDEX IF NOT EXISTS idx_tally_snapshot_window_attempt_latest_complete
  ON tally_snapshot_window_attempts(batch_id, window_id, attempt_ordinal DESC)
  WHERE state = 'complete';

CREATE TABLE IF NOT EXISTS tally_snapshot_window_memberships (
  batch_id TEXT NOT NULL,
  window_id TEXT NOT NULL,
  record_key TEXT NOT NULL,
  canonical_sha256 TEXT NOT NULL CHECK (
    length(canonical_sha256) = 64 AND canonical_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  canonical_payload_json TEXT NOT NULL CHECK (json_valid(canonical_payload_json)),
  exact_decimals_json TEXT NOT NULL CHECK (json_valid(exact_decimals_json)),
  provenance_state TEXT NOT NULL CHECK (
    provenance_state IN ('observed', 'unavailable')
  ),
  source_record_id TEXT,
  observation_id TEXT,
  safe_reason_code TEXT,
  first_seen_attempt_id TEXT NOT NULL,
  last_seen_attempt_id TEXT NOT NULL,
  PRIMARY KEY (batch_id, window_id, record_key),
  CHECK (
    (provenance_state = 'observed' AND source_record_id IS NOT NULL AND
      observation_id IS NOT NULL AND safe_reason_code IS NULL) OR
    (provenance_state = 'unavailable' AND source_record_id IS NULL AND
      observation_id IS NULL AND safe_reason_code IS NOT NULL)
  ),
  FOREIGN KEY (batch_id) REFERENCES tally_observation_batches(id) ON DELETE RESTRICT,
  FOREIGN KEY (source_record_id) REFERENCES tally_source_records(id) ON DELETE RESTRICT,
  FOREIGN KEY (observation_id) REFERENCES tally_record_observations(id) ON DELETE RESTRICT,
  FOREIGN KEY (first_seen_attempt_id, batch_id, window_id)
    REFERENCES tally_snapshot_window_attempts(id, batch_id, window_id) ON DELETE RESTRICT,
  FOREIGN KEY (last_seen_attempt_id, batch_id, window_id)
    REFERENCES tally_snapshot_window_attempts(id, batch_id, window_id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_snapshot_window_membership_last_seen
  ON tally_snapshot_window_memberships(batch_id, window_id, last_seen_attempt_id);

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_attempt_terminal_immutable
BEFORE UPDATE ON tally_snapshot_window_attempts
WHEN OLD.state <> 'open'
BEGIN
  SELECT RAISE(ABORT, 'terminal snapshot window attempt is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_attempt_identity_immutable
BEFORE UPDATE OF id, batch_id, window_id, attempt_ordinal, started_at_unix_ms
ON tally_snapshot_window_attempts
BEGIN
  SELECT RAISE(ABORT, 'snapshot window attempt identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_attempt_no_delete
BEFORE DELETE ON tally_snapshot_window_attempts
BEGIN
  SELECT RAISE(ABORT, 'snapshot window attempts are append-only');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_membership_insert_open_attempt
BEFORE INSERT ON tally_snapshot_window_memberships
WHEN NOT EXISTS (
  SELECT 1 FROM tally_snapshot_window_attempts AS attempt
  WHERE attempt.id = NEW.first_seen_attempt_id
    AND attempt.id = NEW.last_seen_attempt_id
    AND attempt.batch_id = NEW.batch_id
    AND attempt.window_id = NEW.window_id
    AND attempt.state = 'open'
)
BEGIN
  SELECT RAISE(ABORT, 'membership requires its owner-bound open attempt');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_membership_content_immutable
BEFORE UPDATE ON tally_snapshot_window_memberships
WHEN OLD.batch_id IS NOT NEW.batch_id
  OR OLD.window_id IS NOT NEW.window_id
  OR OLD.record_key IS NOT NEW.record_key
  OR OLD.canonical_sha256 IS NOT NEW.canonical_sha256
  OR OLD.canonical_payload_json IS NOT NEW.canonical_payload_json
  OR OLD.exact_decimals_json IS NOT NEW.exact_decimals_json
  OR OLD.provenance_state IS NOT NEW.provenance_state
  OR OLD.source_record_id IS NOT NEW.source_record_id
  OR OLD.observation_id IS NOT NEW.observation_id
  OR OLD.safe_reason_code IS NOT NEW.safe_reason_code
  OR OLD.first_seen_attempt_id IS NOT NEW.first_seen_attempt_id
BEGIN
  SELECT RAISE(ABORT, 'snapshot window membership identity and content are immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_membership_last_seen_advance
BEFORE UPDATE OF last_seen_attempt_id ON tally_snapshot_window_memberships
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_snapshot_window_attempts AS incoming
  JOIN tally_snapshot_window_attempts AS previous
    ON previous.id = OLD.last_seen_attempt_id
  WHERE incoming.id = NEW.last_seen_attempt_id
    AND incoming.batch_id = OLD.batch_id
    AND incoming.window_id = OLD.window_id
    AND incoming.state = 'open'
    AND incoming.attempt_ordinal >= previous.attempt_ordinal
)
BEGIN
  SELECT RAISE(ABORT, 'last seen may only advance to an owner-bound open attempt');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_membership_no_delete
BEFORE DELETE ON tally_snapshot_window_memberships
BEGIN
  SELECT RAISE(ABORT, 'snapshot window memberships are append-only');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (9, 'normalized immutable snapshot window staging', 0);
