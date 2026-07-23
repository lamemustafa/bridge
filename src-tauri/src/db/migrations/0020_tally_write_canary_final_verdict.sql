CREATE TABLE IF NOT EXISTS tally_write_canary_final_verdicts (
  id TEXT PRIMARY KEY,
  dispatch_attempt_id TEXT NOT NULL UNIQUE,
  import_response_sha256 TEXT NOT NULL CHECK (
    length(import_response_sha256) = 64 AND import_response_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  readback_state_sha256 TEXT NOT NULL CHECK (
    length(readback_state_sha256) = 64 AND readback_state_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  identity_coverage_sha256 TEXT NOT NULL CHECK (
    length(identity_coverage_sha256) = 64 AND identity_coverage_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  recorded_at_unix_ms INTEGER NOT NULL CHECK (recorded_at_unix_ms > 0),
  FOREIGN KEY (dispatch_attempt_id) REFERENCES tally_write_canary_dispatch_attempts(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_canary_final_verdict_requires_active_fixture
BEFORE INSERT ON tally_write_canary_final_verdicts
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_write_canary_dispatch_attempts AS dispatch
  JOIN tally_write_canary_preflight_evidence AS evidence ON evidence.id = dispatch.evidence_id
  JOIN tally_write_canary_preflight_attempts AS attempt ON attempt.id = evidence.attempt_id
  JOIN tally_write_canary_payload_bindings AS binding ON binding.id = attempt.payload_binding_id
  JOIN tally_write_canary_reservations AS reservation ON reservation.id = binding.reservation_id
  JOIN tally_write_fixture_enrollments AS enrollment ON enrollment.id = reservation.enrollment_id
  WHERE dispatch.id = NEW.dispatch_attempt_id
    AND NOT EXISTS (
      SELECT 1 FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = enrollment.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'canary final verdict requires an active fixture enrollment');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_final_verdict_no_update
BEFORE UPDATE ON tally_write_canary_final_verdicts
BEGIN
  SELECT RAISE(ABORT, 'canary final verdict is immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_final_verdict_no_delete
BEFORE DELETE ON tally_write_canary_final_verdicts
BEGIN
  SELECT RAISE(ABORT, 'canary final verdict cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (20, 'immutable digest-only final verdict for local Tally synthetic write canaries', 0);
