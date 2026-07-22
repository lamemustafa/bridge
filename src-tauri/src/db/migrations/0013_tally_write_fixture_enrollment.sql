CREATE TABLE IF NOT EXISTS tally_write_fixture_enrollments (
  id TEXT PRIMARY KEY,
  company_id TEXT NOT NULL,
  review_commitment_sha256 TEXT NOT NULL UNIQUE CHECK (
    length(review_commitment_sha256) = 64 AND
    review_commitment_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  enrollment_payload_sha256 TEXT NOT NULL CHECK (
    length(enrollment_payload_sha256) = 64 AND
    enrollment_payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  contract_version INTEGER NOT NULL CHECK (contract_version = 1),
  disposable_company_attested INTEGER NOT NULL CHECK (disposable_company_attested = 1),
  no_customer_data_attested INTEGER NOT NULL CHECK (no_customer_data_attested = 1),
  backup_guidance_acknowledged INTEGER NOT NULL CHECK (backup_guidance_acknowledged = 1),
  enrolled_at_unix_ms INTEGER NOT NULL CHECK (enrolled_at_unix_ms > 0),
  FOREIGN KEY (company_id) REFERENCES tally_companies(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS tally_write_fixture_revocations (
  id TEXT PRIMARY KEY,
  enrollment_id TEXT NOT NULL UNIQUE,
  revocation_payload_sha256 TEXT NOT NULL CHECK (
    length(revocation_payload_sha256) = 64 AND
    revocation_payload_sha256 NOT GLOB '*[^0-9a-f]*'
  ),
  safe_reason_code TEXT NOT NULL CHECK (safe_reason_code = 'operator_revoked'),
  revoked_at_unix_ms INTEGER NOT NULL CHECK (revoked_at_unix_ms > 0),
  FOREIGN KEY (enrollment_id) REFERENCES tally_write_fixture_enrollments(id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_tally_write_fixture_enrollments_company
  ON tally_write_fixture_enrollments(company_id, enrolled_at_unix_ms);

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_enrollment_requires_observed_company
BEFORE INSERT ON tally_write_fixture_enrollments
WHEN NOT EXISTS (
  SELECT 1 FROM tally_companies
  WHERE id = NEW.company_id AND identity_confidence = 'observed'
    AND company_guid IS NOT NULL AND TRIM(company_guid) <> ''
)
BEGIN
  SELECT RAISE(ABORT, 'fixture enrollment requires observed company identity');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_enrollment_one_active_per_company
BEFORE INSERT ON tally_write_fixture_enrollments
WHEN EXISTS (
  SELECT 1 FROM tally_write_fixture_enrollments AS existing
  WHERE existing.company_id = NEW.company_id
    AND NOT EXISTS (
      SELECT 1 FROM tally_write_fixture_revocations AS revocation
      WHERE revocation.enrollment_id = existing.id
    )
)
BEGIN
  SELECT RAISE(ABORT, 'active fixture enrollment already exists');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_enrollments_no_update
BEFORE UPDATE ON tally_write_fixture_enrollments
BEGIN
  SELECT RAISE(ABORT, 'fixture enrollments are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_enrollments_no_delete
BEFORE DELETE ON tally_write_fixture_enrollments
BEGIN
  SELECT RAISE(ABORT, 'fixture enrollments cannot be deleted');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_revocations_no_update
BEFORE UPDATE ON tally_write_fixture_revocations
BEGIN
  SELECT RAISE(ABORT, 'fixture revocations are immutable');
END;

CREATE TRIGGER IF NOT EXISTS tally_write_fixture_revocations_no_delete
BEFORE DELETE ON tally_write_fixture_revocations
BEGIN
  SELECT RAISE(ABORT, 'fixture revocations cannot be deleted');
END;

INSERT OR IGNORE INTO tally_schema_migrations(version, description, applied_at_unix_ms)
VALUES (13, 'local Tally synthetic write-fixture enrollment and revocation evidence', 0);
