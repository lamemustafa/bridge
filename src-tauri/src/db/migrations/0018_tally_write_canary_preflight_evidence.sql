CREATE TABLE IF NOT EXISTS tally_write_canary_preflight_evidence (
  id TEXT PRIMARY KEY,
  attempt_id TEXT NOT NULL UNIQUE,
  readback_state_sha256 TEXT NOT NULL CHECK (
    length(readback_state_sha256) = 64 AND readback_state_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  identity_coverage_sha256 TEXT NOT NULL CHECK (
    length(identity_coverage_sha256) = 64 AND identity_coverage_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  verified_at_unix_ms INTEGER NOT NULL CHECK (verified_at_unix_ms > 0),
  FOREIGN KEY (attempt_id) REFERENCES tally_write_canary_preflight_attempts(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_evidence_requires_active_fixture
BEFORE INSERT ON tally_write_canary_preflight_evidence
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_write_canary_preflight_attempts AS attempt
  JOIN tally_write_canary_payload_bindings AS binding ON binding.id = attempt.payload_binding_id
  JOIN tally_write_canary_reservations AS reservation ON reservation.id = binding.reservation_id
  JOIN tally_write_fixture_enrollments AS enrollment ON enrollment.id = reservation.enrollment_id
  WHERE attempt.id = NEW.attempt_id
    AND NOT EXISTS (
      SELECT 1 FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = enrollment.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'canary preflight evidence requires an active fixture enrollment');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_evidence_no_update
BEFORE UPDATE ON tally_write_canary_preflight_evidence
BEGIN
  SELECT RAISE(ABORT, 'canary preflight evidence is immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_preflight_evidence_no_delete
BEFORE DELETE ON tally_write_canary_preflight_evidence
BEGIN
  SELECT RAISE(ABORT, 'canary preflight evidence cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (18, 'immutable sealed preflight read evidence for local Tally synthetic write canaries', 0);
