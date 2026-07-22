CREATE TABLE IF NOT EXISTS tally_write_canary_payload_bindings (
  id TEXT PRIMARY KEY,
  reservation_id TEXT NOT NULL UNIQUE,
  wire_sha256 TEXT NOT NULL CHECK (
    length(wire_sha256) = 64 AND wire_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  intended_state_sha256 TEXT NOT NULL CHECK (
    length(intended_state_sha256) = 64 AND intended_state_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  identity_query_sha256 TEXT NOT NULL CHECK (
    length(identity_query_sha256) = 64 AND identity_query_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  bound_at_unix_ms INTEGER NOT NULL CHECK (bound_at_unix_ms > 0),
  FOREIGN KEY (reservation_id) REFERENCES tally_write_canary_reservations(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_canary_payload_binding_requires_active_fixture
BEFORE INSERT ON tally_write_canary_payload_bindings
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_write_canary_reservations AS reservation
  JOIN tally_write_fixture_enrollments AS enrollment ON enrollment.id = reservation.enrollment_id
  WHERE reservation.id = NEW.reservation_id
    AND NOT EXISTS (
      SELECT 1 FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = enrollment.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'canary payload binding requires an active fixture enrollment');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_payload_bindings_no_update
BEFORE UPDATE ON tally_write_canary_payload_bindings
BEGIN
  SELECT RAISE(ABORT, 'canary payload bindings are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_payload_bindings_no_delete
BEFORE DELETE ON tally_write_canary_payload_bindings
BEGIN
  SELECT RAISE(ABORT, 'canary payload bindings cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (16, 'immutable payload commitments for one local Tally synthetic write canary', 0);
