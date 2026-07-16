ALTER TABLE tally_snapshot_window_attempts
  ADD COLUMN terminal_safe_reason_code TEXT
  CHECK (
    terminal_safe_reason_code IS NULL OR
    terminal_safe_reason_code = 'local_clock_moved_backwards'
  );

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_attempt_terminal_reason_insert
BEFORE INSERT ON tally_snapshot_window_attempts
WHEN NEW.terminal_safe_reason_code IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'new snapshot window attempt cannot carry terminal evidence');
END;

CREATE TRIGGER IF NOT EXISTS trg_tally_snapshot_window_attempt_terminal_reason_shape
BEFORE UPDATE OF state, terminal_safe_reason_code ON tally_snapshot_window_attempts
WHEN NEW.terminal_safe_reason_code IS NOT NULL AND NEW.state = 'open'
BEGIN
  SELECT RAISE(ABORT, 'snapshot window terminal evidence requires terminal state');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (12, 'durable snapshot window terminal evidence', 0);
