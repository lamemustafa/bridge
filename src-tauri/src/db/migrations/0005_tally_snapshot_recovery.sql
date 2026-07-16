ALTER TABLE tally_snapshot_run_states
  ADD COLUMN row_sha256 TEXT CHECK (
    row_sha256 IS NULL OR (
      length(row_sha256) = 64 AND row_sha256 NOT GLOB '*[^0-9a-f]*'
    )
  );

ALTER TABLE tally_snapshot_run_states
  ADD COLUMN lease_owner TEXT CHECK (
    lease_owner IS NULL OR (length(lease_owner) BETWEEN 1 AND 128)
  );

ALTER TABLE tally_snapshot_run_states
  ADD COLUMN lease_expires_at_unix_ms INTEGER CHECK (
    (lease_owner IS NULL AND lease_expires_at_unix_ms IS NULL) OR
    (lease_owner IS NOT NULL AND lease_expires_at_unix_ms IS NOT NULL)
  );

CREATE UNIQUE INDEX IF NOT EXISTS idx_tally_snapshot_run_states_unique_run
  ON tally_snapshot_run_states(run_id);

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_run_identity_immutable
BEFORE UPDATE OF resume_key, run_id ON tally_snapshot_run_states
WHEN NEW.resume_key <> OLD.resume_key OR NEW.run_id <> OLD.run_id
BEGIN
  SELECT RAISE(ABORT, 'snapshot recovery identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_terminal_immutable
BEFORE UPDATE OF generation, state_sha256, state_json, row_sha256
ON tally_snapshot_run_states
WHEN json_extract(OLD.state_json, '$.progress.phase') IN
  ('completed', 'partial', 'failed', 'cancelled')
BEGIN
  SELECT RAISE(ABORT, 'terminal snapshot recovery state is immutable');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_state_no_delete
BEFORE DELETE ON tally_snapshot_run_states
BEGIN
  SELECT RAISE(ABORT, 'snapshot recovery state is immutable');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (5, 'snapshot recovery CAS lease and row integrity', 0);
