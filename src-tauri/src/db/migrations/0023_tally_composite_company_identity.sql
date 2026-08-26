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

-- A write-fixture enrollment is authority for future canary work. Once the
-- company pin it names is deliberately retired, retaining that authority
-- would let a pre-existing reservation cross the re-verification boundary.
INSERT INTO tally_write_fixture_revocations(
  event_sequence, id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms
)
SELECT
  (SELECT COALESCE(MAX(event_sequence), 0) FROM tally_write_fixture_revocations)
    + ROW_NUMBER() OVER (ORDER BY enrollment.id),
  lower(hex(randomblob(16))), enrollment.id, lower(hex(randomblob(32))), 'operator_revoked',
  CASE WHEN enrollment.enrolled_at_unix_ms > 0 THEN enrollment.enrolled_at_unix_ms ELSE 1 END
FROM tally_write_fixture_enrollments AS enrollment
JOIN tally_companies AS company ON company.id = enrollment.company_id
WHERE company.identity_confidence = 'unknown'
  AND NOT EXISTS (
    SELECT 1 FROM tally_write_fixture_revocations AS prior
    WHERE prior.enrollment_id = enrollment.id
  );

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

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (23, 'Tally composite company identity requires re-verification of GUID-only pins', 0);
