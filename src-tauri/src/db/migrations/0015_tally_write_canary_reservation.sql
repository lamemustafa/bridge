CREATE TABLE IF NOT EXISTS tally_write_canary_reservations (
  id TEXT PRIMARY KEY,
  enrollment_id TEXT NOT NULL UNIQUE,
  reservation_payload_sha256 TEXT NOT NULL CHECK (
    length(reservation_payload_sha256) = 64 AND
    reservation_payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  reserved_at_unix_ms INTEGER NOT NULL CHECK (reserved_at_unix_ms > 0),
  FOREIGN KEY (enrollment_id) REFERENCES tally_write_fixture_enrollments(id) ON DELETE RESTRICT
);

CREATE TRIGGER IF NOT EXISTS tally_write_canary_reservation_requires_active_fixture
BEFORE INSERT ON tally_write_canary_reservations
WHEN NOT EXISTS (
  SELECT 1
  FROM tally_write_fixture_enrollments AS enrollment
  WHERE enrollment.id = NEW.enrollment_id
    AND NOT EXISTS (
      SELECT 1
      FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = enrollment.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'canary reservation requires an active fixture enrollment');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_reservations_no_update
BEFORE UPDATE ON tally_write_canary_reservations
BEGIN
  SELECT RAISE(ABORT, 'canary reservations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_canary_reservations_no_delete
BEFORE DELETE ON tally_write_canary_reservations
BEGIN
  SELECT RAISE(ABORT, 'canary reservations cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (15, 'single-use durable reservation for local Tally synthetic write canaries', 0);
