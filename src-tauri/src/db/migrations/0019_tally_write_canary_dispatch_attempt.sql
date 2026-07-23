CREATE TABLE IF NOT EXISTS tally_write_canary_dispatch_attempts (
  id TEXT PRIMARY KEY,
  evidence_id TEXT NOT NULL UNIQUE,
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  claimed_at_unix_ms INTEGER NOT NULL CHECK (claimed_at_unix_ms > 0),
  FOREIGN KEY (evidence_id) REFERENCES tally_write_canary_preflight_evidence(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_canary_dispatch_attempt_requires_active_fixture
BEFORE INSERT ON tally_write_canary_dispatch_attempts
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_write_canary_preflight_evidence AS evidence
  JOIN tally_write_canary_preflight_attempts AS attempt ON attempt.id = evidence.attempt_id
  JOIN tally_write_canary_payload_bindings AS binding ON binding.id = attempt.payload_binding_id
  JOIN tally_write_canary_reservations AS reservation ON reservation.id = binding.reservation_id
  JOIN tally_write_fixture_enrollments AS enrollment ON enrollment.id = reservation.enrollment_id
  WHERE evidence.id = NEW.evidence_id
    AND NOT EXISTS (
      SELECT 1 FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = enrollment.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'canary dispatch attempt requires an active fixture enrollment');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_dispatch_attempt_no_update
BEFORE UPDATE ON tally_write_canary_dispatch_attempts
BEGIN
  SELECT RAISE(ABORT, 'canary dispatch attempt is immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_dispatch_attempt_no_delete
BEFORE DELETE ON tally_write_canary_dispatch_attempts
BEGIN
  SELECT RAISE(ABORT, 'canary dispatch attempt cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (19, 'immutable no-send dispatch claim for local Tally synthetic write canaries', 0);
