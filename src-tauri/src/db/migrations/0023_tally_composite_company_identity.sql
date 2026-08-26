-- See TALLY_PROTOCOL_REFERENCE.md §9.11b. A Tally year-end split can retain the parent's GUID. A GUID-only pin can
-- therefore resolve to a different book after a rename. The Company
-- collection's observed tuple is the only accepted company identity.
DROP INDEX IF EXISTS uq_tally_companies_guid;

ALTER TABLE tally_companies ADD COLUMN company_number TEXT;
ALTER TABLE tally_companies ADD COLUMN books_from_yyyymmdd TEXT;

-- No historical row has observed the two new fields. Do not invent them from
-- a current listing: doing so could silently bind an old pin to another book.
UPDATE tally_companies
SET identity_confidence = 'unknown'
WHERE identity_confidence = 'observed';

-- Widen the immutable revocation evidence vocabulary before the Rust-owned
-- migration step inserts the canonical re-verification records. SQLite cannot
-- calculate Bridge's canonical JSON SHA-256 payload, so it must not fabricate
-- those rows in SQL.
DROP TRIGGER IF EXISTS tally_write_fixture_revocations_require_sequence;
DROP TRIGGER IF EXISTS tally_write_fixture_revocations_no_update;
DROP TRIGGER IF EXISTS tally_write_fixture_revocations_no_delete;
DROP INDEX IF EXISTS idx_tally_write_fixture_revocations_sequence;
ALTER TABLE tally_write_fixture_revocations RENAME TO tally_write_fixture_revocations_legacy;

CREATE TABLE tally_write_fixture_revocations (
  event_sequence INTEGER NOT NULL UNIQUE CHECK (event_sequence > 0),
  id TEXT PRIMARY KEY,
  enrollment_id TEXT NOT NULL UNIQUE,
  revocation_payload_sha256 TEXT NOT NULL CHECK (
    length(revocation_payload_sha256) = 64 AND
    revocation_payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  safe_reason_code TEXT NOT NULL CHECK (
    safe_reason_code IN ('operator_revoked', 'company_identity_reverification_required')
  ),
  revoked_at_unix_ms INTEGER NOT NULL CHECK (revoked_at_unix_ms > 0),
  FOREIGN KEY (enrollment_id) REFERENCES tally_write_fixture_enrollments(id) ON DELETE RESTRICT
);

INSERT INTO tally_write_fixture_revocations(
  event_sequence, id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms
)
SELECT
  event_sequence, id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms
FROM tally_write_fixture_revocations_legacy;
DROP TABLE tally_write_fixture_revocations_legacy;

CREATE UNIQUE INDEX idx_tally_write_fixture_revocations_sequence
  ON tally_write_fixture_revocations(event_sequence);
CREATE TRIGGER tally_write_fixture_revocations_require_sequence
BEFORE INSERT ON tally_write_fixture_revocations
WHEN NEW.event_sequence <= 0
BEGIN
  SELECT RAISE(ABORT, 'fixture revocation requires durable sequence');
END;
CREATE TRIGGER tally_write_fixture_revocations_no_update
BEFORE UPDATE ON tally_write_fixture_revocations
BEGIN
  SELECT RAISE(ABORT, 'fixture revocations are immutable');
END;
CREATE TRIGGER tally_write_fixture_revocations_no_delete
BEFORE DELETE ON tally_write_fixture_revocations
BEGIN
  SELECT RAISE(ABORT, 'fixture revocations cannot be deleted');
END;

CREATE UNIQUE INDEX uq_tally_companies_observed_identity
  ON tally_companies(
    endpoint_id,
    company_number,
    company_guid COLLATE NOCASE,
    display_name,
    books_from_yyyymmdd
  )
  WHERE company_number IS NOT NULL AND TRIM(company_number) <> ''
    AND company_guid IS NOT NULL AND TRIM(company_guid) <> ''
    AND books_from_yyyymmdd IS NOT NULL AND TRIM(books_from_yyyymmdd) <> '';

DROP TRIGGER IF EXISTS tally_write_fixture_enrollment_requires_observed_company;
CREATE TRIGGER tally_write_fixture_enrollment_requires_observed_company
BEFORE INSERT ON tally_write_fixture_enrollments
WHEN NOT EXISTS (
  SELECT 1 FROM tally_companies
  WHERE id = NEW.company_id AND identity_confidence = 'observed'
    AND company_guid IS NOT NULL AND TRIM(company_guid) <> ''
    AND company_number IS NOT NULL AND TRIM(company_number) <> ''
    AND books_from_yyyymmdd IS NOT NULL AND TRIM(books_from_yyyymmdd) <> ''
)
BEGIN
  SELECT RAISE(ABORT, 'fixture enrollment requires observed composite company identity');
END;
