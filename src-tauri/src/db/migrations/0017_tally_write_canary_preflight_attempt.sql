CREATE TABLE IF NOT EXISTS tally_write_canary_preflight_attempts (
  id TEXT PRIMARY KEY,
  payload_binding_id TEXT NOT NULL UNIQUE,
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  started_at_unix_ms INTEGER NOT NULL CHECK (started_at_unix_ms > 0),
  FOREIGN KEY (payload_binding_id) REFERENCES tally_write_canary_payload_bindings(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_attempt_requires_active_fixture
BEFORE INSERT ON tally_write_canary_preflight_attempts
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_write_canary_payload_bindings AS binding
  JOIN tally_write_canary_reservations AS reservation ON reservation.id = binding.reservation_id
  JOIN tally_write_fixture_enrollments AS enrollment ON enrollment.id = reservation.enrollment_id
  WHERE binding.id = NEW.payload_binding_id
    AND NOT EXISTS (
      SELECT 1 FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = enrollment.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'canary preflight attempt requires an active fixture enrollment');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_attempts_no_update
BEFORE UPDATE ON tally_write_canary_preflight_attempts
BEGIN
  SELECT RAISE(ABORT, 'canary preflight attempts are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_attempts_no_delete
BEFORE DELETE ON tally_write_canary_preflight_attempts
BEGIN
  SELECT RAISE(ABORT, 'canary preflight attempts cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (17, 'single-use preflight read claim for local Tally synthetic write canaries', 0);
