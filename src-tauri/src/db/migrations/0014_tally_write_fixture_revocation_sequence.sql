DROP TRIGGER IF EXISTS tally_write_fixture_revocations_no_update;

ALTER TABLE tally_write_fixture_revocations
  ADD COLUMN event_sequence INTEGER NOT NULL DEFAULT 0;

UPDATE tally_write_fixture_revocations
SET event_sequence = rowid
WHERE event_sequence = 0;

CREATE UNIQUE INDEX IF NOT EXISTS idx_tally_write_fixture_revocations_sequence
  ON tally_write_fixture_revocations(event_sequence);

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_revocations_require_sequence
BEFORE INSERT ON tally_write_fixture_revocations
WHEN NEW.event_sequence <= 0
BEGIN
  SELECT RAISE(ABORT, 'fixture revocation requires durable sequence');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_revocations_no_update
BEFORE UPDATE ON tally_write_fixture_revocations
BEGIN
  SELECT RAISE(ABORT, 'fixture revocations are immutable');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (14, 'durable sequence for local Tally synthetic write-fixture revocations', 0);
